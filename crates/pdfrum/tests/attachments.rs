//! The oracle's attachment reader assertions: names, bytes, `/Params` values
//! and their presentation, MIME subtype, and descriptions of every shape.

use pdfrum::Document;

fn open(name: &str) -> pdfrum::Result<Document> {
    Document::open(format!("tests/fixtures/{name}.pdf"))
}

#[test]
fn two_attachments_are_listed_with_their_names_bytes_and_params() {
    let doc = open("embedded_attachments").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 2);

    let first = &attachments[0];
    assert_eq!(first.file_name(), "1.txt");
    assert_eq!(first.data().unwrap(), b"test");
    assert!(!first.has_param("none"));
    assert_eq!(first.param("none"), None);
    // A number is present but is not text.
    assert!(first.has_param("Size"));
    assert_eq!(first.param("Size").as_deref(), Some(""));
    assert_eq!(
        first.param("CreationDate").as_deref(),
        Some("D:20170712214438-07'00'")
    );

    let second = &attachments[1];
    assert_eq!(second.data().unwrap().len(), 5869);
    assert_eq!(
        second.param("CheckSum").as_deref(),
        Some("<72AFCDDEDF554DDA63C0C88E06F1CE18>")
    );
}

#[test]
fn a_document_without_attachments_lists_none() {
    assert!(open("hello_world").unwrap().attachments().is_empty());
}

#[test]
fn an_attachment_without_an_embedded_file_has_a_name_and_no_data() {
    let doc = open("embedded_attachments_invalid_data").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].file_name(), "1.txt");
    assert!(attachments[0].data().is_none());
}

#[test]
fn a_param_that_is_a_name_reads_as_its_text_and_a_stream_as_nothing() {
    let doc = open("embedded_attachments_invalid_types").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].param("CheckSum").as_deref(), Some("Bad"));
    assert_eq!(attachments[1].param("CheckSum").as_deref(), Some(""));
}

#[test]
fn the_subtype_is_the_embedded_files_mime_type() {
    let doc = open("embedded_attachments").unwrap();
    assert_eq!(
        doc.attachments()[0].subtype().as_deref(),
        Some("text/plain")
    );
}

#[test]
fn a_description_is_its_text_and_anything_else_is_empty() {
    let doc = open("embedded_attachments_with_desc").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 4);
    assert_eq!(attachments[0].description(), "Hello, World!");
    assert_eq!(attachments[1].description(), "", "absent");
    assert_eq!(attachments[2].description(), "", "a number");
    assert_eq!(attachments[3].description(), "", "empty");
}
