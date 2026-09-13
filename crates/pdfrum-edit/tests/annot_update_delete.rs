//! Update and delete existing annotations.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{AnnotSpec, Color, Document, Name, Rect, SaveOptions, Subtype};

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("open hello_world")
}

fn save_reopen(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(bytes)).expect("reopen")
}

#[test]
fn add_update_reopen_keeps_ref_and_regenerates_ap() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
    let annot_ref = edit
        .add_annotation(0, AnnotSpec::highlight(rect, Color::from_rgb8(255, 230, 0)))
        .expect("add");

    edit.update_annotation(
        0,
        annot_ref,
        AnnotSpec::highlight(rect, Color::from_rgb8(0, 200, 0)).with_contents("updated"),
    )
    .expect("update");

    let saved = save_reopen(&edit);
    let page = saved.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype(), Subtype::Highlight);
    assert_eq!(annots[0].contents().as_deref(), Some("updated"));
    assert!(
        annots[0]
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "update should regenerate /AP"
    );
}

#[test]
fn add_delete_gone_on_reopen() {
    let doc = hello();
    let mut edit = doc.edit();
    let a = edit
        .add_annotation(
            0,
            AnnotSpec::text(
                Rect::new(10.0, 10.0, 30.0, 30.0),
                Color::from_rgb8(255, 200, 0),
            ),
        )
        .expect("add a");
    let b = edit
        .add_annotation(
            0,
            AnnotSpec::square(
                Rect::new(40.0, 40.0, 80.0, 80.0),
                Color::from_rgb8(0, 0, 255),
            ),
        )
        .expect("add b");

    assert!(edit.delete_annotation(0, a).expect("delete a"));
    assert!(!edit.delete_annotation(0, a).expect("already gone"));

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype(), Subtype::Square);
    let _ = b; // kept
}

#[test]
fn update_rejects_annot_not_on_page() {
    let doc = hello();
    let mut edit = doc.edit();
    let a = edit
        .add_annotation(
            0,
            AnnotSpec::caret(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
        )
        .expect("add");
    assert!(edit.delete_annotation(0, a).expect("delete"));
    let err = edit
        .update_annotation(
            0,
            a,
            AnnotSpec::caret(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK).with_contents("nope"),
        )
        .expect_err("must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("not on page") || msg.contains("AnnotNotOnPage"),
        "unexpected: {msg}"
    );
}

#[test]
fn update_and_delete_by_annots_index() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::text(
            Rect::new(10.0, 10.0, 30.0, 30.0),
            Color::from_rgb8(255, 200, 0),
        ),
    )
    .expect("add a");
    edit.add_annotation(
        0,
        AnnotSpec::square(
            Rect::new(40.0, 40.0, 80.0, 80.0),
            Color::from_rgb8(0, 0, 255),
        ),
    )
    .expect("add b");

    edit.update_annotation_at(
        0,
        0,
        AnnotSpec::text(
            Rect::new(10.0, 10.0, 30.0, 30.0),
            Color::from_rgb8(255, 200, 0),
        )
        .with_contents("first"),
    )
    .expect("update at 0");

    assert!(edit.delete_annotation_at(0, 1).expect("delete at 1"));

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype(), Subtype::Text);
    assert_eq!(annots[0].contents().as_deref(), Some("first"));
}

#[test]
fn index_helpers_reject_out_of_range() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::caret(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
    )
    .expect("add");
    let err = edit
        .update_annotation_at(
            0,
            3,
            AnnotSpec::caret(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
        )
        .expect_err("oob");
    let msg = err.to_string();
    assert!(
        msg.contains("out of range") || msg.contains("AnnotIndexOutOfRange"),
        "unexpected: {msg}"
    );
    let err = edit.delete_annotation_at(0, 9).expect_err("oob delete");
    let msg = err.to_string();
    assert!(
        msg.contains("out of range") || msg.contains("AnnotIndexOutOfRange"),
        "unexpected: {msg}"
    );
}

#[test]
fn index_helpers_promote_and_delete_inline_annots() {
    use pdfrum_edit::EditDoc;
    use pdfrum_object::{Array, Dict, Name, Object};
    use pdfrum_parser::{LoadOptions, load};

    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("read");
    let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).expect("load");
    let mut edit = EditDoc::new(&base);

    let (page_ref, mut page_dict, _) = edit.page_state(0).expect("state").expect("page");
    let inline = Dict::from_pairs([
        (Name::from("Type"), Object::Name(Name::from("Annot"))),
        (Name::from("Subtype"), Object::Name(Name::from("Text"))),
        (
            Name::from("Rect"),
            Object::Array(Array::of([
                Object::Real(1.0),
                Object::Real(1.0),
                Object::Real(20.0),
                Object::Real(20.0),
            ])),
        ),
        (Name::from("F"), Object::Int(4)),
        (Name::from("P"), Object::Ref(page_ref)),
    ]);
    page_dict.insert(
        Name::from("Annots"),
        Object::Array(Array::of([Object::Dict(inline)])),
    );
    edit.replace(page_ref, Object::Dict(page_dict));

    pdfrum_edit::update_annotation_at(
        &mut edit,
        0,
        0,
        AnnotSpec::text(
            Rect::new(1.0, 1.0, 20.0, 20.0),
            Color::from_rgb8(255, 200, 0),
        )
        .with_contents("promoted"),
    )
    .expect("update inline");

    // After promote+update the slot must be an indirect ref.
    let (_page_ref, page_dict, _) = edit.page_state(0).expect("state").expect("page");
    let annots = page_dict
        .array(&Name::from("Annots"), &edit)
        .expect("Annots");
    assert!(
        matches!(annots.raw_at(0), Some(Object::Ref(_))),
        "inline must be promoted to Ref"
    );

    let mut bytes = Vec::new();
    pdfrum_edit::save(&edit, &pdfrum_edit::SaveOptions::default(), &mut bytes).expect("save");
    let saved = Document::from_bytes(Arc::from(bytes)).expect("reopen");
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].contents().as_deref(), Some("promoted"));

    let mut edit = saved.edit();
    assert!(edit.delete_annotation_at(0, 0).expect("delete"));
    let saved = save_reopen(&edit);
    assert_eq!(saved.page(0).expect("page").annotations().count(), 0);
}
