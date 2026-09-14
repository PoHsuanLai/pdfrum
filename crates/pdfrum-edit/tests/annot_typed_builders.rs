//! Typed per-subtype builders produce the same specs as the enum setters,
//! and only expose the options their subtype actually has.

use kurbo::{Point, Rect};
use pdfrum_edit::{
    AnnotBorder, AnnotBorderStyle, AnnotLinkHighlight, AnnotSpec, CaretSpec, CircleSpec, InkSpec,
    LineEndingStyle, LineSpec, LinkSpec, MarkupKind, MarkupSpec, SquareSpec, TextSpec,
};
use peniko::Color;

fn rect() -> Rect {
    Rect::new(0.0, 0.0, 100.0, 20.0)
}

fn red() -> Color {
    Color::from_rgb8(255, 0, 0)
}

fn blue() -> Color {
    Color::from_rgb8(0, 0, 255)
}

#[test]
fn line_builder_matches_the_enum_setters() {
    let (a, b) = (Point::new(0.0, 0.0), Point::new(100.0, 20.0));
    let border = AnnotBorder::solid(3.0).with_style(AnnotBorderStyle::Dashed);

    let built: AnnotSpec = LineSpec::new(rect(), red(), a, b)
        .border(border)
        .endings(LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow)
        .interior(blue())
        .contents("note")
        .into();

    let expected = AnnotSpec::line(rect(), red(), a, b)
        .with_border(border)
        .with_line_endings(LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow)
        .with_interior(blue())
        .with_contents("note");

    assert_eq!(built, expected);
}

#[test]
fn link_builder_matches_the_enum_setters() {
    let border = AnnotBorder::solid(2.0);
    let built: AnnotSpec = LinkSpec::uri(rect(), "https://example.com")
        .color(blue())
        .border(border)
        .highlight(AnnotLinkHighlight::Outline)
        .contents("link")
        .into();

    let expected = AnnotSpec::link(rect(), "https://example.com")
        .with_color(blue())
        .with_border(border)
        .with_highlight(AnnotLinkHighlight::Outline)
        .with_contents("link");

    assert_eq!(built, expected);
}

#[test]
fn text_builder_matches_the_enum_setters() {
    let built: AnnotSpec = TextSpec::new(rect(), red())
        .icon("Note")
        .open(true)
        .contents("sticky")
        .into();

    let expected = AnnotSpec::text(rect(), red())
        .with_icon("Note")
        .with_open(true)
        .with_contents("sticky");

    assert_eq!(built, expected);
}

#[test]
fn square_circle_and_ink_carry_their_border() {
    let border = AnnotBorder::solid(4.0).with_style(AnnotBorderStyle::Beveled);

    let square: AnnotSpec = SquareSpec::new(rect(), red()).border(border).into();
    assert_eq!(square, AnnotSpec::square(rect(), red()).with_border(border));

    let circle: AnnotSpec = CircleSpec::new(rect(), red()).border(border).into();
    assert_eq!(circle, AnnotSpec::circle(rect(), red()).with_border(border));

    let strokes = vec![vec![Point::new(0.0, 0.0), Point::new(5.0, 5.0)]];
    let ink: AnnotSpec = InkSpec::new(rect(), red(), strokes.clone())
        .border(border)
        .into();
    assert_eq!(
        ink,
        AnnotSpec::ink(rect(), red(), strokes).with_border(border)
    );
}

#[test]
fn every_markup_kind_picks_its_subtype() {
    let r = rect();
    for (kind, expected) in [
        (MarkupKind::Highlight, AnnotSpec::highlight(r, red())),
        (MarkupKind::Underline, AnnotSpec::underline(r, red())),
        (MarkupKind::StrikeOut, AnnotSpec::strike_out(r, red())),
        (MarkupKind::Squiggly, AnnotSpec::squiggly(r, red())),
    ] {
        let built: AnnotSpec = MarkupSpec::new(kind, r, red()).into();
        assert_eq!(built, expected, "{kind:?}");
    }
}

#[test]
fn markup_quads_replace_the_default_single_quad() {
    let r = rect();
    let quads = [
        Rect::new(0.0, 0.0, 50.0, 20.0).into(),
        Rect::new(50.0, 0.0, 100.0, 20.0).into(),
    ];
    let built: AnnotSpec = MarkupSpec::new(MarkupKind::Highlight, r, red())
        .quads(quads)
        .into();

    match built {
        AnnotSpec::Highlight { ref quads, .. } => assert_eq!(quads.len(), 2),
        other => panic!("expected Highlight, got {other:?}"),
    }
}

#[test]
fn caret_builder_round_trips() {
    let built: AnnotSpec = CaretSpec::new(rect(), red()).contents("caret").into();
    assert_eq!(
        built,
        AnnotSpec::caret(rect(), red()).with_contents("caret")
    );
}

#[test]
fn defaults_are_untouched_when_no_option_is_set() {
    let built: AnnotSpec = SquareSpec::new(rect(), red()).into();
    assert_eq!(built, AnnotSpec::square(rect(), red()));

    let built: AnnotSpec = TextSpec::new(rect(), red()).into();
    assert_eq!(built, AnnotSpec::text(rect(), red()));
}
