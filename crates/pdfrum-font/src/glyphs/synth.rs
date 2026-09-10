//! The two synthetic adjustments a substitution asks for: shearing an upright
//! face into an italic one, and dilating a light face into a bold one.
//!
//! Both exist because the replacement face is rarely the slant or the weight
//! the document asked for. PDFium applies them at two places each — the
//! glyph-*bitmap* side (`CFX_Face::RenderGlyph`, `cfx_face.cpp:769-778` and
//! `:806-816`) and the glyph-*path* side (`CFX_Face::LoadGlyphPath`,
//! `cfx_face.cpp:869-876` and `:886-892`) — reading a different skew and a
//! different embolden level on each.
//!
//! The C++ mutates an `FT_Matrix` and an `FT_Outline` in place because that is
//! FreeType's shape, not because the adjustments are stateful. Here the shear
//! is the [`Affine`] it always was, and the dilation returns a new path.

use pdfrum_common::kurbo::{Affine, BezPath, PathEl, Point, Vec2};

/// A resolved pair of synthetic adjustments, ready to apply to an outline.
///
/// The two numbers are already the *right* ones for the call site that built
/// it — the path side and the bitmap side read a different skew and a
/// different embolden level from the same [`crate::SubstFont`], and the
/// difference is not a detail: the bitmap side's level scales with the device
/// matrix while the path side's does not.
///
/// `embolden` is in whatever units the outline being adjusted is expressed in,
/// which is 1000/em on the path side and device pixels on the bitmap side.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SynthGlyph {
    /// Hundredths of a unit of displacement per unit of the other coordinate.
    pub skew: i32,
    /// Whether the shear runs down the page rather than across it.
    pub vertical: bool,
    /// How far to dilate the outline.
    pub embolden: f64,
}

impl SynthGlyph {
    /// No adjustment at all — what an embedded, unsubstituted font gets.
    pub const NONE: Self = Self {
        skew: 0,
        vertical: false,
        embolden: 0.0,
    };

    /// Shear then dilate, in that order.
    ///
    /// The order is the oracle's and it does not commute: the skew rides on
    /// the matrix the glyph is *loaded* through, while the embolden runs on
    /// the outline that comes back (`cfx_face.cpp:771-776` then `:812-815`).
    /// Dilating first would slant the added weight along with the glyph.
    #[must_use]
    pub fn apply(self, path: BezPath) -> BezPath {
        let sheared = if self.skew == 0 {
            path
        } else {
            shear(self.skew, self.vertical) * path
        };
        if self.embolden == 0.0 {
            sheared
        } else {
            embolden(&sheared, self.embolden)
        }
    }
}

/// The shear an effective skew asks for, as a transform on an outline.
///
/// The oracle folds the skew into the FreeType matrix before loading the glyph
/// — non-vertical `ft_matrix.xy -= ft_matrix.xx * skew / 100`, vertical
/// `ft_matrix.yx += ft_matrix.yy * skew / 100` (`cfx_face.cpp:771-776`). With
/// FreeType's `x' = xx*x + xy*y`, `y' = yx*x + yy*y`, factoring the base matrix
/// out on the left leaves exactly this shear on the right, which is why it can
/// be a value here instead of an edit to somebody's matrix: the composition
/// `base * shear` is the matrix the C++ builds.
///
/// The skew is hundredths of a unit of displacement per unit of the other
/// coordinate, and is *negative* for the usual right-leaning italic — so the
/// horizontal arm's minus sign is what makes a `-21` skew push the top of the
/// glyph to the right.
#[must_use]
pub(crate) fn shear(skew: i32, vertical: bool) -> Affine {
    let s = f64::from(skew) / 100.0;
    if vertical {
        Affine::new([1.0, s, 0.0, 1.0, 0.0, 0.0])
    } else {
        Affine::new([1.0, 0.0, -s, 1.0, 0.0, 0.0])
    }
}

/// Dilate an outline by `strength`, reproducing `FT_Outline_Embolden`.
///
/// FreeType pushes every point outward along the bisector of the two edges
/// meeting at it, rather than offsetting the curve properly, and then adds a
/// flat `strength` to both coordinates so the glyph grows away from the origin
/// instead of around it — which is why an emboldened glyph also shifts up and
/// to the right. Both halves are observable in PDFium's output, so both are
/// here.
///
/// `strength` is in the same units as the path's coordinates. It is halved
/// first, exactly as `FT_Outline_EmboldenXY` does (`ftoutln.c:929-932`), and a
/// strength that halves to nothing is a no-op.
///
/// The port works in `f64` where FreeType works in 16.16 fixed point. The
/// fixed-point rounding is not reproducible without carrying FreeType's whole
/// `FT_MulFix`/`FT_MulDiv` arithmetic into a space that is not even the same
/// scale here — our outlines are 1000/em, FreeType's are 26.6 at 64 ppem — and
/// the quantity being computed is a shift of a fraction of a unit. So this
/// ports the *behaviour*: the same bisector, the same orientation flip, the
/// same magnitude restriction, the same ~160° turn cutoff.
#[must_use]
pub(crate) fn embolden(path: &BezPath, strength: f64) -> BezPath {
    let strength = strength / 2.0;
    if strength == 0.0 || !strength.is_finite() {
        return path.clone();
    }
    // `FT_Outline_Get_Orientation` is a shoelace over the control points of
    // the *whole* outline, not per contour (`ftoutln.c:1080-1110`), and it
    // decides the sign of the bisector for every contour at once. An outline
    // with no signed area at all is `FT_ORIENTATION_NONE`, which FreeType
    // returns from without touching a point (`ftoutln.c:934-941`).
    let Some(truetype) = orientation_is_truetype(path) else {
        return path.clone();
    };

    let mut out = BezPath::new();
    for contour in contours(path) {
        let moved = embolden_contour(&contour.points, strength, truetype);
        emit(&mut out, &contour.els, &moved);
    }
    out
}

/// One subpath: its elements, and the control points they carry in order.
struct Contour {
    els: Vec<PathEl>,
    points: Vec<Point>,
}

/// Split a path into subpaths, each with its control points listed the way
/// FreeType lists a contour's — on-curve and off-curve alike, in path order.
fn contours(path: &BezPath) -> Vec<Contour> {
    let mut out: Vec<Contour> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) || out.is_empty() {
            out.push(Contour {
                els: Vec::new(),
                points: Vec::new(),
            });
        }
        let Some(last) = out.last_mut() else {
            continue;
        };
        last.els.push(*el);
        match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => last.points.push(p),
            PathEl::QuadTo(a, b) => last.points.extend([a, b]),
            PathEl::CurveTo(a, b, c) => last.points.extend([a, b, c]),
            PathEl::ClosePath => {}
        }
    }
    out
}

/// Rebuild a contour's elements around a moved point list.
fn emit(out: &mut BezPath, els: &[PathEl], points: &[Point]) {
    let mut n = 0;
    let next = |n: &mut usize| {
        let p = points.get(*n).copied().unwrap_or_default();
        *n += 1;
        p
    };
    for el in els {
        match el {
            PathEl::MoveTo(_) => {
                let p = next(&mut n);
                out.push(PathEl::MoveTo(p));
            }
            PathEl::LineTo(_) => {
                let p = next(&mut n);
                out.push(PathEl::LineTo(p));
            }
            PathEl::QuadTo(..) => {
                let (a, b) = (next(&mut n), next(&mut n));
                out.push(PathEl::QuadTo(a, b));
            }
            PathEl::CurveTo(..) => {
                let (a, b, c) = (next(&mut n), next(&mut n), next(&mut n));
                out.push(PathEl::CurveTo(a, b, c));
            }
            PathEl::ClosePath => out.push(PathEl::ClosePath),
        }
    }
}

/// The sign of the outline's area, over the control-point polygon
/// (`FT_Outline_Get_Orientation`, `ftoutln.c:1080-1157`).
///
/// A negative area is TrueType's clockwise convention, a positive one
/// PostScript's, and exactly zero is `FT_ORIENTATION_NONE` — an outline with
/// no inside, which the caller leaves alone rather than guessing a direction
/// to push its points in.
fn orientation_is_truetype(path: &BezPath) -> Option<bool> {
    let mut area = 0.0;
    for contour in contours(path) {
        let pts = &contour.points;
        let Some(&last) = pts.last() else { continue };
        let mut prev = last;
        for &p in pts {
            area += (p.y - prev.y) * (p.x + prev.x);
            prev = p;
        }
    }
    if area == 0.0 {
        return None;
    }
    Some(area < 0.0)
}

/// The point-shifting loop of `FT_Outline_EmboldenXY` (`ftoutln.c:962-1041`)
/// for one contour, in `f64`.
///
/// The loop is not the obvious "for each point, bisect its two edges":
/// FreeType walks `j` over every point but advances `i` only when a
/// *non-degenerate* edge was found, so a run of coincident points all take the
/// shift computed for the edge that finally leaves them. That is what the two
/// counters do, and dropping it changes which points move.
fn embolden_contour(points: &[Point], strength: f64, truetype: bool) -> Vec<Point> {
    let mut pts = points.to_vec();
    let count = pts.len();
    if count == 0 {
        return pts;
    }
    let last = count - 1;
    // The unit edge vector arriving at `hold`, and the length it had.
    let mut edge_in = Vec2::ZERO;
    let mut l_in = 0.0_f64;
    let mut anchor = Vec2::ZERO;
    let mut l_anchor = 0.0_f64;

    let mut hold = last;
    let mut scan = 0_usize;
    // `first_moved` is the first point the loop moved; FreeType's `-1` until then.
    let mut first_moved: Option<usize> = None;

    while scan != hold && Some(hold) != first_moved {
        let (edge_out, l_out) = if Some(scan) == first_moved {
            (anchor, l_anchor)
        } else {
            let Some((&head, &tail)) = pts.get(scan).zip(pts.get(hold)) else {
                break;
            };
            let edge = head - tail;
            let len = edge.hypot();
            if len == 0.0 {
                // A zero-length edge is skipped entirely: `hold` does not
                // advance, so the next real edge shifts these points too.
                scan = if scan < last { scan + 1 } else { 0 };
                continue;
            }
            (edge / len, len)
        };

        if l_in == 0.0 {
            hold = scan;
        } else {
            if first_moved.is_none() {
                first_moved = Some(hold);
                anchor = edge_in;
                l_anchor = l_in;
            }
            let dot = edge_in.dot(edge_out);
            // FreeType shifts only where the turn is less than about 160°;
            // `-0xF000` of 16.16 is -0.9375 (`ftoutln.c:993`).
            let mut shift = if dot > -0.9375 {
                let dot = dot + 1.0;
                // The lateral bisector, flipped by the outline's orientation.
                let mut bisector = Vec2::new(edge_in.y + edge_out.y, edge_in.x + edge_out.x);
                if truetype {
                    bisector.x = -bisector.x;
                } else {
                    bisector.y = -bisector.y;
                }
                // The magnitude restriction that keeps a collapsing segment
                // from being pushed through itself (`ftoutln.c:1007-1023`).
                let cross = edge_out.x * edge_in.y - edge_out.y * edge_in.x;
                let cross_signed = if truetype { -cross } else { cross };
                let shorter = l_in.min(l_out);
                if strength * cross_signed <= shorter * dot {
                    bisector * (strength / dot)
                } else {
                    bisector * (shorter / cross_signed)
                }
            } else {
                Vec2::ZERO
            };
            shift += Vec2::new(strength, strength);
            while hold != scan {
                if let Some(p) = pts.get_mut(hold) {
                    *p += shift;
                }
                hold = if hold < last { hold + 1 } else { 0 };
            }
        }

        edge_in = edge_out;
        l_in = l_out;
        scan = if scan < last { scan + 1 } else { 0 };
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_common::kurbo::Shape;

    /// A unit square, counter-clockwise in a y-up space.
    fn square(size: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((size, 0.0));
        p.line_to((size, size));
        p.line_to((0.0, size));
        p.close_path();
        p
    }

    #[test]
    fn a_negative_skew_pushes_the_top_of_a_glyph_to_the_right() {
        // `-21` is the skew a -12 degree italic angle asks for, and a right
        // lean means a point one unit up moves 0.21 units right.
        let m = shear(-21, false);
        let p = m * Point::new(0.0, 100.0);
        assert!((p.x - 21.0).abs() < 1e-9, "{p:?}");
        assert!((p.y - 100.0).abs() < 1e-9);
        // The baseline is the fixed line: a point on it does not move.
        let base = m * Point::new(50.0, 0.0);
        assert!((base.x - 50.0).abs() < 1e-9 && base.y.abs() < 1e-9);
    }

    #[test]
    fn a_vertical_skew_displaces_y_from_x_instead() {
        let m = shear(-21, true);
        let p = m * Point::new(100.0, 0.0);
        assert!((p.x - 100.0).abs() < 1e-9);
        assert!((p.y + 21.0).abs() < 1e-9, "{p:?}");
        // ...and a point on the y axis is the one left alone.
        let on_axis = m * Point::new(0.0, 50.0);
        assert!(on_axis.x.abs() < 1e-9 && (on_axis.y - 50.0).abs() < 1e-9);
    }

    #[test]
    fn a_zero_skew_is_the_identity() {
        assert_eq!(shear(0, false), Affine::IDENTITY);
        assert_eq!(shear(0, true), Affine::IDENTITY);
    }

    #[test]
    fn emboldening_grows_a_contour_s_area() {
        let before = square(100.0);
        let after = embolden(&before, 20.0);
        assert!(
            after.area().abs() > before.area().abs(),
            "{} vs {}",
            after.area().abs(),
            before.area().abs()
        );
    }

    #[test]
    fn a_zero_strength_leaves_an_outline_alone() {
        let before = square(100.0);
        assert_eq!(
            format!("{:?}", embolden(&before, 0.0)),
            format!("{before:?}")
        );
        // The halving is a truncation in FreeType and a real division here,
        // so only an exact zero short-circuits; the shape is still a no-op
        // in every practical sense for a strength that small.
        assert_eq!(
            format!("{:?}", embolden(&before, f64::NAN)),
            format!("{before:?}")
        );
    }

    #[test]
    fn emboldening_preserves_the_element_shape_of_a_path() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((10.0, 30.0), (60.0, 30.0), (70.0, 0.0));
        p.quad_to((35.0, -20.0), (0.0, 0.0));
        p.close_path();
        let after = embolden(&p, 8.0);
        let kind = |b: &BezPath| {
            b.elements()
                .iter()
                .map(|e| match e {
                    PathEl::MoveTo(_) => 'M',
                    PathEl::LineTo(_) => 'L',
                    PathEl::QuadTo(..) => 'Q',
                    PathEl::CurveTo(..) => 'C',
                    PathEl::ClosePath => 'Z',
                })
                .collect::<String>()
        };
        assert_eq!(kind(&after), kind(&p));
        assert_ne!(format!("{after:?}"), format!("{p:?}"));
    }

    /// FreeType's flat `+strength` on both coordinates is why an emboldened
    /// glyph does not merely fatten in place — it also drifts up and right.
    #[test]
    fn emboldening_drifts_the_outline_away_from_the_origin() {
        let before = square(100.0);
        let after = embolden(&before, 20.0);
        assert!(after.bounding_box().x1 > before.bounding_box().x1);
        assert!(after.bounding_box().y1 > before.bounding_box().y1);
    }

    #[test]
    fn a_degenerate_outline_survives_emboldening() {
        // Every point coincident: no edge is ever non-degenerate, so the loop
        // finds nothing to shift and must not spin or panic.
        let mut p = BezPath::new();
        p.move_to((5.0, 5.0));
        p.line_to((5.0, 5.0));
        p.line_to((5.0, 5.0));
        p.close_path();
        let after = embolden(&p, 10.0);
        assert_eq!(after.elements().len(), p.elements().len());
        assert!(BezPath::new().elements().is_empty());
        assert_eq!(embolden(&BezPath::new(), 10.0).elements().len(), 0);
    }
}
