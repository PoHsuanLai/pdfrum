//! The 256-entry colour lookup axial and radial shadings sample
//! (ISO 32000-1 §8.7.4.5.3).
//!
//! # The off-by-one is deliberate
//!
//! The table is **sampled** at `t_min + (t_max - t_min) * i / 256` but
//! **indexed** at `s * 255`. So `t_max` itself is never evaluated — the last
//! entry sits at `255/256` of the way along — while a parametric position of
//! exactly 1.0 selects entry 255. Both halves are reproduced verbatim
//! (design brief D16): on a smooth gradient the difference is visible, and
//! the corpus was rendered with it.

use crate::color::{ColorSpace, Rgb};
use crate::function::Function;
use std::sync::Arc;

/// Entries in the table.
pub const STEPS: usize = 256;

/// A sampled colour ramp, ready for a rasterizer to index.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorSteps {
    /// The colours, one per step.
    pub colors: Box<[Rgb; STEPS]>,
}

impl ColorSteps {
    /// Sample the ramp `t_min..t_max` through `functions` and `space`.
    ///
    /// Multiple functions **concatenate** their outputs into one component
    /// vector rather than each producing a colour; the buffer is sized to
    /// `max(total function outputs, space components)` so trailing components
    /// the functions did not fill stay zero.
    ///
    /// Returns `None` when no function produces any output, which is one of
    /// the conditions that makes a shading paint nothing.
    #[must_use]
    pub fn sample(
        functions: &[Arc<Function>],
        space: &ColorSpace,
        t_min: f32,
        t_max: f32,
    ) -> Option<Self> {
        let total_outputs: usize = functions.iter().map(|f| f.output_count()).sum();
        if total_outputs == 0 {
            return None;
        }
        let width = total_outputs.max(space.n_components());
        let mut buffer = vec![0.0f32; width];
        let diff = t_max - t_min;
        let mut colors = Box::new([Rgb::BLACK; STEPS]);
        for (i, slot) in colors.iter_mut().enumerate() {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a step index below 256 is exact in f32"
            )]
            // Note the divisor: 256, not 255. `t_max` is never sampled.
            let input = t_min + diff * (i as f32) / (STEPS as f32);
            buffer.fill(0.0);
            let mut written = 0usize;
            for f in functions {
                let Some(span) = buffer.get_mut(written..) else {
                    break;
                };
                written += f.eval_into(&[input], span);
            }
            *slot = space.to_rgb(&buffer);
        }
        Some(Self { colors })
    }

    /// The colour at parametric position `s`, applying the *indexing* half of
    /// the off-by-one and the extend rules.
    ///
    /// `None` means the pixel is left untouched: an out-of-range position
    /// whose end is not extended shows the backdrop rather than clamping.
    #[must_use]
    pub fn lookup(&self, s: f32, extend_start: bool, extend_end: bool) -> Option<Rgb> {
        if s.is_nan() {
            // A NaN position falls into the low branch, which is where the
            // C++'s undefined float-to-int conversion lands on x86.
            return extend_start.then(|| self.colors.first().copied().unwrap_or(Rgb::BLACK));
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the branches below handle every out-of-range case"
        )]
        let index = (s * 255.0) as i32;
        if index < 0 {
            return extend_start.then(|| self.colors.first().copied().unwrap_or(Rgb::BLACK));
        }
        // Note the asymmetry: `s == 1.0` gives index 255, which is in range,
        // so `extend_end` is not consulted until `s` exceeds 255/256.
        if index >= 256 {
            return extend_end.then(|| self.colors.last().copied().unwrap_or(Rgb::BLACK));
        }
        self.colors.get(usize::try_from(index).ok()?).copied()
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

    use super::{ColorSteps, STEPS};
    use crate::color::{ColorSpace, Rgb};
    use crate::function::{Exponential, Function};
    use std::sync::Arc;

    fn gray_ramp() -> Arc<Function> {
        Arc::new(Function::Exponential(Exponential {
            domain: Box::from(&[0.0f32, 1.0][..]),
            range: Box::from(&[0.0f32, 1.0][..]),
            c0: Box::from(&[0.0f32][..]),
            c1: Box::from(&[1.0f32][..]),
            exponent: 1.0,
            orig_outputs: 1,
            outputs: 1,
        }))
    }

    #[test]
    fn the_last_step_is_sampled_at_255_over_256_not_at_t_max() {
        let steps = ColorSteps::sample(
            std::slice::from_ref(&gray_ramp()),
            &ColorSpace::DeviceGray,
            0.0,
            1.0,
        )
        .expect("samples");
        let last = steps.colors.last().copied().expect("a last step");
        // 255/256 = 0.99609375, not 1.0 — this is the sampling half of the
        // documented off-by-one.
        assert!((last.r - 255.0 / 256.0).abs() < 1e-5, "got {}", last.r);
        let first = steps.colors.first().copied().expect("a first step");
        assert!(first.r.abs() < 1e-6);
    }

    #[test]
    fn a_position_of_exactly_one_lands_in_range_without_extending() {
        let steps = ColorSteps::sample(
            std::slice::from_ref(&gray_ramp()),
            &ColorSpace::DeviceGray,
            0.0,
            1.0,
        )
        .expect("samples");
        // Exactly 1.0 indexes 255, so `extend_end` is not consulted.
        assert!(steps.lookup(1.0, false, false).is_some());
        // Only above 255/256 does the end extend matter.
        assert!(steps.lookup(1.01, false, false).is_none());
        assert!(steps.lookup(1.01, false, true).is_some());
        assert!(steps.lookup(-0.5, false, false).is_none());
        assert!(steps.lookup(-0.5, true, false).is_some());
    }

    #[test]
    fn a_nan_position_falls_into_the_start_branch() {
        let steps = ColorSteps::sample(
            std::slice::from_ref(&gray_ramp()),
            &ColorSpace::DeviceGray,
            0.0,
            1.0,
        )
        .expect("samples");
        assert!(steps.lookup(f32::NAN, false, false).is_none());
        assert_eq!(
            steps.lookup(f32::NAN, true, false),
            Some(Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0
            })
        );
    }

    #[test]
    fn sampling_needs_at_least_one_output() {
        assert!(ColorSteps::sample(&[], &ColorSpace::DeviceGray, 0.0, 1.0).is_none());
        assert_eq!(STEPS, 256);
    }
}
