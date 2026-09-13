//! Line `/LE` endings.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{
    AnnotSpec, Color, Document, LineEndingStyle, Name, Point, Rect, SaveOptions, Subtype,
};

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
fn default_line_omits_le() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::line(
            Rect::new(10.0, 10.0, 100.0, 100.0),
            Color::BLACK,
            Point::new(20.0, 20.0),
            Point::new(80.0, 80.0),
        ),
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
    assert!(
        annot
            .dict()
            .array(&Name::from("LE"), saved.parser())
            .is_none(),
        "default must omit /LE"
    );
}

#[test]
fn line_endings_round_trip() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::line(
            Rect::new(10.0, 10.0, 100.0, 100.0),
            Color::BLACK,
            Point::new(20.0, 20.0),
            Point::new(80.0, 80.0),
        )
        .with_line_endings(LineEndingStyle::None, LineEndingStyle::ClosedArrow),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    let le = annot
        .dict()
        .array(&Name::from("LE"), saved.parser())
        .expect("LE");
    assert_eq!(le.len(), 2);
    assert_eq!(
        le.get(0, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"None".to_vec())
    );
    assert_eq!(
        le.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"ClosedArrow".to_vec())
    );
    assert!(
        annot
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "still has /AP"
    );
}

#[test]
fn line_endings_still_generate_ap() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::line(
            Rect::new(10.0, 10.0, 100.0, 100.0),
            Color::BLACK,
            Point::new(20.0, 20.0),
            Point::new(80.0, 80.0),
        )
        .with_line_endings(LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert!(
        annot
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "/AP present with /LE"
    );
    let le = annot
        .dict()
        .array(&Name::from("LE"), saved.parser())
        .expect("LE");
    assert_eq!(le.len(), 2);
}
