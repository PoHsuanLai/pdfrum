//! Replacing an attachment's bytes keeps what the caller says to keep, and
//! renaming one moves the key and the specification's own names together.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use pdfrum::{AttachmentOptions, Document, SaveOptions};

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("fixture")
}

fn described() -> AttachmentOptions {
    AttachmentOptions::builder()
        .description("The source data")
        .mime_type("text/csv")
        .modified("D:20240102030405Z")
        .build()
}

fn save(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(bytes).expect("reload")
}

/// A document carrying one described attachment.
fn with_one(doc: &Document) -> Document {
    let mut edit = doc.edit();
    edit.add_attachment("data.csv", b"a,b\n1,2\n", &described())
        .expect("add");
    save(&edit)
}

#[test]
fn the_plain_replace_drops_the_mime_type_and_the_date() {
    let doc = with_one(&hello());
    let mut edit = doc.edit();
    assert!(
        edit.set_attachment_file(0, b"x,y\n3,4\n").expect("replace"),
        "there is an attachment at 0",
    );
    let saved = save(&edit);

    let attachment = &saved.attachments()[0];
    assert_eq!(attachment.data().expect("data"), b"x,y\n3,4\n");
    // Documented behaviour, pinned so the overload below has something to be
    // an improvement on. The reader answers `Some("")` rather than `None` for
    // a stream that carries no `/Subtype` at all — the `None` is reserved for
    // an attachment with no stream.
    assert_eq!(attachment.subtype().as_deref(), Some(""));
    assert_eq!(attachment.param("ModDate"), None);
}

#[test]
fn the_overload_carries_the_mime_type_and_the_date_across() {
    let doc = with_one(&hello());
    let mut edit = doc.edit();
    assert!(
        edit.set_attachment_file_with(0, b"x,y\n3,4\n", &described())
            .expect("replace"),
        "there is an attachment at 0",
    );
    let saved = save(&edit);

    let attachment = &saved.attachments()[0];
    assert_eq!(attachment.data().expect("data"), b"x,y\n3,4\n");
    assert_eq!(attachment.subtype().as_deref(), Some("text/csv"));
    assert_eq!(
        attachment.param("ModDate").as_deref(),
        Some("D:20240102030405Z"),
    );
    assert_eq!(attachment.description(), "The source data");
}

#[test]
fn replacing_an_attachment_that_is_not_there_answers_false() {
    let doc = hello();
    let mut edit = doc.edit();
    assert!(
        !edit
            .set_attachment_file_with(0, b"bytes", &described())
            .expect("no error"),
        "an empty document has no attachment 0",
    );
}

#[test]
fn a_rename_moves_the_key_and_the_specification_names_together() {
    let doc = with_one(&hello());
    let mut edit = doc.edit();
    assert!(edit.set_attachment_name(0, "renamed.csv").expect("rename"));
    let saved = save(&edit);

    let attachments = saved.attachments();
    assert_eq!(attachments.len(), 1);
    // The reader answers from the specification's `/UF`, and the tree is
    // keyed by the name; a rename that moved only one would leave a document
    // that cannot be saved under the name it displays.
    assert_eq!(attachments[0].file_name(), "renamed.csv");
    assert_eq!(attachments[0].data().expect("data"), b"a,b\n1,2\n");
}

#[test]
fn a_rename_to_the_name_it_already_has_is_a_no_op() {
    let doc = with_one(&hello());
    let mut edit = doc.edit();
    assert!(edit.set_attachment_name(0, "data.csv").expect("rename"));
    let saved = save(&edit);
    assert_eq!(saved.attachments().len(), 1);
    assert_eq!(saved.attachments()[0].file_name(), "data.csv");
}

#[test]
fn a_rename_onto_another_attachment_is_refused() {
    let doc = with_one(&hello());
    let mut edit = doc.edit();
    edit.add_attachment("other.csv", b"z\n", &described())
        .expect("add a second");
    let doc = save(&edit);

    let mut edit = doc.edit();
    // The tree is keyed by name, so writing a duplicate would leave one of the
    // two unreachable by the only lookup there is.
    assert!(edit.set_attachment_name(0, "other.csv").is_err());
}

#[test]
fn renaming_an_attachment_that_is_not_there_answers_false() {
    let doc = hello();
    let mut edit = doc.edit();
    assert!(!edit.set_attachment_name(0, "x.txt").expect("no error"));
}
