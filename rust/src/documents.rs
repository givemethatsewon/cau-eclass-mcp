//! Validate downloaded PDFs and return bounded, resumable page text.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use lopdf::Document;
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::error::EclassError;

/// Maximum accepted PDF size: 64 MiB.
pub const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

/// Verified metadata for a downloaded PDF.
#[derive(Debug, Clone, Serialize)]
pub struct PdfInfo {
    pub bytes: u64,
    pub sha256: String,
    pub page_count: u32,
    pub content_type: &'static str,
}

/// One page chunk in a `read_material` response.
#[derive(Debug, Clone, Serialize)]
pub struct PdfPage {
    pub page: u32,
    pub text: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub total_chars: usize,
}

/// Resumable page-text response.
#[derive(Debug, Clone, Serialize)]
pub struct PdfRead {
    pub page_count: u32,
    pub start_page: u32,
    pub end_page: u32,
    pub pages: Vec<PdfPage>,
    pub pages_without_text: Vec<u32>,
    pub text_error_pages: Vec<u32>,
    pub range_complete: bool,
    pub text_complete: bool,
    pub document_complete: bool,
    pub has_more: bool,
    /// Page to request next; pair it with `next_offset`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<u32>,
    /// Character offset within `next_page`; omitted when nothing remains.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub warnings: Vec<String>,
}

/// Validate a PDF file and return its size, SHA-256 and page count.
///
/// # Errors
///
/// Returns [`EclassError::InvalidPdf`] when the file is empty, oversized,
/// encrypted, unparsable, or missing the `%PDF-` signature.
pub fn inspect_pdf(path: &Path) -> Result<PdfInfo, EclassError> {
    let metadata = std::fs::metadata(path)?;
    let bytes = metadata.len();
    if bytes == 0 || bytes > MAX_DOCUMENT_BYTES {
        return Err(EclassError::InvalidPdf);
    }
    let mut file = File::open(path)?;
    let mut signature = [0_u8; 5];
    file.read_exact(&mut signature)?;
    if &signature != b"%PDF-" {
        return Err(EclassError::InvalidPdf);
    }
    let mut hasher = Sha256::new();
    let mut stream = File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let document = Document::load(path)?;
    if document.is_encrypted() {
        return Err(EclassError::InvalidPdf);
    }
    let page_count =
        u32::try_from(document.get_pages().len()).map_err(|_| EclassError::InvalidPdf)?;
    if page_count == 0 {
        return Err(EclassError::InvalidPdf);
    }
    Ok(PdfInfo {
        bytes,
        sha256: hex::encode(hasher.finalize()),
        page_count,
        content_type: "application/pdf",
    })
}

/// Read a page range with a character budget and an optional continuation offset.
///
/// # Errors
///
/// Returns [`EclassError::InvalidPageRange`] for an impossible range or offset,
/// [`EclassError::InvalidMaxChars`] outside `1..=100000`, and
/// [`EclassError::PageRange`] when the request exceeds the document.
pub fn read_pdf(
    path: &Path,
    start_page: u32,
    end_page: Option<u32>,
    start_offset: usize,
    max_chars: usize,
) -> Result<PdfRead, EclassError> {
    if start_page == 0 || end_page.is_some_and(|end| end < start_page) {
        return Err(EclassError::InvalidPageRange);
    }
    if !(1..=100_000).contains(&max_chars) {
        return Err(EclassError::InvalidMaxChars);
    }
    let document = Document::load(path)?;
    let page_count =
        u32::try_from(document.get_pages().len()).map_err(|_| EclassError::InvalidPageRange)?;
    let stop = end_page.unwrap_or_else(|| (start_page + 9).min(page_count));
    if start_page > page_count || stop > page_count {
        return Err(EclassError::PageRange(page_count));
    }

    let mut pages = Vec::new();
    let mut blank = Vec::new();
    let mut failed = Vec::new();
    let mut remaining = max_chars;
    let mut next_page = None;
    let mut next_offset = None;

    for number in start_page..=stop {
        let text = match document.extract_text_chunks(&[number]) {
            chunks if chunks.iter().any(Result::is_err) => {
                failed.push(number);
                String::new()
            }
            chunks => chunks
                .into_iter()
                .filter_map(Result::ok)
                .collect::<Vec<_>>()
                .join(""),
        };
        let text = sanitize_text(&text);
        let characters: Vec<char> = text.chars().collect();
        let total = characters.len();
        let offset = if number == start_page {
            start_offset
        } else {
            0
        };
        if offset > total {
            return Err(EclassError::InvalidPageRange);
        }
        if text.trim().is_empty() {
            blank.push(number);
        }
        let end = total.min(offset.saturating_add(remaining));
        let chunk: String = characters[offset..end].iter().collect();
        remaining = remaining.saturating_sub(chunk.chars().count());
        pages.push(PdfPage {
            page: number,
            text: chunk,
            start_offset: offset,
            end_offset: end,
            total_chars: total,
        });
        if end < total {
            next_page = Some(number);
            next_offset = Some(end);
            break;
        }
        if remaining == 0 && number < stop {
            next_page = Some(number + 1);
            next_offset = Some(0);
            break;
        }
    }

    let range_complete = next_page.is_none();
    if range_complete && stop < page_count {
        next_page = Some(stop + 1);
        next_offset = Some(0);
    }
    let mut warnings = Vec::new();
    if !blank.is_empty() {
        warnings
            .push("Some pages have no extracted text; diagrams/scans are not OCRed".to_string());
    }
    if !failed.is_empty() {
        warnings.push("Text extraction failed on some pages".to_string());
    }
    let text_complete =
        range_complete && blank.is_empty() && failed.is_empty() && start_offset == 0;
    Ok(PdfRead {
        page_count,
        start_page,
        end_page: stop,
        pages,
        pages_without_text: blank,
        text_error_pages: failed,
        range_complete,
        text_complete,
        document_complete: start_page == 1
            && stop == page_count
            && range_complete
            && start_offset == 0
            && text_complete,
        has_more: next_page.is_some(),
        next_page,
        next_offset,
        warnings,
    })
}

/// Redact sensitive URL query values before text reaches an MCP response.
#[must_use]
pub fn sanitize_text(value: &str) -> String {
    let Ok(pattern) = Regex::new(r#"https?://[^\s<>"']+"#) else {
        return value.to_string();
    };
    pattern
        .replace_all(value, |captures: &regex::Captures<'_>| {
            safe_url(captures.get(0).map_or("", |value| value.as_str()))
        })
        .into_owned()
}

fn safe_url(value: &str) -> String {
    let decoded = html_escape::decode_html_entities(value).into_owned();
    let Ok(mut url) = Url::parse(&decoded) else {
        return value.to_string();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return "[unsupported URL]".to_string();
    }
    let public = [
        "wrap",
        "page",
        "per_page",
        "include[]",
        "start_date",
        "end_date",
        "ozr_path",
        "ozr_nm",
        "param_odi",
        "param_form",
    ];
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| {
            if public.contains(&key.as_ref()) {
                (key.into_owned(), value.into_owned())
            } else {
                (key.into_owned(), "[REDACTED]".to_string())
            }
        })
        .collect();
    if !pairs.is_empty() {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(&key, &value);
        }
        url.set_query(Some(&serializer.finish()));
    }
    url.set_username("").ok();
    url.set_password(None).ok();
    url.set_fragment(None);
    url.to_string()
}

/// Return a safe PDF path from a caller-supplied directory and unique name.
#[must_use]
pub fn pdf_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(name)
}
