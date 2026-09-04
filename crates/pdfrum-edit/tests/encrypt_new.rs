//! Encrypting an unencrypted document on save: AES-256, revision 6.
//!
//! No oracle can do this, so the test is the round trip: the file this
//! crate writes opens in this crate's own parser with either password, with
//! the permissions asked for, and not with a wrong one; a reproducible save
//! is byte-identical; a document that already has a handler is refused.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_crypt::Permissions;
use pdfrum_edit::{EditDoc, Encryption, Error, IdSource, SaveMode, SaveOptions, save};
use pdfrum_object::names;
use pdfrum_parser::{Document, LoadError, LoadOptions, load};

fn hello() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/files/hello.pdf"
    ))
    .expect("fixture")
}

fn open(bytes: &[u8], password: &[u8]) -> Result<Document, LoadError> {
    load(
        Arc::from(bytes),
        &LoadOptions {
            password: (!password.is_empty()).then(|| password.to_vec()),
            ..LoadOptions::default()
        },
    )
}

fn encrypted(bytes: &[u8], encryption: Encryption, id_source: IdSource) -> Vec<u8> {
    let doc = open(bytes, b"").expect("opens");
    let edit = EditDoc::new(&doc);
    let options = SaveOptions {
        mode: SaveMode::Full,
        id_source,
        encrypt: Some(encryption),
        ..SaveOptions::default()
    };
    let mut out = Vec::new();
    save(&edit, &options, &mut out).expect("saves");
    out
}

fn print_only() -> Encryption {
    Encryption {
        user_password: b"reader".to_vec(),
        owner_password: b"owner".to_vec(),
        permissions: Permissions {
            print: true,
            ..Permissions::NONE
        },
        encrypt_metadata: true,
    }
}

#[test]
fn the_saved_file_opens_with_either_password_and_carries_the_permissions() {
    let out = encrypted(&hello(), print_only(), IdSource::Fixed([7; 16]));
    assert!(open(&out, b"").is_err(), "a user password is required");
    assert!(matches!(
        open(&out, b"wrong"),
        Err(LoadError::WrongPassword)
    ));

    let as_reader = open(&out, b"reader").expect("the user password opens it");
    assert_eq!(as_reader.page_count(), 1);
    let permissions = as_reader.security_handler().permissions();
    assert!(permissions.print && !permissions.copy && !permissions.modify);
    assert!(!as_reader.security_handler().owner_unlocked());

    let as_owner = open(&out, b"owner").expect("the owner password opens it");
    assert!(as_owner.security_handler().owner_unlocked());
    assert_eq!(as_owner.security_handler().revision(), 6);
}

#[test]
fn the_content_survives_the_cipher() {
    let original = open(&hello(), b"").expect("opens");
    let out = encrypted(&hello(), print_only(), IdSource::Fixed([7; 16]));
    let reopened = open(&out, b"reader").expect("opens");
    let contents = |doc: &Document| -> Vec<u8> {
        let page = doc.page(0u32).expect("page");
        let stream = page
            .dict
            .get(names::CONTENTS, doc)
            .expect("contents")
            .get()
            .as_stream()
            .cloned()
            .expect("stream");
        pdfrum_filters::decode_chain(
            &stream,
            0,
            doc,
            &pdfrum_common::Limits::default(),
            &mut pdfrum_common::Diagnostics::default(),
        )
        .data
        .clone()
    };
    assert_eq!(contents(&reopened), contents(&original));
    assert!(
        !out.windows(5).any(|w| w == b"Hello"),
        "the plaintext must not appear in the file"
    );
}

#[test]
fn a_fixed_id_source_makes_the_encrypted_save_reproducible() {
    let a = encrypted(&hello(), print_only(), IdSource::Fixed([3; 16]));
    let b = encrypted(&hello(), print_only(), IdSource::Fixed([3; 16]));
    assert_eq!(a, b);
    let c = encrypted(&hello(), print_only(), IdSource::Fixed([4; 16]));
    assert_ne!(a, c, "a different seed gives different salts and key");
}

#[test]
fn an_encrypted_document_is_not_re_keyed_in_one_save() {
    let once = encrypted(&hello(), print_only(), IdSource::Fixed([1; 16]));
    let doc = open(&once, b"owner").expect("opens");
    let edit = EditDoc::new(&doc);
    let options = SaveOptions {
        encrypt: Some(print_only()),
        ..SaveOptions::default()
    };
    let mut out = Vec::new();
    assert!(matches!(
        save(&edit, &options, &mut out),
        Err(Error::EncryptedSaveUnsupported)
    ));
}
