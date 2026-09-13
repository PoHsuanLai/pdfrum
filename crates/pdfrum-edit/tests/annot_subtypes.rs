//! Circle, Line, Link, and Caret write + reopen.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{AnnotSpec, Color, Document, Name, Point, Rect, SaveOptions, Subtype};

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
fn circle_round_trips_with_appearance() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(80.0, 80.0, 160.0, 160.0);
    edit.add_annotation(0, AnnotSpec::circle(rect, Color::from_rgb8(0, 128, 255)))
        .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.subtype(), Subtype::Circle);
    assert!(
        annot
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "Circle should get /AP from the appearance pipeline"
    );
}

#[test]
fn line_round_trips_endpoints() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(10.0, 10.0, 200.0, 200.0);
    edit.add_annotation(
        0,
        AnnotSpec::line(
            rect,
            Color::from_rgb8(0, 0, 0),
            Point::new(20.0, 20.0),
            Point::new(180.0, 180.0),
        )
        .with_contents("diagonal"),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.subtype(), Subtype::Line);
    assert_eq!(annot.contents().as_deref(), Some("diagonal"));
    let l = annot
        .dict()
        .array(&Name::from("L"), saved.parser())
        .expect("L");
    assert_eq!(l.len(), 4);
    assert!((l.number_at(0).unwrap() - 20.0).abs() < 0.01);
    assert!((l.number_at(3).unwrap() - 180.0).abs() < 0.01);
    assert!(
        annot
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "Line should get /AP from the appearance pipeline"
    );
}

#[test]
fn link_round_trips_uri_action() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link(
            Rect::new(72.0, 700.0, 200.0, 720.0),
            "https://example.test/x",
        )
        .with_contents("go"),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.subtype(), Subtype::Link);
    assert_eq!(annot.contents().as_deref(), Some("go"));
    let action = annot
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"URI"[..])
    );
    let uri = action.string(&Name::from("URI")).expect("URI");
    let decoded = String::from_utf8_lossy(uri.as_bytes());
    assert!(
        decoded.contains("https://example.test/x"),
        "uri: {decoded:?}"
    );
    assert!(
        annot
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "Link should get /AP from the appearance pipeline"
    );
}

#[test]
fn caret_round_trips() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::caret(
            Rect::new(30.0, 30.0, 40.0, 50.0),
            Color::from_rgb8(200, 0, 0),
        )
        .with_contents("insert"),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.subtype(), Subtype::Caret);
    assert_eq!(annot.contents().as_deref(), Some("insert"));
    assert!(
        annot
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "Caret should get /AP from the appearance pipeline"
    );
}
