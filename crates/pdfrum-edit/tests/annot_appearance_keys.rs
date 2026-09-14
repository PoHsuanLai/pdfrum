//! Keys the appearance generator already reads, now that the writer emits
//! them: `/CA` opacity, `/IC` fill on Square and Circle, `/BS /D` dashes, and
//! `/Q` alignment on `FreeText`.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_doc::vt::Alignment;
use pdfrum_edit::{
    AnnotBorder, AnnotBorderStyle, AnnotSpec, CircleSpec, DEFAULT_DA, EditDoc, SaveOptions,
    SquareSpec, add_annotation, save,
};
use pdfrum_object::{Dict, Name, Object, Resolve};
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

/// Writes one annotation and reloads the saved document.
///
/// Takes `AnnotSpec` so the typed builders reach it through their own
/// `Into<AnnotSpec>`; `AnnotWrite` is what `with_opacity` and friends answer,
/// and those call sites convert first.
fn written(spec: impl Into<pdfrum_edit::AnnotWrite>) -> Document {
    let doc = hello();
    let mut edit = EditDoc::new(&doc);
    add_annotation(&mut edit, 0u32, spec).expect("write");
    let mut out = Vec::new();
    save(&edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

/// The saved document's single annotation.
fn annot(doc: &Document) -> Dict {
    let page = doc.page(0u32).expect("page");
    page.dict
        .array(&Name::from("Annots"), doc)
        .and_then(|annots| annots.dict_at(0, doc))
        .expect("one annotation")
}

#[test]
fn opacity_is_written_and_clamped() {
    let doc =
        written(AnnotSpec::highlight(rect(), Color::from_rgb8(255, 255, 0)).with_opacity(0.4));
    let dict = annot(&doc);
    assert_eq!(dict.number(&Name::from("CA"), &doc), Some(0.4));

    // Out of range is clamped rather than refused: a reader takes `/CA`
    // outside 0..1 as undefined, and the nearest legal value is what was
    // meant.
    let doc =
        written(AnnotSpec::highlight(rect(), Color::from_rgb8(255, 255, 0)).with_opacity(2.5));
    assert_eq!(annot(&doc).number(&Name::from("CA"), &doc), Some(1.0));
}

#[test]
fn no_opacity_writes_no_key() {
    // An absent `/CA` reads as fully opaque, which is the right default —
    // writing `1.0` everywhere would be noise.
    let doc = written(AnnotSpec::highlight(rect(), Color::from_rgb8(255, 255, 0)));
    assert!(annot(&doc).get(&Name::from("CA"), &doc).is_none());
}

#[test]
fn a_square_can_be_filled() {
    let doc = written(AnnotSpec::from(
        SquareSpec::new(rect(), Color::from_rgb8(0, 0, 0)).interior(Color::from_rgb8(255, 0, 0)),
    ));
    let dict = annot(&doc);
    let ic = dict.array(&Name::from("IC"), &doc).expect("an /IC");
    assert_eq!(
        (0..ic.len())
            .filter_map(|index| ic.number_at(index))
            .collect::<Vec<_>>(),
        vec![1.0, 0.0, 0.0]
    );
}

#[test]
fn a_circle_can_be_filled() {
    let doc = written(AnnotSpec::from(
        CircleSpec::new(rect(), Color::from_rgb8(0, 0, 0)).interior(Color::from_rgb8(0, 0, 255)),
    ));
    assert!(annot(&doc).array(&Name::from("IC"), &doc).is_some());
}

#[test]
fn an_unfilled_shape_writes_no_interior() {
    // An absent `/IC` and an empty one both say "do not fill"; the absent one
    // is what a shape with no interior means.
    let doc = written(AnnotSpec::from(SquareSpec::new(
        rect(),
        Color::from_rgb8(0, 0, 0),
    )));
    assert!(annot(&doc).get(&Name::from("IC"), &doc).is_none());
}

#[test]
fn a_dashed_border_carries_its_pattern() {
    let doc = written(AnnotSpec::from(
        SquareSpec::new(rect(), Color::from_rgb8(0, 0, 0)).border(
            AnnotBorder::solid(1.0)
                .with_style(AnnotBorderStyle::Dash)
                .with_dash([4, 2, 0]),
        ),
    ));
    let bs = annot(&doc).dict(&Name::from("BS"), &doc).expect("a /BS");
    let d = bs.array(&Name::from("D"), &doc).expect("a /D");
    assert_eq!(
        (0..d.len())
            .filter_map(|index| d.int_at(index))
            .collect::<Vec<_>>(),
        vec![4, 2, 0]
    );
}

#[test]
fn a_solid_border_writes_no_dash_pattern() {
    // `/D` means nothing to a style that is not dashed. Writing it anyway
    // would be a key a reader is entitled to ignore, and a puzzle for anyone
    // reading the file.
    let doc = written(AnnotSpec::from(
        SquareSpec::new(rect(), Color::from_rgb8(0, 0, 0))
            .border(AnnotBorder::solid(1.0).with_dash([4, 2, 0])),
    ));
    let bs = annot(&doc).dict(&Name::from("BS"), &doc).expect("a /BS");
    assert!(bs.get(&Name::from("D"), &doc).is_none());
}

#[test]
fn free_text_alignment_round_trips() {
    for (align, quadding) in [
        (Alignment::Left, 0),
        (Alignment::Center, 1),
        (Alignment::Right, 2),
    ] {
        let doc = written(
            AnnotSpec::free_text(rect(), Color::from_rgb8(0, 0, 0), "text", DEFAULT_DA)
                .with_align(align),
        );
        assert_eq!(
            annot(&doc).int(&Name::from("Q"), &doc),
            Some(quadding),
            "{align:?} writes /Q {quadding}"
        );
    }
}

#[test]
fn free_text_without_an_alignment_writes_no_quadding() {
    let doc = written(AnnotSpec::free_text(
        rect(),
        Color::from_rgb8(0, 0, 0),
        "text",
        DEFAULT_DA,
    ));
    assert!(annot(&doc).get(&Name::from("Q"), &doc).is_none());
}

#[test]
fn opacity_reaches_the_generated_appearance() {
    // The point of `/CA` is that the generator folds it into the appearance's
    // `/ExtGState` — a written key that never reached the stream would leave
    // the annotation opaque in any reader that draws the `/AP`.
    let doc = written(AnnotSpec::square(rect(), Color::from_rgb8(0, 0, 0)).with_opacity(0.25));
    let dict = annot(&doc);
    let ap = dict.dict(&Name::from("AP"), &doc).expect("an /AP");
    let stream = match ap.raw(&Name::from("N")) {
        Some(Object::Ref(reference)) => doc
            .fetch(*reference)
            .ok()
            .and_then(|object| object.as_stream().cloned()),
        Some(Object::Stream(stream)) => Some((**stream).clone()),
        _ => None,
    }
    .expect("a normal appearance stream");

    let gs = stream
        .dict
        .dict(&Name::from("Resources"), &doc)
        .and_then(|resources| resources.dict(&Name::from("ExtGState"), &doc))
        .and_then(|states| states.dict(&Name::from("GS"), &doc))
        .expect("the generated /ExtGState");
    assert_eq!(gs.number(&Name::from("CA"), &doc), Some(0.25));
    assert_eq!(gs.number(&Name::from("ca"), &doc), Some(0.25));
}
