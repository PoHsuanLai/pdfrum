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
            AnnotSpec::text(Rect::new(10.0, 10.0, 30.0, 30.0), Color::from_rgb8(255, 200, 0)),
        )
        .expect("add a");
    let b = edit
        .add_annotation(
            0,
            AnnotSpec::square(Rect::new(40.0, 40.0, 80.0, 80.0), Color::from_rgb8(0, 0, 255)),
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
