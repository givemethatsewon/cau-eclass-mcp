//! Browser-session persistence and Chromium cookie harvesting.
//!
//! The file format is compatible with the Python build: `.cau.ac.kr` cookies,
//! an optional CDP endpoint, and a `version: 1` envelope.

use std::{
    collections::BTreeMap,
    env, fs,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::connect_async;
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
    pub cookies: Vec<Cookie>,
}

impl Session {
    /// Build a session, keeping only CAU cookies and preserving a deterministic order.
    #[must_use]
    pub fn from_cdp(cookies: Vec<Cookie>, endpoint: Option<String>) -> Self {
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
            endpoint,
            cookies: values.into_values().collect(),
        }
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

fn is_cau_domain(value: &str) -> bool {
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

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_home(path: &Path) -> PathBuf {
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

/// Validate a CDP endpoint: loopback only, explicit port, no credentials or query.
pub fn validate_endpoint(raw: &str) -> Result<String, EclassError> {
    let url = Url::parse(raw).map_err(|_| EclassError::InvalidEndpoint)?;
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https" | "ws" | "wss") {
        return Err(EclassError::InvalidEndpoint);
    }
    let host = url.host_str().ok_or(EclassError::InvalidEndpoint)?;
    let bare_host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || bare_host
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false);
    if !loopback
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(EclassError::InvalidEndpoint);
    }
    Ok(raw.to_string())
}

/// The configured CDP endpoint.
pub fn session_endpoint() -> Result<String, EclassError> {
    validate_endpoint(
        &env::var("ECLASS_CDP_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:9222".to_string()),
    )
}

/// Convert a WebSocket CDP URL to the HTTP URL used by `/json/version`.
fn http_endpoint(raw: &str) -> Result<Url, EclassError> {
    let mut url = Url::parse(raw).map_err(|_| EclassError::InvalidEndpoint)?;
    match url.scheme() {
        "ws" => {
            url.set_scheme("http")
                .map_err(|_| EclassError::InvalidEndpoint)?;
        }
        "wss" => {
            url.set_scheme("https")
                .map_err(|_| EclassError::InvalidEndpoint)?;
        }
        "http" | "https" => {}
        _ => return Err(EclassError::InvalidEndpoint),
    }
    url.set_path("");
    Ok(url)
}

/// Read cookies from a local Chromium debugging endpoint, including HttpOnly cookies.
pub async fn harvest_cdp(endpoint: &str) -> Result<Vec<Cookie>, EclassError> {
    let endpoint = validate_endpoint(endpoint)?;
    let http = http_endpoint(&endpoint)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()?;
    let version: serde_json::Value = client
        .get(
            http.join("json/version")
                .map_err(|_| EclassError::CdpUnavailable)?,
        )
        .send()
        .await
        .map_err(|_| EclassError::CdpUnavailable)?
        .json()
        .await
        .map_err(|_| EclassError::CdpUnavailable)?;
    let websocket = version
        .get("webSocketDebuggerUrl")
        .and_then(serde_json::Value::as_str)
        .ok_or(EclassError::CdpUnavailable)?;
    let (mut socket, _) = connect_async(websocket)
        .await
        .map_err(|_| EclassError::CdpUnavailable)?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({"id": 1, "method": "Storage.getCookies", "params": {}})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| EclassError::CdpProtocol)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| EclassError::CdpProtocol)?
            .ok_or(EclassError::CdpProtocol)?
            .map_err(|_| EclassError::CdpProtocol)?;
        let text = message.into_text().map_err(|_| EclassError::CdpProtocol)?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| EclassError::CdpProtocol)?;
        if value.get("id").and_then(serde_json::Value::as_u64) != Some(1) {
            continue;
        }
        if value.get("error").is_some() {
            return Err(EclassError::CdpProtocol);
        }
        let cookies = value
            .get("result")
            .and_then(|result| result.get("cookies"))
            .and_then(serde_json::Value::as_array)
            .ok_or(EclassError::CdpProtocol)?;
        return cookies
            .iter()
            .map(|cookie| {
                serde_json::from_value(cookie.clone()).map_err(|_| EclassError::CdpProtocol)
            })
            .collect();
    }
}

/// Poll an open browser until the user has logged in.
pub async fn login_via_cdp(
    endpoint: &str,
    timeout: std::time::Duration,
    wait_for_browser: bool,
) -> Result<Session, EclassError> {
    let endpoint = validate_endpoint(endpoint)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_error = EclassError::NotLoggedIn;
    while tokio::time::Instant::now() < deadline {
        match harvest_cdp(&endpoint).await {
            Ok(cookies) => {
                let mut session = Session::from_cdp(cookies, Some(endpoint.clone()));
                if verify_session(&session).await {
                    ensure_learningx_token(&mut session, Some(&endpoint)).await;
                    return Ok(session);
                }
                last_error = EclassError::NotLoggedIn;
            }
            Err(error) => {
                if !wait_for_browser && matches!(error, EclassError::CdpUnavailable) {
                    return Err(error);
                }
                last_error = error;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err(last_error)
}

/// Reuse an already open login browser; fail promptly when it is absent.
pub async fn refresh_session(endpoint: &str) -> Result<Session, EclassError> {
    let endpoint = validate_endpoint(endpoint)?;
    let cookies = harvest_cdp(&endpoint).await?;
    let mut session = Session::from_cdp(cookies, Some(endpoint.clone()));
    if !verify_session(&session).await {
        return Err(EclassError::NotLoggedIn);
    }
    ensure_learningx_token(&mut session, Some(&endpoint)).await;
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

/// Ask LearningX for its API token when the login flow has not set it yet.
pub async fn ensure_learningx_token(session: &mut Session, endpoint: Option<&str>) {
    if session.token("xn_api_token").is_some() {
        return;
    }
    let Ok(base) = Url::parse(ORIGIN) else {
        return;
    };
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    for path in ["learningx/main", "learningx/dashboard"] {
        let Ok(url) = base.join(path) else {
            continue;
        };
        let Some(cookie) = session.cookie_header(&url) else {
            break;
        };
        let Ok(response) = client
            .get(url.clone())
            .header("Accept", JSON_ACCEPT)
            .header("User-Agent", USER_AGENT)
            .header("Cookie", cookie)
            .send()
            .await
        else {
            continue;
        };
        for header in response.headers().get_all(reqwest::header::SET_COOKIE) {
            if let Ok(value) = header.to_str() {
                session.merge_set_cookie(value, &url);
            }
        }
        if session.token("xn_api_token").is_some() {
            break;
        }
    }
    if session.token("xn_api_token").is_some() {
        return;
    }
    let Some(endpoint) = endpoint else {
        return;
    };
    let Ok(Some(target)) = open_learningx_tab(endpoint).await else {
        return;
    };
    for _ in 0..5 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if let Ok(cookies) = harvest_cdp(endpoint).await {
            for cookie in cookies {
                if is_cau_domain(&cookie.domain) {
                    session.upsert(cookie);
                }
            }
        }
        if session.token("xn_api_token").is_some() {
            break;
        }
    }
    close_tab(endpoint, &target).await;
}

async fn open_learningx_tab(endpoint: &str) -> Result<Option<String>, EclassError> {
    let http = http_endpoint(endpoint)?;
    let mut url = http.join("json/new")?;
    url.set_query(Some("https://eclass3.cau.ac.kr/learningx/main"));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()?;
    let response = match client.put(url.clone()).send().await {
        Ok(response) if response.status().is_success() => response,
        _ => client
            .get(url)
            .send()
            .await
            .map_err(|_| EclassError::CdpUnavailable)?,
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| EclassError::CdpProtocol)?;
    Ok(value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

async fn close_tab(endpoint: &str, target: &str) {
    let Ok(http) = http_endpoint(endpoint) else {
        return;
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return;
    };
    if let Ok(url) = http.join(&format!("json/close/{target}")) {
        let _ = client.get(url).send().await;
    }
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
    fn rejects_remote_endpoints() {
        for endpoint in [
            "https://example.com:9222",
            "http://127.0.0.1:9222?token=private",
            "http://user:pass@localhost:9222",
            "file:///tmp/devtools",
            "http://0.0.0.0:9222",
        ] {
            assert!(validate_endpoint(endpoint).is_err(), "{endpoint}");
        }
        assert!(validate_endpoint("http://127.0.0.1:9222").is_ok());
        assert!(validate_endpoint("ws://[::1]:9222/devtools/browser/test").is_ok());
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
