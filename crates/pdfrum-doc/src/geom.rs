//! PDF rectangle arithmetic with the exact semantics the annotation and
//! appearance code depends on.
//!
//! A PDF rectangle is `[left bottom right top]` in a y-**up** space, and none
//! of the operations below normalize the way a general-purpose geometry
//! library would. `kurbo::Rect` is the carrier — `(x0, y0, x1, y1)` reads as
//! `(left, bottom, right, top)` — but its own accessors answer differently at
//! every point that matters here, which is why this module exists rather than
//! calling through to them:
//!
//! - [`width`] and [`height`] narrow to `f32`, because a `re` operator's
//!   operands are written from them verbatim and PDF reals are single
//!   precision. `kurbo`'s answer in `f64` would round to a different decimal.
//!   Both are plain subtractions and **can be negative**, as `kurbo`'s are.
//! - `Rect::contains` is half-open on the far edges and does not normalize;
//!   the annotation hit test is inclusive on all four and does.
//! - `Rect::union`, `Rect::intersect` and `Rect::inflate` are documented as
//!   valid only for non-negative extents; the appearance generators feed them
//!   inverted rectangles and rely on normalization happening first.
//! - `Rect::is_empty` was renamed `is_zero_area` in `kurbo` 0.13 and tests
//!   `area() == 0.0`; the emptiness this crate means is `x0 >= x1 || y0 >= y1`,
//!   which is true of an inverted rectangle that has area.
//!
//! The private half of the module is where those four divergences live. The
//! public half is the vocabulary `pdfrum-form` and `pdfrum-tool` share with
//! this crate: the `[left bottom right top]` constructor, the four narrowing
//! edge accessors and the two extents, [`normalize`], and the two matrix
//! operations an appearance stream is placed with.
//!
//! ```
//! use pdfrum_doc::geom;
//! use kurbo::Rect;
//!
//! // An inverted rectangle: right < left. The extents stay signed …
//! let inverted = Rect::new(10.0, 0.0, 4.0, 8.0);
//! assert_eq!(geom::width(inverted), -6.0);
//! // … until something normalizes it.
//! assert_eq!(geom::normalize(inverted), Rect::new(4.0, 0.0, 10.0, 8.0));
//! assert_eq!(geom::width(geom::normalize(inverted)), 6.0);
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
pub(crate) fn is_float_zero(value: f32) -> bool {
    let value = f64::from(value);
    value < EPSILON && value > -EPSILON
}

/// Whether `a` exceeds `b` by more than one epsilon.
#[must_use]
pub(crate) fn is_float_bigger(a: f32, b: f32) -> bool {
    a > b && !is_float_zero(a - b)
}

/// Whether `a` falls below `b` by more than one epsilon.
#[must_use]
pub(crate) fn is_float_smaller(a: f32, b: f32) -> bool {
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
pub(crate) fn is_empty(r: Rect) -> bool {
    r.x0 >= r.x1 || r.y0 >= r.y1
}

/// Grows the rectangle by `x` horizontally and `y` vertically, normalizing
/// first. Negative amounts shrink it (see [`deflate`]).
#[must_use]
pub(crate) fn inflate(r: Rect, x: f32, y: f32) -> Rect {
    let r = normalize(r);
    let (x, y) = (f64::from(x), f64::from(y));
    Rect::new(r.x0 - x, r.y0 - y, r.x1 + x, r.y1 + y)
}

/// Shrinks the rectangle, normalizing first. Exactly `inflate(-x, -y)`.
#[must_use]
pub(crate) fn deflate(r: Rect, x: f32, y: f32) -> Rect {
    inflate(r, -x, -y)
}

/// The smallest rectangle containing both, each normalized first.
#[must_use]
pub(crate) fn union(a: Rect, b: Rect) -> Rect {
    let (a, b) = (normalize(a), normalize(b));
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// Whether the point falls inside, **inclusive on all four edges**, after
/// normalizing a copy of the rectangle.
///
/// Only the tests ask: `nav::link`'s hit test is the one that used to, and it
/// is itself `#[cfg(test)]` now.
#[cfg(test)]
#[must_use]
pub(crate) fn contains(r: Rect, p: Point) -> bool {
    let r = normalize(r);
    p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1
}

/// The largest centred square that fits: half-extent `min(w, h) / 2` about
/// the centre.
#[must_use]
pub(crate) fn center_square(r: Rect) -> Rect {
    let half = f64::from(width(r).abs().min(height(r).abs()) / 2.0);
    let (cx, cy) = (f64::midpoint(r.x0, r.x1), f64::midpoint(r.y0, r.y1));
    Rect::new(cx - half, cy - half, cx + half, cy + half)
}

/// Scales the half-extents about the centre.
#[must_use]
pub(crate) fn scale_from_center(r: Rect, scale: f32) -> Rect {
    let scale = f64::from(scale);
    let (cx, cy) = (f64::midpoint(r.x0, r.x1), f64::midpoint(r.y0, r.y1));
    let (hw, hh) = ((r.x1 - r.x0) / 2.0 * scale, (r.y1 - r.y0) / 2.0 * scale);
    Rect::new(cx - hw, cy - hh, cx + hw, cy + hh)
}

/// Moves the rectangle by `(dx, dy)` without normalizing.
#[must_use]
pub(crate) fn translate(r: Rect, dx: f32, dy: f32) -> Rect {
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

/// A widget annotation's `/MK /R`, normalized to one of the four quadrants.
///
/// ISO 32000-1 Table 189 defines `/R` as "the number of degrees by which the
/// widget annotation shall be rotated counterclockwise relative to the page",
/// and says it "shall be a multiple of 90". Neither clause is a guarantee
/// about real files, so [`WidgetRotation::from_degrees`] rules on both: a
/// negative or out-of-range angle is the *same* quadrant it names modulo a
/// full turn, and an angle that is not a multiple of 90 names no quadrant at
/// all and is upright.
///
/// This is the one normalization both readers of the key share — the
/// appearance builder, which needs the box, and `pdfrum-form`'s routing,
/// which needs the map. They used to fold it separately and disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WidgetRotation {
    /// Upright.
    #[default]
    None,
    /// A quarter turn counterclockwise.
    Quarter,
    /// A half turn.
    Half,
    /// Three quarters counterclockwise.
    ThreeQuarter,
}

impl WidgetRotation {
    /// The quadrant `/MK /R n` names, or [`WidgetRotation::None`] when it
    /// names none.
    ///
    /// The fold is `rem_euclid(360)`, so `-90` is `270` — a quarter turn
    /// *clockwise* is three quarters counterclockwise, which is what a
    /// counterclockwise angle of `-90` means. A value that is not a multiple
    /// of 90 is upright.
    ///
    /// `[oracle-bug]` A negative multiple of 90 keeps its sign through the
    /// fold, so `/R -90` draws and hit-tests as a three-quarter turn — the
    /// specification's "counterclockwise" reading, and pdf.js's.
    // [oracle-bug] PDFium folds with `abs(GetRotation() % 360)`
    // (fpdfsdk/cpdfsdk_widget.cpp:1029 in GetRotatedRect, :1049 in
    // GetMatrix), sending -90 to 90 — the wrong direction. The two agree on
    // the *box*, since 90 and 270 swap the same axes, but GetMatrix builds
    // CFX_Matrix(0, 1, -1, 0, fWidth, 0) for 90 and CFX_Matrix(0, -1, 1, 0,
    // 0, fHeight) for 270 (:1053-1062), so a /R -90 widget lands a half turn
    // away from where the file asked. pdf.js is the tiebreaker and
    // normalizes the way this does — `angle %= 360; if (angle < 0) { angle
    // += 360; }`, then `if (angle % 90 === 0)` before it is kept
    // (src/core/annotation.js, WidgetAnnotation.setRotation).
    //
    // The non-multiple of 90 is not a divergence: the `default:` arm on both
    // of the oracle's switches falls through to the 0/180 case, and pdf.js's
    // `angle % 90 === 0` gate leaves this.rotation at its initialized 0.
    #[must_use]
    pub fn from_degrees(degrees: i64) -> WidgetRotation {
        match degrees.rem_euclid(360) {
            90 => WidgetRotation::Quarter,
            180 => WidgetRotation::Half,
            270 => WidgetRotation::ThreeQuarter,
            _ => WidgetRotation::None,
        }
    }

    /// Whether this rotation exchanges the widget's width and height.
    #[must_use]
    pub fn swaps_axes(self) -> bool {
        matches!(self, WidgetRotation::Quarter | WidgetRotation::ThreeQuarter)
    }
}

#[cfg(test)]
mod tests {
    use super::WidgetRotation;
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
    fn a_negative_quarter_turn_is_three_quarters_counterclockwise() {
        // ISO 32000-1 table 189: `/R` counts degrees *counterclockwise*, so
        // `-90` is a quarter turn clockwise, which is `270`. PDFium's
        // `abs()` answers `90` — see the `[oracle-bug]` note on
        // `from_degrees`.
        assert_eq!(
            WidgetRotation::from_degrees(-90),
            WidgetRotation::ThreeQuarter
        );
        assert_eq!(WidgetRotation::from_degrees(-270), WidgetRotation::Quarter);
        assert_eq!(WidgetRotation::from_degrees(-180), WidgetRotation::Half);
        assert_eq!(WidgetRotation::from_degrees(-360), WidgetRotation::None);
    }

    #[test]
    fn a_rotation_that_is_not_a_quarter_turn_is_upright() {
        // Both readers agree here: PDFium's `default:` falls through to the
        // 0/180 case and pdf.js's `angle % 90 === 0` gate leaves the angle
        // at zero.
        for degrees in [45, -45, 1, 359, 91, 100_000] {
            assert_eq!(
                WidgetRotation::from_degrees(degrees),
                WidgetRotation::None,
                "{degrees}"
            );
        }
    }

    #[test]
    fn a_full_turn_folds_away() {
        assert_eq!(WidgetRotation::from_degrees(450), WidgetRotation::Quarter);
        assert_eq!(
            WidgetRotation::from_degrees(-450),
            WidgetRotation::ThreeQuarter
        );
        assert_eq!(WidgetRotation::from_degrees(720), WidgetRotation::None);
    }

    #[test]
    fn only_the_odd_quadrants_swap_the_axes() {
        assert!(!WidgetRotation::None.swaps_axes());
        assert!(WidgetRotation::Quarter.swaps_axes());
        assert!(!WidgetRotation::Half.swaps_axes());
        assert!(WidgetRotation::ThreeQuarter.swaps_axes());
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
