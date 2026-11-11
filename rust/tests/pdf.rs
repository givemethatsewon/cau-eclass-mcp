use std::path::PathBuf;

use eclass_core::documents::{inspect_pdf, read_pdf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.pdf")
}

#[test]
fn inspects_and_reads_pdf_pages() {
    let info = inspect_pdf(&fixture()).expect("fixture should be a valid PDF");
    assert_eq!(info.page_count, 2);
    assert_eq!(info.content_type, "application/pdf");
    let read = read_pdf(&fixture(), 1, None, 0, 24_000).expect("page text");
    assert_eq!(read.page_count, 2);
    assert_eq!(read.pages.len(), 2);
    assert!(
        read.pages[0].text.contains("Alpha"),
        "{:?}",
        read.pages[0].text
    );
    assert!(
        read.pages[1].text.contains("Beta"),
        "{:?}",
        read.pages[1].text
    );
    assert!(read.document_complete);
}

#[test]
fn rejects_impossible_ranges() {
    assert!(read_pdf(&fixture(), 0, None, 0, 100).is_err());
    assert!(read_pdf(&fixture(), 3, None, 0, 100).is_err());
    assert!(read_pdf(&fixture(), 1, None, 0, 0).is_err());
}
