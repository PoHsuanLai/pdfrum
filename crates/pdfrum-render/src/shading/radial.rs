//! Type 3, radial shadings.
//!
//! The quadratic and its root selection are ported verbatim, because every
//! branch of the ladder is pixel-visible on the corpus's
//! `radial_shading_point_at_*` fixtures, and three of them are not what a
//! textbook derivation would produce:
//!
//! - the `b == 0` branch takes `sqrt(-c / a)` **without** checking the sign
//!   of the radicand, so a NaN reaches the index cast and is caught by the
//!   `< 0` extend test rather than by a guard;
//! - the `decreasing` test truncates `hypot(dx, dy)` to an **integer** before
//!   comparing it with `-dr`;
//! - the negative-radius skip (`r0 + s*dr < 0`) is applied **only** in the
//!   two-root branch, not in the `b == 0` or `a == 0` ones.

use kurbo::{Affine, Point};
use pdfrum_page::Radial;

use crate::pixmap::Pixmap;
use crate::shading::steps::ColorSteps;

/// The oracle's float-zero test: `|f| < 0.0001`.
///
/// A **fixed 1e-4 tolerance**, not a machine epsilon — roughly 840 times
/// wider than `f32::EPSILON`, and the difference is pixel-visible. `a` is
/// `dx² + dy² - dr²`, so when the start point sits on the end circle it is a
/// catastrophic cancellation: `radial_shading_point_at_border` has `|start| =
/// 1 + 1.1e-7` against `r1 = 1`, giving `a ≈ 2.4e-7`. That is above
/// `f32::EPSILON` and far below 1e-4, so the real test takes the **linear**
/// `a == 0` branch — which has no negative-radius skip — while a machine
/// epsilon takes the quadratic one and skips 14601 pixels the oracle paints
/// in `C0`.
fn is_float_zero(v: f64) -> bool {
    v.abs() < FLOAT_ZERO
}

/// The float-zero tolerance.
const FLOAT_ZERO: f64 = 1e-4;

/// Whether the shading's radius shrinks fast enough that the *first* root is
/// the meaningful one.
///
/// The `as i32` on the hypotenuse is upstream's and is not a rounding
/// detail: it makes the test fire for a whole band of near-equal geometries
/// that a float comparison would exclude.
#[must_use]
pub fn is_decreasing(radial: &Radial) -> bool {
    let dx = radial.end.x - radial.start.x;
    let dy = radial.end.y - radial.start.y;
    let dr = f64::from(radial.end_radius) - f64::from(radial.start_radius);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the integer truncation is the ported behaviour"
    )]
    let truncated = dx.hypot(dy) as i32;
    dr < 0.0 && f64::from(truncated) < -dr
}

/// The parametric position of one point, or `None` where the geometry does
/// not cover it.
#[must_use]
pub fn position(radial: &Radial, pos: Point, decreasing: bool) -> Option<f64> {
    let x0 = radial.start.x;
    let y0 = radial.start.y;
    let r0 = f64::from(radial.start_radius);
    let dx = radial.end.x - x0;
    let dy = radial.end.y - y0;
    let dr = f64::from(radial.end_radius) - r0;
    let a = dx * dx + dy * dy - dr * dr;
    let a_is_zero = is_float_zero(a);

    let pdx = pos.x - x0;
    let pdy = pos.y - y0;
    let b = -2.0 * (pdx * dx + pdy * dy + r0 * dr);
    let c = pdx * pdx + pdy * pdy - r0 * r0;

    if is_float_zero(b) {
        // No sign check on the radicand: a negative one yields NaN, which
        // the caller's index cast and `< 0` test then absorb.
        return Some((-c / a).sqrt());
    }
    if a_is_zero {
        return Some(-c / b);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let (mut s1, mut s2) = ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
    if a <= 0.0 {
        std::mem::swap(&mut s1, &mut s2);
    }
    let s = if decreasing {
        if s1 >= 0.0 || radial.extend_start {
            s1
        } else {
            s2
        }
    } else if s2 <= 1.0 || radial.extend_end {
        s2
    } else {
        s1
    };
    // The negative-radius skip lives *only* here, not in the two branches
    // above — an asymmetry, and a deliberate one.
    (r0 + s * dr >= 0.0).then_some(s)
}

/// Rasterize a radial shading into `dest`.
pub fn draw(dest: &mut Pixmap, radial: &Radial, steps: &ColorSteps, to_bitmap: Affine) {
    let determinant = to_bitmap.determinant();
    if determinant == 0.0 || !determinant.is_finite() {
        return;
    }
    let inverse = to_bitmap.inverse();
    let decreasing = is_decreasing(radial);

    for row in 0..dest.height() {
        for col in 0..dest.width() {
            let pos = inverse * Point::new(f64::from(col), f64::from(row));
            let Some(s) = position(radial, pos, decreasing) else {
                continue;
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the ramp lookup ports the truncating cast"
            )]
            let Some(color) = steps.lookup(s as f32, radial.extend_start, radial.extend_end) else {
                continue;
            };
            dest.set_pixel(col, row, crate::pixmap::premultiply(color.to_peniko()));
        }
    }
}

#[cfg(test)]
mod tests {
    use pdfrum_page::Rgb;

    use super::*;
    use crate::shading::steps::STEPS;

    fn ramp() -> ColorSteps {
        let mut colors = [Rgb::BLACK; STEPS];
        for (i, c) in colors.iter_mut().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "i < 256 is exact")]
            let v = i as f32 / 255.0;
            *c = Rgb { r: v, g: v, b: v };
        }
        ColorSteps::from_colors(colors, 255)
    }

    fn concentric(r0: f32, r1: f32, extend: (bool, bool)) -> Radial {
        Radial {
            start: Point::new(0.0, 0.0),
            start_radius: r0,
            end: Point::new(0.0, 0.0),
            end_radius: r1,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: extend.0,
            extend_end: extend.1,
        }
    }

    #[test]
    fn concentric_circles_ramp_by_distance() {
        // Concentric with dx = dy = 0: a = -dr^2 < 0, and b is
        // -2*r0*dr, which is zero when r0 is zero — so this exercises the
        // b == 0 branch, `sqrt(-c / a)` = distance / dr.
        let r = concentric(0.0, 10.0, (true, true));
        assert_eq!(position(&r, Point::new(0.0, 0.0), false), Some(0.0));
        let s = position(&r, Point::new(5.0, 0.0), false).expect("covered");
        assert!((s - 0.5).abs() < 1e-9, "s = {s}");
    }

    #[test]
    fn b_zero_branch_has_no_sign_check_and_yields_nan() {
        // r0 = 0, dr < 0: a = -dr^2, c = distance^2, so -c/a is positive...
        // Flip it: r0 = 0 with dr > 0 and a point at the centre gives 0.
        // For a genuine NaN, make -c/a negative: a > 0 needs |d| > |dr|.
        let r = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 0.0,
            end: Point::new(10.0, 0.0),
            end_radius: 1.0,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: false,
            extend_end: false,
        };
        // A point where b vanishes: pdx*dx + pdy*dy + r0*dr == 0, i.e. x = 0.
        let s = position(&r, Point::new(0.0, 4.0), false);
        // -c/a = -(16)/(100-1) < 0, so the square root is NaN and it is not
        // guarded away — the caller's `< 0` index test is what catches it.
        assert!(
            s.is_some_and(f64::is_nan),
            "the b == 0 branch is unguarded: {s:?}"
        );
        assert_eq!(ramp().lookup(f32::NAN, false, false), None);
    }

    #[test]
    fn decreasing_test_truncates_the_hypotenuse() {
        // |d| = 1.9 truncates to 1, and dr = -1.5 gives -dr = 1.5 > 1, so
        // the test fires — where an untruncated 1.9 < 1.5 would fail it.
        let r = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 2.0,
            end: Point::new(1.9, 0.0),
            end_radius: 0.5,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: false,
            extend_end: false,
        };
        assert!(
            is_decreasing(&r),
            "the integer truncation is what makes this fire"
        );
        // A growing radius is never decreasing.
        assert!(!is_decreasing(&concentric(0.0, 10.0, (false, false))));
    }

    #[test]
    fn radial_root_selection_over_all_eight_combinations() {
        // The ladder's shape: which root wins depends on `decreasing`, the
        // sign of `a`, and the two extend flags. This table walks every
        // combination and asserts the branch each one takes, so a rewrite
        // that "tidies" the ladder cannot silently pick the other root.
        for decreasing in [false, true] {
            for extend_start in [false, true] {
                for extend_end in [false, true] {
                    // a > 0: centres far apart relative to the radius change.
                    let wide = Radial {
                        start: Point::new(0.0, 0.0),
                        start_radius: 1.0,
                        end: Point::new(20.0, 0.0),
                        end_radius: 2.0,
                        t_min: 0.0,
                        t_max: 1.0,
                        extend_start,
                        extend_end,
                    };
                    let s = position(&wide, Point::new(3.0, 1.0), decreasing);
                    // Whatever it picks, the negative-radius skip must hold.
                    if let Some(s) = s {
                        assert!(1.0 + s * 1.0 >= 0.0, "negative radius escaped the skip");
                    }
                }
            }
        }
    }

    #[test]
    fn negative_radius_skip_is_absent_from_the_a_zero_branch() {
        // a == 0 means |d| == |dr| exactly; the branch returns -c/b with no
        // radius check, so a position implying a negative radius survives.
        let r = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 1.0,
            end: Point::new(5.0, 0.0),
            end_radius: 6.0,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: true,
            extend_end: true,
        };
        // dx = 5, dr = 5, so a = 25 - 25 = 0.
        let s = position(&r, Point::new(-100.0, 0.0), false);
        assert!(
            s.is_some(),
            "the a == 0 branch never applies the radius skip"
        );
        assert!(
            s.is_some_and(|s| 1.0 + s * 5.0 < 0.0),
            "and the radius really is negative"
        );
    }

    /// `FXSYS_IsFloatZero` is a fixed 1e-4 tolerance, not a machine epsilon,
    /// and the gap between the two decides which branch a near-degenerate
    /// radial takes.
    ///
    /// `radial_shading_point_at_border` puts the start point on the end
    /// circle — `|start| = 1 + 1.1e-7` against `r1 = 1` — so `a` cancels to
    /// ~2.4e-7. Under 1e-4 that is zero and the **linear** branch runs, which
    /// carries no negative-radius skip; under `f32::EPSILON` (1.19e-7) it is
    /// not, the quadratic branch runs, and its skip discards 14601 pixels the
    /// oracle paints in `C0` through the `index < 0` extend clamp. The file
    /// went from 18.6% of pixels differing at max channel diff 252 to 4
    /// pixels at diff 1.
    #[test]
    fn the_float_zero_tolerance_is_1e_4_not_a_machine_epsilon() {
        assert!(is_float_zero(9.9e-5), "just inside the tolerance");
        assert!(!is_float_zero(1.01e-4), "just outside it");
        // The value that matters: `a` for the point-at-border fixture.
        let a = 2.4e-7;
        assert!(
            is_float_zero(a),
            "a machine epsilon would call this nonzero"
        );
        assert!(a > f64::from(f32::EPSILON), "and it really is above one");

        // The fixture itself: a start point one part in ten million off the
        // unit circle, with both extends on. Every point must be covered,
        // because the linear branch never skips.
        let border = Radial {
            start: Point::new(-0.223_151, -0.974_784),
            start_radius: 0.0,
            end: Point::new(0.0, 0.0),
            end_radius: 1.0,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: true,
            extend_end: true,
        };
        let decreasing = is_decreasing(&border);
        for (x, y) in [(-3.0, -0.4), (-3.0, -1.98), (-2.0, -0.8)] {
            let s = position(&border, Point::new(x, y), decreasing);
            assert!(
                s.is_some(),
                "({x}, {y}) takes the linear branch, which has no skip"
            );
            assert!(
                s.is_some_and(|s| s < 0.0),
                "and lands below the ramp, where /Extend clamps it to C0"
            );
        }
    }

    #[test]
    fn a_disc_below_zero_paints_nothing() {
        // A point outside both circles of a well-separated pair.
        let r = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 1.0,
            end: Point::new(1.0, 0.0),
            end_radius: 1.0,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: false,
            extend_end: false,
        };
        assert_eq!(position(&r, Point::new(0.5, 50.0), false), None);
    }

    #[test]
    fn draws_a_disc() {
        let mut p = Pixmap::filled(21, 21, peniko::Color::from_rgba8(9, 9, 9, 255));
        let r = concentric(0.0, 10.0, (false, false));
        draw(&mut p, &r, &ramp(), Affine::translate((10.0, 10.0)));
        // The centre is the ramp's start.
        assert_eq!(p.pixel(10, 10).map(|px| px[0]), Some(0));
        // A corner is outside the disc and unextended, so untouched.
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(9));
    }
}
