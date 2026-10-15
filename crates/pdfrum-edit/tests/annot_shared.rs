//! Annotations on pages that share objects.
//!
//! A file may point two pages at one indirect `/Annots` array, or list one
//! annotation object from two pages (a widget shown twice). An edit of one
//! page must leave the other as it was, and must not drop an object another
//! page still names. The writer also adds a free-text annotation to documents
//! whose form has no `/DR`, where the generator alone would produce nothing.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use pdfrum::{Color, Document, Name, Rect, Subtype};
use pdfrum_edit::{
    EditDoc, FreeTextSpec, SaveMode, SaveOptions, SquareSpec, add_annotation, delete_annotation,
};

const SHARED_ANNOTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../pdfrum/tests/fixtures/bug_498010830_shared_annots.pdf"
);
const SHARED_WIDGET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../pdfrum/tests/fixtures/bug_498010830_shared_widget.pdf"
);
const TWO_SIGNATURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../pdfrum/tests/fixtures/two_signatures.pdf"
);

fn open(path: &str) -> Document {
    Document::open(path).expect("open fixture")
}

fn count(doc: &Document, page: u32) -> usize {
    doc.page(page).expect("page").annotations().len()
}

fn save(edit: &EditDoc<'_>) -> Document {
    let mut bytes = Vec::new();
    pdfrum_edit::save(edit, &SaveOptions::default(), &mut bytes).expect("save");
    Document::from_bytes(Arc::from(bytes)).expect("reopen")
}

fn square() -> SquareSpec {
    SquareSpec::new(
        Rect::new(10.0, 10.0, 60.0, 60.0),
        Color::from_rgb8(255, 0, 0),
    )
}

// Adding to a page whose `/Annots` array another page shares must not put the
// new annotation on the other page, and deleting it must restore both.
#[test]
fn an_annotation_added_to_a_shared_array_stays_on_its_page() {
    let doc = open(SHARED_ANNOTS);
    let (first, second) = (count(&doc, 0), count(&doc, 1));

    let mut edit = EditDoc::new(doc.parser());
    add_annotation(&mut edit, 0, square()).expect("add");
    let added = save(&edit);
    assert_eq!(count(&added, 0), first + 1, "page 0 gains the square");
    assert_eq!(count(&added, 1), second, "page 1 is untouched");

    let page = added.page(0).expect("page");
    let reference = page
        .annotations()
        .find(|a| a.subtype() == Subtype::Square)
        .and_then(|a| a.obj_ref())
        .expect("the square is listed indirectly");
    let mut edit = EditDoc::new(added.parser());
    assert!(delete_annotation(&mut edit, 0, reference).expect("delete"));
    let deleted = save(&edit);
    assert_eq!(count(&deleted, 0), first, "page 0 is back where it began");
    assert_eq!(count(&deleted, 1), second, "page 1 is still untouched");
}

// Deleting an annotation one page lists must not remove the object while
// another page still names it.
#[test]
fn a_shared_annotation_survives_deletion_from_one_page() {
    let doc = open(SHARED_WIDGET);
    let widget = doc
        .page(0)
        .expect("page")
        .annotations()
        .find(|a| a.subtype() == Subtype::Widget)
        .and_then(|a| a.obj_ref())
        .expect("page 0 lists a widget");
    let second = count(&doc, 1);
    assert!(second > 0, "the fixture shows the widget on page 1 as well");

    let mut edit = EditDoc::new(doc.parser());
    assert!(delete_annotation(&mut edit, 0, widget).expect("delete"));
    let saved = save(&edit);

    assert_eq!(count(&saved, 1), second, "page 1 still lists the widget");
    let still = saved
        .page(1)
        .expect("page")
        .annotations()
        .any(|a| a.subtype() == Subtype::Widget && a.obj_ref() == Some(widget));
    assert!(still, "the widget still resolves as a widget on page 1");
}

// The form of `two_signatures.pdf` has no `/DR`, so the generator alone yields
// nothing for free text. The writer supplies the fallback resources for the
// generation and leaves the document's own form as it was.
#[test]
fn free_text_in_a_form_without_dr_gets_an_appearance() {
    let doc = open(TWO_SIGNATURES);
    let form = |doc: &Document| {
        let catalog = doc.parser().catalog().expect("catalog");
        catalog.dict(&Name::from("AcroForm"), doc.parser())
    };
    assert!(
        form(&doc).is_some_and(|f| f.raw(&Name::from("DR")).is_none()),
        "the fixture's form has no /DR"
    );
    let before = count(&doc, 0);

    let mut edit = EditDoc::new(doc.parser());
    add_annotation(
        &mut edit,
        0,
        FreeTextSpec::new(
            Rect::new(20.0, 20.0, 220.0, 60.0),
            Color::from_rgb8(0, 0, 0),
            "hello".to_owned(),
        ),
    )
    .expect("add");
    let mut bytes = Vec::new();
    pdfrum_edit::save(
        &edit,
        &SaveOptions::builder().mode(SaveMode::Incremental).build(),
        &mut bytes,
    )
    .expect("save");
    let saved = Document::from_bytes(Arc::from(bytes)).expect("reopen");

    assert_eq!(count(&saved, 0), before + 1);
    let page = saved.page(0).expect("page");
    let note = page
        .annotations()
        .find(|a| a.subtype() == Subtype::FreeText)
        .expect("the free text");
    let ap = note
        .dict()
        .dict(&Name::from("AP"), saved.parser())
        .expect("the annotation has an /AP");
    assert!(ap.raw(&Name::from("N")).is_some(), "with a normal state");
    assert!(
        form(&saved).is_some_and(|f| f.raw(&Name::from("DR")).is_none()),
        "the document's own form is not given a /DR"
    );
}
