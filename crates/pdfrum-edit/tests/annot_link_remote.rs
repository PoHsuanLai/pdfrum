//! `GoToR` and Launch link actions.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{AnnotGoToView, AnnotSpec, Document, Name, Rect, SaveOptions, Subtype};

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
fn gotor_and_launch_round_trip() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link_goto_r(
            Rect::new(10.0, 10.0, 80.0, 24.0),
            "other.pdf",
            2,
            AnnotGoToView::Fit,
        ),
    )
    .expect("gotor");
    edit.add_annotation(
        0,
        AnnotSpec::link_launch(Rect::new(10.0, 40.0, 80.0, 54.0), "notes.txt"),
    )
    .expect("launch");

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 2);
    assert_eq!(annots[0].subtype(), Subtype::Link);
    let a0 = annots[0]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        a0.name(&Name::from("S")).map(|n| n.as_bytes().to_vec()),
        Some(b"GoToR".to_vec())
    );
    let f = a0.string(&Name::from("F")).expect("F");
    assert!(String::from_utf8_lossy(f.as_bytes()).contains("other.pdf"));
    let d = a0.array(&Name::from("D"), saved.parser()).expect("D");
    assert_eq!(d.int_at(0), Some(2));
    assert_eq!(
        d.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"Fit".to_vec())
    );

    let a1 = annots[1]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        a1.name(&Name::from("S")).map(|n| n.as_bytes().to_vec()),
        Some(b"Launch".to_vec())
    );
    let f = a1.string(&Name::from("F")).expect("F");
    assert!(String::from_utf8_lossy(f.as_bytes()).contains("notes.txt"));
}
