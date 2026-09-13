//! Extra `GoTo` destination views.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{AnnotGoToView, AnnotSpec, Document, Name, Rect, SaveOptions};
use pdfrum_object::ObjRef;

fn hello_2() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world_2_pages.pdf"
    ))
    .expect("open")
}

fn save_reopen(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(bytes)).expect("reopen")
}

fn page_ref(doc: &Document, index: u32) -> ObjRef {
    doc.parser()
        .page(index)
        .expect("page")
        .reference
        .expect("page object ref")
}

fn dest_name(saved: &Document, annot_index: usize) -> Vec<u8> {
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    let action = annots[annot_index]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    let dest = action.array(&Name::from("D"), saved.parser()).expect("D");
    dest.get(1, saved.parser())
        .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec()))
        .expect("mode name")
}

#[test]
fn fit_family_views_round_trip() {
    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    let views = [
        AnnotGoToView::FitH { top: Some(700.0) },
        AnnotGoToView::FitV { left: Some(72.0) },
        AnnotGoToView::FitR {
            left: 10.0,
            bottom: 20.0,
            right: 200.0,
            top: 300.0,
        },
        AnnotGoToView::FitB,
        AnnotGoToView::FitBH { top: None },
        AnnotGoToView::FitBV { left: Some(0.0) },
    ];
    let ys = [10.0_f64, 30.0, 50.0, 70.0, 90.0, 110.0];
    for (view, y) in views.into_iter().zip(ys) {
        edit.add_annotation(
            0,
            AnnotSpec::link_goto(Rect::new(10.0, y, 80.0, y + 14.0), target, view),
        )
        .expect("add");
    }
    let saved = save_reopen(&edit);
    let expected: [&[u8]; 6] = [b"FitH", b"FitV", b"FitR", b"FitB", b"FitBH", b"FitBV"];
    for (i, name) in expected.into_iter().enumerate() {
        assert_eq!(dest_name(&saved, i), name, "view {i}");
    }
    // FitR carries four numbers after the mode.
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    let dest = annots[2]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A")
        .array(&Name::from("D"), saved.parser())
        .expect("D");
    assert_eq!(dest.len(), 6);
    assert!((dest.number_at(2).unwrap() - 10.0).abs() < 0.01);
    assert!((dest.number_at(5).unwrap() - 300.0).abs() < 0.01);
    // FitBH null top.
    let dest = annots[4]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A")
        .array(&Name::from("D"), saved.parser())
        .expect("D");
    assert!(matches!(
        dest.get(2, saved.parser()).map(|v| v.get().clone()),
        Some(pdfrum_object::Object::Null)
    ));
}
