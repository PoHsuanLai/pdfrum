//! Degenerate fills that PDFium redraws as hairlines.
//!
//! A filled sub-path that encloses no area would paint nothing at all, so the
//! oracle detects three shapes of degeneracy — a there-and-back line, a
//! palindrome, and a vertex where the path folds back on itself — and strokes
//! the collapsed segments as a one-device-pixel line at **quarter alpha**
//! instead. Corpus witness: `single_point_paths`.
//!
//! The detector runs per `MoveTo`-delimited sub-path, and only for a fill
//! with no stroke that is not a glyph outline (`opts.text_mode` excludes
//! those, which is why glyph stems never turn into hairlines).

use kurbo::{BezPath, PathEl, Point};

/// What the detector made of one sub-path.
#[derive(Debug, Clone, PartialEq)]
pub struct ZeroArea {
    /// The segments to stroke instead of filling. Empty means "draw
    /// nothing" — the all-points-identical case.
    pub path: BezPath,
    /// Whether the replacement is a *thin* line, which is what earns the
    /// quarter-alpha reduction.
    pub thin: bool,
    /// Whether the replacement is already in device space, so the caller must
    /// pass an identity matrix downstream.
    pub identity: bool,
}

/// Snap a coordinate to a pixel centre the way the oracle does: `(int)c +
/// 0.5`, i.e. **truncation toward zero** and then a half-pixel offset.
///
/// Truncation, not `floor`: at `-2.7` this gives `-1.5`, where a floor-based
/// reading would give `-2.5`.
#[must_use]
pub fn snap_to_pixel_center(c: f64) -> f64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the truncation toward zero *is* the ported operation — it is \
                  why -2.7 lands at -1.5 and not -2.5 — and Rust's saturating \
                  float-to-int cast turns an out-of-range or NaN coordinate \
                  into a bounded one rather than wrapping"
    )]
    let truncated = c as i32;
    f64::from(truncated) + 0.5
}

/// The two buffers a zero-area scan needs, kept between calls.
///
/// The scan runs on **every fill-only path object of every page** and almost
/// never finds anything, so building a `Vec<SubPath>` plus one `Vec<Point>`
/// per sub-path to answer "no" would be two allocations per path object.
/// Instead one sub-path's points go into a buffer that is cleared and
/// refilled, and the results into a second, and a caller that holds a
/// [`Scratch`] across a page allocates neither after the first path that
/// needs them.
///
/// A record of two buffers, not an object: [`scan_into`] is the operation and
/// this only holds its working memory. It lives on
/// [`RenderCaches`](crate::RenderCaches) with the glyph and image caches, which
/// is where a render session's reusable buffers belong.
#[derive(Debug, Default)]
pub struct Scratch {
    /// One sub-path's points, cleared per sub-path.
    points: Vec<Point>,
    /// The detections, cleared per path.
    found: Vec<ZeroArea>,
}

/// Apply the detector to every `MoveTo`-delimited sub-path of `path`, reusing
/// `scratch`'s buffers, and return what it found.
///
/// The borrow is the whole point: the result borrows `scratch`, so the caller
/// reads the detections and the buffers stay allocated for the next path. A
/// sub-path the detector rejects is *not* in the result and must still be
/// filled normally.
pub fn scan_into<'a>(
    scratch: &'a mut Scratch,
    path: &BezPath,
    transform: Option<kurbo::Affine>,
    adjust: bool,
) -> &'a [ZeroArea] {
    // The timer lives inside the function rather than around the call, because
    // the result borrows `scratch` and a wrapping closure would have to run the
    // scan twice or hand the borrow back out of it. See -P3.md §3.
    let started = crate::walkprofile::phase_start();
    let out = scan_into_inner(scratch, path, transform, adjust);
    started.end(crate::walkprofile::Phase::ZeroScan);
    out
}

/// [`scan_into`] without the phase timer around it.
fn scan_into_inner<'a>(
    scratch: &'a mut Scratch,
    path: &BezPath,
    transform: Option<kurbo::Affine>,
    adjust: bool,
) -> &'a [ZeroArea] {
    scratch.found.clear();
    scratch.points.clear();
    let mut has_curve = false;
    // A sub-path ends where the next `MoveTo` begins and where the elements
    // run out, so the detector is invoked from two places — here on the
    // boundary and once after the loop. Spelling it as a closure would need a
    // second mutable borrow of `scratch`; spelling it twice is three lines.
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if let Some(zero) = zero_area_path(&scratch.points, has_curve, transform, adjust) {
                    scratch.found.push(zero);
                }
                scratch.points.clear();
                has_curve = false;
                scratch.points.push(p);
            }
            PathEl::LineTo(p) => {
                if !scratch.points.is_empty() {
                    scratch.points.push(p);
                }
            }
            // Both curve kinds contribute only their endpoint to the point
            // list; the flag is what suppresses the folding scans downstream.
            PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                if !scratch.points.is_empty() {
                    scratch.points.push(p);
                    has_curve = true;
                }
            }
            PathEl::ClosePath => {}
        }
    }
    if let Some(zero) = zero_area_path(&scratch.points, has_curve, transform, adjust) {
        scratch.found.push(zero);
    }
    &scratch.found
}

/// `CheckSimpleLinePath`: a two-point `Move,Line`, or a three-point
/// `Move,Line,Line` that returns to its start.
///
/// When both points coincide it succeeds with an *empty* path — nothing is
/// drawn at all, which is distinct from failing to match.
fn check_simple_line(
    points: &[Point],
    transform: Option<kurbo::Affine>,
    adjust: bool,
) -> Option<ZeroArea> {
    if points.len() != 2 && points.len() != 3 {
        return None;
    }
    let (Some(&p0), Some(&p1)) = (points.first(), points.get(1)) else {
        return None;
    };
    if points.len() == 3 && points.get(2) != Some(&p0) {
        return None;
    }
    if p0 == p1 {
        // Everything is one point: zero area, and no thin line to draw.
        return Some(ZeroArea {
            path: BezPath::new(),
            thin: false,
            identity: false,
        });
    }
    let mut out = BezPath::new();
    let mut place = |p: Point, first: bool| {
        let p = if adjust {
            let p = transform.map_or(p, |m| m * p);
            Point::new(snap_to_pixel_center(p.x), snap_to_pixel_center(p.y))
        } else {
            p
        };
        if first {
            out.move_to(p);
        } else {
            out.line_to(p);
        }
    };
    place(p0, true);
    place(p1, false);
    Some(ZeroArea {
        path: out,
        thin: true,
        identity: adjust && transform.is_some(),
    })
}

/// `CheckPalindromicPath`: an odd point count above three, every pair
/// mirrored about the midpoint, no curves.
fn check_palindromic(points: &[Point], has_curve: bool) -> Option<ZeroArea> {
    if points.len() <= 3 || points.len().is_multiple_of(2) || has_curve {
        return None;
    }
    let mid = points.len() / 2;
    let mut out = BezPath::new();
    for i in 0..mid {
        let (Some(&left), Some(&right)) = (points.get(mid - i - 1), points.get(mid + i + 1)) else {
            return None;
        };
        if left != right {
            return None;
        }
        let &pivot = points.get(mid - i)?;
        out.move_to(pivot);
        out.line_to(left);
    }
    Some(ZeroArea {
        path: out,
        thin: true,
        identity: false,
    })
}

/// `IsFoldingVerticalLine`: three collinear points on one vertical, with the
/// middle one outside the other two.
#[expect(
    clippy::float_cmp,
    reason = "upstream's fold tests are bit-exact coordinate equality. A path \
              doubles back on itself only when the coordinates are literally \
              the same value; a tolerance would rewrite near-collinear paths \
              into hairlines PDFium fills normally"
)]
fn folding_vertical(a: Point, b: Point, c: Point) -> bool {
    a.x == b.x && b.x == c.x && (b.y - a.y) * (b.y - c.y) > 0.0
}

/// The horizontal mirror.
#[expect(
    clippy::float_cmp,
    reason = "see `folding_vertical`: bit-exact equality is the ported test"
)]
fn folding_horizontal(a: Point, b: Point, c: Point) -> bool {
    a.y == b.y && b.y == c.y && (b.x - a.x) * (b.x - c.x) > 0.0
}

/// The diagonal case: neither axis shared, but the cross-products match, so
/// the three points are collinear and the path doubles back.
#[expect(
    clippy::float_cmp,
    reason = "the cross-product equality is upstream's collinearity test, \
              computed and compared in exactly this order; an epsilon would \
              admit near-collinear triples PDFium rejects"
)]
fn folding_diagonal(a: Point, b: Point, c: Point) -> bool {
    a.x != b.x
        && c.x != b.x
        && a.y != b.y
        && c.y != b.y
        && (a.y - b.y) * (c.x - b.x) == (c.y - b.y) * (a.x - b.x)
}

/// The folding-vertex scan: for every interior line vertex whose neighbours
/// make the path double back, emit the **shorter** of the two segments.
///
/// Vertical folds pick by *y* distance; horizontal and diagonal ones pick by
/// *x*. The result is only marked thin when the sub-path had more than three
/// points and something was actually emitted.
fn check_folding(points: &[Point], has_curve: bool) -> Option<ZeroArea> {
    if has_curve || points.len() < 2 {
        return None;
    }
    let mut out = BezPath::new();
    for i in 1..points.len() {
        let next_index = (i + 1) % points.len();
        let (Some(&prev), Some(&cur), Some(&next)) =
            (points.get(i - 1), points.get(i), points.get(next_index))
        else {
            continue;
        };
        let use_prev = if folding_vertical(prev, cur, next) {
            (cur.y - prev.y).abs() < (cur.y - next.y).abs()
        } else if folding_horizontal(prev, cur, next) || folding_diagonal(prev, cur, next) {
            (cur.x - prev.x).abs() < (cur.x - next.x).abs()
        } else {
            continue;
        };
        let (start, end) = if use_prev { (prev, cur) } else { (cur, next) };
        out.move_to(start);
        out.line_to(end);
    }
    if out.elements().is_empty() {
        return None;
    }
    Some(ZeroArea {
        path: out,
        thin: points.len() > 3,
        identity: false,
    })
}

/// Detect a zero-area sub-path and produce its hairline replacement.
///
/// `adjust` is `!!GetDriverType()`, which the AGG driver answers `1` to — so
/// it is always true for us, and pixel snapping always applies. It is a
/// parameter only because the tests exercise both arms.
#[must_use]
pub fn zero_area_path(
    points: &[Point],
    has_curve: bool,
    transform: Option<kurbo::Affine>,
    adjust: bool,
) -> Option<ZeroArea> {
    if points.len() < 2 {
        return None;
    }
    check_simple_line(points, transform, adjust)
        .or_else(|| check_palindromic(points, has_curve))
        .or_else(|| check_folding(points, has_curve))
}

/// The alpha a thin zero-area replacement is stroked at: the fill's alpha
/// **shifted right by two**, i.e. a quarter.
///
/// Not a scale by 0.25 — a shift, so `255` becomes `63`, not `64`.
#[must_use]
pub fn thin_alpha(fill_alpha: u8) -> u8 {
    fill_alpha >> 2
}

#[cfg(test)]
mod tests {
    /// The detector applied to every `MoveTo`-delimited sub-path of `path`,
    /// owning the result.
    ///
    /// The walk never spells it this way — it calls [`scan_into`] with the
    /// session's [`Scratch`], because the scan runs on every fill-only path
    /// object and the allocations this signature forces were measured at 9866
    /// per render on one corpus document. A test asking the question once can
    /// afford them.
    fn zero_area_sub_paths(
        path: &BezPath,
        transform: Option<kurbo::Affine>,
        adjust: bool,
    ) -> Vec<ZeroArea> {
        let mut scratch = Scratch::default();
        scan_into(&mut scratch, path, transform, adjust).to_vec()
    }

    use kurbo::{Affine, Shape};

    use super::*;

    fn pts(v: &[(f64, f64)]) -> Vec<Point> {
        v.iter().map(|&(x, y)| Point::new(x, y)).collect()
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the snap emits `(int)c + 0.5`, which is exactly representable; \
                  exact equality is what distinguishes truncation from floor, \
                  the whole point of the test"
    )]
    fn simple_line_snap_truncates_toward_zero() {
        // (int)c + 0.5, not floor(c) + 0.5: -2.7 truncates to -2, giving -1.5.
        assert_eq!(snap_to_pixel_center(2.7), 2.5);
        assert_eq!(snap_to_pixel_center(-2.7), -1.5);
        assert_eq!(snap_to_pixel_center(0.9), 0.5);
        assert_eq!(snap_to_pixel_center(-0.9), 0.5);
    }

    #[test]
    fn simple_line_is_snapped_and_identity() {
        let z = zero_area_path(
            &pts(&[(1.2, 3.8), (7.9, 3.8)]),
            false,
            Some(Affine::IDENTITY),
            true,
        )
        .expect("a two-point path is a simple line");
        assert!(z.thin);
        assert!(z.identity, "a snapped path is already in device space");
        let bbox = z.path.bounding_box();
        assert_eq!((bbox.x0, bbox.y0), (1.5, 3.5));
        assert_eq!((bbox.x1, bbox.y1), (7.5, 3.5));
    }

    #[test]
    fn identical_points_draw_nothing() {
        let z = zero_area_path(&pts(&[(4.0, 4.0), (4.0, 4.0)]), false, None, true)
            .expect("matches, with an empty replacement");
        assert!(z.path.elements().is_empty());
        assert!(!z.thin);
    }

    #[test]
    fn three_point_there_and_back_matches() {
        let z = zero_area_path(
            &pts(&[(0.0, 0.0), (9.0, 0.0), (0.0, 0.0)]),
            false,
            None,
            false,
        )
        .expect("A->B->A is a simple line path");
        assert!(z.thin);
        assert!(!z.identity);
    }

    #[test]
    fn three_point_that_does_not_return_falls_through_to_the_fold_scan() {
        // The simple-line check requires the third point to equal the first,
        // so this one declines — but the points are still a horizontal fold,
        // and the fold scan picks it up with `thin` false.
        let p = pts(&[(0.0, 0.0), (9.0, 0.0), (1.0, 0.0)]);
        assert!(check_simple_line(&p, None, false).is_none());
        let z = zero_area_path(&p, false, None, false).expect("the fold scan matches");
        assert!(
            !z.thin,
            "three points never earn the quarter-alpha reduction"
        );
    }

    #[test]
    fn palindromic_needs_an_odd_count_above_three() {
        // 5 points mirrored about index 2.
        let p = pts(&[(0.0, 0.0), (5.0, 0.0), (9.0, 0.0), (5.0, 0.0), (0.0, 0.0)]);
        let z = zero_area_path(&p, false, None, false).expect("palindromic");
        assert!(z.thin);
        // An even count never qualifies.
        let even = pts(&[(0.0, 0.0), (5.0, 0.0), (5.0, 0.0), (0.0, 0.0)]);
        let z = zero_area_path(&even, false, None, false);
        assert!(z.is_none() || !matches!(z, Some(ref v) if v.path.elements().len() == 4));
    }

    #[test]
    fn palindromic_rejects_beziers() {
        let p = pts(&[(0.0, 0.0), (5.0, 0.0), (9.0, 0.0), (5.0, 0.0), (0.0, 0.0)]);
        assert!(check_palindromic(&p, true).is_none());
    }

    #[test]
    fn folding_vertical_picks_by_y_distance() {
        // prev(0,0) cur(0,10) next(0,7): the fold is at cur; |cur-prev| = 10,
        // |cur-next| = 3, so use_prev is false and the emitted segment is
        // cur->next, the shorter one.
        let a = Point::new(0.0, 0.0);
        let b = Point::new(0.0, 10.0);
        let c = Point::new(0.0, 7.0);
        assert!(folding_vertical(a, b, c));
        let z = check_folding(&[a, b, c], false).expect("folds");
        let bbox = z.path.bounding_box();
        assert_eq!((bbox.y0, bbox.y1), (7.0, 10.0));
    }

    #[test]
    fn folding_horizontal_and_diagonal_pick_by_x_distance() {
        let a = Point::new(0.0, 3.0);
        let b = Point::new(10.0, 3.0);
        let c = Point::new(7.0, 3.0);
        assert!(folding_horizontal(a, b, c));
        assert!(!folding_vertical(a, b, c));

        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 10.0);
        let c = Point::new(7.0, 7.0);
        assert!(folding_diagonal(a, b, c));
        // The diagonal predicate is *only* a collinearity test: unlike the
        // vertical and horizontal ones it carries no `> 0` product, so three
        // collinear points that do not double back match it too. Ported as
        // written — the shorter-segment emission is what saves it.
        assert!(folding_diagonal(
            Point::new(0.0, 0.0),
            Point::new(5.0, 5.0),
            Point::new(9.0, 9.0)
        ));
        // Axis-aligned points are excluded from the diagonal arm outright.
        assert!(!folding_diagonal(
            a,
            Point::new(0.0, 5.0),
            Point::new(0.0, 3.0)
        ));
    }

    #[test]
    fn folding_thin_only_above_three_points() {
        // Exactly three points reach the fold scan only when the simple-line
        // check declined, and then `thin` stays false.
        let z = check_folding(&pts(&[(0.0, 0.0), (0.0, 10.0), (0.0, 7.0)]), false).expect("folds");
        assert!(!z.thin, "points.len() > 3 is required for thin");
    }

    #[test]
    fn thin_alpha_quarter_is_a_shift() {
        // >> 2, not * 0.25: 255 becomes 63.
        assert_eq!(thin_alpha(255), 63);
        assert_eq!(thin_alpha(127), 31);
        assert_eq!(thin_alpha(3), 0);
    }

    #[test]
    fn sub_paths_are_detected_independently() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0)); // degenerate: a bare line
        p.move_to((20.0, 20.0));
        p.line_to((30.0, 20.0));
        p.line_to((30.0, 30.0));
        p.line_to((20.0, 30.0));
        p.close_path(); // a real rectangle, not degenerate
        let found = zero_area_sub_paths(&p, None, false);
        assert_eq!(found.len(), 1);
    }

    /// The scratch-reusing scan must answer exactly what the allocating one
    /// answered, on the shapes the sub-path split can differ on: a leading
    /// segment before any `MoveTo`, an empty path, a bare `MoveTo`, several
    /// sub-paths of different kinds, and a curve (whose flag suppresses two of
    /// the three detectors). This is the specification of the change — it is a
    /// memory change and must be nothing else.
    #[test]
    fn the_reused_scratch_finds_exactly_what_a_fresh_one_finds() {
        // The reference is the implementation this replaced, written out here
        // rather than called, so the assertion compares against the old code
        // and not against a wrapper around the new code.
        fn allocating_reference(
            path: &BezPath,
            transform: Option<kurbo::Affine>,
            adjust: bool,
        ) -> Vec<ZeroArea> {
            struct SubPath {
                points: Vec<Point>,
                has_curve: bool,
            }
            let mut subs: Vec<SubPath> = Vec::new();
            for el in path.elements() {
                match *el {
                    PathEl::MoveTo(p) => subs.push(SubPath {
                        points: vec![p],
                        has_curve: false,
                    }),
                    PathEl::LineTo(p) => {
                        if let Some(last) = subs.last_mut() {
                            last.points.push(p);
                        }
                    }
                    PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                        if let Some(last) = subs.last_mut() {
                            last.points.push(p);
                            last.has_curve = true;
                        }
                    }
                    PathEl::ClosePath => {}
                }
            }
            subs.into_iter()
                .filter_map(|sp| zero_area_path(&sp.points, sp.has_curve, transform, adjust))
                .collect()
        }

        // A `ClosePath` between two sub-paths, and a `MoveTo` immediately
        // followed by another: both are places the boundary logic could put a
        // point on the wrong list.
        let mut boundaries = BezPath::new();
        boundaries.move_to((0.0, 0.0));
        boundaries.line_to((9.0, 0.0));
        boundaries.close_path();
        boundaries.move_to((1.0, 1.0));
        boundaries.move_to((2.0, 2.0));
        boundaries.line_to((2.0, 8.0));

        let mut several = BezPath::new();
        several.move_to((0.0, 0.0));
        several.line_to((10.0, 0.0));
        several.move_to((20.0, 20.0));
        several.line_to((30.0, 20.0));
        several.line_to((30.0, 30.0));
        several.line_to((20.0, 30.0));
        several.close_path();
        several.move_to((40.0, 40.0));
        several.line_to((50.0, 50.0));
        several.line_to((40.0, 40.0));

        let mut curved = BezPath::new();
        curved.move_to((0.0, 0.0));
        curved.curve_to((1.0, 1.0), (2.0, 2.0), (3.0, 3.0));

        let mut bare = BezPath::new();
        bare.move_to((7.0, 7.0));

        let paths = [boundaries, several, curved, bare, BezPath::new()];
        // One scratch across all of them, which is the point: the second path
        // runs against buffers the first left behind.
        let mut scratch = Scratch::default();
        for path in &paths {
            for adjust in [false, true] {
                for transform in [None, Some(kurbo::Affine::scale(2.0))] {
                    let fresh = allocating_reference(path, transform, adjust);
                    let reused = scan_into(&mut scratch, path, transform, adjust);
                    assert_eq!(fresh.as_slice(), reused, "{path:?} adjust={adjust}");
                }
            }
        }
    }
}
