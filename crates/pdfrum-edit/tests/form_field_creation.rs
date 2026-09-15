//! A created field is a field the reader recognises: it classifies, it carries
//! its name and value, it is listed on its page, and it has an appearance
//! stream a viewer can draw without rebuilding it.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use kurbo::Rect;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::form::{FieldKind, Form};
use pdfrum_edit::{
    EditDoc, Error, FieldKindSpec, FieldSpec, SaveOptions, StandardFont, WidgetAppearance,
    add_form_field, add_form_font, save,
};
use pdfrum_object::{Dict, Name, Object};
use pdfrum_parser::{Document, LoadOptions, load};
use peniko::Color;

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

fn form_of(doc: &Document) -> Form {
    let catalog = doc.catalog().expect("a catalog");
    Form::load(
        &catalog,
        doc,
        &Limits::default(),
        &mut Diagnostics::default(),
    )
    .expect("the saved document declares an /AcroForm")
}

fn rect() -> Rect {
    Rect::new(72.0, 700.0, 300.0, 720.0)
}

/// Every annotation dictionary on a page, resolved.
fn annots(doc: &Document, page: u32) -> Vec<Dict> {
    let page = doc.page(page).expect("page");
    let Some(array) = page.dict.array(&Name::from("Annots"), doc) else {
        return Vec::new();
    };
    (0..array.len())
        .filter_map(|index| array.dict_at(index, doc))
        .collect()
}

/// Whether a widget carries a non-empty `/AP /N` — directly, or under a state.
fn has_appearance(dict: &Dict, doc: &Document) -> bool {
    let Some(ap) = dict.dict(&Name::from("AP"), doc) else {
        return false;
    };
    let Some(value) = ap.get(&Name::from("N"), doc) else {
        return false;
    };
    let Ok(object) = value.resolve(doc) else {
        return false;
    };
    match object.get() {
        Object::Stream(stream) => !stream.data.is_empty(),
        Object::Dict(states) => states.keys().any(|key| is_drawn_stream(states, key, doc)),
        _ => false,
    }
}

/// Whether `key` in `states` names a stream with bytes in it.
fn is_drawn_stream(states: &Dict, key: &Name, doc: &Document) -> bool {
    let Some(value) = states.get(key, doc) else {
        return false;
    };
    let Ok(object) = value.resolve(doc) else {
        return false;
    };
    matches!(object.get(), Object::Stream(stream) if !stream.data.is_empty())
}

#[test]
fn a_created_text_field_reads_back_as_a_text_field_with_its_value() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text).value("Ada Lovelace"),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let form = form_of(&saved);
    assert_eq!(form.len(), 1);

    let field = form.field("full_name").expect("the field by name");
    assert_eq!(field.kind, FieldKind::Text);
    assert_eq!(field.value(None, &saved), "Ada Lovelace");
}

#[test]
fn a_created_text_field_carries_a_drawn_appearance() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text)
            .value("Ada")
            .appearance(WidgetAppearance::new().border(Color::from_rgb8(0, 0, 0))),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    // The value is laid out at creation, so a viewer that trusts `/AP` shows
    // the field filled rather than empty.
    let widget = annots(&saved, 0)
        .into_iter()
        .find(|dict| dict.byte_string(&Name::from("Subtype"), &saved).as_deref() == Some(b"Widget"))
        .expect("the widget is on the page");
    assert!(has_appearance(&widget, &saved));
}

#[test]
fn a_created_check_box_classifies_and_toggles_with_its_value() {
    for (value, expect_checked) in [(Some("Yes"), true), (None, false)] {
        let doc = blank_doc();
        let mut edit = EditDoc::new(&doc);
        let mut spec = FieldSpec::new("agree", rect(), FieldKindSpec::check());
        if let Some(value) = value {
            spec = spec.value(value);
        }
        add_form_field(&mut edit, 0u32, &spec).expect("create");

        let saved = roundtrip(&edit);
        let form = form_of(&saved);
        let field = form.field("agree").expect("the field by name");
        assert_eq!(field.kind, FieldKind::Check, "{value:?}");
        assert_eq!(field.is_checked(None, &saved), expect_checked, "{value:?}");
    }
}

#[test]
fn a_check_box_files_an_appearance_under_both_of_its_states() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("agree", rect(), FieldKindSpec::check())
            .appearance(WidgetAppearance::new().border(Color::from_rgb8(0, 0, 0))),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let widget = annots(&saved, 0).into_iter().next().expect("the widget");
    let states = widget
        .dict(&Name::from("AP"), &saved)
        .and_then(|ap| ap.dict(&Name::from("N"), &saved))
        .expect("/AP /N is a state dictionary, not a bare stream");

    // A toggle picks its face with `/AS`, so both faces have to be there —
    // one stream filed under both keys would give the box a tick it could not
    // clear.
    let mut keys: Vec<_> = states.keys().map(|key| key.as_bytes().to_vec()).collect();
    keys.sort();
    assert_eq!(keys, vec![b"Off".to_vec(), b"Yes".to_vec()]);
}

#[test]
fn a_created_radio_group_is_one_field_with_one_widget_per_button() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new(
            "shipping",
            rect(),
            FieldKindSpec::Radio {
                buttons: vec![
                    (Rect::new(72.0, 700.0, 88.0, 716.0), "standard".to_owned()),
                    (Rect::new(72.0, 676.0, 88.0, 692.0), "express".to_owned()),
                ],
            },
        )
        .value("express"),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let form = form_of(&saved);
    assert_eq!(form.len(), 1, "two buttons are one field, not two");

    let field = form.field("shipping").expect("the field by name");
    assert_eq!(field.kind, FieldKind::Radio);
    assert_eq!(field.widgets.len(), 2);
    assert_eq!(field.value(None, &saved), "express");
    assert_eq!(annots(&saved, 0).len(), 2);
}

#[test]
fn only_the_chosen_radio_button_shows_its_on_state() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new(
            "shipping",
            rect(),
            FieldKindSpec::Radio {
                buttons: vec![
                    (Rect::new(72.0, 700.0, 88.0, 716.0), "standard".to_owned()),
                    (Rect::new(72.0, 676.0, 88.0, 692.0), "express".to_owned()),
                ],
            },
        )
        .value("express"),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let states: Vec<_> = annots(&saved, 0)
        .iter()
        .filter_map(|dict| dict.byte_string(&Name::from("AS"), &saved))
        .collect();
    // A group whose every `/AS` named its own export value would draw with
    // every button lit, whatever `/V` said.
    assert_eq!(states, vec![b"Off".to_vec(), b"express".to_vec()]);
}

#[test]
fn field_attributes_reach_the_file() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("zip", rect(), FieldKindSpec::Text)
            .max_len(5)
            .comb(true)
            .required(true)
            .tooltip("Postal code"),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let field = form_of(&saved).field("zip").expect("the field").clone();

    assert!(field.flags.is_required());
    assert_eq!(field.flags.bits() & (1 << 24), 1 << 24, "the comb bit");
    assert_eq!(
        field.dict.int(&Name::from("MaxLen"), &saved),
        Some(5),
        "/MaxLen"
    );
    assert_eq!(
        field.dict.text(&Name::from("TU"), &saved).as_deref(),
        Some("Postal code"),
    );
}

#[test]
fn a_created_field_gives_the_form_a_default_face_to_lay_values_out_with() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    let fonts = catalog
        .dict(&Name::from("AcroForm"), &saved)
        .and_then(|form| form.dict(&Name::from("DR"), &saved))
        .and_then(|resources| resources.dict(&Name::from("Font"), &saved))
        .expect("the form carries /DR /Font");
    // The default `/DA` names `Helv`; a `/DR` without it would lay every
    // value out in a substituted face that measures differently.
    assert!(fonts.contains_key(&Name::from("Helv")));
}

#[test]
fn a_registered_face_is_the_one_a_da_can_name() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    let face = edit
        .standard_font(StandardFont::TimesBold)
        .expect("a standard face");
    add_form_font(&mut edit, "TiBo", &face).expect("register");
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text)
            .da("0 g /TiBo 11 Tf")
            .value("Ada"),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    let fonts = catalog
        .dict(&Name::from("AcroForm"), &saved)
        .and_then(|form| form.dict(&Name::from("DR"), &saved))
        .and_then(|resources| resources.dict(&Name::from("Font"), &saved))
        .expect("the form carries /DR /Font");
    assert!(fonts.contains_key(&Name::from("TiBo")));
    // Registering a face does not displace the default one.
    assert!(fonts.contains_key(&Name::from("Helv")));
}

#[test]
fn two_fields_land_on_the_same_form() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("first", rect(), FieldKindSpec::Text),
    )
    .expect("first");
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new(
            "second",
            Rect::new(72.0, 660.0, 300.0, 680.0),
            FieldKindSpec::Text,
        ),
    )
    .expect("second");

    let saved = roundtrip(&edit);
    // A second call that rebuilt `/AcroForm` rather than editing the one the
    // first call made would leave the document with one field.
    assert_eq!(form_of(&saved).len(), 2);
    assert_eq!(annots(&saved, 0).len(), 2);
}

#[test]
fn a_duplicate_name_is_refused_rather_than_merged() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text),
    )
    .expect("first");

    let again = add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text),
    );
    assert!(matches!(again, Err(Error::DuplicateFieldName(name)) if name == "full_name"));
}

#[test]
fn an_unnamed_field_and_an_empty_group_are_refused() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);

    // The reader drops a nameless field, so writing one would lose it
    // silently on the next load.
    assert!(matches!(
        add_form_field(
            &mut edit,
            0u32,
            &FieldSpec::new("", rect(), FieldKindSpec::Text)
        ),
        Err(Error::EmptyFieldName),
    ));

    assert!(matches!(
        add_form_field(
            &mut edit,
            0u32,
            &FieldSpec::new(
                "group",
                rect(),
                FieldKindSpec::Radio {
                    buttons: Vec::new()
                }
            )
        ),
        Err(Error::EmptyRadioGroup),
    ));
}

#[test]
fn a_created_text_field_lays_its_value_out_rather_than_drawing_chrome_alone() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    add_form_field(
        &mut edit,
        0u32,
        &FieldSpec::new("full_name", rect(), FieldKindSpec::Text).value("Ada"),
    )
    .expect("create");

    let saved = roundtrip(&edit);
    let widget = annots(&saved, 0).into_iter().next().expect("the widget");
    let stream = normal_stream(&widget, &saved).expect("/AP /N is a stream");

    // The whole point of creating a field rather than an empty box: the value
    // is in the stream, in the face the default `/DA` names, at a size the
    // auto-size sentinel picked.
    assert!(stream.contains("(Ada) Tj"), "{stream}");
    assert!(stream.contains("/Helv"), "{stream}");
    assert!(stream.contains("/Tx BMC"), "the form-field marked content");
}

/// The decompressed `/AP /N` stream of a widget with a single appearance.
fn normal_stream(dict: &Dict, doc: &Document) -> Option<String> {
    let ap = dict.dict(&Name::from("AP"), doc)?;
    let value = ap.get(&Name::from("N"), doc)?;
    let object = value.resolve(doc).ok()?;
    let Object::Stream(stream) = object.get() else {
        return None;
    };
    let decoded = pdfrum_filters::decode_chain(
        stream,
        0,
        doc,
        &Limits::default(),
        &mut Diagnostics::default(),
    );
    Some(String::from_utf8_lossy(&decoded.data).into_owned())
}
