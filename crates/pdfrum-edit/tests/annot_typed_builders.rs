//! Typed per-subtype builders produce the spec their options describe, and
//! only expose the options their subtype actually has.
//!
//! These used to assert builder-vs-enum parity. The enum's constructors and
//! per-variant setters are gone, so there is no second path to agree with:
//! each test now reads the variant the builder produced.

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
fn a_line_carries_every_option_it_was_given() {
    let (a, b) = (Point::new(0.0, 0.0), Point::new(100.0, 20.0));
    let border = AnnotBorder::solid(3.0).with_style(AnnotBorderStyle::Dash);

    let built: AnnotSpec = LineSpec::new(rect(), red(), a, b)
        .border(border)
        .endings(LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow)
        .interior(blue())
        .contents("note")
        .into();

    match built {
        AnnotSpec::Line {
            start,
            end,
            border: b2,
            line_endings,
            interior,
            contents,
            ..
        } => {
            assert_eq!((start, end), (a, b));
            assert_eq!(b2, border);
            assert_eq!(
                line_endings,
                Some((LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow))
            );
            assert_eq!(interior, Some(blue()));
            assert_eq!(contents.as_deref(), Some("note"));
        }
        other => panic!("expected Line, got {other:?}"),
    }
}

#[test]
fn a_link_carries_every_option_it_was_given() {
    let border = AnnotBorder::solid(2.0);
    let built: AnnotSpec = LinkSpec::uri(rect(), "https://example.com")
        .color(blue())
        .border(border)
        .highlight(AnnotLinkHighlight::Outline)
        .contents("link")
        .into();

    match built {
        AnnotSpec::Link {
            color,
            border: b2,
            highlight,
            contents,
            ..
        } => {
            assert_eq!(color, Some(blue()));
            assert_eq!(b2, border);
            assert_eq!(highlight, AnnotLinkHighlight::Outline);
            assert_eq!(contents.as_deref(), Some("link"));
        }
        other => panic!("expected Link, got {other:?}"),
    }
}

#[test]
fn a_text_note_carries_its_icon_and_open_state() {
    let built: AnnotSpec = TextSpec::new(rect(), red())
        .icon("Note")
        .open(true)
        .contents("sticky")
        .into();

    match built {
        AnnotSpec::Text {
            icon,
            open,
            contents,
            ..
        } => {
            assert_eq!(icon.as_bytes(), b"Note");
            assert!(open);
            assert_eq!(contents.as_deref(), Some("sticky"));
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn square_circle_and_ink_carry_their_border() {
    let border = AnnotBorder::solid(4.0).with_style(AnnotBorderStyle::Beveled);

    let square: AnnotSpec = SquareSpec::new(rect(), red()).border(border).into();
    assert!(matches!(square, AnnotSpec::Square { border: b, .. } if b == border));

    let circle: AnnotSpec = CircleSpec::new(rect(), red()).border(border).into();
    assert!(matches!(circle, AnnotSpec::Circle { border: b, .. } if b == border));

    let strokes = vec![vec![Point::new(0.0, 0.0), Point::new(5.0, 5.0)]];
    let ink: AnnotSpec = InkSpec::new(rect(), red(), strokes.clone())
        .border(border)
        .into();
    match ink {
        AnnotSpec::Ink {
            border: b2,
            strokes: s2,
            ..
        } => {
            assert_eq!(b2, border);
            assert_eq!(s2, strokes);
        }
        other => panic!("expected Ink, got {other:?}"),
    }
}

#[test]
fn every_markup_kind_picks_its_subtype() {
    let r = rect();
    for kind in [
        MarkupKind::Highlight,
        MarkupKind::Underline,
        MarkupKind::StrikeOut,
        MarkupKind::Squiggly,
    ] {
        let built: AnnotSpec = MarkupSpec::new(kind, r, red()).into();
        let matched = matches!(
            (kind, &built),
            (MarkupKind::Highlight, AnnotSpec::Highlight { .. })
                | (MarkupKind::Underline, AnnotSpec::Underline { .. })
                | (MarkupKind::StrikeOut, AnnotSpec::StrikeOut { .. })
                | (MarkupKind::Squiggly, AnnotSpec::Squiggly { .. })
        );
        assert!(matched, "{kind:?} produced {built:?}");
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
    match built {
        AnnotSpec::Caret { contents, .. } => assert_eq!(contents.as_deref(), Some("caret")),
        other => panic!("expected Caret, got {other:?}"),
    }
}

#[test]
fn defaults_are_untouched_when_no_option_is_set() {
    // The defaults the removed constructors used to supply now live in the
    // `From` impls, so this pins them in their new home.
    let built: AnnotSpec = SquareSpec::new(rect(), red()).into();
    match built {
        AnnotSpec::Square {
            border, contents, ..
        } => {
            assert_eq!(border, AnnotBorder::default());
            assert_eq!(contents, None);
        }
        other => panic!("expected Square, got {other:?}"),
    }

    let built: AnnotSpec = TextSpec::new(rect(), red()).into();
    match built {
        AnnotSpec::Text {
            icon,
            open,
            contents,
            ..
        } => {
            assert_eq!(icon.as_bytes(), b"Comment", "the default sticky-note icon");
            assert!(!open, "a note starts closed");
            assert_eq!(contents, None);
        }
        other => panic!("expected Text, got {other:?}"),
    }

    // A link's border is a hairline, not the width-2 the drawn shapes take.
    let built: AnnotSpec = LinkSpec::uri(rect(), "https://example.test/").into();
    match built {
        AnnotSpec::Link {
            border, highlight, ..
        } => {
            assert_eq!(border, AnnotBorder::solid(1.0));
            assert_eq!(highlight, AnnotLinkHighlight::default());
        }
        other => panic!("expected Link, got {other:?}"),
    }
}
