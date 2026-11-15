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

#[test]
fn reads_ucs2_text_from_oz_fonts_without_embedded_unicode_maps() {
    use lopdf::{Document, Stream, dictionary};

    let mut pdf = Document::with_version("1.5");
    let pages = pdf.new_object_id();
    let font_korean = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Gulim",
        "Encoding" => "UniKS-UCS2-H"
    });
    let font_japanese = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "MS-Gothic",
        "Encoding" => "UniJIS-UCS2-H"
    });
    let content = pdf.add_object(Stream::new(
        dictionary! {},
        b"BT /Ko 12 Tf <C218C5C5ACC4D68D> Tj ET BT /Ja 12 Tf <65E5672C8A9E> Tj ET".to_vec(),
    ));
    let page = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages,
        "Resources" => dictionary! { "Font" => dictionary! { "Ko" => font_korean, "Ja" => font_japanese } },
        "Contents" => content
    });
    pdf.set_object(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
    );
    let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    pdf.trailer.set("Root", catalog);
    let directory = tempfile::tempdir().unwrap();
    let pdf_path = directory.path().join("oz.pdf");
    pdf.save(&pdf_path).unwrap();

    let result = read_pdf(&pdf_path, 1, None, 0, 24_000).unwrap();
    assert!(
        result.pages[0].text.contains("수업"),
        "{:?}",
        result.pages[0].text
    );
    assert!(
        result.pages[0].text.contains("日本語"),
        "{:?}",
        result.pages[0].text
    );
    assert!(result.document_complete);
}
