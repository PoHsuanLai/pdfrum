//! Writing annotations through `DocEdit::add_annotation` and reading them
//! back after a save.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{AnnotSpec, Color, Document, Point, Quad, Rect, SaveOptions, Subtype};

fn save_reopen(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(bytes)).expect("reopen")
}

fn yellow() -> Color {
    Color::from_rgb8(255, 230, 0)
}

#[test]
fn highlight_round_trips_subtype_rect_contents_and_quads() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
    edit.add_annotation(
        0,
        AnnotSpec::Highlight {
            rect,
            color: yellow(),
            quads: vec![Quad::from_rect(rect)],
            contents: Some("mark".into()),
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("one annot");
    assert_eq!(annot.subtype(), Subtype::Highlight);
    assert!((annot.rect().x0 - 72.0).abs() < 0.01);
    assert!((annot.rect().y1 - 720.0).abs() < 0.01);
    assert_eq!(annot.contents().as_deref(), Some("mark"));
    assert!(annot.prints());
    let quads: Vec<_> = annot.quad_points().collect();
    assert_eq!(quads.len(), 1);
    assert!((quads[0].width() - rect.width()).abs() < 0.01);
    assert!((quads[0].height() - rect.height()).abs() < 0.01);
}

#[test]
fn text_note_writes_comment_icon_and_contents() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(50.0, 50.0, 70.0, 70.0);
    edit.add_annotation(
        0,
        AnnotSpec::Text {
            rect,
            color: yellow(),
            contents: Some("sticky".into()),
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("one annot");
    assert_eq!(annot.subtype(), Subtype::Text);
    assert_eq!(annot.contents().as_deref(), Some("sticky"));
    assert_eq!(
        annot
            .dict()
            .name(&pdfrum::Name::from("Name"))
            .map(pdfrum::Name::as_bytes),
        Some(&b"Comment"[..])
    );
    assert_eq!(annot.dict().bool(&pdfrum::Name::from("Open")), Some(false));
}

#[test]
fn square_writes_border_style() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(100.0, 100.0, 200.0, 180.0);
    edit.add_annotation(
        0,
        AnnotSpec::Square {
            rect,
            color: Color::from_rgb8(0, 128, 255),
            contents: None,
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("one annot");
    assert_eq!(annot.subtype(), Subtype::Square);
    let bs = annot
        .dict()
        .dict(&pdfrum::Name::from("BS"), saved.parser())
        .expect("BS");
    assert_eq!(
        bs.number(&pdfrum::Name::from("W"), saved.parser()),
        Some(2.0)
    );
}

#[test]
fn underline_round_trips_quads() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(80.0, 600.0, 300.0, 612.0);
    edit.add_annotation(
        0,
        AnnotSpec::Underline {
            rect,
            color: Color::from_rgb8(0, 0, 255),
            quads: vec![Quad::from_rect(rect)],
            contents: Some("u".into()),
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("one annot");
    assert_eq!(annot.subtype(), Subtype::Underline);
    assert_eq!(annot.contents().as_deref(), Some("u"));
    assert_eq!(annot.quad_points().len(), 1);
}

#[test]
fn ink_writes_inklist_strokes() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(40.0, 40.0, 120.0, 120.0);
    let strokes = vec![
        vec![
            Point::new(40.0, 40.0),
            Point::new(80.0, 90.0),
            Point::new(120.0, 50.0),
        ],
        vec![Point::new(50.0, 100.0), Point::new(110.0, 110.0)],
    ];
    edit.add_annotation(
        0,
        AnnotSpec::Ink {
            rect,
            color: Color::from_rgb8(200, 0, 0),
            strokes: strokes.clone(),
            contents: None,
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("one annot");
    assert_eq!(annot.subtype(), Subtype::Ink);
    let ink = annot
        .dict()
        .array(&pdfrum::Name::from("InkList"), saved.parser())
        .expect("InkList");
    assert_eq!(ink.len(), 2);
    let first = ink.array_at(0, saved.parser()).expect("stroke 0");
    assert_eq!(first.len(), 6);
    assert!((first.number_at_or_zero(0) - 40.0).abs() < 0.01);
    assert!((first.number_at_or_zero(5) - 50.0).abs() < 0.01);
}

#[test]
fn free_text_writes_contents_and_da() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(200.0, 200.0, 400.0, 260.0);
    edit.add_annotation(
        0,
        AnnotSpec::FreeText {
            rect,
            color: Color::BLACK,
            contents: "Hello café".into(),
            da: "0 0 0 rg /Helvetica 12 Tf".into(),
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("one annot");
    assert_eq!(annot.subtype(), Subtype::FreeText);
    assert_eq!(annot.contents().as_deref(), Some("Hello café"));
    let da = annot
        .dict()
        .text(&pdfrum::Name::from("DA"), saved.parser())
        .expect("DA");
    assert_eq!(da, "0 0 0 rg /Helvetica 12 Tf");
}

#[test]
fn empty_quads_are_refused() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let err = edit
        .add_annotation(
            0,
            AnnotSpec::Highlight {
                rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                color: yellow(),
                quads: vec![],
                contents: None,
            },
        )
        .expect_err("empty quads");
    let msg = err.to_string();
    assert!(
        msg.contains("quadrilateral") || msg.contains("Quad"),
        "got: {msg}"
    );
}

#[test]
fn multiple_subtypes_share_one_annots_array() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("fixture");
    let mut edit = doc.edit();
    let rect = Rect::new(10.0, 10.0, 30.0, 30.0);
    edit.add_annotation(
        0,
        AnnotSpec::Text {
            rect,
            color: yellow(),
            contents: Some("a".into()),
        },
    )
    .expect("text");
    edit.add_annotation(
        0,
        AnnotSpec::Square {
            rect: Rect::new(40.0, 40.0, 80.0, 80.0),
            color: yellow(),
            contents: None,
        },
    )
    .expect("square");
    let saved = save_reopen(&edit);
    let subtypes: Vec<_> = saved
        .page(0)
        .expect("page")
        .annotations()
        .map(|a| a.subtype())
        .collect();
    assert_eq!(subtypes, [Subtype::Text, Subtype::Square]);
}

#[test]
fn appending_to_existing_annots_keeps_prior_entries() {
    // text_form.pdf has a widget annotation.
    let doc = Document::open("tests/fixtures/text_form.pdf").expect("fixture");
    let before = doc.page(0).expect("page").annotations().count();
    assert!(before >= 1);
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::Text {
            rect: Rect::new(5.0, 5.0, 25.0, 25.0),
            color: yellow(),
            contents: Some("extra".into()),
        },
    )
    .expect("write");
    let saved = save_reopen(&edit);
    let after: Vec<_> = saved
        .page(0)
        .expect("page")
        .annotations()
        .map(|a| a.subtype())
        .collect();
    assert_eq!(after.len(), before + 1);
    assert_eq!(*after.last().expect("last"), Subtype::Text);
}
