//! HTTP-first e-Class transport backed by the saved browser session.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;
use url::Url;

use crate::{
    error::EclassError,
    session::{
        JSON_ACCEPT, ORIGIN, Session, USER_AGENT, chrome_profile_dir, default_session_file,
        load_session, refresh_from_profile, save_session,
    },
};

const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_DOWNLOAD_HOSTS: [&str; 3] = [
    "eclass3.cau.ac.kr",
    "ocs.cau.ac.kr",
    "cau-cms-object.cdn.gov-ntruss.com",
];

/// One successful GET response.
#[derive(Debug, Clone, Serialize)]
pub struct GetResponse {
    pub status: u16,
    pub body: Value,
    pub content_type: String,
    pub link: Option<String>,
}

#[derive(Debug, Clone)]
struct CacheValue {
    expires: Instant,
    value: GetResponse,
}

#[derive(Debug)]
struct CacheSlot {
    value: TokioMutex<Option<CacheValue>>,
    load: Arc<TokioMutex<()>>,
}

#[derive(Debug, Default)]
struct TtlCache {
    values: TokioMutex<HashMap<(String, bool, bool), Arc<CacheSlot>>>,
    ttl: Duration,
}

impl TtlCache {
    fn new(ttl: Duration) -> Self {
        Self {
            values: TokioMutex::new(HashMap::new()),
            ttl,
        }
    }

    async fn clear(&self) {
        self.values.lock().await.clear();
    }

    async fn slot(&self, key: &(String, bool, bool)) -> Arc<CacheSlot> {
        let mut values = self.values.lock().await;
        values
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(CacheSlot {
                    value: TokioMutex::new(None),
                    load: Arc::new(TokioMutex::new(())),
                })
            })
            .clone()
    }
}

/// HTTP transport with a short TTL cache and one loader per key.
#[derive(Debug)]
pub struct HttpTransport {
    client: reqwest::Client,
    base: Url,
    session: StdMutex<Session>,
    session_file: PathBuf,
    profile_dir: PathBuf,
    cache: TtlCache,
    auto_refresh: bool,
    allowed_hosts: HashSet<String>,
    has_session: AtomicBool,
}

impl HttpTransport {
    /// Build a transport for a saved session.
    ///
    /// # Errors
    ///
    /// Returns [`EclassError::Url`] when the base URL is invalid.
    pub fn new(
        session: Session,
        session_file: PathBuf,
        base: Url,
        cache_ttl: Duration,
    ) -> Result<Self, EclassError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            base,
            session: StdMutex::new(session),
            session_file,
            profile_dir: chrome_profile_dir(),
            cache: TtlCache::new(cache_ttl),
            auto_refresh: std::env::var("ECLASS_AUTO_REFRESH").as_deref() != Ok("0"),
            allowed_hosts: DEFAULT_DOWNLOAD_HOSTS
                .iter()
                .map(|host| (*host).to_string())
                .collect(),
            has_session: AtomicBool::new(true),
        })
    }

    /// Load the session from `ECLASS_SESSION_FILE` and use the production origin.
    ///
    /// A missing file still yields a transport so the MCP server can start and
    /// answer `get_onboarding_status`; other tools will report a missing session.
    pub fn from_env() -> Result<Self, EclassError> {
        Self::from_session(None, None)
    }

    /// Load a session from an explicit path or the environment, with an optional Chrome profile.
    pub fn from_session(
        path: Option<PathBuf>,
        profile: Option<PathBuf>,
    ) -> Result<Self, EclassError> {
        let path = path.unwrap_or_else(default_session_file);
        let loaded = load_session(&path)?;
        let has_session = loaded.is_some();
        let session = loaded.unwrap_or_else(|| Session::from_browser(Vec::new(), None));
        let ttl = std::env::var("ECLASS_CACHE_TTL")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(20);
        let mut transport =
            Self::new(session, path, Url::parse(ORIGIN)?, Duration::from_secs(ttl))?;
        transport.has_session.store(has_session, Ordering::Relaxed);
        if let Some(profile) = profile {
            transport.profile_dir = profile;
        }
        Ok(transport)
    }

    /// Set the Chrome profile used for automatic session refresh.
    #[must_use]
    pub fn with_profile_dir(mut self, profile: PathBuf) -> Self {
        self.profile_dir = profile;
        self
    }

    /// Whether a session file was loaded (it may still be expired).
    #[must_use]
    pub fn has_session(&self) -> bool {
        self.has_session.load(Ordering::Relaxed)
    }

    /// The saved session, cloned for read-only inspection.
    #[must_use]
    pub fn session(&self) -> Option<Session> {
        self.session.lock().ok().map(|session| session.clone())
    }

    /// The session file used by this transport.
    #[must_use]
    pub fn session_file(&self) -> &Path {
        &self.session_file
    }

    /// The configured base URL.
    #[must_use]
    pub fn base(&self) -> &Url {
        &self.base
    }

    /// Clear cached GET responses.
    pub async fn invalidate(&self) {
        self.cache.clear().await;
    }

    /// Refresh the session from the dedicated Chrome profile and persist it.
    pub async fn refresh(&self) -> Result<(), EclassError> {
        let browser = self
            .session
            .lock()
            .ok()
            .and_then(|session| session.browser.clone())
            .unwrap_or_else(|| "chrome".to_string());
        let mut session = refresh_from_profile(&self.profile_dir, &browser).await?;
        save_session(&mut session, &self.session_file)?;
        *self
            .session
            .lock()
            .map_err(|_| EclassError::InvalidSession)? = session;
        self.has_session.store(true, Ordering::Relaxed);
        self.cache.clear().await;
        Ok(())
    }

    /// Send a GET and parse JSON (or HTML when `html` is true).
    ///
    /// # Errors
    ///
    /// Returns [`EclassError::SessionExpired`] when the session cannot be used,
    /// [`EclassError::Http`] for non-success status codes, and
    /// [`EclassError::ExpectedJson`] for an unexpected body.
    pub async fn get(
        &self,
        path: &str,
        learningx: bool,
        html: bool,
    ) -> Result<GetResponse, EclassError> {
        let path = local_path(path)?;
        let key = (path.clone(), learningx, html);
        let slot = self.cache.slot(&key).await;
        {
            let value = slot.value.lock().await;
            if let Some(value) = value.as_ref()
                && value.expires > Instant::now()
            {
                return Ok(value.value.clone());
            }
        }
        let _guard = slot.load.lock().await;
        {
            let value = slot.value.lock().await;
            if let Some(value) = value.as_ref()
                && value.expires > Instant::now()
            {
                return Ok(value.value.clone());
            }
        }
        let response = self.request_with_retry(&path, learningx, html).await?;
        {
            let mut value = slot.value.lock().await;
            *value = Some(CacheValue {
                expires: Instant::now() + self.cache.ttl,
                value: response.clone(),
            });
        }
        Ok(response)
    }

    async fn request_with_retry(
        &self,
        path: &str,
        learningx: bool,
        html: bool,
    ) -> Result<GetResponse, EclassError> {
        for attempt in 0..2 {
            let result = self.request_once(path, learningx, html).await;
            match result {
                Ok(response) => {
                    if response.status == 401 && attempt == 0 {
                        self.try_refresh().await;
                        continue;
                    }
                    if response.status == 401 {
                        return Err(EclassError::SessionExpired);
                    }
                    if response.status == 403 {
                        return Err(EclassError::Http(403));
                    }
                    if response.status >= 400 {
                        return Err(EclassError::Http(response.status));
                    }
                    if !html && response.content_type.contains("html") {
                        if attempt == 0 && self.try_refresh().await {
                            continue;
                        }
                        return Err(EclassError::SessionExpired);
                    }
                    return Ok(response);
                }
                Err(EclassError::LoginRedirect) if attempt == 0 => {
                    self.try_refresh().await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(EclassError::SessionExpired)
    }

    async fn try_refresh(&self) -> bool {
        if !self.auto_refresh {
            return false;
        }
        self.refresh().await.is_ok()
    }

    async fn request_once(
        &self,
        path: &str,
        learningx: bool,
        html: bool,
    ) -> Result<GetResponse, EclassError> {
        let mut current = self.base.join(path)?;
        let mut request_path = path.to_string();
        for _ in 0..5 {
            let headers = self.headers(&request_path, learningx, html, &current);
            let response = self
                .client
                .get(current.clone())
                .headers(headers)
                .send()
                .await
                .map_err(|_| EclassError::Network)?;
            self.merge_response_cookies(&response, &current);
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(EclassError::CrossOrigin)?;
                let target = current.join(location)?;
                if target.scheme() != self.base.scheme()
                    || target.host_str() != self.base.host_str()
                    || target.port_or_known_default() != self.base.port_or_known_default()
                    || !target.username().is_empty()
                    || target.password().is_some()
                {
                    return Err(EclassError::CrossOrigin);
                }
                if target.path().to_ascii_lowercase().contains("login") {
                    return Err(EclassError::LoginRedirect);
                }
                request_path = target.path().to_string();
                current = target;
                continue;
            }
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let link = response
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let bytes = response.bytes().await.map_err(|_| EclassError::Network)?;
            if bytes.len() > if html { MAX_HTML_BYTES } else { MAX_JSON_BYTES } {
                return Err(EclassError::ResponseTooLarge);
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let body = if status >= 400 {
                Value::Null
            } else if html {
                Value::String(text)
            } else {
                let text = strip_canvas_prefix(&text);
                serde_json::from_str(text).map_err(|_| EclassError::ExpectedJson)?
            };
            return Ok(GetResponse {
                status,
                body,
                content_type,
                link,
            });
        }
        Err(EclassError::TooManyRedirects)
    }

    fn headers(
        &self,
        path: &str,
        learningx: bool,
        html: bool,
        url: &Url,
    ) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        let accept = if html {
            "text/html,application/xhtml+xml"
        } else {
            JSON_ACCEPT
        };
        if let Ok(value) = reqwest::header::HeaderValue::from_str(accept) {
            headers.insert(reqwest::header::ACCEPT, value);
        }
        if let Ok(value) = reqwest::header::HeaderValue::from_str(USER_AGENT) {
            headers.insert(reqwest::header::USER_AGENT, value);
        }
        if let Ok(value) = reqwest::header::HeaderValue::from_str(ORIGIN) {
            headers.insert(reqwest::header::REFERER, value);
        }
        let Ok(session) = self.session.lock() else {
            return headers;
        };
        if let Some(cookie) = session.cookie_header(url)
            && let Ok(value) = reqwest::header::HeaderValue::from_str(&cookie)
        {
            headers.insert(reqwest::header::COOKIE, value);
        }
        if (learningx || path.starts_with("/learningx/"))
            && let Some(token) = session.token("xn_api_token")
            && let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        headers
    }

    fn merge_response_cookies(&self, response: &reqwest::Response, url: &Url) {
        let Ok(mut session) = self.session.lock() else {
            return;
        };
        for header in response.headers().get_all(reqwest::header::SET_COOKIE) {
            if let Ok(value) = header.to_str() {
                session.merge_set_cookie(value, url);
            }
        }
    }

    fn trusted_download(&self, value: &str) -> Result<Url, EclassError> {
        let url = Url::parse(value).map_err(|_| EclassError::UntrustedHost)?;
        let host = url.host_str().ok_or(EclassError::UntrustedHost)?;
        let same_base = host == self.base.host_str().unwrap_or_default()
            && url.port_or_known_default() == self.base.port_or_known_default();
        let cau_host = url.scheme() == "https"
            && self.allowed_hosts.contains(host)
            && url.port().is_none_or(|port| port == 443);
        if (!same_base && !cau_host)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.scheme() != "https" && !same_base
        {
            return Err(EclassError::UntrustedHost);
        }
        Ok(url)
    }

    /// Download a same-origin e-Class path to a new local file.
    pub async fn download(
        &self,
        path: &str,
        destination: &Path,
        max_bytes: u64,
    ) -> Result<DownloadResult, EclassError> {
        let url = self.base.join(&local_path(path)?)?;
        self.download_url(url.as_str(), destination, max_bytes, None)
            .await
    }

    /// Download an absolute URL after host validation.
    pub async fn download_url(
        &self,
        value: &str,
        destination: &Path,
        max_bytes: u64,
        expected: Option<u64>,
    ) -> Result<DownloadResult, EclassError> {
        if max_bytes == 0 {
            return Err(EclassError::DownloadTooLarge);
        }
        let mut target = self.trusted_download(value)?;
        for _ in 0..6 {
            let headers = self.download_headers(&target);
            let response = self
                .client
                .get(target.clone())
                .headers(headers)
                .send()
                .await
                .map_err(|_| EclassError::Network)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(EclassError::TooManyRedirects)?;
                target = self.trusted_download(target.join(location)?.as_str())?;
                continue;
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !response.status().is_success()
                || content_type.contains("html")
                || content_type.contains("json")
            {
                return Err(EclassError::NotAFile);
            }
            if response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|length| length > max_bytes)
            {
                return Err(EclassError::DownloadTooLarge);
            }
            let created = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .map_err(EclassError::Io);
            let mut file = match created {
                Ok(file) => file,
                Err(error) => return Err(error),
            };
            set_private_mode(destination)?;
            let mut size = 0_u64;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| EclassError::Network)?;
                size += chunk.len() as u64;
                if size > max_bytes {
                    drop(file);
                    let _ = std::fs::remove_file(destination);
                    return Err(EclassError::DownloadTooLarge);
                }
                use std::io::Write;
                if let Err(error) = file.write_all(&chunk) {
                    drop(file);
                    let _ = std::fs::remove_file(destination);
                    return Err(EclassError::Io(error));
                }
            }
            drop(file);
            if size == 0 || expected.is_some_and(|expected| expected != size) {
                let _ = std::fs::remove_file(destination);
                return Err(EclassError::SizeMismatch);
            }
            return Ok(DownloadResult {
                bytes: size,
                content_type,
            });
        }
        Err(EclassError::TooManyRedirects)
    }

    fn download_headers(&self, url: &Url) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = reqwest::header::HeaderValue::from_str("*/*") {
            headers.insert(reqwest::header::ACCEPT, value);
        }
        if let Ok(value) = reqwest::header::HeaderValue::from_str(USER_AGENT) {
            headers.insert(reqwest::header::USER_AGENT, value);
        }
        if let Ok(value) = reqwest::header::HeaderValue::from_str(ORIGIN) {
            headers.insert(reqwest::header::REFERER, value);
        }
        if let Ok(session) = self.session.lock()
            && let Some(cookie) = session.cookie_header(url)
            && let Ok(value) = reqwest::header::HeaderValue::from_str(&cookie)
        {
            headers.insert(reqwest::header::COOKIE, value);
        }
        headers
    }

    async fn fetch_text(&self, value: &str, limit: usize) -> Result<String, EclassError> {
        let mut target = self.trusted_download(value)?;
        for _ in 0..6 {
            let response = self
                .client
                .get(target.clone())
                .headers(self.download_headers(&target))
                .send()
                .await
                .map_err(|_| EclassError::MetadataFailed)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(EclassError::TooManyRedirects)?;
                target = self.trusted_download(target.join(location)?.as_str())?;
                continue;
            }
            if !response.status().is_success() {
                return Err(EclassError::MetadataFailed);
            }
            let mut size = 0_usize;
            let mut chunks = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| EclassError::MetadataFailed)?;
                size += chunk.len();
                if size > limit {
                    return Err(EclassError::MetadataTooLarge);
                }
                chunks.extend_from_slice(&chunk);
            }
            return String::from_utf8(chunks).map_err(|_| EclassError::MetadataFailed);
        }
        Err(EclassError::TooManyRedirects)
    }

    async fn modules(&self, course_id: u32) -> Result<Vec<Value>, EclassError> {
        let response = self
            .get(
                &format!("/learningx/api/v1/courses/{course_id}/modules?include_detail=true"),
                true,
                false,
            )
            .await?;
        response
            .body
            .as_array()
            .cloned()
            .ok_or(EclassError::UnexpectedList)
    }

    async fn media_info(
        &self,
        course_id: u32,
        item_id: u32,
        kind: &str,
    ) -> Result<(Option<Value>, Value), EclassError> {
        let modules = self.modules(course_id).await?;
        let item = find_item(&Value::Array(modules), item_id).cloned();
        let Some(item) = item else {
            return Ok((
                None,
                json!({"available": false, "reason": format!("{kind} item not found in course")}),
            ));
        };
        if item
            .get("course_id")
            .and_then(positive_id)
            .is_some_and(|owner| owner != course_id)
        {
            return Ok((
                None,
                json!({"available": false, "reason": format!("{kind} item not found in this course")}),
            ));
        }
        let data = item
            .get("item_content_data")
            .cloned()
            .unwrap_or(Value::Null);
        if locked(&item)
            || locked(&data)
            || data.get("content_id").is_none()
            || data.get("content_id").and_then(Value::as_str) == Some("not_open")
        {
            return Ok((
                None,
                json!({"available": false, "reason": format!("{kind} is not open to this user")}),
            ));
        }
        Ok((Some(item), data))
    }

    async fn commons_metadata(&self, content_id: &str) -> Result<Value, EclassError> {
        let path = format!(
            "/learningx/api/v1/commons/contents?content_id={}",
            url::form_urlencoded::byte_serialize(content_id.as_bytes()).collect::<String>()
        );
        let response = self.get(&path, true, false).await?;
        response
            .body
            .get("result")
            .cloned()
            .filter(Value::is_object)
            .ok_or(EclassError::MetadataFailed)
    }

    /// Download a Commons native PDF.
    pub async fn commons_pdf(
        &self,
        course_id: u32,
        item_id: u32,
        destination: &Path,
        max_bytes: u64,
    ) -> Result<Value, EclassError> {
        let (item, data) = self.media_info(course_id, item_id, "PDF").await?;
        let Some(item) = item else {
            return Ok(data);
        };
        if item.get("item_content_type").and_then(Value::as_str) != Some("commons")
            || data.get("content_type").and_then(Value::as_str) != Some("pdf")
        {
            return Ok(
                json!({"available": false, "reason": "Only Commons PDF items are supported"}),
            );
        }
        let content_id = data
            .get("content_id")
            .and_then(Value::as_str)
            .ok_or(EclassError::PdfUnavailable)?;
        let metadata = self.commons_metadata(content_id).await?;
        if metadata
            .get("is_original_file_deleted")
            .and_then(Value::as_bool)
            == Some(true)
            || !metadata
                .get("extension")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
        {
            return Ok(json!({
                "available": false,
                "reason": "A native original PDF is unavailable; rendered pages are not synthesized"
            }));
        }
        let expected = metadata.get("original_file_length").and_then(positive_u64);
        if expected.is_some_and(|expected| expected > max_bytes) {
            return Err(EclassError::DownloadTooLarge);
        }
        let info = json!({
            "available": true,
            "title": item.get("title").cloned().unwrap_or(Value::Null),
            "source": "commons_original",
            "original": true,
            "expected_bytes": expected,
        });
        if let Some(download_url) = metadata.get("download_url").and_then(Value::as_str) {
            match self
                .download_url(download_url, destination, max_bytes, expected)
                .await
            {
                Ok(result) => return Ok(merge_download(info, result)),
                Err(error) if endpoint_unavailable(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let Some(view_url) = metadata.get("view_url").and_then(Value::as_str) else {
            return Err(EclassError::PdfUnavailable);
        };
        let html = self.fetch_text(view_url, MAX_METADATA_BYTES).await?;
        let pattern = Regex::new(r#"var\s+contentUri\s*=\s*['"]([^'"]+)['"]"#)
            .map_err(|_| EclassError::PdfViewerUnsupported)?;
        let captures = pattern
            .captures(&html)
            .ok_or(EclassError::PdfViewerUnsupported)?;
        let content_uri =
            html_escape::decode_html_entities(captures.get(1).map_or("", |value| value.as_str()));
        let xml_url = Url::parse(view_url)?.join(&content_uri)?;
        let xml = self
            .fetch_text(xml_url.as_str(), MAX_METADATA_BYTES)
            .await?;
        let pattern = Regex::new(r"<content_download_uri>\s*(.*?)\s*</content_download_uri>")
            .map_err(|_| EclassError::PdfViewerUnsupported)?;
        let captures = pattern
            .captures(&xml)
            .ok_or(EclassError::PdfViewerUnsupported)?;
        let mut raw = captures
            .get(1)
            .map_or("", |value| value.as_str())
            .to_string();
        if raw.starts_with("<![CDATA[") && raw.ends_with("]]>") {
            raw = raw[9..raw.len() - 3].to_string();
        }
        let target = Url::parse(view_url)?.join(&html_escape::decode_html_entities(&raw))?;
        let result = self
            .download_url(target.as_str(), destination, max_bytes, expected)
            .await?;
        Ok(merge_download(info, result))
    }

    /// Download a Commons original MP4, optionally falling back to a playback copy.
    pub async fn video_original(
        &self,
        course_id: u32,
        item_id: u32,
        destination: Option<&Path>,
        allow_playback_copy: bool,
    ) -> Result<Value, EclassError> {
        let (item, data) = self.media_info(course_id, item_id, "Video").await?;
        let Some(item) = item else {
            return Ok(data);
        };
        if data.get("content_type").and_then(Value::as_str) != Some("mp4") {
            return Ok(
                json!({"available": false, "reason": "Only original MP4 downloads are supported"}),
            );
        }
        let content_id = data
            .get("content_id")
            .and_then(Value::as_str)
            .ok_or(EclassError::VideoUnavailable)?;
        let metadata = self.commons_metadata(content_id).await?;
        if metadata
            .get("is_original_file_deleted")
            .and_then(Value::as_bool)
            == Some(true)
            || metadata.get("download_url").is_none()
        {
            return Ok(json!({"available": false, "reason": "Original file is unavailable"}));
        }
        let expected = metadata.get("original_file_length").and_then(positive_u64);
        let info = json!({
            "available": true,
            "title": item.get("title").cloned().unwrap_or(Value::Null),
            "expected_bytes": expected,
        });
        let Some(destination) = destination else {
            return Ok(info);
        };
        let limit = 1024_u64 * 1024 * 1024;
        if expected.is_some_and(|expected| expected > limit) {
            return Err(EclassError::DownloadTooLarge);
        }
        let url = metadata
            .get("download_url")
            .and_then(Value::as_str)
            .ok_or(EclassError::VideoUnavailable)?;
        let parsed = Url::parse(url).map_err(|_| EclassError::UntrustedHost)?;
        if parsed.host_str() != Some("ocs.cau.ac.kr") {
            return Err(EclassError::UntrustedHost);
        }
        match self.download_url(url, destination, limit, expected).await {
            Ok(result) => Ok(merge_download(info, result)),
            Err(error) if endpoint_unavailable(&error) => {
                if !allow_playback_copy {
                    return Err(EclassError::PlaybackRequired);
                }
                let view_url = metadata
                    .get("view_url")
                    .and_then(Value::as_str)
                    .ok_or(EclassError::PlaybackRequired)?;
                let playback = self.playback_url(view_url).await?;
                let result = self
                    .download_url(playback.as_str(), destination, limit, None)
                    .await?;
                Ok(merge_download(
                    json!({
                        "available": true,
                        "title": info.get("title").cloned().unwrap_or(Value::Null),
                        "expected_bytes": null,
                        "source": "commons_playback_mp4",
                        "original": false
                    }),
                    result,
                ))
            }
            Err(error) => Err(error),
        }
    }

    async fn playback_url(&self, view_url: &str) -> Result<Url, EclassError> {
        let html = self.fetch_text(view_url, MAX_METADATA_BYTES).await?;
        let pattern = Regex::new(r"var contentUri\s*=\s*'([^']+)'")
            .map_err(|_| EclassError::PlaybackMetadataUnsupported)?;
        let captures = pattern
            .captures(&html)
            .ok_or(EclassError::PlaybackMetadataUnsupported)?;
        let xml_url = Url::parse(view_url)?.join(&html_escape::decode_html_entities(
            captures.get(1).map_or("", |value| value.as_str()),
        ))?;
        let xml = self
            .fetch_text(xml_url.as_str(), MAX_METADATA_BYTES)
            .await?;
        let template = Regex::new(r#"<media_uri method="progressive" target="all">([^<]+)"#)
            .map_err(|_| EclassError::PlaybackMetadataUnsupported)?;
        let files = Regex::new(r#"<main_media media_id="[^"]+">([^<]+)"#)
            .map_err(|_| EclassError::PlaybackMetadataUnsupported)?;
        let template = template
            .captures(&xml)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str())
            .ok_or(EclassError::PlaybackMetadataUnsupported)?;
        let files: Vec<_> = files
            .captures_iter(&xml)
            .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_string()))
            .collect();
        if files.len() != 1
            || !Regex::new(r"^[-a-zA-Z0-9_.]+\.mp4$")
                .map_err(|_| EclassError::PlaybackMetadataUnsupported)?
                .is_match(&files[0])
        {
            return Err(EclassError::PlaybackUnsupported);
        }
        let target = template.replace("[MEDIA_FILE]", &files[0]);
        self.trusted_download(&target)
    }

    /// OZ native PDF export is a browser UI flow.
    pub async fn syllabus_pdf(
        &self,
        _course_id: u32,
        _destination: &Path,
    ) -> Result<(), EclassError> {
        Err(EclassError::BrowserOnly(
            "download_syllabus_pdf requires the Python/browser build",
        ))
    }
}

/// A completed download.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadResult {
    pub bytes: u64,
    pub content_type: String,
}

fn merge_download(mut info: Value, result: DownloadResult) -> Value {
    if let Some(object) = info.as_object_mut() {
        object.insert("bytes".to_string(), json!(result.bytes));
        object.insert("content_type".to_string(), json!(result.content_type));
    }
    info
}

fn endpoint_unavailable(error: &EclassError) -> bool {
    matches!(
        error,
        EclassError::NotAFile | EclassError::Network | EclassError::TooManyRedirects
    )
}

fn set_private_mode(path: &Path) -> Result<(), EclassError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn strip_canvas_prefix(value: &str) -> &str {
    let value = value.trim_start_matches('\u{feff}').trim_start();
    value
        .strip_prefix("while(1);")
        .map_or(value, str::trim_start)
}

fn positive_id(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(text) => text.parse::<u32>().ok(),
        _ => None,
    }
}

fn positive_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

fn locked(value: &Value) -> bool {
    value.get("opened").and_then(Value::as_bool) == Some(false)
        || value.get("published").and_then(Value::as_bool) == Some(false)
        || value.get("locked").and_then(Value::as_bool) == Some(true)
        || value.get("locked_for_user").and_then(Value::as_bool) == Some(true)
        || value
            .get("lecture_period_status")
            .and_then(Value::as_str)
            .is_some_and(|value| matches!(value, "locked" | "not_open" | "before_open" | "closed"))
}

fn find_item(value: &Value, item_id: u32) -> Option<&Value> {
    if let Some(object) = value.as_object() {
        if object.get("item_id").and_then(positive_id) == Some(item_id)
            && object.get("item_content_data").is_some()
        {
            return Some(value);
        }
        for child in object.values() {
            if let Some(found) = find_item(child, item_id) {
                return Some(found);
            }
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            if let Some(found) = find_item(child, item_id) {
                return Some(found);
            }
        }
    }
    None
}

/// Validate a same-origin relative path.
pub fn local_path(value: &str) -> Result<String, EclassError> {
    let lower = value.to_ascii_lowercase();
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('\\')
        || lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || value
            .split('?')
            .next()
            .unwrap_or(value)
            .split('/')
            .any(|segment| segment == "..")
    {
        return Err(EclassError::InvalidPath);
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    fn session() -> Session {
        Session::from_cdp(
            vec![crate::session::Cookie {
                name: "xn_api_token".to_string(),
                value: "synthetic".to_string(),
                domain: ".cau.ac.kr".to_string(),
                path: "/".to_string(),
                expires: None,
                secure: false,
                http_only: true,
                same_site: None,
                host_only: None,
            }],
            None,
        )
    }

    async fn transport(server: &MockServer) -> HttpTransport {
        HttpTransport::new(
            session(),
            PathBuf::from("/tmp/session.json"),
            Url::parse(&server.uri()).unwrap(),
            Duration::from_secs(20),
        )
        .unwrap()
    }

    #[test]
    fn rejects_unsafe_relative_paths() {
        for value in [
            "//example.com",
            "https://example.com",
            "/a/../b",
            "/%2e%2e/b",
        ] {
            assert!(local_path(value).is_err(), "{value}");
        }
        assert!(local_path("/api/v1/courses?page=1").is_ok());
    }

    #[tokio::test]
    async fn parses_canvas_prefix_and_link_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/courses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .insert_header(
                        "link",
                        "<https://eclass3.cau.ac.kr/api/v1/courses?page=2>; rel=\"next\"",
                    )
                    .set_body_string("while(1);[{\"id\":1}]"),
            )
            .mount(&server)
            .await;
        let transport = transport(&server).await;
        let response = transport
            .get("/api/v1/courses", false, false)
            .await
            .unwrap();
        assert_eq!(response.body[0]["id"], 1);
        assert!(response.link.unwrap().contains("rel=\"next\""));
    }

    #[tokio::test]
    async fn caches_repeated_gets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/courses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(json!([{"id": 1}])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let transport = transport(&server).await;
        assert!(transport.get("/api/v1/courses", false, false).await.is_ok());
        assert!(transport.get("/api/v1/courses", false, false).await.is_ok());
    }

    #[tokio::test]
    async fn sends_learningx_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/learningx/api/v1/courses/1/modules"))
            .and(header("authorization", "Bearer synthetic"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(json!([])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let transport = transport(&server).await;
        assert!(
            transport
                .get("/learningx/api/v1/courses/1/modules", true, false)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn download_rejects_untrusted_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://evil.test/x"),
            )
            .mount(&server)
            .await;
        let transport = transport(&server).await;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("file.pdf");
        let result = transport.download("/file", &target, 10).await;
        assert!(result.is_err());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn download_enforces_limit_and_no_overwrite() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(b"123456".to_vec()),
            )
            .mount(&server)
            .await;
        let transport = transport(&server).await;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("file.pdf");
        assert!(transport.download("/file", &target, 4).await.is_err());
        assert!(!target.exists());
        std::fs::write(&target, b"keep").unwrap();
        assert!(transport.download("/file", &target, 100).await.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
}
