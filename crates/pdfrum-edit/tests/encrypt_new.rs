//! Encrypting an unencrypted document on save: AES-256, revision 6.
//!
//! No oracle can do this, so the test is the round trip: the file this
//! crate writes opens in this crate's own parser with either password, with
//! the permissions asked for, and not with a wrong one; a document that
//! already has a handler is refused. The secrets come from the operating
//! system, so the seed that pins `/ID` and the subset tags leaves the key
//! and the ciphertext free to differ between two saves.

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

// A seed governs the identifiers, never the secrets: /UE and /OE wrap the
// file key, so two saves under one seed carrying different /UE carry
// different keys, and the ciphertext differs with them.
#[test]
fn one_seed_still_gives_two_encrypted_saves_two_keys() {
    let a = encrypted(&hello(), print_only(), IdSource::Fixed([3; 16]));
    let b = encrypted(&hello(), print_only(), IdSource::Fixed([3; 16]));
    assert_ne!(a, b, "the same seed must not reproduce a file key");
    assert_ne!(wrapped_key(&a), wrapped_key(&b));
}

// Every enciphered payload differs between two saves of one document: the
// vectors are drawn per save, so even a repeated key would not repeat a
// leading block. A plaintext stream — the XMP packet ISO 32000-1 §14.3.2
// exempts — is the one that legitimately matches.
#[test]
fn two_encryptions_of_one_document_use_different_vectors() {
    let a = streams(&encrypted(&hello(), print_only(), IdSource::Fixed([3; 16])));
    let b = streams(&encrypted(&hello(), print_only(), IdSource::Fixed([3; 16])));
    assert_ne!(a, b);
    assert!(
        a.iter().zip(&b).any(|(left, right)| left != right),
        "no enciphered payload changed between two saves"
    );
    for (left, right) in a.iter().zip(&b) {
        assert!(
            left == right || left.get(..16) != right.get(..16),
            "two payloads differ but share a leading block, so they share a vector"
        );
    }
}

// What a seed does still govern: an unencrypted save is byte-for-byte the
// same file every time, `/ID` and subset tags included.
#[test]
fn a_fixed_id_source_makes_an_unencrypted_save_reproducible() {
    let plain = |seed| {
        let doc = open(&hello(), b"").expect("opens");
        let edit = EditDoc::new(&doc);
        let options = SaveOptions {
            mode: SaveMode::Full,
            id_source: IdSource::Fixed(seed),
            ..SaveOptions::default()
        };
        let mut out = Vec::new();
        save(&edit, &options, &mut out).expect("saves");
        out
    };
    assert_eq!(plain([3; 16]), plain([3; 16]));
    assert_ne!(plain([3; 16]), plain([4; 16]));
}

/// The `/UE` string of a written file: the file key wrapped under the user
/// password's intermediate hash (ISO 32000-2 §7.6.4.4.7, algorithm 8). Two
/// files with the same passwords and different `/UE` have different keys.
fn wrapped_key(bytes: &[u8]) -> Vec<u8> {
    let doc = open(bytes, b"owner").expect("opens");
    let (dict, _) = doc.encrypt_dict().expect("has /Encrypt");
    dict.get(names::UE, &doc)
        .expect("/UE")
        .get()
        .as_string()
        .expect("/UE is a string")
        .encode()
}

/// Every stream payload the file carries, as written — ciphertext, since a
/// save encrypts what it writes.
fn streams(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while let Some(at) = rest.windows(6).position(|w| w == b"stream") {
        let body = rest.get(at + 6..).unwrap_or_default();
        let body = body
            .strip_prefix(b"\r\n".as_slice())
            .or_else(|| body.strip_prefix(b"\n".as_slice()))
            .unwrap_or(body);
        let Some(end) = body.windows(9).position(|w| w == b"endstream") else {
            break;
        };
        out.push(body.get(..end).unwrap_or_default().to_vec());
        rest = body.get(end + 9..).unwrap_or_default();
    }
    out
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
