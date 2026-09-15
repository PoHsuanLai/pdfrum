//! Every typed builder can say everything the enum could.
//!
//! The builders were added to make illegal states unrepresentable, but until
//! now `contents` was the only shared setter — so an annotation needing an
//! author had to convert to `AnnotSpec` and use the enum's own setters, which
//! is the path the builders exist to retire.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_doc::AnnotFlags;
use pdfrum_edit::{
    AnnotMeta, AnnotSpec, AnnotWrite, CaretSpec, CircleSpec, EditDoc, FreeTextSpec, InkSpec,
    LineSpec, LinkSpec, MarkupKind, MarkupSpec, SaveOptions, SquareSpec, TextSpec, add_annotation,
    save,
};
use pdfrum_object::{Dict, Name};
use pdfrum_parser::{Document, LoadOptions, load};
use peniko::Color;

fn hello() -> Document {
    let bytes: Arc<[u8]> = Arc::from(
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/files/hello.pdf"
        ))[..],
    );
    load(bytes, &LoadOptions::default()).expect("the fixture loads")
}

fn rect() -> kurbo::Rect {
    kurbo::Rect::new(10.0, 10.0, 60.0, 40.0)
}

fn black() -> Color {
    Color::from_rgb8(0, 0, 0)
}

/// Writes one annotation and returns it as saved.
fn written(write: AnnotWrite) -> (Document, Dict) {
    let doc = hello();
    let mut edit = EditDoc::new(&doc);
    add_annotation(&mut edit, 0u32, write).expect("write");
    let mut out = Vec::new();
    save(&edit, &SaveOptions::default(), &mut out).expect("save");
    let saved = load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload");
    let dict = saved
        .page(0u32)
        .expect("page")
        .dict
        .array(&Name::from("Annots"), &saved)
        .and_then(|annots| annots.dict_at(0, &saved))
        .expect("one annotation");
    (saved, dict)
}

/// `/T` as written.
fn author(doc: &Document, dict: &Dict) -> Option<String> {
    dict.text(&Name::from("T"), doc)
}

#[test]
fn every_builder_can_name_an_author_without_touching_the_enum() {
    // The point of the change: each of these used to require
    // `.into()` to an `AnnotSpec` first.
    let cases: Vec<AnnotWrite> = vec![
        MarkupSpec::new(MarkupKind::Highlight, rect(), black()).author("a"),
        TextSpec::new(rect(), black()).author("a"),
        SquareSpec::new(rect(), black()).author("a"),
        CircleSpec::new(rect(), black()).author("a"),
        InkSpec::new(rect(), black(), Vec::new()).author("a"),
        LineSpec::new(
            rect(),
            black(),
            kurbo::Point::new(10.0, 10.0),
            kurbo::Point::new(60.0, 40.0),
        )
        .author("a"),
        LinkSpec::uri(rect(), "https://example.test/").author("a"),
        CaretSpec::new(rect(), black()).author("a"),
        FreeTextSpec::new(rect(), black(), "text").author("a"),
    ];

    for write in cases {
        let (doc, dict) = written(write);
        assert_eq!(author(&doc, &dict).as_deref(), Some("a"));
    }
}

#[test]
fn the_other_shared_setters_reach_the_file() {
    let (doc, dict) = written(TextSpec::new(rect(), black()).name("unique-id"));
    assert_eq!(
        dict.text(&Name::from("NM"), &doc).as_deref(),
        Some("unique-id")
    );

    let (doc, dict) = written(TextSpec::new(rect(), black()).modified("D:20260101000000Z"));
    assert_eq!(
        dict.text(&Name::from("M"), &doc).as_deref(),
        Some("D:20260101000000Z")
    );

    let (doc, dict) = written(TextSpec::new(rect(), black()).flags(AnnotFlags::HIDDEN));
    assert_eq!(
        dict.int(&Name::from("F"), &doc),
        Some(AnnotFlags::HIDDEN.bits())
    );

    let (doc, dict) =
        written(TextSpec::new(rect(), black()).meta(AnnotMeta::default().with_author("from-meta")));
    assert_eq!(author(&doc, &dict).as_deref(), Some("from-meta"));
}

#[test]
fn a_builder_setter_chains_after_contents() {
    // `contents` stays on the builder and answers `Self`, so it composes
    // before the metadata setters hand back an `AnnotWrite`.
    let (doc, dict) = written(TextSpec::new(rect(), black()).contents("note").author("a"));
    assert_eq!(author(&doc, &dict).as_deref(), Some("a"));
    assert_eq!(
        dict.text(&Name::from("Contents"), &doc).as_deref(),
        Some("note")
    );
}

#[test]
fn free_text_defaults_its_da_and_takes_an_override() {
    let (doc, dict) = written(AnnotSpec::from(FreeTextSpec::new(rect(), black(), "text")).into());
    assert_eq!(
        dict.text(&Name::from("DA"), &doc).as_deref(),
        Some(pdfrum_edit::DEFAULT_DA)
    );

    let (doc, dict) = written(
        AnnotSpec::from(FreeTextSpec::new(rect(), black(), "text").da("0 0 1 rg /Helv 18 Tf"))
            .into(),
    );
    assert_eq!(
        dict.text(&Name::from("DA"), &doc).as_deref(),
        Some("0 0 1 rg /Helv 18 Tf")
    );
}
