//! `/MK` appearance characteristics round-trip, and a partial edit keeps the
//! keys it says nothing about.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_edit::{EditDoc, SaveOptions, WidgetAppearance, save, set_widget_appearance};
use pdfrum_object::{Dict, Name, ObjRef, Resolve};
use pdfrum_parser::{Document, LoadOptions, load};
use peniko::Color;

const WIDGET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../pdfrum/tests/fixtures/text_form.pdf"
);

fn form_doc() -> Document {
    let bytes: Arc<[u8]> = Arc::from(&std::fs::read(WIDGET).expect("fixture")[..]);
    load(bytes, &LoadOptions::default()).expect("load")
}

/// The fixture's single widget.
fn widget_ref(doc: &Document) -> ObjRef {
    let page = doc.page(0u32).expect("page");
    let annots = page
        .dict
        .array(&Name::from("Annots"), doc)
        .expect("the fixture has /Annots");
    annots.reference_at(0).expect("an indirect widget")
}

fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

/// The widget's `/MK`, as written.
fn mk(doc: &Document) -> Dict {
    let reference = widget_ref(doc);
    doc.fetch(reference)
        .ok()
        .and_then(|object| object.as_dict().cloned())
        .and_then(|dict| dict.dict(&Name::from("MK"), doc))
        .unwrap_or_default()
}

/// One `/MK` colour entry as three components.
fn color(mk: &Dict, key: &str, doc: &Document) -> Option<Vec<f32>> {
    let array = mk.array(&Name::from(key), doc)?;
    Some(
        (0..array.len())
            .filter_map(|index| array.number_at(index))
            .collect(),
    )
}

#[test]
fn background_and_border_colours_round_trip() {
    let doc = form_doc();
    let reference = widget_ref(&doc);
    let mut edit = EditDoc::new(&doc);
    set_widget_appearance(
        &mut edit,
        reference,
        &WidgetAppearance::new()
            .background(Color::from_rgb8(255, 0, 0))
            .border(Color::from_rgb8(0, 0, 255)),
    )
    .expect("the widget resolves");

    let saved = roundtrip(&edit);
    let mk = mk(&saved);
    assert_eq!(color(&mk, "BG", &saved), Some(vec![1.0, 0.0, 0.0]));
    assert_eq!(color(&mk, "BC", &saved), Some(vec![0.0, 0.0, 1.0]));
}

#[test]
fn an_unset_characteristic_keeps_what_the_widget_had() {
    let doc = form_doc();
    let reference = widget_ref(&doc);

    // First write both.
    let mut edit = EditDoc::new(&doc);
    set_widget_appearance(
        &mut edit,
        reference,
        &WidgetAppearance::new()
            .background(Color::from_rgb8(255, 0, 0))
            .border(Color::from_rgb8(0, 0, 255)),
    )
    .expect("widget");
    let both = roundtrip(&edit);

    // Then change only the background. The border must survive: a builder
    // that said nothing about `/BC` is not asking for it to be cleared.
    let reference = widget_ref(&both);
    let mut edit = EditDoc::new(&both);
    set_widget_appearance(
        &mut edit,
        reference,
        &WidgetAppearance::new().background(Color::from_rgb8(0, 255, 0)),
    )
    .expect("widget");
    let changed = roundtrip(&edit);

    let mk = mk(&changed);
    assert_eq!(color(&mk, "BG", &changed), Some(vec![0.0, 1.0, 0.0]));
    assert_eq!(
        color(&mk, "BC", &changed),
        Some(vec![0.0, 0.0, 1.0]),
        "the border colour was not mentioned, so it stays"
    );
}

#[test]
fn rotation_and_caption_are_written() {
    let doc = form_doc();
    let reference = widget_ref(&doc);
    let mut edit = EditDoc::new(&doc);
    set_widget_appearance(
        &mut edit,
        reference,
        &WidgetAppearance::new().rotation(90).caption("Press"),
    )
    .expect("widget");

    let saved = roundtrip(&edit);
    let mk = mk(&saved);
    assert_eq!(mk.int(&Name::from("R"), &saved), Some(90));
    assert_eq!(mk.text(&Name::from("CA"), &saved).as_deref(), Some("Press"),);
}

#[test]
fn an_unresolvable_reference_is_an_error() {
    let doc = form_doc();
    let mut edit = EditDoc::new(&doc);
    let nowhere = ObjRef::new(9999, 0);
    assert!(
        set_widget_appearance(&mut edit, nowhere, &WidgetAppearance::new().rotation(90)).is_err(),
        "a reference to nothing cannot gain an /MK"
    );
}
