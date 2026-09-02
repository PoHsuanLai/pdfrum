//! `Separation`, `DeviceN` and `Pattern` — the three families whose
//! components are not colour (ISO 32000-1 §8.6.6.4, §8.6.6.5, §8.7.3).
//!
//! `Separation` and `DeviceN` differ in strictness in a way worth stating
//! once, because it looks like an inconsistency and is not:
//!
//! | | `Separation` | `DeviceN` |
//! |---|---|---|
//! | tint transform | optional; a failed load leaves the space usable | **mandatory**; a failed load fails the space |
//! | too few outputs | drops the function, keeps the space | fails the space |
//! | components | always **1** | `len(/Names)`, uncapped |
//! | no function | broadcasts the tint into every alternate component | unreachable |
//!
//! `/None` short-circuits `Separation` before any of that and paints nothing.
//! `/All` gets **no** special handling at all — the string never appears in
//! the C++ — so it loads as an ordinary named colorant, diverging from
//! ISO 32000-1 on purpose.

use super::{ColorSpace, Conversion, Rgb};
use crate::function::Function;
use std::sync::Arc;

/// The scratch floor both families size their output buffer to, because an
/// alternate space may read past the tint transform's declared output count.
const SCRATCH_FLOOR: usize = 16;

/// The largest number of components a pattern colour may carry.
pub const MAX_PATTERN_COMPONENTS: usize = 16;

/// A `Separation` colorspace: one tint driving an alternate space.
#[derive(Debug, Clone, PartialEq)]
pub struct Separation {
    /// Set when the colorant is `/None`, which paints nothing at all.
    pub none: bool,
    /// The alternate space. Absent only for `/None`.
    pub alternate: Option<Box<ColorSpace>>,
    /// The tint transform, shared because one function may back several
    /// spaces. Absent when it failed to load or produced too few outputs —
    /// which is not fatal here.
    pub tint: Option<Arc<Function>>,
}

impl Separation {
    /// Convert one tint value.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32], conversion: Conversion) -> Option<Rgb> {
        if self.none {
            return None;
        }
        let alternate = self.alternate.as_ref()?;
        let tint = comps.first().copied().unwrap_or(0.0);
        let Some(func) = &self.tint else {
            // With no transform the single tint is broadcast into every
            // alternate component — a crude but deliberate fallback.
            let broadcast = vec![tint; alternate.n_components()];
            return Some(alternate.to_rgb_with(&broadcast, conversion));
        };
        let mut results = vec![0.0f32; func.output_count().max(SCRATCH_FLOOR)];
        let produced = func.eval_into(&[tint], &mut results);
        if produced == 0 {
            return None;
        }
        Some(alternate.to_rgb_with(&results, conversion))
    }
}

/// A `DeviceN` colorspace: several colorants driving an alternate space.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceN {
    /// The colorant names. Its length is the component count.
    pub names: Box<[pdfrum_object::Name]>,
    /// The alternate space, which is mandatory here.
    pub alternate: Box<ColorSpace>,
    /// The tint transform, mandatory here.
    pub tint: Arc<Function>,
}

impl DeviceN {
    /// Convert a colorant vector.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32], conversion: Conversion) -> Option<Rgb> {
        let n = self.names.len();
        let inputs: Vec<f32> = (0..n)
            .map(|i| comps.get(i).copied().unwrap_or(0.0))
            .collect();
        let mut results = vec![0.0f32; self.tint.output_count().max(SCRATCH_FLOOR)];
        let produced = self.tint.eval_into(&inputs, &mut results);
        if produced == 0 {
            return None;
        }
        Some(self.alternate.to_rgb_with(&results, conversion))
    }
}

/// A `Pattern` colorspace: a marker with, for uncoloured patterns, the space
/// its `scn` operands live in.
///
/// The base is genuinely optional: a `[/Pattern <unloadable>]` array loads
/// **successfully** as a one-component pattern space with no base, which is
/// the uncoloured-pattern case.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PatternSpace {
    /// The underlying space for an uncoloured pattern's colour operands.
    pub base: Option<Box<ColorSpace>>,
}

impl PatternSpace {
    /// The component count: one for a bare `/Pattern`, one more than the
    /// base's otherwise.
    #[must_use]
    pub fn n_components(&self) -> usize {
        self.base.as_ref().map_or(1, |b| b.n_components() + 1)
    }

    /// Resolve an uncoloured pattern's colour from its operands.
    ///
    /// The base sees the **whole** operand array regardless of its own
    /// component count, matching `GetPatternRGB`.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32], conversion: Conversion) -> Option<Rgb> {
        let base = self.base.as_ref()?;
        Some(base.to_rgb_with(comps, conversion))
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

    use super::Conversion;
    use super::{DeviceN, PatternSpace, Separation};
    use crate::color::ColorSpace;
    use crate::function::{Exponential, Function};
    use std::sync::Arc;

    /// A tint transform mapping one input to three outputs, `t -> (t, t, t)`.
    fn ramp3() -> Arc<Function> {
        Arc::new(Function::Exponential(Exponential {
            domain: Box::from(&[0.0f32, 1.0][..]),
            range: Box::from(&[0.0f32, 1.0, 0.0, 1.0, 0.0, 1.0][..]),
            c0: Box::from(&[0.0f32, 0.0, 0.0][..]),
            c1: Box::from(&[1.0f32, 1.0, 1.0][..]),
            exponent: 1.0,
            orig_outputs: 3,
            outputs: 3,
        }))
    }

    #[test]
    fn none_separations_paint_nothing() {
        let sep = Separation {
            none: true,
            alternate: None,
            tint: None,
        };
        assert!(sep.to_rgb(&[1.0], Conversion::Managed).is_none());
    }

    #[test]
    fn a_separation_without_a_transform_broadcasts_its_tint() {
        let sep = Separation {
            none: false,
            alternate: Some(Box::new(ColorSpace::DeviceRgb)),
            tint: None,
        };
        let rgb = sep.to_rgb(&[0.25], Conversion::Managed).expect("colour");
        assert!((rgb.r - 0.25).abs() < 1e-6);
        assert!((rgb.g - 0.25).abs() < 1e-6);
        assert!((rgb.b - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_separation_with_a_transform_uses_it() {
        let sep = Separation {
            none: false,
            alternate: Some(Box::new(ColorSpace::DeviceRgb)),
            tint: Some(ramp3()),
        };
        let rgb = sep.to_rgb(&[0.5], Conversion::Managed).expect("colour");
        assert!((rgb.r - 0.5).abs() < 1e-5);
    }

    #[test]
    fn device_n_reads_exactly_its_name_count() {
        let cs = DeviceN {
            names: Box::from(&[pdfrum_object::Name::from("A")][..]),
            alternate: Box::new(ColorSpace::DeviceRgb),
            tint: ramp3(),
        };
        assert_eq!(cs.names.len(), 1);
        // Surplus operands are ignored, missing ones read as zero.
        let a = cs
            .to_rgb(&[0.5, 9.0, 9.0], Conversion::Managed)
            .expect("colour");
        let b = cs.to_rgb(&[0.5], Conversion::Managed).expect("colour");
        assert!((a.r - b.r).abs() < 1e-6);
    }

    #[test]
    fn a_pattern_space_without_a_base_has_one_component_and_no_colour() {
        let cs = PatternSpace::default();
        assert_eq!(cs.n_components(), 1);
        assert!(cs.to_rgb(&[0.5], Conversion::Managed).is_none());
    }

    #[test]
    fn a_pattern_space_with_a_base_adds_one_component() {
        let cs = PatternSpace {
            base: Some(Box::new(ColorSpace::DeviceCmyk)),
        };
        assert_eq!(cs.n_components(), 5);
        assert!(
            cs.to_rgb(&[0.0, 0.0, 0.0, 0.0], Conversion::Managed)
                .is_some()
        );
    }
}
