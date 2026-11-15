//! Browser-session persistence and Chrome profile cookie import.
//!
//! The file stores `.cau.ac.kr` cookies, an optional browser id and a
//! `version: 1` envelope compatible with existing installs.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::EclassError;

pub const ORIGIN: &str = "https://eclass3.cau.ac.kr/";
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
pub const JSON_ACCEPT: &str = "application/json+canvas-string-ids";
const COOKIE_DOMAIN: &str = "cau.ac.kr";

/// A cookie as Chromium reports it through `Storage.getCookies`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub expires: Option<f64>,
    #[serde(default)]
    pub secure: bool,
    #[serde(rename = "httpOnly", default)]
    pub http_only: bool,
    #[serde(rename = "sameSite", default)]
    pub same_site: Option<String>,
    #[serde(rename = "hostOnly", default)]
    pub host_only: Option<bool>,
}

fn root_path() -> String {
    "/".to_string()
}

/// A saved browser session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub saved_at: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub browser: Option<String>,
    /// When the LearningX `xn_api_token` was last minted through an LTI launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learningx_token_at: Option<String>,
    pub cookies: Vec<Cookie>,
}

impl Session {
    /// Build a session from browser cookies, keeping only CAU cookies.
    #[must_use]
    pub fn from_browser(cookies: Vec<Cookie>, browser: Option<String>) -> Self {
        let mut values: BTreeMap<(String, String, String), Cookie> = BTreeMap::new();
        for cookie in cookies {
            if cookie.name.is_empty() || !is_cau_domain(&cookie.domain) {
                continue;
            }
            let domain = normalize_domain(&cookie.domain);
            let key = (
                cookie.name.clone(),
                domain,
                if cookie.path.is_empty() {
                    "/".to_string()
                } else {
                    cookie.path.clone()
                },
            );
            values.insert(key, cookie);
        }
        Self {
            version: 1,
            saved_at: now_iso(),
            endpoint: None,
            browser,
            learningx_token_at: None,
            cookies: values.into_values().collect(),
        }
    }

    /// Compatibility wrapper for older callers and session fixtures.
    #[must_use]
    pub fn from_cdp(cookies: Vec<Cookie>, endpoint: Option<String>) -> Self {
        let mut session = Self::from_browser(cookies, None);
        session.endpoint = endpoint;
        session
    }

    /// Return the first non-expired value for a cookie name.
    #[must_use]
    pub fn token(&self, name: &str) -> Option<&str> {
        let now = unix_now();
        self.cookies
            .iter()
            .find(|cookie| {
                cookie.name == name
                    && !cookie
                        .expires
                        .is_some_and(|expires| expires > 0.0 && expires <= now)
            })
            .map(|cookie| cookie.value.as_str())
    }

    /// Age of the current LearningX token in seconds, when its mint time is known.
    #[must_use]
    pub fn learningx_token_age_secs(&self) -> Option<i64> {
        let minted = DateTime::parse_from_rfc3339(self.learningx_token_at.as_deref()?).ok()?;
        Some(
            (Utc::now() - minted.with_timezone(&Utc))
                .num_seconds()
                .max(0),
        )
    }

    /// Build the `Cookie` header for one URL.
    #[must_use]
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let request_path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        let secure = url.scheme() == "https";
        let now = unix_now();
        let mut values = Vec::new();
        for cookie in &self.cookies {
            if cookie.secure && !secure {
                continue;
            }
            if cookie
                .expires
                .is_some_and(|expires| expires > 0.0 && expires <= now)
            {
                continue;
            }
            let domain = normalize_domain(&cookie.domain);
            if host != domain && !host.ends_with(&format!(".{domain}")) {
                continue;
            }
            let path = if cookie.path.is_empty() {
                "/"
            } else {
                cookie.path.as_str()
            };
            if !request_path.starts_with(path) {
                continue;
            }
            values.push(format!("{}={}", cookie.name, cookie.value));
        }
        (!values.is_empty()).then(|| values.join("; "))
    }

    /// Merge one `Set-Cookie` header into this session.
    pub fn merge_set_cookie(&mut self, header: &str, url: &Url) {
        let mut parts = header.split(';');
        let Some(pair) = parts.next() else {
            return;
        };
        let Some((name, value)) = pair.split_once('=') else {
            return;
        };
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let mut domain = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let mut path = "/".to_string();
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = None;
        for attribute in parts {
            let attribute = attribute.trim();
            if attribute.eq_ignore_ascii_case("secure") {
                secure = true;
            } else if attribute.eq_ignore_ascii_case("httponly") {
                http_only = true;
            } else if let Some((key, value)) = attribute.split_once('=') {
                match key.trim().to_ascii_lowercase().as_str() {
                    "domain" => domain = normalize_domain(value),
                    "path" => path = value.trim().to_string(),
                    "samesite" => same_site = Some(value.trim().to_string()),
                    _ => {}
                }
            }
        }
        self.upsert(Cookie {
            name: name.to_string(),
            value: value.trim().to_string(),
            domain,
            path,
            expires: None,
            secure,
            http_only,
            same_site,
            host_only: None,
        });
    }

    /// Insert or replace a cookie by `(name, domain, path)`.
    pub fn upsert(&mut self, cookie: Cookie) {
        let key = (
            cookie.name.clone(),
            normalize_domain(&cookie.domain),
            if cookie.path.is_empty() {
                "/".to_string()
            } else {
                cookie.path.clone()
            },
        );
        self.cookies.retain(|existing| {
            (
                existing.name.clone(),
                normalize_domain(&existing.domain),
                if existing.path.is_empty() {
                    "/".to_string()
                } else {
                    existing.path.clone()
                },
            ) != key
        });
        self.cookies.push(cookie);
        self.cookies
            .sort_by(|left, right| (&left.name, &left.domain).cmp(&(&right.name, &right.domain)));
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn normalize_domain(value: &str) -> String {
    value.trim_start_matches('.').to_ascii_lowercase()
}

pub(crate) fn is_cau_domain(value: &str) -> bool {
    let domain = normalize_domain(value);
    domain == COOKIE_DOMAIN || domain.ends_with(&format!(".{COOKIE_DOMAIN}"))
}

/// Return the platform-appropriate session path unless `ECLASS_SESSION_FILE` overrides it.
#[must_use]
pub fn default_session_file() -> PathBuf {
    if let Some(value) = env::var_os("ECLASS_SESSION_FILE") {
        return expand_home(&PathBuf::from(value));
    }
    let base = if cfg!(target_os = "macos") {
        home_dir().join("Library/Application Support/cau-eclass")
    } else if cfg!(windows) {
        PathBuf::from(env::var_os("APPDATA").unwrap_or_else(|| home_dir().into_os_string()))
            .join("cau-eclass")
    } else {
        PathBuf::from(
            env::var_os("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config").into()),
        )
        .join("cau-eclass")
    };
    base.join("session.json")
}

pub(crate) fn home_dir() -> PathBuf {
    env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn expand_home(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        path.to_path_buf()
    }
}

/// Load a session file; a missing file is `Ok(None)`.
pub fn load_session(path: &Path) -> Result<Option<Session>, EclassError> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(EclassError::Io(error)),
    };
    let session: Session = serde_json::from_str(&value).map_err(|_| EclassError::InvalidSession)?;
    if session.version != 1 {
        return Err(EclassError::InvalidSession);
    }
    Ok(Some(session))
}

/// Save a session atomically with owner-only permissions on Unix.
pub fn save_session(session: &mut Session, path: &Path) -> Result<PathBuf, EclassError> {
    session.saved_at = now_iso();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_mode(parent, 0o700)?;
    }
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    let encoded = serde_json::to_vec(session)?;
    fs::write(&temporary, encoded)?;
    set_mode(&temporary, 0o600)?;
    fs::rename(&temporary, path)?;
    set_mode(path, 0o600)?;
    Ok(path.to_path_buf())
}

fn set_mode(path: &Path, mode: u32) -> Result<(), EclassError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// Delete a saved session.
pub fn delete_session(path: &Path) -> Result<bool, EclassError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(EclassError::Io(error)),
    }
}

/// Default Chrome profile used for the one-time sign-in.
#[must_use]
pub fn chrome_profile_dir() -> PathBuf {
    if let Some(value) = env::var_os("ECLASS_CHROME_PROFILE") {
        return expand_home(&PathBuf::from(value));
    }
    home_dir().join(".cau-eclass/chrome-profile")
}

fn cookie_store_candidates(profile: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![profile.join("Default")];
    if let Ok(entries) = fs::read_dir(profile) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("Profile ") {
                dirs.push(entry.path());
            }
        }
    }
    dirs.into_iter()
        .flat_map(|dir| [dir.join("Network/Cookies"), dir.join("Cookies")])
        .filter(|path| path.is_file())
        .collect()
}

/// Read cookies from a Chrome/Chromium profile store.
///
/// The browser may need to be closed if it is locking the SQLite store.
pub fn read_browser_cookies(profile: &Path, browser_id: &str) -> Result<Vec<Cookie>, EclassError> {
    let candidates = cookie_store_candidates(profile);
    if candidates.is_empty() {
        return Err(EclassError::BrowserProfileUnavailable);
    }
    let mut last_error = EclassError::CookieStoreUnavailable;
    for candidate in candidates {
        #[cfg(unix)]
        let request =
            rookie_cookies::direct_path::PathExtractRequest::unix_identity(candidate, browser_id);
        #[cfg(windows)]
        let request = rookie_cookies::direct_path::PathExtractRequest::windows_local_state(
            candidate,
            profile.join("Local State"),
        );
        match rookie_cookies::direct_path::extract_from_path(request) {
            Ok(cookies) => {
                return Ok(cookies
                    .into_iter()
                    .map(|cookie| {
                        let same_site = match cookie.same_site {
                            0 => Some("None".to_string()),
                            1 => Some("Lax".to_string()),
                            2 => Some("Strict".to_string()),
                            _ => None,
                        };
                        Cookie {
                            name: cookie.name,
                            value: cookie.value,
                            domain: cookie.domain,
                            path: cookie.path,
                            expires: cookie.expires.map(|expires| expires as f64),
                            secure: cookie.secure,
                            http_only: cookie.http_only,
                            same_site,
                            host_only: None,
                        }
                    })
                    .collect());
            }
            Err(error) => {
                last_error = if error.code() == "source_inspection_failed" {
                    EclassError::CookieStoreUnavailable
                } else {
                    EclassError::BrowserProfileUnavailable
                };
            }
        }
    }
    Err(last_error)
}

/// Wait for a user login in the dedicated Chrome profile and snapshot its cookies.
pub async fn login_from_profile(
    profile: &Path,
    browser_id: &str,
    timeout: std::time::Duration,
) -> Result<Session, EclassError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_error;
    loop {
        match read_browser_cookies(profile, browser_id) {
            Ok(cookies) => {
                let mut session = Session::from_browser(cookies, Some(browser_id.to_string()));
                if verify_session(&session).await {
                    if let Ok(base) = Url::parse(ORIGIN) {
                        ensure_learningx_token(&mut session, &base).await;
                    }
                    return Ok(session);
                }
                last_error = EclassError::NotLoggedIn;
            }
            Err(error) => last_error = error,
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(last_error);
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// Reuse cookies from an already signed-in Chrome profile; fail promptly when absent.
pub async fn refresh_from_profile(
    profile: &Path,
    browser_id: &str,
) -> Result<Session, EclassError> {
    let cookies = read_browser_cookies(profile, browser_id)?;
    let mut session = Session::from_browser(cookies, Some(browser_id.to_string()));
    if !verify_session(&session).await {
        return Err(EclassError::NotLoggedIn);
    }
    if let Ok(base) = Url::parse(ORIGIN) {
        ensure_learningx_token(&mut session, &base).await;
    }
    Ok(session)
}

/// Verify that the profile endpoint accepts the session.
pub async fn verify_session(session: &Session) -> bool {
    let Ok(url) = Url::parse(ORIGIN) else {
        return false;
    };
    let Some(cookie) = session.cookie_header(&url) else {
        return false;
    };
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    let Ok(response) = client
        .get(url.join("api/v1/users/self/profile").unwrap_or(url))
        .header("Accept", JSON_ACCEPT)
        .header("User-Agent", USER_AGENT)
        .header("Cookie", cookie)
        .send()
        .await
    else {
        return false;
    };
    if response.status() != reqwest::StatusCode::OK {
        return false;
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("html"))
    {
        return false;
    }
    let Ok(text) = response.text().await else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let text = text.strip_prefix("while(1);").map_or(text, str::trim_start);
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("id").cloned())
        .is_some_and(|id| !id.is_null())
}

/// Mint the LearningX API token without a browser by replaying the LTI launch.
///
/// The board/module tool posts a signed LTI form from the Canvas course page.
/// That POST sets `xn_api_token`, which is the only credential the LearningX
/// JSON API needs beyond the Canvas session cookies.
pub async fn mint_learningx_token(
    session: &mut Session,
    base: &Url,
    course_id: Option<u32>,
    force: bool,
) -> Result<bool, EclassError> {
    if !force && session.token("xn_api_token").is_some() {
        return Ok(true);
    }
    if force {
        session
            .cookies
            .retain(|cookie| cookie.name != "xn_api_token");
    }
    if session
        .cookie_header(&base.join("api/v1/users/self/profile")?)
        .is_none()
    {
        return Err(EclassError::NoSession);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let course = match course_id {
        Some(course) => course,
        None => first_course_id(session, base, &client).await?,
    };
    let tool = first_learningx_tool(session, base, &client, course).await?;
    let page_url = base.join(&format!("courses/{course}/external_tools/{tool}"))?;
    let html = get_text(session, &client, page_url.clone()).await?;
    let Some((action, fields)) = lti_form(&html) else {
        return Err(EclassError::LearningxTokenMissing);
    };
    let action_url = page_url
        .join(&action)
        .map_err(|_| EclassError::InvalidPath)?;
    if action_url.scheme() != base.scheme()
        || action_url.host_str() != base.host_str()
        || action_url.port_or_known_default() != base.port_or_known_default()
    {
        return Err(EclassError::CrossOrigin);
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in &fields {
        serializer.append_pair(name, value);
    }
    let body = serializer.finish();
    let cookie = session
        .cookie_header(&action_url)
        .ok_or(EclassError::NoSession)?;
    let response = client
        .post(action_url.clone())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, page_url.as_str())
        .header(reqwest::header::COOKIE, cookie)
        .body(body)
        .send()
        .await?;
    for header in response.headers().get_all(reqwest::header::SET_COOKIE) {
        if let Ok(value) = header.to_str() {
            session.merge_set_cookie(value, &action_url);
        }
    }
    if session.token("xn_api_token").is_some() {
        session.learningx_token_at = Some(now_iso());
        return Ok(true);
    }
    Ok(false)
}

async fn first_course_id(
    session: &Session,
    base: &Url,
    client: &reqwest::Client,
) -> Result<u32, EclassError> {
    let url = base.join("api/v1/courses?include[]=term&per_page=100")?;
    let response = authorized_get(session, client, url).await?;
    let text = decode_canvas_prefix(response)
        .await
        .ok_or(EclassError::CanvasSessionExpired)?;
    let courses: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| EclassError::ExpectedJson)?;
    let year = Utc::now().year().to_string();
    let active = |course: &&serde_json::Value| {
        course
            .get("workflow_state")
            .and_then(serde_json::Value::as_str)
            != Some("completed")
    };
    let current = |course: &&serde_json::Value| {
        course
            .get("term")
            .and_then(|term| term.get("name"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| name.contains(&year))
    };
    courses
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|course| active(course) && current(course))
                .or_else(|| items.iter().find(active))
                .and_then(|course| course.get("id").and_then(positive_id))
        })
        .ok_or(EclassError::LearningxTokenMissing)
}

async fn first_learningx_tool(
    session: &Session,
    base: &Url,
    client: &reqwest::Client,
    course_id: u32,
) -> Result<u32, EclassError> {
    let url = base.join(&format!("courses/{course_id}"))?;
    let html = get_text(session, client, url).await?;
    let document = scraper::Html::parse_document(&html);
    let selector = scraper::Selector::parse("a[href*='external_tools']")
        .map_err(|_| EclassError::InvalidPath)?;
    let pattern = Regex::new(r"/external_tools/([0-9]+)").map_err(|_| EclassError::InvalidPath)?;
    let mut fallback = None;
    for anchor in document.select(&selector) {
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let Some(captures) = pattern.captures(href) else {
            continue;
        };
        let Some(tool) = captures
            .get(1)
            .and_then(|value| value.as_str().parse().ok())
        else {
            continue;
        };
        let label = anchor.text().collect::<String>();
        if ["게시판", "board", "강의자료", "주차학습", "모듈"]
            .iter()
            .any(|needle| label.to_ascii_lowercase().contains(needle))
        {
            return Ok(tool);
        }
        fallback.get_or_insert(tool);
    }
    fallback.ok_or(EclassError::LearningxTokenMissing)
}

async fn get_text(
    session: &Session,
    client: &reqwest::Client,
    url: Url,
) -> Result<String, EclassError> {
    let response = authorized_get(session, client, url.clone()).await?;
    if response.url().path().to_ascii_lowercase().contains("login") {
        return Err(EclassError::LoginRedirect);
    }
    if !response.status().is_success() {
        return Err(EclassError::Http(response.status().as_u16()));
    }
    response.text().await.map_err(|_| EclassError::Network)
}

async fn authorized_get(
    session: &Session,
    client: &reqwest::Client,
    url: Url,
) -> Result<reqwest::Response, EclassError> {
    let cookie = session
        .cookie_header(&url)
        .ok_or(EclassError::CanvasSessionExpired)?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, JSON_ACCEPT)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(EclassError::CanvasSessionExpired);
    }
    Ok(response)
}

async fn decode_canvas_prefix(response: reqwest::Response) -> Option<String> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("html") {
        return None;
    }
    let text = response.text().await.ok()?;
    let text = text.trim_start_matches('\u{feff}').trim_start();
    Some(
        text.strip_prefix("while(1);")
            .map_or(text, str::trim_start)
            .to_string(),
    )
}

fn lti_form(html: &str) -> Option<(String, Vec<(String, String)>)> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("form#tool_form, form[action*='learningx']").ok()?;
    let form = document.select(&selector).next()?;
    let action = form.value().attr("action")?.to_string();
    let input = scraper::Selector::parse("input[name]").ok()?;
    let fields = form
        .select(&input)
        .filter_map(|input| {
            let name = input.value().attr("name")?;
            let value = input.value().attr("value").unwrap_or_default();
            Some((
                html_escape::decode_html_entities(name).into_owned(),
                html_escape::decode_html_entities(value).into_owned(),
            ))
        })
        .collect();
    Some((
        html_escape::decode_html_entities(&action).into_owned(),
        fields,
    ))
}

fn positive_id(value: &serde_json::Value) -> Option<u32> {
    match value {
        serde_json::Value::Number(number) => {
            number.as_u64().and_then(|value| u32::try_from(value).ok())
        }
        serde_json::Value::String(text) => text.parse::<u32>().ok(),
        _ => None,
    }
}

/// Ask LearningX for its API token with the saved cookies.
pub async fn ensure_learningx_token(session: &mut Session, base: &Url) {
    let _ = mint_learningx_token(session, base, None, false).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, value: &str, domain: &str) -> Cookie {
        Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain: domain.to_string(),
            path: "/".to_string(),
            expires: None,
            secure: false,
            http_only: true,
            same_site: None,
            host_only: None,
        }
    }

    #[test]
    fn filters_non_cau_cookies_and_deduplicates() {
        let session = Session::from_cdp(
            vec![
                cookie("sid", "a", ".cau.ac.kr"),
                cookie("sid", "b", ".cau.ac.kr"),
                cookie("other", "x", ".example.com"),
                cookie("canvas", "c", "canvas.cau.ac.kr"),
            ],
            None,
        );
        assert_eq!(session.token("sid"), Some("b"));
        assert_eq!(session.token("canvas"), Some("c"));
        assert_eq!(session.token("other"), None);
        assert_eq!(session.cookies.len(), 2);
    }

    #[test]
    fn cookie_header_respects_domain_path_and_expiry() {
        let mut session = Session::from_cdp(
            vec![
                cookie("shared", "yes", ".cau.ac.kr"),
                cookie("host", "yes", "eclass3.cau.ac.kr"),
                cookie("expired", "no", "eclass3.cau.ac.kr"),
            ],
            None,
        );
        if let Some(item) = session
            .cookies
            .iter_mut()
            .find(|item| item.name == "expired")
        {
            item.expires = Some(1.0);
        }
        let url = Url::parse("https://eclass3.cau.ac.kr/api/v1/x").unwrap();
        let header = session.cookie_header(&url).unwrap();
        assert!(header.contains("shared=yes"));
        assert!(header.contains("host=yes"));
        assert!(!header.contains("expired"));
        let other = Url::parse("https://canvas.cau.ac.kr/x").unwrap();
        let header = session.cookie_header(&other).unwrap();
        assert!(header.contains("shared=yes"));
        assert!(!header.contains("host=yes"));
    }

    #[test]
    fn merge_set_cookie_updates_session() {
        let mut session = Session::from_cdp(vec![], None);
        let url = Url::parse("https://eclass3.cau.ac.kr/learningx/main").unwrap();
        session.merge_set_cookie(
            "xn_api_token=token; Path=/; Domain=.cau.ac.kr; Secure; HttpOnly; SameSite=Lax",
            &url,
        );
        assert_eq!(session.token("xn_api_token"), Some("token"));
        assert_eq!(session.cookies.len(), 1);
        let cookie = &session.cookies[0];
        assert!(cookie.secure);
        assert!(cookie.http_only);
        assert_eq!(cookie.same_site.as_deref(), Some("Lax"));
    }

    #[test]
    fn missing_profile_returns_actionable_error() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_browser_cookies(directory.path(), "chrome"),
            Err(EclassError::BrowserProfileUnavailable)
        ));
    }

    #[test]
    fn session_file_round_trips_and_is_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut session = Session::from_cdp(vec![cookie("sid", "value", ".cau.ac.kr")], None);
        save_session(&mut session, &path).unwrap();
        let loaded = load_session(&path).unwrap().unwrap();
        assert_eq!(loaded.token("sid"), Some("value"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
