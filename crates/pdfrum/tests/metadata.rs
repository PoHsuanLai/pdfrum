//! `DocEdit::set_metadata`: the `/Info` dictionary written, reopened through
//! our own parser and — when the checkout is there — through the oracle's
//! `pdfium_test --show-metadata`.

#![expect(
    clippy::unwrap_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pdfrum::{Document, IdSource, Metadata, SaveOptions};

const HELLO: &str = "tests/fixtures/hello_world.pdf";

fn reproducible() -> SaveOptions {
    let mut options = SaveOptions::default();
    options.id_source = IdSource::Fixed([0x44; 16]);
    options
}

fn saved(edit: &pdfrum::DocEdit<'_>, options: &SaveOptions) -> Vec<u8> {
    let mut out = Vec::new();
    edit.write_to(&mut out, options).unwrap();
    out
}

fn reopen(bytes: Vec<u8>) -> Document {
    Document::from_bytes(Arc::from(bytes)).unwrap()
}

fn full() -> Metadata {
    let mut metadata = Metadata::default();
    metadata.title = Some("A Title".into());
    metadata.author = Some("An Author".into());
    metadata.subject = Some("A Subject".into());
    metadata.keywords = Some("one, two".into());
    metadata.creator = Some("A Creator".into());
    metadata.producer = Some("pdfrum".into());
    metadata.creation_date = Some("D:20260905100000Z00'00'".into());
    metadata.modification_date = Some("D:20260905110000Z00'00'".into());
    metadata
}

#[test]
fn every_field_round_trips_and_a_reproducible_save_keeps_the_given_mod_date() {
    let doc = Document::open(HELLO).unwrap();
    assert_eq!(doc.metadata(), Metadata::default(), "fixture premise");
    let mut edit = doc.edit();
    edit.set_metadata(&full());
    let saved = reopen(saved(&edit, &reproducible()));
    assert_eq!(saved.metadata(), full());
}

#[test]
fn a_default_save_stamps_mod_date_with_its_own_time() {
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.set_metadata(&full());
    let saved = reopen(saved(&edit, &SaveOptions::default()));
    let stamped = saved.metadata().modification_date.unwrap();
    assert_ne!(stamped, "D:20260905110000Z00'00'", "not the value given");
    assert_eq!(stamped.len(), "D:20260905110000Z00'00'".len());
    assert!(stamped.starts_with("D:20"), "got {stamped}");
    assert!(stamped.ends_with("Z00'00'"), "got {stamped}");
    // Everything else is as given: the stamp touches one key.
    let mut expected = full();
    expected.modification_date = Some(stamped);
    assert_eq!(saved.metadata(), expected);
}

#[test]
fn a_none_or_empty_field_removes_its_key() {
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.set_metadata(&full());
    let first = reopen(saved(&edit, &reproducible()));

    let mut metadata = first.metadata();
    metadata.title = None;
    metadata.keywords = Some(String::new());
    let mut edit = first.edit();
    edit.set_metadata(&metadata);
    let second = reopen(saved(&edit, &reproducible()));

    let mut expected = full();
    expected.title = None;
    expected.keywords = None;
    assert_eq!(second.metadata(), expected);
}

#[test]
fn text_outside_pdfdoc_encoding_round_trips() {
    let doc = Document::open(HELLO).unwrap();
    let mut metadata = Metadata::default();
    metadata.title = Some("\u{7f51}\u{9875} \u{1F3A8}".into());
    metadata.author = Some("\u{fc}ber".into());
    let mut edit = doc.edit();
    edit.set_metadata(&metadata);
    let saved = reopen(saved(&edit, &reproducible()));
    assert_eq!(saved.metadata(), metadata);
}

#[test]
fn a_reproducible_save_is_byte_identical_and_the_session_holds_no_stamp() {
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.set_metadata(&full());
    assert_eq!(saved(&edit, &reproducible()), saved(&edit, &reproducible()));
    // A stamped save in between leaves the session's own value alone.
    let _ = saved(&edit, &SaveOptions::default());
    assert_eq!(reopen(saved(&edit, &reproducible())).metadata(), full());
}

#[test]
fn the_metadata_survives_beside_a_page_edit() {
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.set_metadata(&full());
    edit.set_rotation(0, 90).unwrap();
    let mut page = doc.page(0).unwrap().edit();
    page.remove(1);
    let mut out = Vec::new();
    edit.write_pages_to(&mut out, &[page], &reproducible())
        .unwrap();
    let saved = reopen(out);
    assert_eq!(saved.metadata(), full());
    assert_eq!(saved.page(0).unwrap().rotation().degrees(), 90);
    assert_eq!(saved.page(0).unwrap().edit().len(), 1);
}

// ---- the oracle

/// The oracle's `pdfium_test`, when this machine has one: `$PDFRUM_ORACLE_BIN`,
/// else `$PDFRUM_ORACLE_CHECKOUT/out/Release/pdfium_test`, else the sibling
/// `../pdfium-c++` checkout. The same six lines as `embed_image.rs`, for the
/// same reason.
fn oracle_bin() -> Option<PathBuf> {
    let bin = std::env::var_os("PDFRUM_ORACLE_BIN").map_or_else(
        || {
            let checkout = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
                PathBuf::from,
            );
            checkout.join("out/Release/pdfium_test")
        },
        PathBuf::from,
    );
    bin.is_file().then_some(bin)
}

#[test]
fn the_oracle_reads_the_metadata_we_wrote() {
    let Some(bin) = oracle_bin() else {
        eprintln!("pdfium_test is absent; skipping the oracle round trip");
        return;
    };
    let doc = Document::open(HELLO).unwrap();
    let mut edit = doc.edit();
    edit.set_metadata(&full());
    let dir = std::env::temp_dir().join("pdfrum-metadata-oracle");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("info.pdf");
    std::fs::write(&path, saved(&edit, &reproducible())).unwrap();

    let out = Command::new(bin)
        .arg("--show-metadata")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        "Title        = A Title (",
        "Author       = An Author (",
        "Subject      = A Subject (",
        "Keywords     = one, two (",
        "Creator      = A Creator (",
        "Producer     = pdfrum (",
        "CreationDate = D:20260905100000Z00'00' (",
        "ModDate      = D:20260905110000Z00'00' (",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}
