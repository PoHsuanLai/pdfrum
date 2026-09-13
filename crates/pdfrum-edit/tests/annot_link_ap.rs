//! Richer Link /BS /C /H appearance keys.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{
    AnnotBorder, AnnotBorderStyle, AnnotLinkHighlight, AnnotSpec, Color, Document, Name, Rect,
    SaveOptions, Subtype,
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
fn link_writes_bs_color_and_highlight() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link(
            Rect::new(72.0, 700.0, 200.0, 720.0),
            "https://example.test/",
        )
        .with_color(Color::from_rgb8(0, 0, 255))
        .with_border(AnnotBorder::solid(1.5).with_style(AnnotBorderStyle::Underline))
        .with_highlight(AnnotLinkHighlight::Outline),
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
    let dict = annot.dict();
    let c = dict.array(&Name::from("C"), saved.parser()).expect("C");
    assert!((c.number_at(2).unwrap() - 1.0).abs() < 0.01);
    let bs = dict.dict(&Name::from("BS"), saved.parser()).expect("BS");
    assert!((bs.number(&Name::from("W"), saved.parser()).unwrap() - 1.5).abs() < 0.01);
    assert_eq!(
        bs.name(&Name::from("S")).map(|n| n.as_bytes().to_vec()),
        Some(b"U".to_vec())
    );
    assert_eq!(
        dict.name(&Name::from("H")).map(|n| n.as_bytes().to_vec()),
        Some(b"O".to_vec())
    );
    assert!(
        dict.dict(&Name::from("AP"), saved.parser()).is_some(),
        "AP present"
    );
}
