//! Type 3: radial shadings (ISO 32000-1 §8.7.4.5.4).
//!
//! A gradient between two circles. Finding a pixel's parametric position
//! means solving a quadratic, and the C++'s solver has **three branches with
//! different amounts of guarding** — a fact worth stating plainly, because
//! two of them are visibly under-guarded and both are ported as they are:
//!
//! | Branch | Condition | Guards |
//! |---|---|---|
//! | 1 | `b` ≈ 0 | **none at all**: no discriminant check, no root selection, no radius check, and no `a == 0` guard, so `sqrt(-c/a)` may be NaN |
//! | 2 | `a` ≈ 0 | linear, `-c/b`; **no radius check** |
//! | 3 | otherwise | discriminant check, root selection by direction, and a negative-radius check |
//!
//! The "decreasing" test that picks between the roots truncates a hypotenuse
//! to an integer before comparing it, which is its own small quirk.

// The quadratic's coefficients are `a`, `b` and `c`, and the deltas `dx`,
// `dy`, `dr`; these are the names ISO 32000-1 §8.7.4.5.4 and every derivation
// of it use.
#![expect(
    clippy::many_single_char_names,
    reason = "quadratic coefficients are single-letter by convention"
)]

use super::{read_domain, read_extend};
use crate::names;
use kurbo::Point;
use pdfrum_object::{Dict, Resolve};

/// A type 3 shading's geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radial {
    /// The first circle's centre.
    pub start: Point,
    /// The first circle's radius.
    pub start_radius: f32,
    /// The second circle's centre.
    pub end: Point,
    /// The second circle's radius.
    pub end_radius: f32,
    /// `/Domain`'s low bound.
    pub t_min: f32,
    /// `/Domain`'s high bound.
    pub t_max: f32,
    /// Whether the gradient continues past the first circle.
    pub extend_start: bool,
    /// Whether it continues past the second.
    pub extend_end: bool,
}

/// What the quadratic solver produced for one point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RadialPosition {
    /// A parametric position along the gradient.
    At(f32),
    /// The pixel is not covered: the discriminant was negative, or the
    /// interpolated radius would be.
    Uncovered,
}

impl Radial {
    /// Load from a shading dictionary. `/Coords` is six numbers, again with
    /// no length check.
    pub(super) fn load(dict: &Dict, r: &impl Resolve) -> Option<Self> {
        let coords = dict.array(names::COORDS, r)?;
        let at = |i: usize| coords.number_at_or_zero(i);
        let (t_min, t_max) = read_domain(dict, r);
        let (extend_start, extend_end) = read_extend(dict, r);
        Some(Self {
            start: Point::new(f64::from(at(0)), f64::from(at(1))),
            start_radius: at(2),
            end: Point::new(f64::from(at(3)), f64::from(at(4))),
            end_radius: at(5),
            t_min,
            t_max,
            extend_start,
            extend_end,
        })
    }

    /// Whether the circles shrink as the gradient advances, which decides
    /// which root of the quadratic to prefer.
    ///
    /// The centre distance is **truncated to an integer** before the
    /// comparison, so a shading whose centres are 3.9 units apart with
    /// `dr == -3.5` counts as decreasing while one with `dr == -3.0` does
    /// not. Reproduced.
    #[must_use]
    pub fn is_decreasing(&self) -> bool {
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        let dr = self.end_radius - self.start_radius;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the integer truncation of the hypotenuse is the quirk being ported"
        )]
        let hypot = dx.hypot(dy) as i32;
        dr < 0.0 && f64::from(hypot) < f64::from(-dr)
    }

    /// The parametric position of a point, or that it is uncovered.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ solves this quadratic in f32 throughout, and the \
                  branch selection is sensitive to that precision"
    )]
    pub fn position(&self, p: Point) -> RadialPosition {
        let dx = (self.end.x - self.start.x) as f32;
        let dy = (self.end.y - self.start.y) as f32;
        let dr = self.end_radius - self.start_radius;
        let a = dx * dx + dy * dy - dr * dr;
        let a_is_zero = is_float_zero(a);

        let pdx = (p.x - self.start.x) as f32;
        let pdy = (p.y - self.start.y) as f32;
        let b = -2.0 * (pdx * dx + pdy * dy + self.start_radius * dr);
        let c = pdx * pdx + pdy * pdy - self.start_radius * self.start_radius;

        // Branch 1: no guards whatsoever, so this may well be NaN.
        if is_float_zero(b) {
            return RadialPosition::At((-c / a).sqrt());
        }
        // Branch 2: linear, and with no radius check.
        if a_is_zero {
            return RadialPosition::At(-c / b);
        }
        // Branch 3: the fully guarded one.
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            return RadialPosition::Uncovered;
        }
        let root = disc.sqrt();
        let (mut s1, mut s2) = ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
        if a <= 0.0 {
            // Ensure `s1 <= s2`.
            std::mem::swap(&mut s1, &mut s2);
        }
        let s = if self.is_decreasing() {
            if s1 >= 0.0 || self.extend_start {
                s1
            } else {
                s2
            }
        } else if s2 <= 1.0 || self.extend_end {
            s2
        } else {
            s1
        };
        // A negative interpolated radius means the pixel is outside both
        // circles' sweep.
        if self.start_radius + s * dr < 0.0 {
            return RadialPosition::Uncovered;
        }
        RadialPosition::At(s)
    }
}

/// Whether `v` is zero to the shading tolerance: `|v| < 1e-4`.
///
/// A **fixed 1e-4 tolerance**, not a machine epsilon — roughly 840 times
/// wider than `f32::EPSILON`. `a` is `dx² + dy² - dr²`, a catastrophic
/// cancellation whenever the start point sits on the end circle, and the two
/// tolerances then disagree about which branch runs: the linear `a == 0` one,
/// which has no negative-radius skip, or the quadratic one, which does.
/// `radial_shading_point_at_border` lands in exactly that gap at `a ≈ 2.4e-7`.
///
/// The comparison widens to `f64` before the test, so a `f32` operand is not
/// rounded against a `f32` bound.
// The oracle's `FXSYS_IsFloatZero` (`fx_system.h:36`),
// `(f) < 0.0001 && (f) > -0.0001`: its operand is a `float` but the literals
// are `double`, so the comparison happens in `double` there too.
fn is_float_zero(v: f32) -> bool {
    f64::from(v).abs() < FLOAT_ZERO
}

/// The `FXSYS_IsFloatZero` tolerance.
const FLOAT_ZERO: f64 = 1e-4;

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{Radial, RadialPosition};
    use kurbo::Point;

    fn concentric() -> Radial {
        Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 0.0,
            end: Point::new(0.0, 0.0),
            end_radius: 10.0,
            t_min: 0.0,
            t_max: 1.0,
            extend_start: false,
            extend_end: false,
        }
    }

    #[test]
    fn a_concentric_gradient_maps_radius_to_position() {
        let s = concentric();
        // At the centre `b` is zero, so branch 1 runs.
        let RadialPosition::At(v) = s.position(Point::new(0.0, 0.0)) else {
            panic!("the centre should have a position");
        };
        assert!(v.abs() < 1e-6, "got {v}");
        // Halfway out.
        let RadialPosition::At(v) = s.position(Point::new(5.0, 0.0)) else {
            panic!("expected a position");
        };
        assert!((v - 0.5).abs() < 1e-5, "got {v}");
    }

    #[test]
    fn the_linear_branch_runs_when_a_is_zero() {
        // Equal radii and coincident centres make `a` zero for a translated
        // gradient: dx² + dy² == dr².
        let s = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 0.0,
            end: Point::new(3.0, 4.0),
            end_radius: 5.0,
            ..concentric()
        };
        // `a = 9 + 16 - 25 = 0`.
        let RadialPosition::At(v) = s.position(Point::new(1.0, 1.0)) else {
            panic!("expected a position");
        };
        assert!(v.is_finite(), "got {v}");
    }

    #[test]
    fn an_uncovered_pixel_is_reported_rather_than_clamped() {
        // Two separated circles leave points outside their sweep uncovered.
        let s = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 1.0,
            end: Point::new(100.0, 0.0),
            end_radius: 1.0,
            ..concentric()
        };
        assert_eq!(
            s.position(Point::new(50.0, 500.0)),
            RadialPosition::Uncovered
        );
    }

    #[test]
    fn the_decreasing_test_truncates_the_centre_distance() {
        // Centres 3.9 apart with dr = −3.5: the truncated hypotenuse is 3,
        // which is below 3.5, so the shading counts as decreasing.
        let s = Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 4.0,
            end: Point::new(3.9, 0.0),
            end_radius: 0.5,
            ..concentric()
        };
        assert!(s.is_decreasing());
        // With dr = −3.0 the truncated 3 is not below 3, so it is not.
        let s = Radial {
            end_radius: 1.0,
            ..s
        };
        assert!(!s.is_decreasing());
    }
}
