//! Stroke parameters and the dash normalization ladder
//! (ISO 32000-1 §8.4.3).
//!
//! # Dashes are normalized here, not in the rasterizer
//!
//! PDFium normalizes dash arrays inside its AGG driver, which means its two
//! backends disagree about odd-length arrays: AGG lets the dasher cycle while
//! Skia doubles the array. Since the normalization is a pure function of the
//! array and the device scale, it belongs one layer up, where both backends
//! consume the same answer. We take the AGG behaviour, because that is what
//! the oracle renders.
//!
//! The ladder, in order:
//!
//! 1. An empty array is a solid line.
//! 2. **Any non-finite element abandons the pattern entirely** — a single NaN
//!    makes the whole line solid.
//! 3. A total cycle below `0.1` device pixels is a solid line.
//! 4. Each element is `fabs(len * scale)` **after** substituting `0.1` for
//!    anything at or below one part in a million. So a zero becomes `0.1`,
//!    and a **negative becomes `0.1` too** — the `<=` catches it before the
//!    absolute value, so negatives never survive as their magnitude.

use crate::ops::{LineCap, LineJoin};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use smallvec::SmallVec;

/// The shortest total dash cycle that still dashes, in device pixels.
pub const MIN_DASH_CYCLE: f32 = 0.1;

/// The threshold below which a dash element is replaced by `0.1`.
const DASH_EPSILON: f32 = 0.000_001;

/// How a path is stroked.
///
/// The dash array here is **already normalized** for a given device scale;
/// [`StrokeParams::normalized_dash`] does the work.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeParams {
    /// Line width. Negative and zero widths are stored as given — PDFium
    /// validates nothing here.
    pub width: f32,
    /// End-cap style.
    pub cap: LineCap,
    /// Corner style.
    pub join: LineJoin,
    /// Miter limit.
    pub miter_limit: f32,
    /// The dash pattern as the file stated it, unnormalized.
    pub dash: SmallVec<[f32; 4]>,
    /// The distance into the pattern at which to start.
    pub dash_phase: f32,
}

impl Default for StrokeParams {
    /// PDFium's defaults: width 1.0, miter limit 10.0, butt caps, miter
    /// joins, no dashes.
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: SmallVec::new(),
            dash_phase: 0.0,
        }
    }
}

/// A dash pattern ready for a rasterizer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NormalizedDash {
    /// The on/off lengths in device units. Empty means a solid line.
    pub lengths: SmallVec<[f32; 4]>,
    /// The starting offset in device units, unclamped.
    pub phase: f32,
}

impl StrokeParams {
    /// The dash pattern to hand a rasterizer at `scale` device units per user
    /// unit.
    ///
    /// An empty result means "draw solid", which is the answer for an empty
    /// array, a non-finite element, and a sub-threshold cycle alike.
    #[must_use]
    pub fn normalized_dash(&self, scale: f32, diags: &mut Diagnostics) -> NormalizedDash {
        if self.dash.is_empty() {
            return NormalizedDash::default();
        }
        // A single non-finite element abandons the whole pattern.
        if self.dash.iter().any(|v| !v.is_finite()) {
            diags.record(Severity::Recovered, DiagKind::DashPatternDropped, None);
            return NormalizedDash::default();
        }
        let cycle: f32 = self.dash.iter().map(|v| v.max(0.0)).sum();
        if cycle * scale < MIN_DASH_CYCLE {
            diags.record(Severity::Recovered, DiagKind::DashPatternDropped, None);
            return NormalizedDash::default();
        }
        let mut clamped = false;
        let lengths = self
            .dash
            .iter()
            .map(|v| {
                // The substitution comes *before* the absolute value, so a
                // negative becomes 0.1 rather than its magnitude.
                let len = if *v <= DASH_EPSILON {
                    clamped = true;
                    MIN_DASH_CYCLE
                } else {
                    *v
                };
                (len * scale).abs()
            })
            .collect();
        if clamped {
            diags.record(Severity::Recovered, DiagKind::DashElementClamped, None);
        }
        NormalizedDash {
            lengths,
            phase: self.dash_phase * scale,
        }
    }
}

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

    use super::{MIN_DASH_CYCLE, StrokeParams};
    use crate::ops::{LineCap, LineJoin};
    use pdfrum_common::{DiagKind, Diagnostics};
    use smallvec::SmallVec;

    fn params(dash: &[f32]) -> StrokeParams {
        StrokeParams {
            dash: SmallVec::from_slice(dash),
            ..StrokeParams::default()
        }
    }

    #[test]
    fn the_defaults_are_pdfiums() {
        let p = StrokeParams::default();
        assert!((p.width - 1.0).abs() < 1e-6);
        assert!((p.miter_limit - 10.0).abs() < 1e-6);
        assert_eq!(p.cap, LineCap::Butt);
        assert_eq!(p.join, LineJoin::Miter);
        assert!(p.dash.is_empty());
        assert!(p.dash_phase.abs() < 1e-6);
    }

    #[test]
    fn an_empty_array_is_a_solid_line() {
        let mut diags = Diagnostics::default();
        assert!(
            params(&[])
                .normalized_dash(1.0, &mut diags)
                .lengths
                .is_empty()
        );
        assert!(diags.is_empty(), "a solid line is not a recovery");
    }

    #[test]
    fn a_single_non_finite_element_abandons_the_whole_pattern() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut diags = Diagnostics::default();
            let got = params(&[3.0, bad, 3.0]).normalized_dash(1.0, &mut diags);
            assert!(got.lengths.is_empty(), "for {bad}");
            assert!(diags.contains(&DiagKind::DashPatternDropped));
        }
    }

    #[test]
    fn a_sub_threshold_cycle_is_a_solid_line() {
        let mut diags = Diagnostics::default();
        // 0.01 + 0.01 = 0.02, below the 0.1 threshold.
        let got = params(&[0.01, 0.01]).normalized_dash(1.0, &mut diags);
        assert!(got.lengths.is_empty());
        assert!(diags.contains(&DiagKind::DashPatternDropped));

        // The same array at ten times the scale is 0.2 and does dash.
        let mut diags = Diagnostics::default();
        let got = params(&[0.01, 0.01]).normalized_dash(10.0, &mut diags);
        assert!(!got.lengths.is_empty());
    }

    #[test]
    fn zeros_and_negatives_both_become_one_tenth() {
        let mut diags = Diagnostics::default();
        let got = params(&[0.0, 3.0]).normalized_dash(1.0, &mut diags);
        assert!((got.lengths[0] - MIN_DASH_CYCLE).abs() < 1e-6);
        assert!((got.lengths[1] - 3.0).abs() < 1e-6);
        assert!(diags.contains(&DiagKind::DashElementClamped));

        // A negative does **not** become its magnitude: the substitution
        // catches it first.
        let mut diags = Diagnostics::default();
        let got = params(&[-2.0, 3.0]).normalized_dash(1.0, &mut diags);
        assert!(
            (got.lengths[0] - MIN_DASH_CYCLE).abs() < 1e-6,
            "got {}, expected 0.1 rather than 2.0",
            got.lengths[0]
        );
    }

    #[test]
    fn an_odd_length_array_is_used_as_given() {
        let mut diags = Diagnostics::default();
        // `[3]` means three on, three off — the dasher cycles.
        let got = params(&[3.0]).normalized_dash(1.0, &mut diags);
        assert_eq!(got.lengths.len(), 1);
        assert!((got.lengths[0] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn the_phase_is_scaled_and_unclamped() {
        let mut diags = Diagnostics::default();
        let p = StrokeParams {
            dash: SmallVec::from_slice(&[3.0, 3.0]),
            dash_phase: -5.0,
            ..StrokeParams::default()
        };
        let got = p.normalized_dash(2.0, &mut diags);
        assert!((got.phase + 10.0).abs() < 1e-6, "negatives survive here");
    }
}
