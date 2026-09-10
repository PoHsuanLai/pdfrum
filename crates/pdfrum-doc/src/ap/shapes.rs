//! The six glyph shapes a checkbox or radio button draws when it is on.
//!
//! These are **paths, not glyphs**. The "ZapfDingbats" the upstream comment
//! mentions refers only to which code point names which shape: the first
//! character of the widget's `/MK /CA` caption selects one of six, using that
//! font's encoding as a naming convention, and the shape itself is then drawn
//! from a hard-coded outline. Nothing loads a font.
//!
//! The colour comes from the widget's `/DA`, and a transparent one writes no
//! colour operator — but the path and its paint operator are emitted anyway,
//! so the shape is still a page object even when it paints nothing. That is
//! what makes an unadorned radio button report one path rather than none.

// The midpoint below is written as `(a + b) / 2.0` on purpose: the standard
// library's is *more* accurate, and these coordinates go straight into a
// content stream whose bytes must match.
#![allow(clippy::manual_midpoint)]

use kurbo::Rect;

use crate::ap::emit::{Content, Float, PaintOp, color_op, color_op_via};
use crate::color::Color;
use crate::geom;

/// The circle-approximation constant: `4·(√2 − 1)/3`, the control-point
/// offset that turns a quarter circle into one cubic Bézier.
const BEZIER: f32 = 0.552_284_8;

/// Which shape a checkbox or radio button draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStyle {
    /// A check mark.
    Check,
    /// A circle.
    Circle,
    /// An X.
    Cross,
    /// A diamond.
    Diamond,
    /// A square.
    Square,
    /// A five-pointed star.
    Star,
}

/// Reads the style from a caption's first character.
///
/// Returns nothing for an unrecognized character *and* for an empty caption,
/// which lets the two callers apply their own defaults: a checkbox falls back
/// to a check mark, a radio button to a circle.
#[must_use]
pub fn style_from_caption(caption: &str) -> Option<CheckStyle> {
    match caption.chars().next()? {
        '4' => Some(CheckStyle::Check),
        '8' => Some(CheckStyle::Cross),
        'H' => Some(CheckStyle::Star),
        'l' => Some(CheckStyle::Circle),
        'n' => Some(CheckStyle::Square),
        'u' => Some(CheckStyle::Diamond),
        _ => None,
    }
}

/// A checkbox's on-state shape, centred and scaled into its box.
///
/// Every style but the check mark and the cross shrinks to two thirds.
#[must_use]
pub fn check_box(bbox: Rect, style: CheckStyle, color: Color) -> String {
    let centre = geom::center_square(bbox);
    let fitted = match style {
        CheckStyle::Check | CheckStyle::Cross => centre,
        _ => geom::scale_from_center(centre, 2.0 / 3.0),
    };
    shape(fitted, style, color)
}

/// A radio button's on-state shape.
///
/// Same as a checkbox's except that a circle shrinks to **one half** rather
/// than two thirds.
#[must_use]
pub fn radio_button(bbox: Rect, style: CheckStyle, color: Color) -> String {
    let centre = geom::center_square(bbox);
    let fitted = match style {
        CheckStyle::Check | CheckStyle::Cross => centre,
        CheckStyle::Circle => geom::scale_from_center(centre, 0.5),
        _ => geom::scale_from_center(centre, 2.0 / 3.0),
    };
    shape(fitted, style, color)
}

/// The grey a combo box's drop button is filled with, as a byte triple over
/// 255 — the value the source writes rather than the fraction it equals.
const DROP_BUTTON_GREY: f32 = 220.0 / 255.0;

/// The fixed two-unit beveled border the drop button is drawn with.
const DROP_BUTTON_BORDER: crate::ap::border::BorderStyleInfo = crate::ap::border::BorderStyleInfo {
    width: 2.0,
    style: crate::ap::border::BorderStyle::Beveled,
    dash: crate::ap::border::Dash {
        on: 3,
        gap: 0,
        phase: 0,
    },
};

/// The arrow-and-bevel button at the right end of a combo box.
///
/// Three pieces, each in its own graphics state: a pale grey fill, a beveled
/// border with fixed greys, and — only when the box exceeds six units on
/// **both** axes — a downward triangle six wide and three tall about its
/// centre. An empty box draws nothing at all.
///
/// The grey is written through the six-digit writer, which is the one visible
/// difference between the two producers of this button: 220/255 is
/// `0.862745` there and `.862745098` through the shortest writer. Every other
/// number in the button is exact in both.
#[must_use]
pub fn drop_button(bbox: Rect) -> String {
    if geom::is_empty(bbox) {
        return String::new();
    }
    let grey = color_op_via(Color::Gray(DROP_BUTTON_GREY), PaintOp::Fill, Float::G6);
    let mut out = Content::new();

    out.raw("q\n");
    out.raw(&grey);
    out.rect(bbox, Float::Shortest);
    out.raw("re f\n");
    out.raw("Q\n");

    let border = crate::ap::border::border_path(bbox, DROP_BUTTON_BORDER, Color::Gray(0.0));
    if !border.is_empty() {
        out.raw("q\n");
        out.raw(&border);
        out.raw("Q\n");
    }

    let (cx, cy) = (
        (geom::left(bbox) + geom::right(bbox)) / 2.0,
        (geom::top(bbox) + geom::bottom(bbox)) / 2.0,
    );
    if geom::is_float_bigger(geom::width(bbox), 6.0)
        && geom::is_float_bigger(geom::height(bbox), 6.0)
    {
        out.raw("q\n0 g\n");
        for (x, y, op) in [
            (cx - 3.0, cy + 1.5, "m\n"),
            (cx + 3.0, cy + 1.5, "l\n"),
            (cx, cy - 1.5, "l\n"),
            (cx - 3.0, cy + 1.5, "l f\n"),
        ] {
            out.point(x, y, Float::Shortest);
            out.raw(op);
        }
        // The grey is repeated after the arrow, which leaves the state set to
        // it rather than to black — harmless inside the `q`/`Q`, and part of
        // the bytes.
        out.raw(&grey);
        out.raw("Q\n");
    }
    out.as_str().to_owned()
}

/// One shape, wrapped in its own graphics state.
///
/// The cross strokes; everything else fills — including the star, which
/// `GetAppStream_Star` paints with `kFillOperator`. A transparent colour
/// writes no colour operator, and the path is drawn regardless.
fn shape(bbox: Rect, style: CheckStyle, color: Color) -> String {
    let (paint, operator) = match style {
        CheckStyle::Cross => (PaintOp::Stroke, "S\n"),
        _ => (PaintOp::Fill, "f\n"),
    };
    let mut out = Content::new();
    out.raw("q\n");
    out.raw(&color_op(color, paint));
    out.raw(&path(bbox, style));
    out.raw(operator);
    out.raw("Q\n");
    out.as_str().to_owned()
}

/// The bare path for one shape.
#[must_use]
pub fn path(bbox: Rect, style: CheckStyle) -> String {
    match style {
        CheckStyle::Check => check_path(bbox),
        CheckStyle::Circle => circle_path(bbox),
        CheckStyle::Cross => cross_path(bbox),
        CheckStyle::Diamond => diamond_path(bbox),
        CheckStyle::Square => square_path(bbox),
        CheckStyle::Star => star_path(bbox),
    }
}

/// The check mark's eight-segment outline, given as normalized control
/// triples and scaled into the box.
fn check_path(bbox: Rect) -> String {
    /// Each row is `(on-curve point, outgoing handle, incoming handle of the
    /// next segment)`, in a unit square.
    const ROWS: [[(f32, f32); 3]; 8] = [
        [(0.28, 0.52), (0.27, 0.48), (0.29, 0.40)],
        [(0.30, 0.33), (0.31, 0.29), (0.31, 0.28)],
        [(0.39, 0.28), (0.49, 0.29), (0.77, 0.67)],
        [(0.76, 0.68), (0.78, 0.69), (0.76, 0.75)],
        [(0.76, 0.75), (0.73, 0.80), (0.68, 0.75)],
        [(0.68, 0.74), (0.68, 0.74), (0.44, 0.47)],
        [(0.43, 0.47), (0.40, 0.47), (0.41, 0.58)],
        [(0.40, 0.60), (0.28, 0.66), (0.30, 0.56)],
    ];
    let (w, h) = (geom::width(bbox), geom::height(bbox));
    let (left, bottom) = (geom::left(bbox), geom::bottom(bbox));
    let place = |(x, y): (f32, f32)| (x * w + left, y * h + bottom);

    let rows: Vec<[(f32, f32); 3]> = ROWS
        .iter()
        .map(|row| [place(row[0]), place(row[1]), place(row[2])])
        .collect();

    let mut out = Content::new();
    let Some(first) = rows.first() else {
        return String::new();
    };
    move_to(&mut out, first[0]);
    for (index, row) in rows.iter().enumerate() {
        // The last segment closes back onto the first point.
        let next = rows.get(index + 1).unwrap_or(first)[0];
        let (px1, py1) = (row[1].0 - row[0].0, row[1].1 - row[0].1);
        let (px2, py2) = (row[2].0 - next.0, row[2].1 - next.1);
        curve_to(
            &mut out,
            (row[0].0 + px1 * BEZIER, row[0].1 + py1 * BEZIER),
            (next.0 + px2 * BEZIER, next.1 + py2 * BEZIER),
            next,
        );
    }
    out.as_str().to_owned()
}

/// A circle, as four Bézier quarters through the box's edge midpoints.
fn circle_path(bbox: Rect) -> String {
    let (w, h) = (geom::width(bbox), geom::height(bbox));
    let (left, bottom, right, top) = (
        geom::left(bbox),
        geom::bottom(bbox),
        geom::right(bbox),
        geom::top(bbox),
    );
    let p1 = (left, bottom + h / 2.0);
    let p2 = (left + w / 2.0, top);
    let p3 = (right, bottom + h / 2.0);
    let p4 = (left + w / 2.0, bottom);

    let mut out = Content::new();
    move_to(&mut out, p1);
    for (from, to, flip) in [
        (p1, p2, false),
        (p2, p3, true),
        (p3, p4, true),
        (p4, p1, false),
    ] {
        let (px, py) = if flip {
            (to.0 - from.0, from.1 - to.1)
        } else {
            (to.0 - from.0, to.1 - from.1)
        };
        // Each quarter's two handles sit one Bézier constant along the
        // tangents at its ends.
        let (h1, h2) = if flip {
            ((from.0 + px * BEZIER, from.1), (to.0, to.1 + py * BEZIER))
        } else {
            ((from.0, from.1 + py * BEZIER), (to.0 - px * BEZIER, to.1))
        };
        curve_to(&mut out, h1, h2, to);
    }
    out.as_str().to_owned()
}

/// Two crossing diagonals.
fn cross_path(bbox: Rect) -> String {
    let mut out = Content::new();
    move_to(&mut out, (geom::left(bbox), geom::top(bbox)));
    line_to(&mut out, (geom::right(bbox), geom::bottom(bbox)));
    move_to(&mut out, (geom::left(bbox), geom::bottom(bbox)));
    line_to(&mut out, (geom::right(bbox), geom::top(bbox)));
    out.as_str().to_owned()
}

/// A diamond through the box's edge midpoints.
fn diamond_path(bbox: Rect) -> String {
    let (w, h) = (geom::width(bbox), geom::height(bbox));
    let (left, bottom, right, top) = (
        geom::left(bbox),
        geom::bottom(bbox),
        geom::right(bbox),
        geom::top(bbox),
    );
    closed_loop(&[
        (left, bottom + h / 2.0),
        (left + w / 2.0, top),
        (right, bottom + h / 2.0),
        (left + w / 2.0, bottom),
    ])
}

/// The box itself.
fn square_path(bbox: Rect) -> String {
    let (left, bottom, right, top) = (
        geom::left(bbox),
        geom::bottom(bbox),
        geom::right(bbox),
        geom::top(bbox),
    );
    closed_loop(&[(left, top), (right, top), (right, bottom), (left, bottom)])
}

/// A five-pointed star, drawn as a pentagram through alternating vertices.
fn star_path(bbox: Rect) -> String {
    let radius =
        (geom::top(bbox) - geom::bottom(bbox)) / (1.0 + (std::f32::consts::PI / 5.0).cos());
    let centre = (
        (geom::left(bbox) + geom::right(bbox)) / 2.0,
        (geom::top(bbox) + geom::bottom(bbox)) / 2.0,
    );
    let mut points = [(0.0_f32, 0.0_f32); 5];
    let mut angle = std::f32::consts::PI / 10.0;
    for point in &mut points {
        *point = (
            centre.0 + radius * angle.cos(),
            centre.1 + radius * angle.sin(),
        );
        angle += std::f32::consts::PI * 2.0 / 5.0;
    }
    let mut out = Content::new();
    let Some(first) = points.first().copied() else {
        return String::new();
    };
    move_to(&mut out, first);
    // Skipping a vertex each step is what makes it a star rather than a
    // pentagon.
    for index in [2, 4, 1, 3, 0] {
        if let Some(point) = points.get(index) {
            line_to(&mut out, *point);
        }
    }
    out.as_str().to_owned()
}

/// A polygon closed back onto its first point.
fn closed_loop(points: &[(f32, f32)]) -> String {
    let mut out = Content::new();
    let Some(first) = points.first() else {
        return String::new();
    };
    move_to(&mut out, *first);
    for point in points.iter().skip(1) {
        line_to(&mut out, *point);
    }
    line_to(&mut out, *first);
    out.as_str().to_owned()
}

fn move_to(out: &mut Content, (x, y): (f32, f32)) {
    out.point(x, y, Float::Shortest);
    out.raw("m\n");
}

fn line_to(out: &mut Content, (x, y): (f32, f32)) {
    out.point(x, y, Float::Shortest);
    out.raw("l\n");
}

fn curve_to(out: &mut Content, h1: (f32, f32), h2: (f32, f32), to: (f32, f32)) {
    out.point(h1.0, h1.1, Float::Shortest);
    out.point(h2.0, h2.1, Float::Shortest);
    out.point(to.0, to.1, Float::Shortest);
    out.raw("c\n");
}

#[cfg(test)]
mod tests {
    use super::{CheckStyle, check_box, path, radio_button, style_from_caption};
    use crate::color::Color;
    use crate::geom;

    #[test]
    fn the_caption_table_maps_six_dingbat_code_points() {
        assert_eq!(style_from_caption("4"), Some(CheckStyle::Check));
        assert_eq!(style_from_caption("8"), Some(CheckStyle::Cross));
        assert_eq!(style_from_caption("H"), Some(CheckStyle::Star));
        assert_eq!(style_from_caption("l"), Some(CheckStyle::Circle));
        assert_eq!(style_from_caption("n"), Some(CheckStyle::Square));
        assert_eq!(style_from_caption("u"), Some(CheckStyle::Diamond));
        // Only the first character counts.
        assert_eq!(style_from_caption("4abc"), Some(CheckStyle::Check));
        // Anything else, and an empty caption, leaves the choice to the
        // caller's own default.
        assert_eq!(style_from_caption("x"), None);
        assert_eq!(style_from_caption(""), None);
    }

    #[test]
    fn a_transparent_colour_still_leaves_a_path_behind() {
        let box_ = geom::rect(0.0, 0.0, 12.0, 12.0);
        let drawn = check_box(box_, CheckStyle::Circle, Color::Transparent);
        // No colour operator, but the path and its paint operator are there.
        assert!(!drawn.contains(" rg\n"), "{drawn}");
        assert!(drawn.starts_with("q\n"), "{drawn}");
        assert!(drawn.ends_with("f\nQ\n"), "{drawn}");
        assert!(drawn.contains(" c\n"), "{drawn}");
    }

    #[test]
    fn the_cross_strokes_where_the_rest_fill() {
        let box_ = geom::rect(0.0, 0.0, 12.0, 12.0);
        assert!(
            check_box(box_, CheckStyle::Cross, Color::Gray(0.0)).ends_with("S\nQ\n"),
            "the cross strokes"
        );
        for style in [
            CheckStyle::Check,
            CheckStyle::Circle,
            CheckStyle::Diamond,
            CheckStyle::Square,
            CheckStyle::Star,
        ] {
            assert!(
                check_box(box_, style, Color::Gray(0.0)).ends_with("f\nQ\n"),
                "{style:?}"
            );
        }
    }

    #[test]
    fn a_radio_circle_shrinks_further_than_a_checkbox_one() {
        let box_ = geom::rect(0.0, 0.0, 12.0, 12.0);
        let as_check = check_box(box_, CheckStyle::Circle, Color::Gray(0.0));
        let as_radio = radio_button(box_, CheckStyle::Circle, Color::Gray(0.0));
        assert_ne!(as_check, as_radio);
        // The centred square is the whole 12-unit box; two thirds of it
        // leaves a half-extent of 4, so the circle starts near 2, and one
        // half leaves 3, so the radio's starts at exactly 3. The "near" is
        // real single-precision arithmetic, not a rounding we could tidy —
        // the bytes upstream writes carry the same drift.
        assert!(as_check.contains("\n1.9999999 6 m\n"), "{as_check}");
        assert!(as_radio.contains("\n3 6 m\n"), "{as_radio}");
    }

    #[test]
    fn the_check_mark_closes_its_outline() {
        let drawn = path(geom::rect(0.0, 0.0, 100.0, 100.0), CheckStyle::Check);
        // Eight segments, each one curve, and the last returns to the start.
        assert_eq!(drawn.matches(" c\n").count(), 8);
        assert_eq!(drawn.matches(" m\n").count(), 1);
        assert!(drawn.starts_with("28 52 m\n"), "{drawn}");
        assert!(drawn.ends_with("28 52 c\n"), "{drawn}");
    }

    #[test]
    fn the_polygons_return_to_their_first_point() {
        for style in [CheckStyle::Diamond, CheckStyle::Square] {
            let drawn = path(geom::rect(0.0, 0.0, 10.0, 10.0), style);
            assert_eq!(drawn.matches(" m\n").count(), 1, "{style:?}");
            assert_eq!(drawn.matches(" l\n").count(), 4, "{style:?}");
        }
    }

    #[test]
    fn the_cross_lifts_the_pen_between_its_two_strokes() {
        let drawn = path(geom::rect(0.0, 0.0, 10.0, 10.0), CheckStyle::Cross);
        assert_eq!(drawn, "0 10 m\n10 0 l\n0 0 m\n10 10 l\n");
    }

    #[test]
    fn the_star_skips_a_vertex_each_step() {
        let drawn = path(geom::rect(0.0, 0.0, 10.0, 10.0), CheckStyle::Star);
        assert_eq!(drawn.matches(" m\n").count(), 1);
        assert_eq!(drawn.matches(" l\n").count(), 5);
    }
}
