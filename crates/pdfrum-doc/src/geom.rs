//! PDF rectangle arithmetic with the exact semantics the annotation and
//! appearance code depends on.
//!
//! A PDF rectangle is `[left bottom right top]` in a y-**up** space, and none
//! of the operations below normalize the way a general-purpose geometry
//! library would. `kurbo::Rect` is the carrier — `(x0, y0, x1, y1)` reads as
//! `(left, bottom, right, top)` — but its own `inflate`/`union` normalize on
//! different rules, so nothing in this crate calls them. These free functions
//! are the only rectangle vocabulary `pdfrum-doc` uses.
//!
//! Two asymmetries drive most of the surprises:
//!
//! - [`inflate`] and [`deflate`] normalize their input first; [`is_empty`]
//!   does not. So an inverted rectangle is "empty", yet deflating it produces
//!   a well-formed one.
//! - [`width`] and [`height`] are plain subtractions and can be negative,
//!   because a `re` operator's operands are written from them verbatim.
//!
//! ```
//! use pdfrum_doc::geom;
//! use kurbo::Rect;
//!
//! // An inverted rectangle: right < left.
//! let inverted = Rect::new(10.0, 0.0, 4.0, 8.0);
//! assert!(geom::is_empty(inverted));
//! assert_eq!(geom::width(inverted), -6.0);
//! // Deflating normalizes first, so the result is well-formed.
//! assert_eq!(geom::deflate(inverted, 1.0, 1.0), Rect::new(5.0, 1.0, 9.0, 7.0));
//! ```

// PDF real numbers are single-precision, and `kurbo::Rect` carries doubles.
// Narrowing at every edge accessor is the design: it is what keeps the
// numbers this crate writes into a content stream identical to the ones the
// file declared, rather than to doubles that merely round to them.
#![allow(clippy::cast_possible_truncation)]

use kurbo::{Affine, Point, Rect};

/// The comparison epsilon the layout engine uses, as a `f64` literal so the
/// comparison happens in double precision even for `f32` inputs.
const EPSILON: f64 = 1e-4;

/// Whether a value is within one epsilon of zero.
#[must_use]
pub fn is_float_zero(value: f32) -> bool {
    let value = f64::from(value);
    value < EPSILON && value > -EPSILON
}

/// Whether `a` exceeds `b` by more than one epsilon.
#[must_use]
pub fn is_float_bigger(a: f32, b: f32) -> bool {
    a > b && !is_float_zero(a - b)
}

/// Whether `a` falls below `b` by more than one epsilon.
#[must_use]
pub fn is_float_smaller(a: f32, b: f32) -> bool {
    a < b && !is_float_zero(a - b)
}

/// Builds a rectangle from PDF's `[left bottom right top]` ordering.
#[must_use]
pub fn rect(left: f32, bottom: f32, right: f32, top: f32) -> Rect {
    Rect::new(
        f64::from(left),
        f64::from(bottom),
        f64::from(right),
        f64::from(top),
    )
}

/// The rectangle's left edge.
#[must_use]
pub fn left(r: Rect) -> f32 {
    r.x0 as f32
}

/// The rectangle's bottom edge.
#[must_use]
pub fn bottom(r: Rect) -> f32 {
    r.y0 as f32
}

/// The rectangle's right edge.
#[must_use]
pub fn right(r: Rect) -> f32 {
    r.x1 as f32
}

/// The rectangle's top edge.
#[must_use]
pub fn top(r: Rect) -> f32 {
    r.y1 as f32
}

/// `right - left`, which is **negative** for an inverted rectangle.
#[must_use]
pub fn width(r: Rect) -> f32 {
    right(r) - left(r)
}

/// `top - bottom`, which is **negative** for an inverted rectangle.
#[must_use]
pub fn height(r: Rect) -> f32 {
    top(r) - bottom(r)
}

/// Swaps whichever pairs of edges are out of order.
#[must_use]
pub fn normalize(r: Rect) -> Rect {
    let (x0, x1) = if r.x0 > r.x1 {
        (r.x1, r.x0)
    } else {
        (r.x0, r.x1)
    };
    let (y0, y1) = if r.y0 > r.y1 {
        (r.y1, r.y0)
    } else {
        (r.y0, r.y1)
    };
    Rect::new(x0, y0, x1, y1)
}

/// Whether the rectangle encloses nothing — tested **without** normalizing,
/// so an inverted rectangle is empty even though it has area.
#[must_use]
pub fn is_empty(r: Rect) -> bool {
    r.x0 >= r.x1 || r.y0 >= r.y1
}

/// Grows the rectangle by `x` horizontally and `y` vertically, normalizing
/// first. Negative amounts shrink it (see [`deflate`]).
#[must_use]
pub fn inflate(r: Rect, x: f32, y: f32) -> Rect {
    let r = normalize(r);
    let (x, y) = (f64::from(x), f64::from(y));
    Rect::new(r.x0 - x, r.y0 - y, r.x1 + x, r.y1 + y)
}

/// Shrinks the rectangle, normalizing first. Exactly `inflate(-x, -y)`.
#[must_use]
pub fn deflate(r: Rect, x: f32, y: f32) -> Rect {
    inflate(r, -x, -y)
}

/// The smallest rectangle containing both, each normalized first.
#[must_use]
pub fn union(a: Rect, b: Rect) -> Rect {
    let (a, b) = (normalize(a), normalize(b));
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// The overlap of both, each normalized first. May come out inverted when
/// they do not overlap; the caller decides what that means.
#[must_use]
pub fn intersect(a: Rect, b: Rect) -> Rect {
    let (a, b) = (normalize(a), normalize(b));
    Rect::new(
        a.x0.max(b.x0),
        a.y0.max(b.y0),
        a.x1.min(b.x1),
        a.y1.min(b.y1),
    )
}

/// Whether the point falls inside, **inclusive on all four edges**, after
/// normalizing a copy of the rectangle.
#[must_use]
pub fn contains(r: Rect, p: Point) -> bool {
    let r = normalize(r);
    p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1
}

/// The largest centred square that fits: half-extent `min(w, h) / 2` about
/// the centre.
#[must_use]
pub fn center_square(r: Rect) -> Rect {
    let half = f64::from(width(r).abs().min(height(r).abs()) / 2.0);
    let (cx, cy) = (f64::midpoint(r.x0, r.x1), f64::midpoint(r.y0, r.y1));
    Rect::new(cx - half, cy - half, cx + half, cy + half)
}

/// Scales the half-extents about the centre.
#[must_use]
pub fn scale_from_center(r: Rect, scale: f32) -> Rect {
    let scale = f64::from(scale);
    let (cx, cy) = (f64::midpoint(r.x0, r.x1), f64::midpoint(r.y0, r.y1));
    let (hw, hh) = ((r.x1 - r.x0) / 2.0 * scale, (r.y1 - r.y0) / 2.0 * scale);
    Rect::new(cx - hw, cy - hh, cx + hw, cy + hh)
}

/// Moves the rectangle by `(dx, dy)` without normalizing.
#[must_use]
pub fn translate(r: Rect, dx: f32, dy: f32) -> Rect {
    let (dx, dy) = (f64::from(dx), f64::from(dy));
    Rect::new(r.x0 + dx, r.y0 + dy, r.x1 + dx, r.y1 + dy)
}

/// The scale-and-translate that maps `src` onto `dest`.
///
/// A degenerate axis (source extent under `0.001`) contributes a scale of 1
/// rather than a division by nearly zero, which is how an appearance stream
/// with a zero-width `/BBox` still lands somewhere sensible.
#[must_use]
pub fn match_rect(dest: Rect, src: Rect) -> Affine {
    let a = if (src.x0 - src.x1).abs() < 0.001 {
        1.0
    } else {
        (dest.x0 - dest.x1) / (src.x0 - src.x1)
    };
    let d = if (src.y0 - src.y1).abs() < 0.001 {
        1.0
    } else {
        (dest.y0 - dest.y1) / (src.y0 - src.y1)
    };
    Affine::new([a, 0.0, 0.0, d, dest.x0 - src.x0 * a, dest.y0 - src.y0 * d])
}

/// Maps all four corners and returns their bounding box.
#[must_use]
pub fn transform_rect(m: Affine, r: Rect) -> Rect {
    let corners = [
        m * Point::new(r.x0, r.y0),
        m * Point::new(r.x1, r.y0),
        m * Point::new(r.x0, r.y1),
        m * Point::new(r.x1, r.y1),
    ];
    let mut out = Rect::new(corners[0].x, corners[0].y, corners[0].x, corners[0].y);
    for c in &corners[1..] {
        out = Rect::new(
            out.x0.min(c.x),
            out.y0.min(c.y),
            out.x1.max(c.x),
            out.y1.max(c.y),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        contains, deflate, height, inflate, is_empty, is_float_bigger, is_float_smaller,
        is_float_zero, match_rect, normalize, union, width,
    };
    use kurbo::{Point, Rect};

    #[test]
    fn is_empty_does_not_normalize_but_deflate_does() {
        let inverted = Rect::new(10.0, 0.0, 4.0, 8.0);
        assert!(is_empty(inverted));
        assert!((width(inverted) - -6.0).abs() < f32::EPSILON);
        assert!((height(inverted) - 8.0).abs() < f32::EPSILON);
        // Normalized to (4, 0, 10, 8), then shrunk by one on each side.
        assert_eq!(deflate(inverted, 1.0, 1.0), Rect::new(5.0, 1.0, 9.0, 7.0));
        assert_eq!(inflate(inverted, 1.0, 0.0), Rect::new(3.0, 0.0, 11.0, 8.0));
    }

    #[test]
    fn contains_is_inclusive_on_every_edge() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        for p in [
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 5.0),
            Point::new(5.0, 10.0),
        ] {
            assert!(contains(r, p), "{p:?}");
        }
        assert!(!contains(r, Point::new(-0.001, 5.0)));
        // An inverted rectangle is normalized for the test.
        assert!(contains(
            Rect::new(10.0, 10.0, 0.0, 0.0),
            Point::new(5.0, 5.0)
        ));
    }

    #[test]
    fn union_normalizes_both_operands() {
        let a = Rect::new(4.0, 5.0, 2.0, 3.0);
        let b = Rect::new(4.0, 3.0, 6.0, 5.0);
        assert_eq!(union(a, b), Rect::new(2.0, 3.0, 6.0, 5.0));
        assert_eq!(normalize(a), Rect::new(2.0, 3.0, 4.0, 5.0));
    }

    #[test]
    fn epsilon_comparisons_use_double_precision() {
        assert!(is_float_zero(5e-5));
        assert!(!is_float_zero(2e-4));
        assert!(is_float_bigger(1.0, 0.5));
        assert!(!is_float_bigger(1.0, 1.0 - 5e-5));
        assert!(is_float_smaller(0.5, 1.0));
        assert!(!is_float_smaller(1.0 - 5e-5, 1.0));
    }

    #[test]
    fn match_rect_keeps_a_degenerate_axis_at_unit_scale() {
        let m = match_rect(
            Rect::new(0.0, 0.0, 20.0, 10.0),
            Rect::new(0.0, 0.0, 10.0, 0.0),
        );
        let c = m.as_coeffs();
        assert!((c[0] - 2.0).abs() < 1e-9);
        assert!((c[3] - 1.0).abs() < 1e-9);
    }
}
