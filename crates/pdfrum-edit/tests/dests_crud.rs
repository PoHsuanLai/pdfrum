//! Named destinations: list and delete, alongside the set that already
//! existed.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_edit::{
    AnnotGoToView, EditDoc, SaveOptions, delete_named_destination, named_destinations, save,
    set_named_destination,
};
use pdfrum_parser::{Document, LoadOptions, load};

fn hello() -> Document {
    let bytes: Arc<[u8]> = Arc::from(
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/files/hello.pdf"
        ))[..],
    );
    load(bytes, &LoadOptions::default()).expect("the fixture loads")
}

/// The first page's reference, to point destinations at.
fn page_ref(doc: &Document) -> pdfrum_object::ObjRef {
    doc.page(0u32).expect("page").reference.expect("indirect")
}

fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

#[test]
fn names_are_listed_after_being_set() {
    let doc = hello();
    let page = page_ref(&doc);
    let mut edit = EditDoc::new(&doc);
    set_named_destination(&mut edit, "Beta", page, AnnotGoToView::Fit).expect("set");
    set_named_destination(&mut edit, "Alpha", page, AnnotGoToView::Fit).expect("set");

    // Sorted, so a caller gets a stable listing rather than tree order.
    assert_eq!(
        named_destinations(&edit).expect("list"),
        vec!["Alpha".to_owned(), "Beta".to_owned()]
    );
}

#[test]
fn deleting_answers_whether_the_name_was_there() {
    let doc = hello();
    let page = page_ref(&doc);
    let mut edit = EditDoc::new(&doc);
    set_named_destination(&mut edit, "Gone", page, AnnotGoToView::Fit).expect("set");

    assert!(
        delete_named_destination(&mut edit, "Gone").expect("delete"),
        "the name was there"
    );
    assert!(
        !delete_named_destination(&mut edit, "Gone").expect("delete"),
        "a second delete has nothing to do, which is not an error"
    );
    assert!(
        !delete_named_destination(&mut edit, "never-existed").expect("delete"),
        "a name the document never carried is the same case"
    );
}

#[test]
fn a_deleted_name_does_not_survive_a_save() {
    let doc = hello();
    let page = page_ref(&doc);
    let mut edit = EditDoc::new(&doc);
    set_named_destination(&mut edit, "Keep", page, AnnotGoToView::Fit).expect("set");
    set_named_destination(&mut edit, "Drop", page, AnnotGoToView::Fit).expect("set");
    delete_named_destination(&mut edit, "Drop").expect("delete");

    let saved = roundtrip(&edit);
    let edit = EditDoc::new(&saved);
    assert_eq!(
        named_destinations(&edit).expect("list"),
        vec!["Keep".to_owned()]
    );
}

#[test]
fn a_document_with_no_destinations_lists_nothing() {
    let doc = hello();
    let edit = EditDoc::new(&doc);
    assert!(named_destinations(&edit).expect("list").is_empty());
}
