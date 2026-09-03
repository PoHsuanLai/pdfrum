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

// ---- the writers: the oracle's add, set, describe and delete assertions,
// read back through a save and a reopen.

use pdfrum::SaveOptions;
use std::sync::Arc;

fn saved(edit: &pdfrum::DocEdit<'_>) -> pdfrum::Result<Document> {
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())?;
    Document::from_bytes(Arc::from(out))
}

#[test]
fn an_added_attachment_is_sorted_by_name_and_carries_its_bytes() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    assert_eq!(edit.add_attachment("0.txt", b"Hello!").unwrap(), 0);
    assert_eq!(edit.add_attachment("z.txt", b"World!").unwrap(), 3);
    let doc = saved(&edit).unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 4);
    assert_eq!(attachments[0].file_name(), "0.txt");
    assert_eq!(attachments[0].data().unwrap(), b"Hello!");
    assert_eq!(attachments[3].file_name(), "z.txt");
    assert_eq!(attachments[3].data().unwrap(), b"World!");
}

#[test]
fn an_added_attachment_takes_a_date_a_checksum_and_an_empty_file() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    let index = edit.add_attachment("5.txt", b"Hello World!").unwrap();
    assert_eq!(index, 1);
    assert!(
        edit.set_attachment_param(index, "CreationDate", "D:20170720161527-04'00'")
            .unwrap()
    );
    assert!(
        edit.set_attachment_param(index, "CheckSum", "<ABCDEF01234567899876543210FEDCBA>")
            .unwrap()
    );
    {
        let doc = saved(&edit).unwrap();
        let attachment = &doc.attachments()[1];
        assert_eq!(attachment.file_name(), "5.txt");
        assert_eq!(attachment.data().unwrap(), b"Hello World!");
        assert_eq!(
            attachment.param("CreationDate").as_deref(),
            Some("D:20170720161527-04'00'")
        );
        assert_eq!(
            attachment.param("CheckSum").as_deref(),
            Some("<ABCDEF01234567899876543210FEDCBA>")
        );
    }
    // An empty file, and the checksum the writer computes for it.
    assert!(edit.set_attachment_file(index, b"").unwrap());
    let doc = saved(&edit).unwrap();
    let attachment = &doc.attachments()[1];
    assert_eq!(attachment.data().unwrap(), b"");
    assert_eq!(
        attachment.param("CheckSum").as_deref(),
        Some("<D41D8CD98F00B204E9800998ECF8427E>")
    );
}

#[test]
fn a_document_without_attachments_grows_a_tree() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    assert_eq!(edit.add_attachment("0.txt", b"Hello!").unwrap(), 0);
    assert_eq!(edit.add_attachment("z.txt", b"World!").unwrap(), 1);
    let doc = saved(&edit).unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].file_name(), "0.txt");
    assert_eq!(attachments[0].data().unwrap(), b"Hello!");
    assert_eq!(attachments[1].file_name(), "z.txt");
    assert_eq!(attachments[1].data().unwrap(), b"World!");
}

#[test]
fn a_non_ascii_param_round_trips() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    let index = edit
        .add_attachment("attachment.txt", b"Test Contents")
        .unwrap();
    assert!(
        edit.set_attachment_param(index, "test", "\u{f6}\u{e4}")
            .unwrap()
    );
    let doc = saved(&edit).unwrap();
    assert_eq!(
        doc.attachments()[0].param("test").as_deref(),
        Some("\u{f6}\u{e4}")
    );
}

#[test]
fn deleting_the_first_attachment_promotes_the_second() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    assert!(edit.delete_attachment(0).unwrap());
    assert!(!edit.delete_attachment(5).unwrap());
    let doc = saved(&edit).unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].file_name(), "attached.pdf");
}

#[test]
fn an_attachment_without_data_can_still_be_deleted() {
    let doc = open("embedded_attachments_invalid_data").unwrap();
    let mut edit = doc.edit();
    assert!(edit.delete_attachment(0).unwrap());
    assert!(saved(&edit).unwrap().attachments().is_empty());
}

#[test]
fn a_description_is_written_as_text() {
    let doc = open("embedded_attachments_with_desc").unwrap();
    let mut edit = doc.edit();
    assert!(edit.set_attachment_description(0, "\u{fc}ber").unwrap());
    let doc = saved(&edit).unwrap();
    assert_eq!(doc.attachments()[0].description(), "\u{fc}ber");
}
