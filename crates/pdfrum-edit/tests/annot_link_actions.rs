//! Link /A URI and GoTo round-trips.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{
    AnnotGoToView, AnnotLinkAction, AnnotSpec, Document, Name, Rect, SaveOptions, Subtype,
};
use pdfrum_object::ObjRef;

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("open hello_world")
}

fn hello_2() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world_2_pages.pdf"
    ))
    .expect("open hello_world_2_pages")
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

#[test]
fn uri_link_still_round_trips() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link(
            Rect::new(72.0, 700.0, 200.0, 720.0),
            "https://example.test/uri",
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
    assert_eq!(annot.subtype(), Subtype::Link);
    let action = annot
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"URI"[..])
    );
    let uri = action.string(&Name::from("URI")).expect("URI");
    assert!(
        String::from_utf8_lossy(uri.as_bytes()).contains("https://example.test/uri"),
        "uri bytes"
    );
}

#[test]
fn goto_fit_round_trips_via_dict() {
    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link_goto(
            Rect::new(10.0, 10.0, 80.0, 24.0),
            target,
            AnnotGoToView::Fit,
        )
        .with_contents("to page 2"),
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
    assert_eq!(annot.contents().as_deref(), Some("to page 2"));
    let action = annot
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"GoTo"[..])
    );
    let dest = action.array(&Name::from("D"), saved.parser()).expect("D");
    assert_eq!(dest.len(), 2);
    assert_eq!(dest.reference_at(0), Some(target));
    assert_eq!(
        dest.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"Fit".to_vec())
    );
}

#[test]
fn goto_xyz_and_named_write_expected_keys() {
    let doc = hello_2();
    let target = page_ref(&doc, 0);
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link_goto(
            Rect::new(10.0, 40.0, 80.0, 54.0),
            target,
            AnnotGoToView::Xyz {
                left: Some(0.0),
                top: Some(792.0),
                zoom: None,
            },
        ),
    )
    .expect("xyz");
    edit.add_annotation(
        0,
        AnnotSpec::link_named(Rect::new(10.0, 60.0, 80.0, 74.0), "Chapter1"),
    )
    .expect("named");

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 2);

    let xyz_action = annots[0]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    let dest = xyz_action
        .array(&Name::from("D"), saved.parser())
        .expect("D");
    assert_eq!(dest.len(), 5);
    assert_eq!(
        dest.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"XYZ".to_vec())
    );
    assert!((dest.number_at(3).unwrap() - 792.0).abs() < 0.01);
    assert!(matches!(
        dest.get(4, saved.parser()).map(|v| v.get().clone()),
        Some(pdfrum_object::Object::Null)
    ));

    let named_action = annots[1]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        named_action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"GoTo"[..])
    );
    let name = named_action.string(&Name::from("D")).expect("D string");
    assert!(
        String::from_utf8_lossy(name.as_bytes()).contains("Chapter1"),
        "named dest"
    );

    // Typed construction stays available for callers matching on the write payload.
    let _ = AnnotLinkAction::Uri(String::from("x"));
}
