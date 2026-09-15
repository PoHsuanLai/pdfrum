//! `/AcroForm /NeedAppearances` round-trips, and clearing it removes the key.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_edit::{EditDoc, SaveOptions, save, set_need_appearances};
use pdfrum_object::{Name, Object, names};
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

/// Saves `doc` with `edit` applied and reloads the result.
fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

/// The flag as a reader sees it: strictly a boolean, absent meaning "trust
/// the `/AP`".
fn need_appearances(doc: &Document) -> Option<bool> {
    doc.catalog()
        .ok()?
        .dict(names::ACRO_FORM, doc)?
        .get(&Name::from("NeedAppearances"), doc)
        .and_then(|value| value.as_direct().and_then(Object::as_bool))
}

#[test]
fn setting_the_flag_writes_it_through_a_save() {
    let doc = hello();
    let mut edit = EditDoc::new(&doc);
    set_need_appearances(&mut edit, true).expect("the fixture has a catalog");
    assert_eq!(need_appearances(&roundtrip(&edit)), Some(true));
}

#[test]
fn a_document_without_a_form_gains_one() {
    let doc = hello();
    assert!(
        doc.catalog()
            .expect("a catalog")
            .dict(names::ACRO_FORM, &doc)
            .is_none(),
        "the fixture is not a form to begin with"
    );

    let mut edit = EditDoc::new(&doc);
    set_need_appearances(&mut edit, true).expect("the fixture has a catalog");
    // The flag needs a form to hang on, or the next reader to rewrite the
    // catalog would drop it.
    assert_eq!(need_appearances(&roundtrip(&edit)), Some(true));
}

#[test]
fn clearing_the_flag_removes_the_key_rather_than_writing_false() {
    let doc = hello();
    let mut edit = EditDoc::new(&doc);
    set_need_appearances(&mut edit, true).expect("catalog");
    let set = roundtrip(&edit);
    assert_eq!(need_appearances(&set), Some(true));

    let mut edit = EditDoc::new(&set);
    set_need_appearances(&mut edit, false).expect("catalog");
    let cleared = roundtrip(&edit);

    // A missing key and an explicit `false` mean the same thing to a reader,
    // and the absent one is what a document that never had the flag looks
    // like.
    assert_eq!(need_appearances(&cleared), None);
    assert!(
        cleared
            .catalog()
            .expect("a catalog")
            .dict(names::ACRO_FORM, &cleared)
            .is_some(),
        "clearing the flag keeps the form it hung on"
    );
}
