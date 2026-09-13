//! Written annotations carry `/AP` and survive flatten.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use pdfrum::{AnnotSpec, Color, Document, FlattenMode, Flattened, Name, Rect, SaveOptions};
use std::sync::Arc;

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("open hello_world")
}

#[test]
fn highlight_write_includes_appearance_and_flattens() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
    edit.add_annotation(0, AnnotSpec::highlight(rect, Color::from_rgb8(255, 230, 0)))
        .expect("add");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");

    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 1, "expected one highlight");
    let ap = annots[0]
        .dict()
        .dict(&Name::from("AP"), reopened.parser())
        .expect("AP present");
    assert!(
        ap.get(&Name::from("N"), reopened.parser()).is_some(),
        "expected /AP /N stream"
    );

    // Flatten should succeed now that appearances exist.
    let mut edit = reopened.edit();
    assert_eq!(
        edit.flatten(0, FlattenMode::Display).expect("flatten"),
        Flattened::Done
    );
}

#[test]
fn ink_and_underline_get_appearances() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(50.0, 50.0, 150.0, 150.0);
    edit.add_annotation(
        0,
        AnnotSpec::ink(
            rect,
            Color::from_rgb8(0, 0, 0),
            vec![vec![
                kurbo::Point::new(60.0, 60.0),
                kurbo::Point::new(140.0, 140.0),
            ]],
        ),
    )
    .expect("ink");
    edit.add_annotation(
        0,
        AnnotSpec::underline(
            Rect::new(72.0, 680.0, 200.0, 700.0),
            Color::from_rgb8(0, 0, 255),
        ),
    )
    .expect("underline");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");
    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 2);
    for ann in &annots {
        assert!(
            ann.dict()
                .dict(&Name::from("AP"), reopened.parser())
                .is_some(),
            "missing AP on {:?}",
            ann.subtype()
        );
    }
}

#[test]
fn strike_out_and_squiggly_get_appearances() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::strike_out(
            Rect::new(72.0, 660.0, 200.0, 680.0),
            Color::from_rgb8(200, 0, 0),
        ),
    )
    .expect("strike_out");
    edit.add_annotation(
        0,
        AnnotSpec::squiggly(
            Rect::new(72.0, 640.0, 200.0, 660.0),
            Color::from_rgb8(0, 160, 0),
        ),
    )
    .expect("squiggly");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");
    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 2);
    for ann in &annots {
        assert!(
            ann.dict()
                .dict(&Name::from("AP"), reopened.parser())
                .is_some(),
            "missing AP on {:?}",
            ann.subtype()
        );
    }
}

#[test]
fn line_link_caret_get_appearances() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::line(
            Rect::new(10.0, 10.0, 100.0, 100.0),
            Color::from_rgb8(0, 0, 0),
            kurbo::Point::new(20.0, 20.0),
            kurbo::Point::new(80.0, 80.0),
        ),
    )
    .expect("line");
    edit.add_annotation(
        0,
        AnnotSpec::link(
            Rect::new(72.0, 700.0, 200.0, 720.0),
            "https://example.test/",
        ),
    )
    .expect("link");
    edit.add_annotation(
        0,
        AnnotSpec::caret(
            Rect::new(30.0, 30.0, 40.0, 50.0),
            Color::from_rgb8(200, 0, 0),
        ),
    )
    .expect("caret");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");
    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 3);
    for ann in &annots {
        assert!(
            ann.dict()
                .dict(&Name::from("AP"), reopened.parser())
                .is_some(),
            "missing AP on {:?}",
            ann.subtype()
        );
    }
}
