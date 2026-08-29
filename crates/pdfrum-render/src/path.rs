//! The geometry decisions that must be identical whichever rasterizer is
//! behind the device (`cfx_renderdevice.cpp:688-830`, `cfx_path.cpp`).
//!
//! In the C++ these live above the driver, in `CFX_RenderDevice`; here they
//! live in the engine for the same reason — they are pure functions of path,
//! matrix and colour, and Tier C's contract can only demand that the two
//! backends receive *byte-identical* geometry if the engine is what decides
//! it. Three of them change pixels on a large fraction of the corpus:
//!
//! - a degeneracy filter that is deliberately not a determinant test;
//! - an axis-aligned rectangle fast path that is **never antialiased** and
//!   snaps to integers with a shrink-the-wider-side rule;
//! - a zero-area sub-path detector that redraws degenerate fills as
//!   quarter-alpha hairlines.

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};

/// The per-axis coordinate clamp AGG applies before rasterizing
/// (`HardClip`, `cfx_agg_devicedriver.cpp:53-58`).
///
/// It is a *clamp*, not a clip: geometry beyond it is distorted rather than
/// cut off, which is observable on pathological corpus files. Both our
/// rasterizers clip properly instead, so the engine applies the clamp itself
/// — one of the few places where matching the oracle means deliberately
/// introducing an artefact, and the only way both backends can agree.
pub const MAX_POS: f64 = 32000.0;

/// `IsAvailableMatrix` (`cpdf_renderstatus.cpp:148-158`): whether a matrix is
/// worth drawing through.
///
/// Deliberately **not** a determinant test — a genuinely singular matrix like
/// `(1, 1, 1, 1)` passes. It rejects only the two ways a matrix collapses by
/// having a zero in each diagonal.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "a..d are the affine matrix coefficients, named as in the PDF `cm` operands"
)]
pub fn is_available_matrix(m: Affine) -> bool {
    let [a, b, c, d, _, _] = m.as_coeffs();
    if a == 0.0 || d == 0.0 {
        return b != 0.0 && c != 0.0;
    }
    if b == 0.0 || c == 0.0 {
        return a != 0.0 && d != 0.0;
    }
    true
}

/// Clamp every coordinate of a device-space path to `+/- MAX_POS` per axis.
#[must_use]
pub fn hard_clip(path: &BezPath) -> BezPath {
    let clamp = |p: Point| Point::new(p.x.clamp(-MAX_POS, MAX_POS), p.y.clamp(-MAX_POS, MAX_POS));
    let mut out = BezPath::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => out.move_to(clamp(p)),
            PathEl::LineTo(p) => out.line_to(clamp(p)),
            PathEl::QuadTo(a, b) => out.quad_to(clamp(a), clamp(b)),
            PathEl::CurveTo(a, b, c) => out.curve_to(clamp(a), clamp(b), clamp(c)),
            PathEl::ClosePath => out.close_path(),
        }
    }
    out
}

/// Nudge a degenerate one-point subpath's line endpoint one device pixel
/// right, so a stroke has something to expand (`BuildAggPath`,
/// `cfx_agg_devicedriver.cpp:951-956`).
///
/// AGG's `vertex_sequence::add` drops a vertex within `1e-14` of the previous
/// one, and `vcgen_stroke` then bails with fewer than two vertices, so
/// `50 40 m 50 40 l S` would stroke *nothing*. PDFium's answer is not to
/// special-case the stroke but to move the endpoint:
///
/// ```text
/// if (i > 0 && points[i - 1].IsTypeAndOpen(kMove) &&
///     (i + 1 == points.size() || points[i + 1].IsTypeAndOpen(kMove)) &&
///     points[i].point_ == points[i - 1].point_) {
///   pos.x += 1;
/// }
/// ```
///
/// Three details of that condition are load-bearing and each changes which
/// files it fires on:
///
/// - **The equality is on the *pre*-transform points** while the `+1` lands on
///   the *post*-transform one. So the test asks whether the content stream
///   repeated a coordinate, and the answer is one device pixel wherever that
///   coordinate ended up.
/// - **`IsTypeAndOpen` is `type == kMove && !close_figure_`.** A move that the
///   `h` rewrite already closed does not qualify.
/// - **The line must end its subpath.** Three identical points in a row leave
///   `points[i + 1]` a `kLine`, the nudge does not fire, and AGG collapses
///   them all — which is why `single_point_paths.in`'s five-fold repetition
///   draws nothing at all.
///
/// `path` is in the space the stroke is built in and `user` is the same path
/// before the matrix, so the two must have identical element sequences; when
/// they do not, nothing is nudged.
#[must_use]
pub fn nudge_degenerate_subpaths(path: &BezPath, user: &BezPath) -> BezPath {
    // The elements as (kind, point) pairs, in both spaces at once.
    let els: Vec<PathEl> = path.elements().to_vec();
    let user_els: Vec<PathEl> = user.elements().to_vec();
    if els.len() != user_els.len() {
        return path.clone();
    }
    let mut out = BezPath::new();
    // A `ClosePath` that a nudged line owns is dropped rather than emitted.
    //
    // AGG closes the polygon too, and `vcgen_stroke` then needs three
    // vertices before it emits anything — so the *closed* two-vertex form is
    // not what paints the oracle's dot. What paints it is the stadium the
    // open form gives: `stroke_calc_cap` puts one semicircle at each end of
    // the nudged unit segment. kurbo reaches the same stadium from an open
    // subpath and a very different shape from a closed one, where it joins
    // instead of capping and a round dot becomes a one-pixel bar. Dropping
    // the close is what makes the two agree.
    let mut skip_close = false;
    for (i, el) in els.iter().enumerate() {
        match *el {
            PathEl::LineTo(p) if should_nudge(&els, &user_els, i) => {
                out.line_to(Point::new(p.x + 1.0, p.y));
                skip_close = matches!(els.get(i + 1), Some(PathEl::ClosePath));
            }
            PathEl::MoveTo(p) => out.move_to(p),
            PathEl::LineTo(p) => out.line_to(p),
            PathEl::QuadTo(a, b) => out.quad_to(a, b),
            PathEl::CurveTo(a, b, c) => out.curve_to(a, b, c),
            PathEl::ClosePath if skip_close => skip_close = false,
            PathEl::ClosePath => out.close_path(),
        }
    }
    out
}

/// Whether element `i` — known to be a `LineTo` — meets `BuildAggPath`'s
/// three-part nudge condition.
fn should_nudge(els: &[PathEl], user: &[PathEl], i: usize) -> bool {
    // `points[i - 1]` must be an *open* move. In kurbo's spelling a `MoveTo`
    // is always open — `close_figure_` attaches to the point it follows, and
    // kurbo emits it as a separate `ClosePath` after the *line* — so the
    // element kind is the whole test.
    if !matches!(
        (i > 0).then(|| els.get(i - 1)).flatten(),
        Some(PathEl::MoveTo(_))
    ) {
        return false;
    }
    // The line must end its subpath. kurbo spells the C++'s `close_figure_`
    // as its own `ClosePath` element, so both a bare end and a close count.
    let next_ends_subpath = match els.get(i + 1) {
        None | Some(PathEl::MoveTo(_) | PathEl::ClosePath) => true,
        Some(_) => false,
    };
    if !next_ends_subpath {
        return false;
    }
    // A `ClosePath` may be followed only by the end or another move; anything
    // else means this line did not end its subpath after all.
    if matches!(els.get(i + 1), Some(PathEl::ClosePath))
        && !matches!(els.get(i + 2), None | Some(PathEl::MoveTo(_)))
    {
        return false;
    }
    // Finally: the *pre-transform* points must be equal.
    let (Some(PathEl::MoveTo(um)), Some(PathEl::LineTo(ul))) =
        ((i > 0).then(|| user.get(i - 1)).flatten(), user.get(i))
    else {
        return false;
    };
    um == ul
}

/// The points of a path that is a candidate rectangle: four or five line
/// segments after an opening move, no curves.
fn rect_candidate_points(path: &BezPath) -> Option<Vec<Point>> {
    let mut points = Vec::with_capacity(6);
    let mut closed = false;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if !points.is_empty() {
                    return None; // A second sub-path is never a rect.
                }
                points.push(p);
            }
            PathEl::LineTo(p) => {
                if points.is_empty() || closed {
                    return None;
                }
                points.push(p);
            }
            // `close_figure_` in the C++ marks the *preceding* point; kurbo
            // spells it as its own element. A close after four or five points
            // implies the closing segment back to the start.
            PathEl::ClosePath => closed = true,
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => return None,
        }
        if points.len() > 32 {
            return None; // Far past anything normalization could rescue.
        }
    }
    if closed && points.len() >= 4 && points.first() != points.last() {
        let first = *points.first()?;
        points.push(first);
    }
    Some(points)
}

/// `GetNormalizedPoints` (`cfx_path.cpp:70-108`): collapse zero-length
/// segments in a path of more than five points, bailing when more than five
/// survive.
fn normalize_points(points: &[Point]) -> Option<Vec<Point>> {
    if points.len() <= 5 {
        return Some(points.to_vec());
    }
    if points.first() != points.last() {
        return None;
    }
    let mut out: Vec<Point> = Vec::with_capacity(6);
    out.push(*points.first()?);
    for (i, p) in points.iter().enumerate().skip(1) {
        // Exactly five points left: stop normalizing and take the remainder.
        if out.len() + (points.len() - i) == 5 {
            out.extend_from_slice(points.get(i..)?);
            break;
        }
        if out.last() == Some(p) {
            continue; // The line does not move.
        }
        out.push(*p);
        if out.len() > 5 {
            return None;
        }
    }
    (out.len() == 5).then_some(out)
}

/// `XYBothNotEqual`: two points that differ on *both* axes cannot be adjacent
/// corners of an axis-aligned rectangle.
#[expect(
    clippy::float_cmp,
    reason = "upstream's `XYBothNotEqual` is bit-exact inequality: a rectangle \
              corner is a corner only when the coordinates are literally the \
              same value, and a tolerance would classify near-rectangles into \
              the never-antialiased fast path PDFium sends through the \
              general one"
)]
fn xy_both_differ(a: Point, b: Point) -> bool {
    a.x != b.x && a.y != b.y
}

/// `IsRectPreTransform` (`cfx_path.cpp:18-40`): four or five points, closing
/// exactly, non-degenerate diagonals, every segment a line.
fn is_rect_pre_transform(points: &[Point]) -> bool {
    if points.len() != 5 && points.len() != 4 {
        return false;
    }
    let (Some(&p0), Some(&p1), Some(&p2), Some(&p3)) =
        (points.first(), points.get(1), points.get(2), points.get(3))
    else {
        return false;
    };
    if points.len() == 5 && points.get(4) != Some(&p0) {
        return false;
    }
    // Coincident diagonal corners mean it is a line, not a rectangle.
    p0 != p2 && p1 != p3
}

/// `CFX_Path::GetRect(matrix)` (`cfx_path.cpp:429-466`): the axis-aligned
/// rectangle a path describes after `matrix`, or `None`.
///
/// Axis-alignment is tested with **exact float equality**, both before and
/// after the transform, so a 90-degree rotation qualifies and a 45-degree one
/// does not — and a rectangle that a floating-point transform nudges off-axis
/// by one ulp falls out of the fast path entirely, which is the behaviour
/// PDFium has and we must share.
#[must_use]
pub fn path_rect(path: &BezPath, matrix: Affine) -> Option<Rect> {
    let candidate = rect_candidate_points(path)?;
    let points = normalize_points(&candidate)?;
    if !is_rect_pre_transform(&points) {
        return None;
    }
    let transformed: Vec<Point> = points.iter().map(|&p| matrix * p).collect();
    for i in 1..transformed.len() {
        let (Some(&cur), Some(&prev)) = (transformed.get(i), transformed.get(i - 1)) else {
            return None;
        };
        if xy_both_differ(cur, prev) {
            return None;
        }
    }
    let (Some(&p0), Some(&p2), Some(&p3)) =
        (transformed.first(), transformed.get(2), transformed.get(3))
    else {
        return None;
    };
    if xy_both_differ(p0, p3) {
        return None;
    }
    Some(Rect::from_points(p0, p2))
}

/// An integer device rectangle, in the y-down convention both rasterizers use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRect {
    /// Left edge, inclusive.
    pub left: i32,
    /// Top edge, inclusive.
    pub top: i32,
    /// Right edge, exclusive.
    pub right: i32,
    /// Bottom edge, exclusive.
    pub bottom: i32,
}

impl IntRect {
    /// Whether the rectangle encloses at least one pixel.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.right > self.left && self.bottom > self.top
    }

    /// Width in pixels, saturating.
    #[must_use]
    pub fn width(self) -> i32 {
        self.right.saturating_sub(self.left)
    }

    /// Height in pixels, saturating.
    #[must_use]
    pub fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }

    /// As a `kurbo::Rect`.
    #[must_use]
    pub fn to_rect(self) -> Rect {
        Rect::new(
            f64::from(self.left),
            f64::from(self.top),
            f64::from(self.right),
            f64::from(self.bottom),
        )
    }

    /// The intersection, which may be empty.
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

/// `CFX_FloatRect::GetOuterRect`: the smallest integer rectangle containing
/// a float one.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the clamp bounds every finite coordinate to +/-i32::MAX/2 first, \
              so the narrowing is exact; a NaN survives the clamp and Rust's \
              saturating float-to-int cast turns it into 0, an empty rect"
)]
pub fn outer_rect(r: Rect) -> IntRect {
    let clamp = |v: f64| v.clamp(f64::from(i32::MIN) / 2.0, f64::from(i32::MAX) / 2.0);
    IntRect {
        left: clamp(r.x0.floor()) as i32,
        top: clamp(r.y0.floor()) as i32,
        right: clamp(r.x1.ceil()) as i32,
        bottom: clamp(r.y1.ceil()) as i32,
    }
}

/// The axis-aligned rectangle fill fast path
/// (`cfx_renderdevice.cpp:710-771`), reproduced exactly.
///
/// An axis-aligned rectangle in PDFium is **never antialiased** unless
/// `bRectAA` is set; it is snapped to the outer integer rect, promoted to at
/// least one pixel per axis, and then shrunk on whichever side sits further
/// from the true edge. The `>` in that comparison is strict, so a **tie
/// trims the right or bottom**. This rule is responsible for a large share of
/// the corpus's pixel-exact rectangles.
///
/// Returns `None` when a checked add overflows, which is upstream's `false`
/// and drops the object.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the two `ceil() as i32` narrowings are upstream's `(int)ceil(..)` \
              on a rect the caller has already hard-clipped to +/-32000; Rust's \
              saturating cast makes an out-of-range or NaN width 0 or i32::MAX, \
              both of which the `< 1` promotion and the checked adds below \
              handle without wrapping"
)]
pub fn snap_rect(rect_f: Rect) -> Option<IntRect> {
    let mut rect_i = outer_rect(rect_f);
    if !rect_i.is_valid() {
        return None;
    }

    let mut width = (rect_f.x1 - rect_f.x0).ceil() as i32;
    if width < 1 {
        width = 1;
        if rect_i.left == rect_i.right {
            rect_i.right = rect_i.right.checked_add(1)?;
        }
    }
    let mut height = (rect_f.y1 - rect_f.y0).ceil() as i32;
    if height < 1 {
        height = 1;
        if rect_i.top == rect_i.bottom {
            rect_i.bottom = rect_i.bottom.checked_add(1)?;
        }
    }

    if rect_i.width() >= width.checked_add(1)? {
        // Shrink whichever side lies further from the float edge; on a tie
        // the strict `>` sends the trim to the right.
        if rect_f.x0 - f64::from(rect_i.left) > f64::from(rect_i.right) - rect_f.x1 {
            rect_i.left = rect_i.left.checked_add(1)?;
        } else {
            rect_i.right = rect_i.right.checked_sub(1)?;
        }
    }
    if rect_i.height() >= height.checked_add(1)? {
        if rect_f.y0 - f64::from(rect_i.top) > f64::from(rect_i.bottom) - rect_f.y1 {
            rect_i.top = rect_i.top.checked_add(1)?;
        } else {
            rect_i.bottom = rect_i.bottom.checked_sub(1)?;
        }
    }
    Some(rect_i)
}

/// A path's device-space bounding box, as `kurbo` computes it.
#[must_use]
pub fn transformed_bbox(path: &BezPath, matrix: Affine) -> Rect {
    (matrix * path.clone()).bounding_box()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    #[test]
    fn is_available_matrix_is_not_a_determinant_test() {
        // A genuinely singular matrix passes; only the two collapsing
        // zero patterns are rejected (cpdf_renderstatus.cpp:148-158).
        assert!(is_available_matrix(Affine::new([
            1.0, 1.0, 1.0, 1.0, 0.0, 0.0
        ])));
        assert!(is_available_matrix(Affine::IDENTITY));
        // a == 0 needs both b and c non-zero: a 90-degree rotation.
        assert!(is_available_matrix(Affine::new([
            0.0, 1.0, -1.0, 0.0, 0.0, 0.0
        ])));
        assert!(!is_available_matrix(Affine::new([
            0.0, 0.0, 1.0, 1.0, 0.0, 0.0
        ])));
        assert!(!is_available_matrix(Affine::new([
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0
        ])));
        assert!(!is_available_matrix(Affine::new([0.0; 6])));
    }

    #[test]
    fn path_rect_accepts_four_and_five_point_rects() {
        let r = path_rect(&rect_path(1.0, 2.0, 5.0, 8.0), Affine::IDENTITY);
        assert_eq!(r, Some(Rect::new(1.0, 2.0, 5.0, 8.0)));
    }

    #[test]
    fn path_rect_rejects_a_45_degree_rotation_but_accepts_90() {
        let p = rect_path(0.0, 0.0, 10.0, 4.0);
        // An exact quarter turn — spelled as coefficients, because
        // `Affine::rotate(PI/2)` leaves a sub-ulp residue on the diagonal and
        // the axis test is exact float equality, so even the oracle would
        // reject that spelling. That strictness is the behaviour, not a bug.
        let quarter = Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            path_rect(&p, quarter),
            Some(Rect::new(-4.0, 0.0, 0.0, 10.0))
        );
        let eighth = Affine::rotate(std::f64::consts::FRAC_PI_4);
        assert!(path_rect(&p, eighth).is_none(), "45 degrees is not a rect");
    }

    #[test]
    fn path_rect_rejects_curves_and_degenerate_diagonals() {
        let mut curved = BezPath::new();
        curved.move_to((0.0, 0.0));
        curved.curve_to((1.0, 0.0), (2.0, 0.0), (3.0, 0.0));
        curved.close_path();
        assert!(path_rect(&curved, Affine::IDENTITY).is_none());

        // Coincident diagonal corners: a line, not a rect.
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((5.0, 0.0));
        line.line_to((0.0, 0.0));
        line.line_to((5.0, 0.0));
        line.close_path();
        assert!(path_rect(&line, Affine::IDENTITY).is_none());
    }

    #[test]
    fn path_rect_normalizes_six_plus_points() {
        // SixPlusPointRect: a repeated point collapses and the rect survives.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((0.0, 0.0)); // zero-length, collapses
        p.line_to((4.0, 0.0));
        p.line_to((4.0, 3.0));
        p.line_to((0.0, 3.0));
        p.close_path();
        assert_eq!(
            path_rect(&p, Affine::IDENTITY),
            Some(Rect::new(0.0, 0.0, 4.0, 3.0))
        );
    }

    #[test]
    fn rect_snap_promotes_sub_pixel_extents() {
        // A rect thinner than a pixel still paints exactly one column.
        let snapped = snap_rect(Rect::new(2.2, 5.0, 2.4, 9.0)).expect("snaps");
        assert_eq!(snapped.width(), 1);
        assert_eq!(snapped.left, 2);
        assert_eq!(snapped.right, 3);
    }

    #[test]
    fn rect_snap_rejects_an_exactly_zero_axis_before_promoting() {
        // The `rect_i.valid()` check runs *before* the sub-pixel promotion,
        // so a rectangle whose outer rect is already degenerate is dropped
        // rather than widened. The promotion only rescues a rect whose outer
        // rect is non-degenerate but whose float extent rounds below one.
        assert_eq!(snap_rect(Rect::new(3.0, 1.0, 3.0, 4.0)), None);
        assert_eq!(snap_rect(Rect::new(1.0, 2.0, 4.0, 2.0)), None);
    }

    #[test]
    fn rect_snap_shrinks_the_wider_side() {
        // outer = [1,5], width = ceil(4.6-1.4) = 4, so 4 >= 5 does not fire
        // and both edges survive.
        let snapped = snap_rect(Rect::new(1.4, 0.0, 4.6, 2.0)).expect("snaps");
        assert_eq!((snapped.left, snapped.right), (1, 5));

        // outer = [1,5] again but width = ceil(2.2) = 3, so 4 >= 4 fires.
        // Left overhang 0.9 vs right overhang 0.9 is a tie, and the strict
        // `>` sends the trim to the right.
        let snapped = snap_rect(Rect::new(1.9, 0.0, 4.1, 2.0)).expect("snaps");
        assert_eq!((snapped.left, snapped.right), (1, 4));

        // A genuinely lopsided rect trims the far side: outer [1,6],
        // width = ceil(4.0) = 4, 5 >= 5 fires, left overhang 0.95 beats
        // right overhang 0.05, so the *left* moves in.
        let snapped = snap_rect(Rect::new(1.95, 0.0, 5.95, 2.0)).expect("snaps");
        assert_eq!((snapped.left, snapped.right), (2, 6));
    }

    #[test]
    fn rect_snap_tie_goes_to_right_and_bottom() {
        // Equal overhangs on both sides: the strict `>` fails, so the right
        // and bottom are the ones trimmed.
        let snapped = snap_rect(Rect::new(1.5, 2.5, 4.5, 6.5)).expect("snaps");
        assert_eq!((snapped.left, snapped.right), (1, 4));
        assert_eq!((snapped.top, snapped.bottom), (2, 6));
    }

    #[test]
    fn rect_snap_leaves_integer_rects_alone() {
        let snapped = snap_rect(Rect::new(2.0, 3.0, 7.0, 11.0)).expect("snaps");
        assert_eq!(
            snapped,
            IntRect {
                left: 2,
                top: 3,
                right: 7,
                bottom: 11
            }
        );
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the clamp writes MAX_POS and copies 5.0/7.0 through verbatim; \
                  exact equality is what pins that no rounding crept in"
    )]
    fn hard_clip_clamps_rather_than_clips() {
        let mut p = BezPath::new();
        p.move_to((-99_999.0, 5.0));
        p.line_to((99_999.0, 7.0));
        let clipped = hard_clip(&p);
        let bbox = clipped.bounding_box();
        assert_eq!(bbox.x0, -MAX_POS);
        assert_eq!(bbox.x1, MAX_POS);
        // The y coordinates are untouched: it distorts, it does not cut.
        assert_eq!(bbox.y0, 5.0);
        assert_eq!(bbox.y1, 7.0);
    }

    /// The elements of a path as a comparable shape summary.
    fn shape(p: &BezPath) -> Vec<(&'static str, Option<(f64, f64)>)> {
        p.elements()
            .iter()
            .map(|el| match *el {
                PathEl::MoveTo(q) => ("m", Some((q.x, q.y))),
                PathEl::LineTo(q) => ("l", Some((q.x, q.y))),
                PathEl::QuadTo(_, q) => ("q", Some((q.x, q.y))),
                PathEl::CurveTo(_, _, q) => ("c", Some((q.x, q.y))),
                PathEl::ClosePath => ("h", None),
            })
            .collect()
    }

    #[test]
    fn a_degenerate_open_subpath_is_nudged_one_pixel_right() {
        // `50 40 m 50 40 l S` under a doubling matrix. The equality test is
        // on the *user*-space points; the `+1` lands in device space, so the
        // endpoint moves exactly one pixel however the matrix scales.
        let mut user = BezPath::new();
        user.move_to((50.0, 40.0));
        user.line_to((50.0, 40.0));
        let device = Affine::scale(2.0) * user.clone();
        let out = nudge_degenerate_subpaths(&device, &user);
        assert_eq!(
            shape(&out),
            vec![("m", Some((100.0, 80.0))), ("l", Some((101.0, 80.0)))],
            "one device pixel, not one user unit"
        );
    }

    #[test]
    fn a_degenerate_closed_subpath_is_nudged_and_loses_its_close() {
        // `50 40 m h S` after the page builder's round-cap rewrite. The close
        // must go: kurbo joins a closed two-vertex loop instead of capping
        // it, which turns the oracle's 21-wide stadium into a 1-wide bar.
        let mut user = BezPath::new();
        user.move_to((50.0, 40.0));
        user.line_to((50.0, 40.0));
        user.close_path();
        let out = nudge_degenerate_subpaths(&user, &user);
        assert_eq!(
            shape(&out),
            vec![("m", Some((50.0, 40.0))), ("l", Some((51.0, 40.0)))]
        );
    }

    #[test]
    fn three_identical_points_are_not_nudged() {
        // `40 140 m 40 140 l 40 140 l S`. `points[i + 1]` is a `kLine`, so
        // the condition fails, AGG collapses every vertex into one, and the
        // subpath paints nothing at all. `single_point_paths.in` relies on
        // exactly this.
        let mut user = BezPath::new();
        user.move_to((40.0, 140.0));
        user.line_to((40.0, 140.0));
        user.line_to((40.0, 140.0));
        assert_eq!(
            shape(&nudge_degenerate_subpaths(&user, &user)),
            shape(&user)
        );
    }

    #[test]
    fn a_real_segment_is_left_alone() {
        // The points differ, so nothing is degenerate and nothing moves.
        let mut user = BezPath::new();
        user.move_to((10.0, 10.0));
        user.line_to((20.0, 10.0));
        assert_eq!(
            shape(&nudge_degenerate_subpaths(&user, &user)),
            shape(&user)
        );
        // And a degenerate line that is *not* the first after its move is not
        // reached either: `points[i - 1]` must be the move itself.
        let mut two = BezPath::new();
        two.move_to((10.0, 10.0));
        two.line_to((20.0, 10.0));
        two.line_to((20.0, 10.0));
        assert_eq!(shape(&nudge_degenerate_subpaths(&two, &two)), shape(&two));
    }

    #[test]
    fn every_degenerate_subpath_of_a_multi_subpath_path_is_nudged() {
        let mut user = BezPath::new();
        user.move_to((1.0, 1.0));
        user.line_to((1.0, 1.0));
        user.move_to((5.0, 5.0));
        user.line_to((5.0, 5.0));
        assert_eq!(
            shape(&nudge_degenerate_subpaths(&user, &user)),
            vec![
                ("m", Some((1.0, 1.0))),
                ("l", Some((2.0, 1.0))),
                ("m", Some((5.0, 5.0))),
                ("l", Some((6.0, 5.0))),
            ]
        );
    }

    #[test]
    fn outer_rect_rounds_outward() {
        assert_eq!(
            outer_rect(Rect::new(1.2, 2.8, 3.1, 4.0)),
            IntRect {
                left: 1,
                top: 2,
                right: 4,
                bottom: 4
            }
        );
    }
}
