//! Path construction and painting operators (ISO 32000-1 §8.5).
//!
//! # The rectangle fast path
//!
//! A path that is exactly a rectangle — four corners, axis-aligned, closed —
//! is written as one `re` rather than four segments, because that is what the
//! C++ emits and what its goldens pin. The width and height are computed as
//! differences and **may be negative**: `re` accepts that, and normalising it
//! would change bytes for no gain.
//!
//! # Beziers consume three points at a time
//!
//! A `c` operator takes three points. A run that ends mid-triple is
//! malformed, and the C++ answers by writing ` h` and abandoning **every
//! remaining point** — not just the incomplete triple. We reproduce that,
//! because a truncated path is a visible thing and a reader that saw the
//! oracle's output would see the same shape.
//!
//! `v` and `y`, the shorthands that reuse a control point, are never emitted:
//! `c` spells every curve.

use pdfrum_common::kurbo::{BezPath, PathEl, Point, Rect};
use pdfrum_page::FillRule;

use crate::content::num::{write_float, write_point};

/// Whether a painted path also draws its outline.
///
/// The second half of what selects a paint operator, and an enum rather than a
/// `bool` because its neighbour in [`paint_operator`] is a [`FillRule`]: two
/// arguments that jointly index one table should read the same way at the
/// call. [`Stroked::of`] narrows `pdfrum-page`'s `PathObject::stroke` bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stroked {
    /// The outline is drawn: `S`, `B` or `B*`.
    Yes,
    /// Only the fill, if any: `n`, `f` or `f*`.
    No,
}

impl Stroked {
    /// Narrow `pdfrum-page`'s `PathObject::stroke`.
    pub(crate) const fn of(stroke: bool) -> Self {
        if stroke { Self::Yes } else { Self::No }
    }
}

/// The paint operator for a fill rule and whether the path also strokes
/// (ISO 32000-1 table 60), including its leading space.
///
/// There is no `b`/`b*` here — a closed-fill-stroke is written as its
/// `B`/`B*` equivalent with an explicit `h` in the path — and no `W n`,
/// because clipping is emitted by the graphics frame rather than by the paint
/// operator.
#[must_use]
pub(crate) fn paint_operator(fill: FillRule, stroke: Stroked) -> &'static str {
    match (fill, stroke) {
        (FillRule::None, Stroked::No) => " n",
        (FillRule::None, Stroked::Yes) => " S",
        (FillRule::Winding, Stroked::No) => " f",
        (FillRule::Winding, Stroked::Yes) => " B",
        (FillRule::EvenOdd, Stroked::No) => " f*",
        (FillRule::EvenOdd, Stroked::Yes) => " B*",
    }
}

/// Write a path's construction operators.
///
/// Points are separated by single spaces and each operator follows its
/// operands with a leading space, so a subpath reads `3.1 4.6 m 5.4 .2 l`.
pub(crate) fn emit_path_points(out: &mut String, path: &BezPath) {
    if let Some(rect) = as_rectangle(path) {
        // `left bottom width height re` — the extents may be negative.
        write_point(out, Point::new(rect.x0, rect.y0));
        out.push(' ');
        #[expect(
            clippy::cast_possible_truncation,
            reason = "PDF numbers are f32; the geometry vocabulary is f64"
        )]
        {
            write_float(out, (rect.x1 - rect.x0) as f32);
            out.push(' ');
            write_float(out, (rect.y1 - rect.y0) as f32);
        }
        out.push_str(" re");
        return;
    }

    let mut first = true;
    for el in path.elements() {
        if !first {
            out.push(' ');
        }
        match el {
            PathEl::MoveTo(p) => {
                write_point(out, *p);
                out.push_str(" m");
            }
            PathEl::LineTo(p) => {
                write_point(out, *p);
                out.push_str(" l");
            }
            PathEl::CurveTo(c1, c2, end) => {
                write_point(out, *c1);
                out.push(' ');
                write_point(out, *c2);
                out.push(' ');
                write_point(out, *end);
                out.push_str(" c");
            }
            // A quadratic has no PDF operator; raise it to the cubic that
            // draws the identical curve rather than approximating it.
            PathEl::QuadTo(c, end) => {
                let Some(start) = current_point(path, el) else {
                    continue;
                };
                let (c1, c2) = quad_to_cubic(start, *c, *end);
                write_point(out, c1);
                out.push(' ');
                write_point(out, c2);
                out.push(' ');
                write_point(out, *end);
                out.push_str(" c");
            }
            PathEl::ClosePath => out.push('h'),
        }
        first = false;
    }
}

/// The point a segment starts from, for the quadratic conversion.
fn current_point(path: &BezPath, target: &PathEl) -> Option<Point> {
    let mut last = None;
    for el in path.elements() {
        if std::ptr::eq(el, target) {
            return last;
        }
        // Every segment but `ClosePath` ends at a point, and that point is
        // where the next one starts.
        last = match el {
            PathEl::MoveTo(p)
            | PathEl::LineTo(p)
            | PathEl::CurveTo(_, _, p)
            | PathEl::QuadTo(_, p) => Some(*p),
            PathEl::ClosePath => last,
        };
    }
    None
}

/// The cubic control points that draw the same curve as a quadratic
/// (the standard degree elevation: two thirds of the way to the control).
fn quad_to_cubic(start: Point, control: Point, end: Point) -> (Point, Point) {
    let third = 2.0 / 3.0;
    (
        Point::new(
            start.x + third * (control.x - start.x),
            start.y + third * (control.y - start.y),
        ),
        Point::new(
            end.x + third * (control.x - end.x),
            end.y + third * (control.y - end.y),
        ),
    )
}

/// The rectangle a path draws, when it draws exactly one.
///
/// Four corners in either winding, axis-aligned, optionally closed. Anything
/// else — a fifth point, a curve, a diagonal — is not a rectangle and takes
/// the general path.
fn as_rectangle(path: &BezPath) -> Option<Rect> {
    let els = path.elements();
    let mut points: Vec<Point> = Vec::with_capacity(5);
    for el in els {
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(*p),
            PathEl::ClosePath => {}
            _ => return None,
        }
    }
    // A closed four-corner path may repeat its first point as the fifth.
    if points.len() == 5 && points.first() == points.last() {
        points.pop();
    }
    let [a, b, c, d] = points.as_slice() else {
        return None;
    };

    // Two windings make a rectangle: across-then-up, and up-then-across.
    let horizontal_first = near(a.y, b.y) && near(b.x, c.x) && near(c.y, d.y) && near(d.x, a.x);
    let vertical_first = near(a.x, b.x) && near(b.y, c.y) && near(c.x, d.x) && near(d.y, a.y);
    if !horizontal_first && !vertical_first {
        return None;
    }
    // A degenerate "rectangle" of zero extent is still a rectangle to `re`,
    // and writing it as one matches what the C++ does.
    Some(Rect::new(a.x, a.y, c.x, c.y))
}

/// Coordinates compare exactly: these came from a file, and a path built as a
/// rectangle holds the same bits at both ends of each edge.
#[expect(
    clippy::float_cmp,
    reason = "exact is the question being asked: a path is a rectangle only \
              when its edges share coordinates exactly, and a tolerance here \
              would spell a near-rectangle as `re` and move its corners"
)]
fn near(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || a == b
}

#[cfg(test)]
mod tests {
    use super::{Stroked, emit_path_points, paint_operator};
    use pdfrum_common::kurbo::{BezPath, Point};
    use pdfrum_page::FillRule;

    fn emit(path: &BezPath) -> String {
        let mut out = String::new();
        emit_path_points(&mut out, path);
        out
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    // cpdf_pagecontentgenerator_unittest.cpp ProcessRect (:55-79): a
    // four-point closed path takes the `re` fast path, extents and all.
    #[test]
    fn a_four_point_closed_path_becomes_one_re() {
        assert_eq!(emit(&rect(10.0, 5.0, 13.0, 30.0)), "10 5 3 25 re");
        assert_eq!(emit(&rect(0.0, 0.0, 5.2, 3.78)), "0 0 5.2 3.78 re");
    }

    // The extents are differences, so they may be negative — `re` accepts it
    // and normalising would change bytes for nothing.
    #[test]
    fn a_reversed_rectangle_keeps_its_negative_extents() {
        assert_eq!(emit(&rect(13.0, 30.0, 10.0, 5.0)), "13 30 -3 -25 re");
    }

    // The other winding is a rectangle too.
    #[test]
    fn the_vertical_first_winding_is_recognised() {
        let mut p = BezPath::new();
        p.move_to((10.0, 5.0));
        p.line_to((10.0, 30.0));
        p.line_to((13.0, 30.0));
        p.line_to((13.0, 5.0));
        p.close_path();
        assert_eq!(emit(&p), "10 5 3 25 re");
    }

    // ProcessPath (:151-186), verbatim.
    #[test]
    fn a_mixed_path_writes_m_l_and_c() {
        let mut p = BezPath::new();
        p.move_to((3.102, 4.67));
        p.line_to((5.45, 0.29));
        p.curve_to((4.24, 3.15), (4.65, 2.98), (3.456, 0.24));
        p.line_to((10.6, 11.15));
        p.line_to((11.0, 12.5));
        p.curve_to((11.46, 12.67), (11.84, 12.96), (12.0, 13.64));
        p.close_path();
        assert_eq!(
            emit(&p),
            "3.102 4.67 m 5.45 .29 l 4.24 3.15 4.65 2.98 3.456 .24 c \
             10.6 11.15 l 11 12.5 l 11.46 12.67 11.84 12.96 12 13.64 c h"
        );
    }

    // ProcessFormWithPath (:425-448): the round-trip float golden. A value
    // that is the nearest f32 to a shorter decimal prints as that decimal.
    #[test]
    fn float_spelling_shortens_where_the_bits_allow() {
        let mut p = BezPath::new();
        p.move_to((3.102, 4.670_000_1));
        p.line_to((5.450_001_2, 0.289_999_99));
        p.curve_to((4.239_999_8, 3.149_999_9), (4.65, 2.98), (3.456, 0.24));
        p.line_to((3.102, 4.670_000_1));
        p.close_path();
        assert_eq!(
            emit(&p),
            "3.102 4.67 m 5.4500012 .29 l 4.24 3.1499999 4.65 2.98 3.456 .24 c \
             3.102 4.67 l h"
        );
    }

    // Bug937 (:81-149): extreme magnitudes stay positional, never scientific.
    #[test]
    fn extreme_coordinates_stay_positional() {
        let mut p = BezPath::new();
        p.move_to((1e-21, 1e-21));
        p.line_to((100.0, 100.0));
        let out = emit(&p);
        assert!(out.starts_with(".000000000000000000001 .000000000000000000001 m"));
        assert!(!out.contains('e') && !out.contains('E'));
    }

    // A five-point path is not a rectangle, whatever its corners.
    #[test]
    fn a_five_point_path_takes_the_general_route() {
        let mut p = BezPath::new();
        for xy in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 2.0)] {
            if p.elements().is_empty() {
                p.move_to(xy);
            } else {
                p.line_to(xy);
            }
        }
        assert!(!emit(&p).contains("re"));
    }

    #[test]
    fn a_path_holding_a_curve_is_never_a_rectangle() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((1.0, 0.0));
        p.curve_to((1.0, 0.5), (1.0, 0.5), (1.0, 1.0));
        p.line_to((0.0, 1.0));
        p.close_path();
        assert!(!emit(&p).contains("re"));
    }

    // A quadratic has no PDF operator; it is raised to the cubic that draws
    // the identical curve rather than flattened.
    #[test]
    fn a_quadratic_is_raised_to_a_cubic() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.quad_to((3.0, 3.0), (6.0, 0.0));
        assert_eq!(emit(&p), "0 0 m 2 2 4 2 6 0 c");
    }

    // The paint matrix, every cell (:796-802).
    #[test]
    fn the_paint_operator_matrix() {
        assert_eq!(paint_operator(FillRule::None, Stroked::No), " n");
        assert_eq!(paint_operator(FillRule::None, Stroked::Yes), " S");
        assert_eq!(paint_operator(FillRule::Winding, Stroked::No), " f");
        assert_eq!(paint_operator(FillRule::Winding, Stroked::Yes), " B");
        assert_eq!(paint_operator(FillRule::EvenOdd, Stroked::No), " f*");
        assert_eq!(paint_operator(FillRule::EvenOdd, Stroked::Yes), " B*");
        // The narrowing that feeds it.
        assert_eq!(Stroked::of(true), Stroked::Yes);
        assert_eq!(Stroked::of(false), Stroked::No);
    }

    #[test]
    fn an_empty_path_writes_nothing() {
        assert_eq!(emit(&BezPath::new()), "");
    }

    // A degenerate rectangle of zero extent is still written as `re`.
    #[test]
    fn a_zero_extent_rectangle_is_still_a_rectangle() {
        let mut p = BezPath::new();
        for xy in [(5.0, 5.0), (5.0, 5.0), (5.0, 5.0), (5.0, 5.0)] {
            if p.elements().is_empty() {
                p.move_to(Point::new(xy.0, xy.1));
            } else {
                p.line_to(Point::new(xy.0, xy.1));
            }
        }
        p.close_path();
        assert_eq!(emit(&p), "5 5 0 0 re");
    }
}
