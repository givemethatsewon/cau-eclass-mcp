//! Errors shared by the Rust core.

use serde_json::{Value, json};
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
    #[error(
        "Canvas session expired; run `eclass-mcp login` or `eclass-mcp auth refresh --via aside`"
    )]
    CanvasSessionExpired,
    #[error("LearningX API token is missing; run `eclass-mcp auth refresh --via aside`")]
    LearningxTokenMissing,
    #[error("LearningX rejected the API token; run `eclass-mcp auth refresh --via aside`")]
    LearningxUnauthorized,
    #[error("e-Class redirected to login; run `eclass-mcp login`")]
    LoginRedirect,
    #[error("expected JSON was not returned; login may have expired")]
    ExpectedJson,
    #[error("no authenticated user profile was returned")]
    NoProfile,
    #[error("the account is not allowed to access this resource")]
    PermissionDenied,
    #[error("course was not found; pass course_id or an exact course name")]
    CourseNotFound,
    #[error("course `{query}` matches {count} courses; pass course_id")]
    AmbiguousCourse {
        query: String,
        count: usize,
        candidates: Value,
    },
    #[error("{0}")]
    InvalidArgument(String),
    #[error("submission failed at {step}: {message}")]
    SubmissionStep { step: &'static str, message: String },
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

impl EclassError {
    /// Stable machine-readable error code for MCP clients.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Network => "NETWORK",
            Self::Http(401) => "HTTP_UNAUTHORIZED",
            Self::Http(403) => "PERMISSION_DENIED",
            Self::Http(404) => "NOT_FOUND",
            Self::Http(_) => "HTTP_ERROR",
            Self::SessionExpired => "SESSION_EXPIRED",
            Self::CanvasSessionExpired => "CANVAS_SESSION_EXPIRED",
            Self::LearningxTokenMissing => "LEARNINGX_TOKEN_MISSING",
            Self::LearningxUnauthorized => "LEARNINGX_UNAUTHORIZED",
            Self::LoginRedirect => "LOGIN_REQUIRED",
            Self::ExpectedJson => "EXPECTED_JSON",
            Self::NoProfile => "NO_PROFILE",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::CourseNotFound => "COURSE_NOT_FOUND",
            Self::AmbiguousCourse { .. } => "AMBIGUOUS_COURSE",
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::SubmissionStep { .. } => "SUBMISSION_STEP",
            Self::InvalidPath => "INVALID_ARGUMENT",
            Self::CrossOrigin => "CROSS_ORIGIN",
            Self::NotAFile => "NOT_A_FILE",
            Self::DownloadTooLarge => "DOWNLOAD_TOO_LARGE",
            Self::SizeMismatch => "SIZE_MISMATCH",
            Self::UntrustedHost => "UNTRUSTED_HOST",
            Self::TooManyRedirects => "TOO_MANY_REDIRECTS",
            Self::MaterialNotFound => "MATERIAL_NOT_FOUND",
            Self::MaterialUnavailable => "MATERIAL_UNAVAILABLE",
            Self::UnsupportedMaterial => "UNSUPPORTED_MATERIAL",
            Self::PdfUnavailable => "PDF_UNAVAILABLE",
            Self::VideoUnavailable => "VIDEO_UNAVAILABLE",
            Self::UnsupportedVideo => "UNSUPPORTED_VIDEO",
            Self::PlaybackRequired => "PLAYBACK_REQUIRED",
            Self::PlaybackUnsupported => "PLAYBACK_UNSUPPORTED",
            Self::InvalidPdf => "INVALID_PDF",
            Self::PageRange(_) => "PAGE_RANGE",
            Self::InvalidPageRange => "INVALID_ARGUMENT",
            Self::InvalidMaxChars => "INVALID_ARGUMENT",
            Self::BrowserOnly(_) => "BROWSER_ONLY",
            Self::OzHelper(_) => "OZ_HELPER",
            Self::UnknownTool(_) => "UNKNOWN_TOOL",
            Self::InvalidSession => "INVALID_SESSION",
            Self::NoSession => "NO_SESSION",
            Self::BrowserProfileUnavailable => "BROWSER_PROFILE_UNAVAILABLE",
            Self::CookieStoreUnavailable => "COOKIE_STORE_UNAVAILABLE",
            Self::NotLoggedIn => "NOT_LOGGED_IN",
            Self::BrowserNotFound => "BROWSER_NOT_FOUND",
            Self::ResponseTooLarge => "RESPONSE_TOO_LARGE",
            Self::MetadataFailed => "METADATA_FAILED",
            Self::MetadataTooLarge => "METADATA_TOO_LARGE",
            Self::PdfViewerUnsupported => "PDF_VIEWER_UNSUPPORTED",
            Self::PlaybackMetadataUnsupported => "PLAYBACK_METADATA_UNSUPPORTED",
            Self::UnexpectedList => "UNEXPECTED_LIST",
            Self::TooManyPlans => "TOO_MANY_PLANS",
            Self::SessionRead => "SESSION_READ",
            Self::Io(_) => "IO",
            Self::Json(_) => "JSON",
            Self::Url(_) => "URL",
            Self::Pdf(_) => "PDF",
        }
    }

    /// Suggested recovery command, when one exists.
    #[must_use]
    pub fn next_action(&self) -> Option<&'static str> {
        match self {
            Self::SessionExpired
            | Self::CanvasSessionExpired
            | Self::LoginRedirect
            | Self::NoSession
            | Self::NotLoggedIn
            | Self::NoProfile => Some("eclass-mcp login"),
            Self::LearningxTokenMissing | Self::LearningxUnauthorized => {
                Some("eclass-mcp auth refresh --via aside")
            }
            Self::BrowserProfileUnavailable | Self::CookieStoreUnavailable | Self::SessionRead => {
                Some("eclass-mcp login --no-launch")
            }
            Self::BrowserNotFound => Some("eclass-mcp login --browser-path <path>"),
            Self::CourseNotFound => Some("use list_courses to find a course_id"),
            Self::AmbiguousCourse { .. } => {
                Some("pass course_id from error.details.candidates[].course_id")
            }
            Self::MaterialNotFound => {
                Some("call list_materials and pass its resource_id unchanged")
            }
            Self::Http(404) => {
                Some("verify the course_id or resource id with list_courses or list_materials")
            }
            _ => None,
        }
    }

    /// Extra machine-readable context for the error payload.
    #[must_use]
    pub fn details(&self) -> Option<Value> {
        match self {
            Self::AmbiguousCourse { candidates, .. } => Some(json!({"candidates": candidates})),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for EclassError {
    fn from(_: reqwest::Error) -> Self {
        Self::Network
    }
}
