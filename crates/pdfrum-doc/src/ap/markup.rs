//! The nine per-subtype appearance generators that draw shapes rather than
//! text.
//!
//! Every one of these writes its coordinates through the **six-significant-
//! digit** writer, not the shortest-round-trip one that the newer form-field
//! and border code uses — these are the older generators and were never
//! converted. So a coordinate of `0.5` appears here as `0.5` and in a border
//! as `.5`, in the same stream. That is the byte parity being matched.
//!
//! Two of them also change the annotation's rectangle: a sticky note becomes
//! a fixed 20×20 box anchored at its original bottom-left, and an ink
//! annotation inflates by half its border width so wide strokes are not
//! clipped away. Neither mutates anything — both report the new rectangle to
//! the caller, which records it in the overlay.

// The midpoints below are written as `(a + b) / 2.0` on purpose: the
// standard-library midpoint is *more* accurate than that, and these numbers
// are written straight into a content stream whose bytes must match. The
// narrowing casts are equally deliberate — the arithmetic happens in double
// precision and is narrowed once, exactly where upstream narrows it.
#![allow(
    clippy::manual_midpoint,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names
)]

use kurbo::Rect;
use pdfrum_object::{Array, Dict, Resolve, names as obj_names};

use crate::annot::quad;
use crate::ap::border;
use crate::ap::emit::{Content, Float, PaintOp, color_op, color_with_default, paint_operator};
use crate::color::Color;
use crate::geom;
use crate::names;

/// The prologue every one of these streams opens with.
///
/// Note the trailing **space**, not a newline — except in the pop-up
/// generator, which uses a newline. Both are reproduced.
const GS_SPACE: &str = "/GS gs ";

/// One generator's output.
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    /// The content-stream bytes.
    pub stream: Vec<u8>,
    /// A rewritten `/Rect`, when this generator moved one.
    pub rect_override: Option<Rect>,
    /// Whether the appearance's bounding box comes from the quadrilaterals
    /// rather than from the rectangle.
    pub is_text_markup: bool,
    /// The blend mode the graphics state names.
    pub blend_multiply: bool,
    /// The `/Font` sub-dictionary the appearance stream's `/Resources` needs,
    /// as `{ resource name: font dictionary }`.
    ///
    /// Empty for every generator that writes no text — which is all of them
    /// but [`crate::ap::freetext::free_text`], since a stream with no `Tf`
    /// needs no font to resolve. Upstream draws the same distinction by
    /// passing `nullptr` to `GenerateResourcesDict` from every generator
    /// except the free-text one, which passes `GenerateResourceFontDict`
    /// (`cpdf_generateap.cpp:1141-1144`).
    pub font_resources: Option<Dict>,
}

impl Generated {
    fn plain(stream: Content) -> Generated {
        Generated {
            stream: stream.into_bytes(),
            rect_override: None,
            is_text_markup: false,
            blend_multiply: false,
            font_resources: None,
        }
    }

    fn markup(stream: Content) -> Generated {
        Generated {
            is_text_markup: true,
            ..Generated::plain(stream)
        }
    }
}

/// A `Highlight`: one filled quadrilateral per attachment point, composited
/// with the **Multiply** blend mode so the text below shows through.
#[must_use]
pub fn highlight<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    let mut out = Content::new();
    out.raw(GS_SPACE);
    out.raw(&color_with_default(
        dict.array(names::C, r).as_ref(),
        Color::Rgb(1.0, 1.0, 0.0),
        PaintOp::Fill,
    ));
    if let Some(quads) = dict.array(names::QUAD_POINTS, r) {
        for index in 0..quad::quad_point_count(Some(&quads)) {
            let rect = geom::normalize(quad::rect_from_quad_points_array(&quads, index));
            let (l, b, right, t) = corners(rect);
            out.point(l, t, Float::G6);
            out.raw("m ");
            out.point(right, t, Float::G6);
            out.raw("l ");
            out.point(right, b, Float::G6);
            out.raw("l ");
            out.point(l, b, Float::G6);
            out.raw("l h f\n");
        }
    }
    Generated {
        blend_multiply: true,
        ..Generated::markup(out)
    }
}

/// An `Underline`: a one-unit line just above each quadrilateral's bottom.
#[must_use]
pub fn underline<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    let mut out = Content::new();
    out.raw(GS_SPACE);
    out.raw(&stroke_default_black(dict, r));
    if let Some(quads) = dict.array(names::QUAD_POINTS, r) {
        // The width is written once, before the loop.
        out.raw("1 w ");
        for index in 0..quad::quad_point_count(Some(&quads)) {
            let rect = geom::normalize(quad::rect_from_quad_points_array(&quads, index));
            let (l, b, right, _) = corners(rect);
            out.point(l, b + 1.0, Float::G6);
            out.raw("m ");
            out.point(right, b + 1.0, Float::G6);
            out.raw("l S\n");
        }
    }
    Generated::markup(out)
}

/// A `StrikeOut`: a line through each quadrilateral's middle.
///
/// The width is written **inside** the loop here, unlike the underline and
/// squiggly generators — so a two-quad strikeout writes `1 w` twice.
#[must_use]
pub fn strike_out<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    let mut out = Content::new();
    out.raw(GS_SPACE);
    out.raw(&stroke_default_black(dict, r));
    if let Some(quads) = dict.array(names::QUAD_POINTS, r) {
        for index in 0..quad::quad_point_count(Some(&quads)) {
            let rect = geom::normalize(quad::rect_from_quad_points_array(&quads, index));
            let (l, b, right, t) = corners(rect);
            let y = (t + b) / 2.0;
            out.raw("1 w ");
            out.point(l, y, Float::G6);
            out.raw("m ");
            out.point(right, y, Float::G6);
            out.raw("l S\n");
        }
    }
    Generated::markup(out)
}

/// A `Squiggly`: a zig-zag along each quadrilateral's bottom edge.
///
/// The zig-zag steps two units at a time and alternates between the bottom
/// and two units above it; the final segment lands wherever the remainder
/// puts it, which is why a quadrilateral narrower than one step draws only
/// its opening move and that closing segment.
#[must_use]
pub fn squiggly<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    const DELTA: f32 = 2.0;
    let mut out = Content::new();
    out.raw(GS_SPACE);
    out.raw(&stroke_default_black(dict, r));
    if let Some(quads) = dict.array(names::QUAD_POINTS, r) {
        out.raw("1 w ");
        for index in 0..quad::quad_point_count(Some(&quads)) {
            let rect = geom::normalize(quad::rect_from_quad_points_array(&quads, index));
            let (l, bottom, right, _) = corners(rect);
            let top = bottom + DELTA;
            out.point(l, top, Float::G6);
            out.raw("m ");

            let mut x = l + DELTA;
            let mut upwards = false;
            while x < right {
                out.point(x, if upwards { top } else { bottom }, Float::G6);
                out.raw("l ");
                x += DELTA;
                upwards = !upwards;
            }
            let remainder = right - (x - DELTA);
            let y = if upwards {
                bottom + remainder
            } else {
                top - remainder
            };
            out.point(right, y, Float::G6);
            out.raw("l ");
            out.raw("S\n");
        }
    }
    Generated::markup(out)
}

/// An `Ink`: the freehand strokes of `/InkList`.
///
/// Returns nothing at all — not an empty appearance, no appearance — when
/// `/InkList` is missing or empty, or when the border width is not positive.
/// Each sub-array's first point is written **twice**, once as the move and
/// again as the first line, because the line loop starts at index zero.
#[must_use]
pub fn ink<R: Resolve>(dict: &Dict, r: &R) -> Option<Generated> {
    let ink_list = dict.array(names::INK_LIST, r).filter(|a| !a.is_empty())?;
    let width = border::border_width(dict, r);
    if width <= 0.0 {
        return None;
    }

    let mut out = Content::new();
    out.raw(GS_SPACE);
    out.raw(&stroke_default_black(dict, r));
    out.num(width, Float::G6);
    out.raw("w ");
    out.raw(&border::dash_pattern_string(dict, r));

    for index in 0..ink_list.len() {
        let Some(points) = ink_list.array_at(index, r).filter(|a| a.len() >= 2) else {
            continue;
        };
        out.point(
            points.number_at_or_zero(0),
            points.number_at_or_zero(1),
            Float::G6,
        );
        out.raw("m ");
        // Stepping in pairs from zero, so the first point repeats and an odd
        // trailing coordinate is left out.
        let mut at = 0;
        while at + 1 < points.len() {
            out.point(
                points.number_at_or_zero(at),
                points.number_at_or_zero(at + 1),
                Float::G6,
            );
            out.raw("l ");
            at += 2;
        }
        out.raw("S\n");
    }

    // Wide strokes near the edge would be clipped away by the original
    // rectangle, so it grows by half the width.
    let rect = dict.rect(obj_names::RECT, r);
    Some(Generated {
        rect_override: Some(geom::inflate(rect, width / 2.0, width / 2.0)),
        ..Generated::plain(out)
    })
}

/// A `Square`: the rectangle, stroked and filled per its two colour keys.
#[must_use]
pub fn square<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    let (mut out, rect, stroke, fill) = shape_preamble(dict, r);
    out.rect(rect, Float::G6);
    out.raw("re ");
    out.raw(paint_operator(stroke, fill));
    out.raw("\n");
    Generated::plain(out)
}

/// A `Circle`: four Bézier arcs inscribed in the rectangle.
#[must_use]
pub fn circle<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    // A precalculated approximation of `4·tan(π/8)/3`, which times the radius
    // gives the control-point offset for a quarter arc.
    const K: f64 = 0.5523;

    let (mut out, rect, stroke, fill) = shape_preamble(dict, r);
    let (left, bottom, right, top) = corners(rect);
    let mid_x = (left + right) / 2.0;
    let mid_y = (top + bottom) / 2.0;
    // Computed in double precision and narrowed once, as upstream does.
    let dx = (K * f64::from(geom::width(rect)) / 2.0) as f32;
    let dy = (K * f64::from(geom::height(rect)) / 2.0) as f32;

    out.point(mid_x, top, Float::G6);
    out.raw("m\n");
    for (points, op) in [
        (
            [(mid_x + dx, top), (right, mid_y + dy), (right, mid_y)],
            "c\n",
        ),
        (
            [(right, mid_y - dy), (mid_x + dx, bottom), (mid_x, bottom)],
            "c\n",
        ),
        (
            [(mid_x - dx, bottom), (left, mid_y - dy), (left, mid_y)],
            "c\n",
        ),
        ([(left, mid_y + dy), (mid_x - dx, top), (mid_x, top)], "c\n"),
    ] {
        for (x, y) in points {
            out.point(x, y, Float::G6);
        }
        out.raw(op);
    }
    out.raw(paint_operator(stroke, fill));
    out.raw("\n");
    Generated::plain(out)
}

/// A `Text` sticky note: a fixed 20×20 icon, which **replaces** the
/// annotation's rectangle.
///
/// The new rectangle is anchored at the raw, unnormalized rectangle's
/// bottom-left corner, so an inverted `/Rect` still produces a well-formed
/// note box in a possibly surprising place.
#[must_use]
pub fn text<R: Resolve>(dict: &Dict, r: &R) -> Generated {
    const NOTE: f32 = 20.0;
    let rect = dict.rect(obj_names::RECT, r);
    let (left, bottom) = (geom::left(rect), geom::bottom(rect));
    let note = geom::rect(left, bottom, left + NOTE, bottom + NOTE);

    let mut out = Content::new();
    out.raw(GS_SPACE);
    out.raw(&text_symbol(note));
    Generated {
        rect_override: Some(note),
        ..Generated::plain(out)
    }
}

/// The sticky-note icon: a page outline with a folded corner and three
/// ruled lines.
#[must_use]
pub fn text_symbol(rect: Rect) -> String {
    const TIP: f32 = 4.0;
    let mut out = Content::new();
    out.raw(&color_op(Color::Rgb(1.0, 1.0, 0.0), PaintOp::Fill));
    out.raw(&color_op(Color::Rgb(0.0, 0.0, 0.0), PaintOp::Stroke));
    out.raw("1 w\n");

    let outer = geom::translate(geom::deflate(rect, 0.5, 0.5), 0.0, 0.0);
    let (ol, ob, or_, ot) = (
        geom::left(outer),
        geom::bottom(outer) + TIP,
        geom::right(outer),
        geom::top(outer),
    );
    // The fold: a small square whose "top" sits *below* its "bottom". These
    // are read out as coordinates, never normalized.
    let (fl, fb) = (ol + TIP, ob);
    let (fr, ft) = (fl + TIP, fb - TIP);
    let fold_mid = (fl + fr) / 2.0;

    for (x, y, op) in [
        (ol, ob, "m\n"),
        (ol, ot, "l\n"),
        (or_, ot, "l\n"),
        (or_, ob, "l\n"),
        (fr, fb, "l\n"),
        (fold_mid, ft, "l\n"),
        (fl, fb, "l\n"),
        (ol, ob, "l\n"),
    ] {
        out.point(x, y, Float::Shortest);
        out.raw(op);
    }

    let (line_left, line_right) = (ol + 2.0, or_ - 2.0);
    let step = (ot - ob) / 4.0;
    let mut y = ot;
    for _ in 0..3 {
        y -= step;
        out.point(line_left, y, Float::Shortest);
        out.raw("m\n");
        out.point(line_right, y, Float::Shortest);
        out.raw("l\n");
    }
    out.raw("B*\n");
    out.as_str().to_owned()
}

/// A `Popup`'s chrome: a yellow box with a black outline. The text inside it
/// is added by the caller, which owns the layout engine.
#[must_use]
pub fn popup_frame<R: Resolve>(dict: &Dict, r: &R) -> Content {
    let mut out = Content::new();
    // A newline here, where every other generator writes a space.
    out.raw("/GS gs\n");
    out.raw(&color_op(Color::Rgb(1.0, 1.0, 0.0), PaintOp::Fill));
    out.raw(&color_op(Color::Rgb(0.0, 0.0, 0.0), PaintOp::Stroke));
    out.raw("1 w\n");
    let rect = geom::deflate(geom::normalize(dict.rect(obj_names::RECT, r)), 0.5, 0.5);
    out.rect(rect, Float::G6);
    out.raw("re b\n");
    out
}

/// The shared opening of the square and circle generators: the two colours,
/// the border width and dash pattern, and the rectangle they draw into.
fn shape_preamble<R: Resolve>(dict: &Dict, r: &R) -> (Content, Rect, bool, bool) {
    let mut out = Content::new();
    out.raw(GS_SPACE);

    let interior = dict.array(names::IC, r);
    out.raw(&color_with_default(
        interior.as_ref(),
        Color::Transparent,
        PaintOp::Fill,
    ));
    out.raw(&stroke_default_black(dict, r));

    let width = border::border_width(dict, r);
    let stroke = width > 0.0;
    if stroke {
        out.num(width, Float::G6);
        out.raw("w ");
        out.raw(&border::dash_pattern_string(dict, r));
    }

    let mut rect = geom::normalize(dict.rect(obj_names::RECT, r));
    if stroke {
        // A stroke paints half a width either side of the path, so the path
        // moves inward to keep the ink inside the rectangle.
        rect = geom::deflate(rect, width / 2.0, width / 2.0);
    }
    // A present-but-empty `/IC` says "do not fill"; an absent one says the
    // same, and only a usable array fills.
    let fill = interior.is_some_and(|array: Array| !array.is_empty());
    (out, rect, stroke, fill)
}

/// The stroke colour from `/C`, defaulting to black.
fn stroke_default_black<R: Resolve>(dict: &Dict, r: &R) -> String {
    color_with_default(
        dict.array(names::C, r).as_ref(),
        Color::Rgb(0.0, 0.0, 0.0),
        PaintOp::Stroke,
    )
}

/// A rectangle's four edges, in the order the emitters read them.
fn corners(rect: Rect) -> (f32, f32, f32, f32) {
    (
        geom::left(rect),
        geom::bottom(rect),
        geom::right(rect),
        geom::top(rect),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        circle, highlight, ink, square, squiggly, strike_out, text, text_symbol, underline,
    };
    use crate::geom;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn numbers(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::from)))
    }

    /// One quadrilateral spanning (10, 10) to (30, 20), written in the
    /// canonical corner order.
    fn one_quad() -> Object {
        numbers(&[10.0, 20.0, 30.0, 20.0, 10.0, 10.0, 30.0, 10.0])
    }

    fn stream(generated: &super::Generated) -> String {
        String::from_utf8_lossy(&generated.stream).into_owned()
    }

    #[test]
    fn a_highlight_fills_each_quadrilateral_and_asks_for_multiply() {
        let annot = dict(&[("QuadPoints", one_quad())]);
        let got = highlight(&annot, &NoResolve);
        assert!(got.blend_multiply);
        assert!(got.is_text_markup);
        assert_eq!(
            stream(&got),
            "/GS gs 1 1 0 rg\n10 20 m 30 20 l 30 10 l 10 10 l h f\n"
        );
    }

    #[test]
    fn a_highlights_default_colour_is_yellow_and_an_empty_array_beats_it() {
        let with_empty = dict(&[
            ("C", Object::Array(Array::new())),
            ("QuadPoints", one_quad()),
        ]);
        let got = stream(&highlight(&with_empty, &NoResolve));
        assert!(got.starts_with("/GS gs 10 20 m"), "{got}");
    }

    #[test]
    fn an_underline_writes_its_width_once_and_a_strikeout_writes_it_per_quad() {
        let two_quads = numbers(&[
            10.0, 20.0, 30.0, 20.0, 10.0, 10.0, 30.0, 10.0, 10.0, 40.0, 30.0, 40.0, 10.0, 30.0,
            30.0, 30.0,
        ]);
        let annot = dict(&[("QuadPoints", two_quads)]);
        assert_eq!(
            stream(&underline(&annot, &NoResolve))
                .matches("1 w ")
                .count(),
            1
        );
        assert_eq!(
            stream(&strike_out(&annot, &NoResolve))
                .matches("1 w ")
                .count(),
            2
        );
    }

    #[test]
    fn an_underline_sits_one_unit_above_the_bottom_edge() {
        let annot = dict(&[("QuadPoints", one_quad())]);
        assert_eq!(
            stream(&underline(&annot, &NoResolve)),
            "/GS gs 0 0 0 RG\n1 w 10 11 m 30 11 l S\n"
        );
    }

    #[test]
    fn a_strikeout_runs_through_the_vertical_middle() {
        let annot = dict(&[("QuadPoints", one_quad())]);
        assert_eq!(
            stream(&strike_out(&annot, &NoResolve)),
            "/GS gs 0 0 0 RG\n1 w 10 15 m 30 15 l S\n"
        );
    }

    #[test]
    fn a_squiggly_alternates_every_two_units() {
        let annot = dict(&[(
            "QuadPoints",
            numbers(&[10.0, 20.0, 16.0, 20.0, 10.0, 10.0, 16.0, 10.0]),
        )]);
        assert_eq!(
            stream(&squiggly(&annot, &NoResolve)),
            "/GS gs 0 0 0 RG\n1 w 10 12 m 12 10 l 14 12 l 16 10 l S\n"
        );
    }

    #[test]
    fn a_quadrilateral_narrower_than_one_step_draws_only_its_ends() {
        // Width 1, so the zig-zag loop never runs.
        let annot = dict(&[(
            "QuadPoints",
            numbers(&[10.0, 20.0, 11.0, 20.0, 10.0, 10.0, 11.0, 10.0]),
        )]);
        // The closing segment lands at `top - remainder`, and the remainder
        // is the full width because the loop never advanced past the start.
        let got = stream(&squiggly(&annot, &NoResolve));
        assert_eq!(got, "/GS gs 0 0 0 RG\n1 w 10 12 m 11 11 l S\n");
    }

    #[test]
    fn ink_repeats_its_first_point_and_drops_an_odd_trailing_coordinate() {
        let annot = dict(&[
            (
                "InkList",
                Object::Array(Array::of([numbers(&[1.0, 2.0, 3.0, 4.0, 5.0])])),
            ),
            ("Rect", numbers(&[0.0, 0.0, 10.0, 10.0])),
        ]);
        let got = ink(&annot, &NoResolve).expect("has an ink list");
        assert_eq!(stream(&got), "/GS gs 0 0 0 RG\n1 w 1 2 m 1 2 l 3 4 l S\n");
        // The rectangle grew by half the border width.
        assert_eq!(got.rect_override, Some(geom::rect(-0.5, -0.5, 10.5, 10.5)));
    }

    #[test]
    fn ink_declines_entirely_without_a_usable_list_or_a_positive_width() {
        assert!(ink(&Dict::new(), &NoResolve).is_none());
        assert!(
            ink(
                &dict(&[("InkList", Object::Array(Array::new()))]),
                &NoResolve
            )
            .is_none()
        );
        let zero_width = dict(&[
            (
                "InkList",
                Object::Array(Array::of([numbers(&[1.0, 2.0, 3.0, 4.0])])),
            ),
            ("Border", numbers(&[0.0, 0.0, 0.0])),
        ]);
        assert!(ink(&zero_width, &NoResolve).is_none());
    }

    #[test]
    fn a_square_deflates_by_half_its_border_and_names_its_paint_operator() {
        let annot = dict(&[("Rect", numbers(&[0.0, 0.0, 10.0, 10.0]))]);
        // No interior colour: stroke only.
        assert_eq!(
            stream(&square(&annot, &NoResolve)),
            "/GS gs 0 0 0 RG\n1 w 0.5 0.5 9 9 re s\n"
        );

        let filled = dict(&[
            ("Rect", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("IC", numbers(&[0.0, 0.0, 1.0])),
        ]);
        let got = stream(&square(&filled, &NoResolve));
        assert!(got.contains("0 0 1 rg\n"), "{got}");
        assert!(got.ends_with("re b\n"), "{got}");
    }

    #[test]
    fn a_present_but_empty_interior_colour_leaves_the_square_unfilled() {
        let annot = dict(&[
            ("Rect", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("IC", Object::Array(Array::new())),
        ]);
        assert!(stream(&square(&annot, &NoResolve)).ends_with("re s\n"));
    }

    #[test]
    fn a_circle_draws_four_bezier_arcs() {
        let annot = dict(&[("Rect", numbers(&[0.0, 0.0, 10.0, 10.0]))]);
        let got = stream(&circle(&annot, &NoResolve));
        assert_eq!(got.matches(" c\n").count(), 4);
        assert!(got.contains("5 9.5 m\n"), "{got}");
        assert!(got.ends_with("s\n"), "{got}");
    }

    #[test]
    fn a_sticky_note_replaces_its_rectangle_with_a_twenty_unit_box() {
        let annot = dict(&[("Rect", numbers(&[234.372, 340.046, 400.0, 500.0]))]);
        let got = text(&annot, &NoResolve);
        assert_eq!(
            got.rect_override,
            Some(geom::rect(234.372, 340.046, 254.372, 360.046))
        );
        assert!(stream(&got).starts_with("/GS gs 1 1 0 rg\n0 0 0 RG\n1 w\n"));
    }

    #[test]
    fn the_note_icon_folds_a_corner_and_rules_three_lines() {
        let got = text_symbol(geom::rect(0.0, 0.0, 20.0, 20.0));
        // Eight outline points, then three ruled pairs, then the paint op.
        assert_eq!(got.matches(" m\n").count(), 4);
        assert_eq!(got.matches(" l\n").count(), 10);
        assert!(got.ends_with("B*\n"), "{got}");
    }
}
