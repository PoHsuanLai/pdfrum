//! The `/Info` dictionary through a save: written where the trailer names
//! it, in a full rewrite and in an incremental append.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_edit::{EditDoc, IdSource, SaveMode, SaveOptions, save, set_info_entry};
use pdfrum_object::names;
use pdfrum_parser::{Document, LoadOptions, load};

const HELLO: &[u8] = include_bytes!("files/hello.pdf");

fn open(bytes: &[u8]) -> Document {
    load(Arc::from(bytes), &LoadOptions::default()).expect("opens")
}

fn commit(edit: &EditDoc<'_>, mode: SaveMode) -> Vec<u8> {
    let options = SaveOptions {
        mode,
        id_source: IdSource::Fixed([0x33; 16]),
        ..SaveOptions::default()
    };
    let mut out = Vec::new();
    save(edit, &options, &mut out).expect("saves");
    out
}

fn info_text(doc: &Document, key: &pdfrum_object::Name) -> Option<String> {
    doc.trailer().dict(names::INFO, doc)?.text(key, doc)
}

#[test]
fn a_document_without_an_info_saves_one_the_trailer_names() {
    let doc = open(HELLO);
    assert!(!doc.trailer().contains_key(names::INFO), "fixture premise");
    let mut edit = EditDoc::new(&doc);
    set_info_entry(&mut edit, names::TITLE, Some("Hello"));
    set_info_entry(&mut edit, names::AUTHOR, Some("\u{7f51}\u{9875}"));

    let saved = open(&commit(&edit, SaveMode::Full));
    assert_eq!(info_text(&saved, names::TITLE).as_deref(), Some("Hello"));
    assert_eq!(
        info_text(&saved, names::AUTHOR).as_deref(),
        Some("\u{7f51}\u{9875}")
    );
}

#[test]
fn an_incremental_save_appends_the_info_and_its_trailer_entry() {
    let doc = open(HELLO);
    let mut edit = EditDoc::new(&doc);
    set_info_entry(&mut edit, names::SUBJECT, Some("Appended"));

    let bytes = commit(&edit, SaveMode::Incremental);
    assert!(
        bytes.starts_with(HELLO),
        "the original bytes are the prefix"
    );
    let saved = open(&bytes);
    assert_eq!(
        info_text(&saved, names::SUBJECT).as_deref(),
        Some("Appended")
    );
}

#[test]
fn a_second_session_edits_the_info_the_first_one_wrote() {
    let doc = open(HELLO);
    let mut edit = EditDoc::new(&doc);
    set_info_entry(&mut edit, names::TITLE, Some("First"));
    set_info_entry(&mut edit, names::KEYWORDS, Some("a, b"));
    let first = open(&commit(&edit, SaveMode::Full));

    let mut edit = EditDoc::new(&first);
    set_info_entry(&mut edit, names::TITLE, None);
    let second = open(&commit(&edit, SaveMode::Full));
    assert_eq!(info_text(&second, names::TITLE), None, "removed");
    assert_eq!(
        info_text(&second, names::KEYWORDS).as_deref(),
        Some("a, b"),
        "kept"
    );
}

#[test]
fn a_reproducible_save_with_an_info_is_reproducible() {
    let doc = open(HELLO);
    let mut edit = EditDoc::new(&doc);
    set_info_entry(&mut edit, names::TITLE, Some("Same"));
    assert_eq!(commit(&edit, SaveMode::Full), commit(&edit, SaveMode::Full));
}
