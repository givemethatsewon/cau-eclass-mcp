//! Errors shared by the Rust core.

use thiserror::Error;

/// A failure that can be returned to an MCP client.
#[derive(Debug, Error)]
pub enum EclassError {
    #[error("e-Class network request failed")]
    Network,
    #[error("e-Class HTTP {0} (session, permission, or unsupported endpoint)")]
    Http(u16),
    #[error("e-Class session expired; run `eclass-mcp login` while the browser is open")]
    SessionExpired,
    #[error("e-Class redirected to login; run `eclass-mcp login`")]
    LoginRedirect,
    #[error("expected JSON was not returned; login may have expired")]
    ExpectedJson,
    #[error("no authenticated user profile was returned")]
    NoProfile,
    #[error("only same-origin relative e-Class paths are allowed")]
    InvalidPath,
    #[error("cross-origin e-Class redirect rejected")]
    CrossOrigin,
    #[error("download did not return a file")]
    NotAFile,
    #[error("file exceeds download limit")]
    DownloadTooLarge,
    #[error("downloaded file size mismatch")]
    SizeMismatch,
    #[error("untrusted download host")]
    UntrustedHost,
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("material was not found in this course's current list")]
    MaterialNotFound,
    #[error("this material is not open")]
    MaterialUnavailable,
    #[error("only Commons PDF items are supported")]
    UnsupportedMaterial,
    #[error("a native original PDF is unavailable; rendered pages are not synthesized")]
    PdfUnavailable,
    #[error("original file is unavailable")]
    VideoUnavailable,
    #[error("only original MP4 downloads are supported")]
    UnsupportedVideo,
    #[error("original download endpoint failed; optionally allow a playback MP4 copy")]
    PlaybackRequired,
    #[error("only a single provided playback MP4 is supported")]
    PlaybackUnsupported,
    #[error("downloaded PDF validation failed (invalid, encrypted, empty, or oversized)")]
    InvalidPdf,
    #[error("requested page range exceeds PDF page count ({0})")]
    PageRange(u32),
    #[error("invalid page range or text offset")]
    InvalidPageRange,
    #[error("invalid max_chars (must be between 1 and 100000)")]
    InvalidMaxChars,
    #[error("unsupported operation in the Rust core: {0}")]
    BrowserOnly(&'static str),
    #[error("OZ PDF helper failed: {0}")]
    OzHelper(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("session file is invalid or unsupported")]
    InvalidSession,
    #[error("no saved e-Class session; run `eclass-mcp login` once")]
    NoSession,
    #[error("Chrome profile cookie store was not found; sign in once in the dedicated profile")]
    BrowserProfileUnavailable,
    #[error(
        "Chrome profile cookie store could not be read; close Chrome if it is locking the profile"
    )]
    CookieStoreUnavailable,
    #[error("the browser is not logged in to e-Class yet")]
    NotLoggedIn,
    #[error("browser executable was not found; pass --browser-path")]
    BrowserNotFound,
    #[error("response exceeds the read limit")]
    ResponseTooLarge,
    #[error("download metadata request failed")]
    MetadataFailed,
    #[error("download metadata exceeds the read limit")]
    MetadataTooLarge,
    #[error("native PDF viewer metadata is unsupported")]
    PdfViewerUnsupported,
    #[error("playback metadata is unsupported")]
    PlaybackMetadataUnsupported,
    #[error("todo list response was not a list")]
    UnexpectedList,
    #[error("too many pending plans")]
    TooManyPlans,
    #[error("session file could not be read; run `eclass-mcp login` again")]
    SessionRead,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Pdf(#[from] lopdf::Error),
}

impl From<reqwest::Error> for EclassError {
    fn from(_: reqwest::Error) -> Self {
        Self::Network
    }
}
