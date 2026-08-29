//! `load` with a password — the encrypted-document path end to end.
//!
//! Separate from `parser_load` because the password changes which code runs,
//! not just which answer comes back: a supplied password takes the document
//! through owner-then-user key derivation, the `/R` 5 and 6 SHA-2 ladders,
//! and the Latin-1/UTF-8 re-encoding retry — none of which the unencrypted
//! target reaches. Seeded with the oracle's `encrypted_*.pdf` resources.
//!
//! Property: never panics. Every document that opens must decrypt its
//! strings and streams without panicking.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfrum_parser::{LoadOptions, load};

/// The passwords the oracle's own encrypted fixtures use, plus the shapes
/// that exercise the re-encoding retry: a Latin-1 byte that is not UTF-8, a
/// valid multi-byte UTF-8 sequence, and one past the 127-byte ISO limit the
/// crate deliberately does not enforce.
fn password(selector: u8, from_input: &[u8]) -> Option<Vec<u8>> {
    match selector % 8 {
        0 => None,
        1 => Some(Vec::new()),
        2 => Some(b"hello".to_vec()),
        3 => Some(b"world".to_vec()),
        4 => Some(b"h\xF4tel".to_vec()),
        5 => Some("hôtel".as_bytes().to_vec()),
        6 => Some(vec![b'x'; 300]),
        _ => Some(from_input.to_vec()),
    }
}

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let selector = split.byte();
    let supplied = split.take();
    let body = split.rest();

    let opts = LoadOptions {
        password: password(selector, supplied),
        limits: pdfrum_fuzz::limits(),
    };

    let Ok(doc) = load(Arc::from(body), &opts) else {
        return;
    };

    let _ = doc.is_encrypted();
    let _ = doc.permissions(false);
    let _ = doc.permissions(true);

    // Decryption happens lazily as objects are fetched, so a document that
    // merely opened has not yet exercised the cipher. Fetching the objects
    // the xref names is what runs it.
    let store = doc.store();
    for num in doc.xref().object_numbers().take(512) {
        let _ = store.get(num);
    }
    let _ = doc.catalog();
    for index in 0..doc.page_count().min(16) {
        let _ = doc.page(index);
    }
});
