//! An associated file is the same attachment, declared as related — that is
//! what `/AF` says and what PDF/A-3 and the invoice profiles read.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_common::Limits;
use pdfrum_edit::{
    AttachmentOptions, EditDoc, Relationship, SaveOptions, add_attachment,
    associate_file_with_document, associate_file_with_page, document_associated_files,
    page_associated_files, save,
};
use pdfrum_object::{Name, Object};
use pdfrum_parser::{Document, LoadOptions, load};

const HELLO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hello_world.pdf"
);

fn blank_doc() -> Document {
    let bytes: Arc<[u8]> = Arc::from(&std::fs::read(HELLO).expect("fixture")[..]);
    load(bytes, &LoadOptions::default()).expect("load")
}

fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

/// Attach `name` and answer its index.
fn attach(edit: &mut EditDoc<'_>, name: &str, bytes: &[u8]) -> usize {
    add_attachment(
        edit,
        &Limits::default(),
        name,
        bytes,
        &AttachmentOptions::builder().mime_type("text/xml").build(),
    )
    .expect("attach")
}

#[test]
fn an_associated_file_is_listed_with_its_relationship() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let index = attach(&mut edit, "invoice.xml", b"<invoice/>");
    assert!(
        associate_file_with_document(
            &mut edit,
            &Limits::default(),
            index,
            Relationship::Alternative
        )
        .expect("associate"),
    );

    let saved = roundtrip(&edit);
    let edit = EditDoc::new(&saved);
    assert_eq!(
        document_associated_files(&edit, &Limits::default()).expect("read"),
        vec![(0, Relationship::Alternative)],
    );
}

#[test]
fn the_relationship_is_written_onto_the_specification() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let index = attach(&mut edit, "invoice.xml", b"<invoice/>");
    associate_file_with_document(&mut edit, &Limits::default(), index, Relationship::Source)
        .expect("associate");

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    let af = catalog
        .array(&Name::from("AF"), &saved)
        .expect("the catalog carries /AF");
    assert_eq!(af.len(), 1);
    let spec = af.dict_at(0, &saved).expect("a file specification");
    // A validator reads `/AFRelationship` off the specification, not off the
    // array entry, which is why one file cannot hold two relationships.
    assert_eq!(
        spec.raw(&Name::from("AFRelationship"))
            .and_then(Object::as_name)
            .map(|name| name.as_bytes().to_vec()),
        Some(b"Source".to_vec()),
    );
}

#[test]
fn the_file_is_still_an_attachment() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let index = attach(&mut edit, "invoice.xml", b"<invoice/>");
    associate_file_with_document(
        &mut edit,
        &Limits::default(),
        index,
        Relationship::Alternative,
    )
    .expect("associate");

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    // `/AF` adds a *second* reference to the same specification; the tree
    // entry is untouched, so a reader still offers the file to save.
    assert!(
        catalog
            .dict(&Name::from("Names"), &saved)
            .and_then(|names| names.dict(&Name::from("EmbeddedFiles"), &saved))
            .is_some(),
        "the embedded-files tree survives",
    );
}

#[test]
fn associating_twice_updates_the_relationship_rather_than_listing_it_twice() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let index = attach(&mut edit, "invoice.xml", b"<invoice/>");
    let limits = Limits::default();
    associate_file_with_document(&mut edit, &limits, index, Relationship::Supplement)
        .expect("first");
    associate_file_with_document(&mut edit, &limits, index, Relationship::Alternative)
        .expect("second");

    let saved = roundtrip(&edit);
    let edit = EditDoc::new(&saved);
    // The relationship lives on the specification, so a second entry could
    // not say anything the first does not.
    assert_eq!(
        document_associated_files(&edit, &limits).expect("read"),
        vec![(0, Relationship::Alternative)],
    );
}

#[test]
fn a_page_association_is_separate_from_the_documents() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let limits = Limits::default();
    let data = attach(&mut edit, "chart.csv", b"x,y\n1,2\n");
    associate_file_with_page(&mut edit, &limits, 0u32, data, Relationship::Data)
        .expect("associate");

    let saved = roundtrip(&edit);
    let edit = EditDoc::new(&saved);
    assert_eq!(
        page_associated_files(&edit, &limits, 0u32).expect("read"),
        vec![(0, Relationship::Data)],
    );
    // A page-level association says nothing about the document as a whole.
    assert!(
        document_associated_files(&edit, &limits)
            .expect("read")
            .is_empty(),
    );
}

#[test]
fn several_files_keep_the_order_they_were_associated_in() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let limits = Limits::default();
    // The tree is sorted by name, so adding `b.xml` second shifts `z.xml` from
    // index 0 to index 1 — an index `add_attachment` returned before the
    // second add is stale. Both are attached first, then associated by their
    // settled indices: `b.xml` at 0 and `z.xml` at 1.
    attach(&mut edit, "z.xml", b"<z/>");
    attach(&mut edit, "b.xml", b"<b/>");
    associate_file_with_document(&mut edit, &limits, 1, Relationship::Source).expect("z");
    associate_file_with_document(&mut edit, &limits, 0, Relationship::Data).expect("b");

    let saved = roundtrip(&edit);
    let edit = EditDoc::new(&saved);
    let listed = document_associated_files(&edit, &limits).expect("read");
    assert_eq!(listed.len(), 2);
    // `/AF` keeps insertion order; the indices are the tree's.
    assert!(listed.contains(&(1, Relationship::Source)), "{listed:?}");
    assert!(listed.contains(&(0, Relationship::Data)), "{listed:?}");
}

#[test]
fn associating_a_file_that_is_not_there_answers_false() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let limits = Limits::default();
    assert!(
        !associate_file_with_document(&mut edit, &limits, 0, Relationship::Source)
            .expect("no error"),
        "an empty document has no attachment 0",
    );
    assert!(
        !associate_file_with_page(&mut edit, &limits, 0u32, 0, Relationship::Source)
            .expect("no error"),
    );
}

#[test]
fn a_document_with_no_af_lists_nothing() {
    let doc = blank_doc();
    let edit = EditDoc::new(&doc);
    let limits = Limits::default();
    assert!(
        document_associated_files(&edit, &limits)
            .expect("read")
            .is_empty(),
    );
    assert!(
        page_associated_files(&edit, &limits, 0u32)
            .expect("read")
            .is_empty(),
    );
}

#[test]
fn adding_a_second_attachment_keeps_the_first_one_indirect() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    attach(&mut edit, "a.xml", b"<a/>");
    attach(&mut edit, "b.xml", b"<b/>");

    let saved = roundtrip(&edit);
    let files = saved
        .catalog()
        .expect("a catalog")
        .dict(&Name::from("Names"), &saved)
        .and_then(|names| names.dict(&Name::from("EmbeddedFiles"), &saved))
        .and_then(|tree| tree.array(&Name::from("Names"), &saved))
        .expect("the tree's leaf");

    // Every write path rewrites the whole tree, and it used to rewrite it from
    // *resolved* entries — so adding a second attachment inlined the first
    // specification, losing its object identity and duplicating the dictionary
    // on the next save. `/AF` is what made that visible: an associated file
    // has to be the same object as the attachment.
    assert_eq!(files.len(), 4, "two name/value pairs");
    for slot in [1, 3] {
        assert!(
            files.reference_at(slot).is_some(),
            "the specification at slot {slot} is inline, not a reference",
        );
    }
}
