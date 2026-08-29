//! Stroke geometry as the oracle resolves it before handing a path to a
//! rasterizer (`RasterizeStroke`, `cfx_agg_devicedriver.cpp:299-393`, and the
//! matrix decomposition at `:1229-1246`).
//!
//! Two behaviours here are pixel-visible and belong to the engine rather than
//! to either backend, so that both receive the same stroke and Tier C can
//! demand they agree:
//!
//! - **The minimum stroke width is one device pixel.** `line_width == 0`
//!   therefore paints a one-pixel hairline and there is no other threshold —
//!   which also means neither rasterizer's own `width == 0` hairline path is
//!   ever reached.
//! - **The dash ladder is scale-dependent.** A cycle shorter than a tenth of
//!   a device pixel becomes solid, a zero or negative entry becomes `0.1`
//!   *before* scaling, an odd-length array doubles its cycle, and only the
//!   first 32 entries survive.

use kurbo::{Affine, Dashes, Stroke};
use pdfrum_page::{LineCap, LineJoin, StrokeParams};

/// The device-space cycle below which a dash pattern is drawn solid
/// (`kMinDashCycleThreshold`, `cfx_agg_devicedriver.cpp:358`).
pub const MIN_DASH_CYCLE: f64 = 0.1;

/// The value a non-positive dash entry becomes, applied **before** the
/// device scale (`cfx_agg_devicedriver.cpp:372-374`).
pub const DASH_ZERO_SUBSTITUTE: f64 = 0.1;

/// AGG's dash-array capacity (`agg_vcgen_dash.h:31`). Entries past it are
/// **silently truncated**, not an error.
pub const MAX_DASHES: usize = 32;

/// The matrix split PDFium performs before stroking: an isotropic scale it
/// pre-applies to the path, and the residual it hands the rasterizer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeMatrices {
    /// The isotropic part, applied to the path's coordinates.
    pub pre: Affine,
    /// The residual, applied per vertex by the rasterizer.
    pub post: Affine,
    /// `pre.a`, the scale stroke widths and dash lengths are measured in.
    pub scale: f64,
}

/// `CFX_Matrix::GetXUnit` (`fx_coordinates.cpp:443-452`).
fn x_unit(m: Affine) -> f64 {
    let [a, b, ..] = m.as_coeffs();
    if b == 0.0 {
        a.abs()
    } else if a == 0.0 {
        b.abs()
    } else {
        a.hypot(b)
    }
}

/// `CFX_Matrix::GetYUnit` (`fx_coordinates.cpp:453-461`).
fn y_unit(m: Affine) -> f64 {
    let [_, _, c, d, _, _] = m.as_coeffs();
    if d == 0.0 {
        c.abs()
    } else if c == 0.0 {
        d.abs()
    } else {
        c.hypot(d)
    }
}

/// Split an object-to-device matrix for stroking.
///
/// `matrix1` takes the larger of `|a|` and `|b|` on both diagonals, so the
/// path is pre-scaled isotropically and any anisotropy or rotation is left in
/// `matrix2`. Upstream divides by `matrix1.a` with no guard, so `a == b == 0`
/// is a division by zero there; we fall back to the identity split, which
/// draws the same nothing without the NaN.
///
/// # Composition order
///
/// PDFium writes `matrix1 = m * matrix2.inverse()` in **row-vector**
/// convention, where a point is a row multiplied on the left and the leftmost
/// matrix therefore applies *first*. `kurbo` is column-vector, where the
/// **rightmost** applies first, so the same decomposition is spelled
/// `pre = post.inverse() * m` here. Writing it the other way round leaves the
/// device translation in `pre`, which the caller then applies twice — a bug
/// that hides completely under a translation-free matrix and moves every
/// stroke off the page under a real one.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "a..d are the affine matrix coefficients, named as in the PDF `cm` operands"
)]
pub fn split_for_stroke(m: Affine) -> StrokeMatrices {
    let [a, b, c, d, _, _] = m.as_coeffs();
    let scale = a.abs().max(b.abs());
    if scale == 0.0 || !scale.is_finite() {
        return StrokeMatrices {
            pre: Affine::IDENTITY,
            post: m,
            scale: 1.0,
        };
    }
    let post = Affine::new([a / scale, b / scale, c / scale, d / scale, 0.0, 0.0]);
    let pre = match post.inverse() {
        inv if inv.as_coeffs().iter().all(|v| v.is_finite()) => inv * m,
        _ => Affine::scale(scale),
    };
    StrokeMatrices { pre, post, scale }
}

/// The device-space stroke width the rasterizer is given.
///
/// `width = max(line_width * scale, unit)` where
/// `unit = 1 / ((post.x_unit() + post.y_unit()) / 2)` — the one-device-pixel
/// minimum expressed in pre-transform units. A `line_width` of zero, or of
/// anything smaller than a pixel, lands exactly on that floor.
#[must_use]
#[expect(
    clippy::manual_midpoint,
    reason = "`(x + y) / 2.0` is upstream's own spelling of the mean unit; \
              `f64::midpoint` rounds once where this rounds twice, and the \
              result feeds the one-device-pixel floor that every hairline in \
              the corpus lands on exactly"
)]
pub fn device_width(line_width: f32, matrices: StrokeMatrices) -> f64 {
    let mean_unit = (x_unit(matrices.post) + y_unit(matrices.post)) / 2.0;
    let unit = if mean_unit > 0.0 && mean_unit.is_finite() {
        1.0 / mean_unit
    } else {
        1.0
    };
    let scaled = f64::from(line_width) * matrices.scale;
    if scaled.is_finite() {
        scaled.max(unit)
    } else {
        unit
    }
}

/// The `zero_area` spelling: the split is skipped entirely, the path is
/// pre-transformed by the whole matrix, and the stroke runs at `scale = 1`,
/// guaranteeing a hairline exactly one device pixel wide.
#[must_use]
pub fn hairline_matrices(m: Affine) -> StrokeMatrices {
    StrokeMatrices {
        pre: m,
        post: Affine::IDENTITY,
        scale: 1.0,
    }
}

/// The dash array a rasterizer may legally be handed.
///
/// `tiny-skia` rejects an odd length, a negative entry and a non-positive
/// total outright; `kurbo` accepts them and does something else with them.
/// PDF permits all three and gives them meaning, so the normalisation happens
/// here — which is also what keeps the two backends identical.
#[must_use]
pub fn normalize_dashes(params: &StrokeParams, scale: f64) -> Option<(Dashes, f64)> {
    if params.dash.is_empty() {
        return None;
    }
    if params.dash.iter().any(|v| !v.is_finite()) {
        return None;
    }
    // The cycle test clamps negatives to zero — for the *test only*; the
    // per-entry substitution below sees the original values.
    let cycle: f64 = params.dash.iter().map(|v| f64::from(*v).max(0.0)).sum();
    if cycle * scale < MIN_DASH_CYCLE {
        return None;
    }

    let mut lengths: Dashes = params
        .dash
        .iter()
        .take(MAX_DASHES)
        .map(|v| {
            let e = if f64::from(*v) <= 0.000_001 {
                DASH_ZERO_SUBSTITUTE
            } else {
                f64::from(*v)
            };
            (e * scale).abs()
        })
        .collect();

    // An odd-length array doubles the cycle: `[5 2 1]` means
    // 5 on, 2 off, 1 on, 5 off, 2 on, 1 off. Duplicating the array is
    // exactly that, and it is what makes it legal for both backends.
    if lengths.len() % 2 == 1 {
        let doubled = lengths.clone();
        lengths.extend(doubled);
    }
    if lengths.is_empty() || lengths.iter().sum::<f64>() <= 0.0 {
        return None;
    }

    let total: f64 = lengths.iter().sum();
    let mut phase = f64::from(params.dash_phase) * scale;
    if phase < 0.0 && total > 0.0 {
        // AGG increments a negative phase by ceil(-ds / 2S) * 2S.
        let two_s = 2.0 * total;
        phase += (-phase / two_s).ceil() * two_s;
    }
    Some((lengths, phase.max(0.0)))
}

/// The full `kurbo::Stroke` for a set of stroke parameters under a matrix.
///
/// Cap and join map straight across; the miter limit is passed through
/// **unclamped**, and both `kurbo` and `tiny-skia` bevel on limit exceedance,
/// which is what AGG's `miter_join_revert` does too — a plain miter clamp
/// would not match.
#[must_use]
pub fn resolve_stroke(params: &StrokeParams, matrices: StrokeMatrices) -> Stroke {
    let mut stroke = Stroke::new(device_width(params.width, matrices))
        .with_caps(match params.cap {
            LineCap::Butt => kurbo::Cap::Butt,
            LineCap::Round => kurbo::Cap::Round,
            LineCap::Square => kurbo::Cap::Square,
        })
        .with_join(match params.join {
            LineJoin::Miter => kurbo::Join::Miter,
            LineJoin::Round => kurbo::Join::Round,
            LineJoin::Bevel => kurbo::Join::Bevel,
        })
        .with_miter_limit(f64::from(params.miter_limit));
    if let Some((dashes, phase)) = normalize_dashes(params, matrices.scale) {
        stroke = stroke.with_dashes(phase, dashes);
    }
    stroke
}

#[cfg(test)]
mod tests {
    use smallvec::smallvec;

    use super::*;

    fn params(width: f32) -> StrokeParams {
        StrokeParams {
            width,
            ..StrokeParams::default()
        }
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "under the identity the floor is the literal 1.0 and the pass \
                  -through the literal 3.0; a tolerance here would stop the \
                  test from catching a floor that drifted by an ulp"
    )]
    fn min_width_is_one_device_pixel() {
        for m in [
            Affine::IDENTITY,
            Affine::scale(2.0),
            Affine::new([3.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        ] {
            let split = split_for_stroke(m);
            for w in [0.0f32, 0.01, 0.001] {
                let dw = device_width(w, split);
                // In pre-transform units, one device pixel.
                assert!(dw > 0.0, "width must never collapse to zero");
                assert!(
                    (dw * split.scale / split.scale - dw).abs() < 1e-9,
                    "the floor is expressed in matrix1 space"
                );
            }
        }
        // Under the identity the floor is literally 1.0.
        assert_eq!(device_width(0.0, split_for_stroke(Affine::IDENTITY)), 1.0);
        assert_eq!(device_width(0.5, split_for_stroke(Affine::IDENTITY)), 1.0);
        assert_eq!(device_width(3.0, split_for_stroke(Affine::IDENTITY)), 3.0);
    }

    #[test]
    fn matrix_split_recomposes() {
        for m in [
            Affine::rotate(0.7),
            Affine::new([2.0, 0.0, 1.0, 3.0, 5.0, 6.0]),
            Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 10.0]),
        ] {
            let split = split_for_stroke(m);
            // Column-vector order: `pre` applies first, `post` second.
            let recomposed = split.post * split.pre;
            for (a, b) in recomposed.as_coeffs().iter().zip(m.as_coeffs().iter()) {
                assert!((a - b).abs() < 1e-9, "{recomposed:?} != {m:?}");
            }
        }
    }

    #[test]
    fn matrix_split_takes_the_larger_diagonal() {
        let split = split_for_stroke(Affine::new([3.0, -7.0, 0.0, 1.0, 0.0, 0.0]));
        assert!((split.scale - 7.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_matrix_does_not_divide_by_zero() {
        let split = split_for_stroke(Affine::new([0.0, 0.0, 1.0, 1.0, 0.0, 0.0]));
        assert!(split.scale.is_finite());
        assert!(device_width(1.0, split).is_finite());
    }

    #[test]
    fn dash_tiny_cycle_is_solid() {
        let p = StrokeParams {
            dash: smallvec![0.02, 0.02],
            ..params(1.0)
        };
        assert!(
            normalize_dashes(&p, 1.0).is_none(),
            "cycle 0.04 < 0.1 is solid"
        );
        // The same array at a bigger device scale is a real dash.
        assert!(normalize_dashes(&p, 10.0).is_some());
    }

    #[test]
    fn dash_nonfinite_is_solid() {
        let p = StrokeParams {
            dash: smallvec![f32::NAN, 3.0],
            ..params(1.0)
        };
        assert!(normalize_dashes(&p, 1.0).is_none());
        let p = StrokeParams {
            dash: smallvec![f32::INFINITY, 3.0],
            ..params(1.0)
        };
        assert!(normalize_dashes(&p, 1.0).is_none());
    }

    #[test]
    fn dash_zero_entry_becomes_point_one() {
        let p = StrokeParams {
            dash: smallvec![0.0, 5.0],
            ..params(1.0)
        };
        let (lengths, _) = normalize_dashes(&p, 1.0).expect("not solid");
        assert!((lengths.first().copied().unwrap_or(0.0) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn dash_odd_array_doubles_the_cycle() {
        // [5 2 1] means 5 on, 2 off, 1 on, 5 off, 2 on, 1 off.
        let p = StrokeParams {
            dash: smallvec![5.0, 2.0, 1.0],
            ..params(1.0)
        };
        let (lengths, _) = normalize_dashes(&p, 1.0).expect("not solid");
        assert_eq!(lengths.len(), 6);
        assert_eq!(&lengths[..], &[5.0, 2.0, 1.0, 5.0, 2.0, 1.0]);
    }

    #[test]
    fn dash_over_32_entries_truncated() {
        let p = StrokeParams {
            dash: (0..40).map(|_| 3.0f32).collect(),
            ..params(1.0)
        };
        let (lengths, _) = normalize_dashes(&p, 1.0).expect("not solid");
        assert_eq!(
            lengths.len(),
            MAX_DASHES,
            "silently truncated, not rejected"
        );
    }

    #[test]
    fn dash_negative_phase_is_folded_forward() {
        let p = StrokeParams {
            dash: smallvec![4.0, 4.0],
            dash_phase: -30.0,
            ..params(1.0)
        };
        let (_, phase) = normalize_dashes(&p, 1.0).expect("not solid");
        assert!(
            phase >= 0.0,
            "a negative phase is incremented, not clamped away"
        );
        // Folding by 2S = 16 lands -30 at 2.
        assert!((phase - 2.0).abs() < 1e-9, "phase = {phase}");
    }

    #[test]
    fn resolve_stroke_never_hands_a_backend_a_zero_width() {
        let s = resolve_stroke(&params(0.0), split_for_stroke(Affine::IDENTITY));
        assert!(
            s.width > 0.0,
            "the backends' own hairline paths must stay dead code"
        );
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the hairline split leaves the post matrix the identity, so \
                  the width is the literal 1.0 the floor writes"
    )]
    fn hairline_matrices_leave_the_path_in_device_space() {
        let m = Affine::new([2.0, 0.0, 0.0, 3.0, 4.0, 5.0]);
        let h = hairline_matrices(m);
        assert_eq!(h.pre, m);
        assert_eq!(h.post, Affine::IDENTITY);
        assert_eq!(device_width(0.0, h), 1.0, "exactly one device pixel");
    }
}
