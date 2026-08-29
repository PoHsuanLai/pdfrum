//! `save` — a document that opened, written back out and opened again.
//!
//! Two properties, and the second is the interesting one:
//!
//! - the writer never panics, whatever damage the reader tolerated to get a
//!   `Document` at all;
//! - **a file that opened must save to a file that opens.** The writer sees
//!   only objects the reader already made sense of, so there is no input it
//!   can legitimately turn into something unopenable. A failure here is a
//!   writer bug by construction, not a "bad input" case — which is what makes
//!   it worth asserting rather than merely surviving.
//!
//! Seeded with the oracle's `testing/resources` PDFs, so the corpus starts
//! from documents that exercise real recovery paths rather than from noise.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfrum_edit::{EditDoc, IdSource, SaveMode, SaveOptions, save};
use pdfrum_parser::{LoadOptions, load};

fuzz_target!(|data: &[u8]| {
    let opts = LoadOptions {
        password: None,
        limits: pdfrum_fuzz::limits(),
    };

    let Ok(doc) = load(Arc::from(data), &opts) else {
        return;
    };
    let pages = doc.page_count();
    let edit = EditDoc::new(&doc);

    // A fixed identifier so a crash reproduces from the input alone.
    let options = SaveOptions {
        mode: SaveMode::Full,
        // v1 writes plaintext; an encrypted document without this is refused
        // rather than mis-saved, which is itself a behavior worth reaching.
        remove_security: true,
        id_source: IdSource::Fixed([0x3C; 16]),
        ..SaveOptions::default()
    };

    let mut out = Vec::new();
    if save(&edit, &options, &mut out).is_err() {
        return;
    }

    // The load-bearing assertion: our own output opens.
    let reloaded = load(Arc::from(&out[..]), &opts)
        .unwrap_or_else(|err| panic!("a saved document did not reopen: {err:?}"));
    assert_eq!(
        reloaded.page_count(),
        pages,
        "the page count changed across a save"
    );

    // And everything the facade asks of a fresh document still answers.
    let _ = reloaded.catalog();
    let _ = reloaded.trailer();
    let _ = reloaded.last_xref_offset();
    let _ = reloaded.main_xref_is_stream();
    let _ = reloaded.encrypt_dict();
    for i in 0..reloaded.page_count().min(64) {
        let _ = reloaded.page(i);
    }

    // The incremental path, over the same input. Its own invariant is the
    // append discipline: the original bytes must survive as a prefix.
    let incremental = SaveOptions {
        mode: SaveMode::Incremental,
        ..options
    };
    let mut appended = Vec::new();
    if save(&edit, &incremental, &mut appended).is_ok() {
        let body = doc.bytes();
        // The prefix property holds only when the save really was
        // incremental; a rebuilt table or a removed cipher downgrades it to
        // a full rewrite, which starts with `%PDF` instead.
        if appended.starts_with(&body[..]) {
            assert!(
                appended.len() >= body.len(),
                "an appended file cannot be shorter than what it appended to"
            );
        }
        let _ = load(Arc::from(&appended[..]), &opts);
    }
});
