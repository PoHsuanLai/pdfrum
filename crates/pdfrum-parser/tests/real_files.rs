//! Reading real PDFs, against page counts the oracle recorded.
//!
//! Everything else in this crate's tests is a document written to provoke one
//! decision. These are files that exist — a plain one, several the reader can
//! only open by scanning for objects because their `startxref` is gone, and
//! encrypted ones that need the right password. They catch the failure the
//! unit tests structurally cannot: a repair that works in isolation and does
//! not compose.
//!
//! The corpus lives outside the repository, so every test here skips when it
//! is absent rather than failing.

use std::path::PathBuf;
use std::sync::Arc;

use pdfrum_crypt::Permissions;
use pdfrum_parser::{Document, LoadError, LoadOptions, load};

/// Where the oracle's test files live, when this checkout has them.
fn resources() -> Option<PathBuf> {
    let path = PathBuf::from("/mnt/data2/pdfium/pdfium-c++/testing/resources");
    path.is_dir().then_some(path)
}

/// Open one file by name, or `None` when the corpus is absent.
fn open(name: &str, password: Option<&[u8]>) -> Option<Result<Document, LoadError>> {
    let path = resources()?.join(name);
    let bytes: Arc<[u8]> = Arc::from(std::fs::read(path).ok()?);
    let opts = LoadOptions {
        password: password.map(<[u8]>::to_vec),
        ..LoadOptions::default()
    };
    Some(load(bytes, &opts))
}

/// Assert a file opens with the page count the oracle recorded.
fn expect_pages(name: &str, password: Option<&[u8]>, pages: u32) {
    let Some(result) = open(name, password) else {
        return;
    };
    let doc = result.unwrap_or_else(|e| unreachable!("{name} failed to open: {e}"));
    assert_eq!(doc.page_count(), pages, "page count of {name}");
    // Every page the count promises has to actually resolve.
    for i in 0..pages {
        assert!(doc.page(i).is_ok(), "page {i} of {name}");
    }
}

#[test]
fn a_plain_document_opens_without_repairs() {
    let Some(result) = open("hello_world.pdf", None) else {
        return;
    };
    let doc = result.expect("hello_world.pdf opens");
    assert_eq!(doc.page_count(), 1);
    assert!(!doc.xref_was_rebuilt());
    assert!(!doc.is_encrypted());
    assert!(doc.catalog().is_ok());
    assert_eq!(doc.version(), Some(pdfrum_common::PdfVersion::PDF_1_7));
}

#[test]
fn documents_whose_cross_reference_is_gone_open_by_scanning() {
    // None of these carries a usable `startxref`; each one opens only
    // because the whole file is read looking for object headers.
    for name in [
        "bug_1301.pdf",
        "bug_1327884.pdf",
        "bug_782596.pdf",
        "bug_828049.pdf",
    ] {
        let Some(result) = open(name, None) else {
            return;
        };
        let doc = result.unwrap_or_else(|e| unreachable!("{name} failed to open: {e}"));
        assert!(doc.xref_was_rebuilt(), "{name} should have been rebuilt");
        assert_eq!(doc.page_count(), 1, "page count of {name}");
        assert!(doc.page(0).is_ok(), "page 0 of {name}");
    }
}

#[test]
fn the_rebuild_finds_the_objects_the_oracle_records() {
    let Some(result) = open("parser_rebuildxref_correct.pdf", None) else {
        return;
    };
    let doc = result.expect("parser_rebuildxref_correct.pdf opens");
    assert!(doc.xref_was_rebuilt());

    // The offsets and generations the scan is expected to recover.
    let expected: [(u32, u64, u16); 6] = [
        (1, 15, 0),
        (2, 61, 2),
        (3, 154, 4),
        (4, 296, 6),
        (5, 374, 8),
        (6, 450, 0),
    ];
    let xref = doc.xref();
    for (num, offset, generation) in expected {
        assert_eq!(
            xref.entry(num),
            Some(pdfrum_parser::Entry::Offset(offset)),
            "object {num}"
        );
        assert_eq!(xref.generation(num), generation, "generation of {num}");
    }
    // The trailer came from a bare `trailer` keyword, not an object.
    assert_eq!(doc.trailer_object_number(), 0);
}

#[test]
fn a_hybrid_files_xref_stream_supplies_the_revised_pages_node() {
    // bug_1484283.pdf is a hybrid: its second revision's plain table lists
    // only objects 4-6, and the revised `/Pages` node (object 2, carrying
    // `/MediaBox [0 0 200 350]`) is reachable *only* through the trailer's
    // `/XRefStm`, which puts it in an object stream. Ignoring that pointer
    // leaves object 2 at the first revision's 200x300 node, which is the
    // wrong page size and no other symptom.
    let Some(result) = open("pixel/bug_1484283.pdf", None) else {
        return;
    };
    let doc = result.expect("bug_1484283.pdf opens");
    assert!(!doc.xref_was_rebuilt(), "the chain should read cleanly");
    assert_eq!(doc.page_count(), 1);

    // Object 2 has to have come from the object stream, not from the first
    // revision's offset.
    assert!(
        matches!(
            doc.xref().entry(2),
            Some(pdfrum_parser::Entry::InObjStream { .. })
        ),
        "object 2 should resolve through the /XRefStm's object stream, got {:?}",
        doc.xref().entry(2)
    );

    // The page states no `/MediaBox` of its own, so this is the inherited
    // one — the whole point of the file.
    let page = doc.page(0).expect("page 0 resolves");
    let media = page
        .inherited(pdfrum_object::names::MEDIA_BOX, doc.store().as_ref())
        .expect("/MediaBox is inherited from the /Pages node");
    let array = media.as_array().expect("/MediaBox is an array");
    let values: Vec<f32> = (0..4).map(|i| array.number_at_or_zero(i)).collect();
    assert_eq!(
        values,
        vec![0.0, 0.0, 200.0, 350.0],
        "the revised /Pages node's box, not the first revision's 200x300"
    );
}

#[test]
fn a_file_with_neither_a_table_nor_a_trailer_does_not_open() {
    let Some(result) = open("parser_rebuildxref_error_notrailer.pdf", None) else {
        return;
    };
    assert!(matches!(result, Err(LoadError::Broken(_))));
}

#[test]
fn an_encrypted_document_needs_its_password() {
    let Some(no_password) = open("encrypted.pdf", None) else {
        return;
    };
    assert_eq!(no_password.err(), Some(LoadError::WrongPassword));

    let Some(wrong) = open("encrypted.pdf", Some(b"tiger")) else {
        return;
    };
    assert_eq!(wrong.err(), Some(LoadError::WrongPassword));
}

#[test]
fn either_password_opens_an_encrypted_document() {
    // The user password and the owner password both open the file; only the
    // permissions they report differ.
    let Some(user) = open("encrypted.pdf", Some(b"1234")) else {
        return;
    };
    let user = user.expect("the user password opens encrypted.pdf");
    assert_eq!(user.page_count(), 1);
    assert!(user.is_encrypted());
    assert!(user.page(0).is_ok());

    let Some(owner) = open("encrypted.pdf", Some(b"5678")) else {
        return;
    };
    let owner = owner.expect("the owner password opens encrypted.pdf");
    assert_eq!(owner.page_count(), 1);
    // The owner sees every permission granted; the same document read with
    // the user password reports only what `/P` allows. The raw words behind
    // these — `0xFFFF_FFFC` and `0xFFFF_F2C0`, with the reserved bits the
    // standard handler forces — are pinned in `pdfrum-crypt`'s own tests;
    // here they are decoded, which is what the public API now hands out.
    assert_eq!(owner.owner_permissions(), Permissions::ALL);
    // `/P` here is `0xFFFF_F2C0`, which of table 22's eight named bits sets
    // only bit 10 — accessibility extraction. Everything else, printing
    // included, is denied.
    let restricted = Permissions {
        extract: true,
        ..Permissions::NONE
    };
    assert_eq!(owner.permissions(), restricted);
    assert_eq!(user.owner_permissions(), restricted);
    assert_eq!(user.permissions(), restricted);
}

#[test]
fn every_standard_revision_opens() {
    // One file per handler revision, all sharing the owner password "âge" —
    // spelled here as UTF-8, which is how these files were written.
    const AGE_UTF8: &[u8] = b"\xc3\xa2ge";
    for name in [
        "encrypted_hello_world_r2.pdf",
        "encrypted_hello_world_r3.pdf",
        "encrypted_hello_world_r5.pdf",
        "encrypted_hello_world_r6.pdf",
    ] {
        let Some(result) = open(name, Some(AGE_UTF8)) else {
            return;
        };
        let doc = result.unwrap_or_else(|e| unreachable!("{name} failed to open: {e}"));
        assert_eq!(doc.page_count(), 1, "page count of {name}");
        assert!(doc.is_encrypted(), "{name} should be encrypted");
        assert!(doc.page(0).is_ok(), "page 0 of {name}");
    }
}

#[test]
fn documents_with_cross_reference_streams_open() {
    // These carry their cross-reference information as a stream, and most of
    // their objects live inside object streams.
    expect_pages("annotation_stamp_with_ap.pdf", None, 1);
    expect_pages("bug_1229106.pdf", None, 4);
}

#[test]
fn a_page_whose_type_is_wrong_is_still_a_page() {
    // The second page declares `/Type /Template`. It has no `/Kids`, so it
    // is a page anyway — the structure outranks the declaration.
    expect_pages("bad_page_type.pdf", None, 2);
}

#[test]
fn multi_page_documents_report_every_page() {
    expect_pages("hello_world_2_pages.pdf", None, 2);
}

#[test]
fn a_subtree_listed_twice_counts_twice() {
    // This file's tree shares nodes: the root lists object 3 twice, and
    // object 3 lists object 4 three times, so six pages exist even though
    // only two objects describe them. Skipping a node the walk has seen
    // before — rather than only one it is currently inside — would report
    // three.
    expect_pages("no_page_count.pdf", None, 6);
}

#[test]
fn no_corpus_file_makes_the_reader_panic() {
    // The blunt sweep: read every file the corpus has, with and without a
    // password. Nothing here checks *what* is read — only that reading a real
    // file, however damaged, always returns rather than panicking.
    let Some(dir) = resources() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut opened = 0usize;
    let mut read = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        read += 1;
        let shared: Arc<[u8]> = Arc::from(bytes);
        for password in [None, Some(b"password".to_vec())] {
            let opts = LoadOptions {
                password,
                ..LoadOptions::default()
            };
            if let Ok(doc) = load(Arc::clone(&shared), &opts) {
                opened += 1;
                // Touch every page, since the walk is where the tree's
                // damage shows up.
                for i in 0..doc.page_count().min(64) {
                    let _ = doc.page(i);
                }
            }
        }
    }
    assert!(read > 100, "the corpus should hold hundreds of files");
    assert!(opened > 0, "some of them should open");
}
