//! `/Decode`: remapping raw samples onto component values
//! (ISO 32000-1 §8.9.5.2).
//!
//! Each component gets a base and a step, and the value of a raw sample is
//! `base + step * raw`. Two rules matter:
//!
//! - **A short or malformed array is not rejected.** Out-of-range reads yield
//!   `0.0`, so a missing pair makes that component a constant zero — and, by
//!   differing from the default, flips `default_decode` to false.
//! - **`Indexed` uses the raw sample maximum as its high bound**, not the
//!   base space's, because an indexed sample *is* an index.

use crate::color::ColorSpace;
use pdfrum_object::Array;

/// The per-component remapping.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeMap {
    /// The value a raw sample of zero maps to, per component.
    pub base: Box<[f32]>,
    /// The value one raw step adds, per component.
    pub step: Box<[f32]>,
    /// Whether the mapping is the space's own default, which is what a
    /// one-bit stencil mask's inversion keys on.
    pub default: bool,
}

impl DecodeMap {
    /// Build the mapping for `space` at `bpc` bits per component.
    #[must_use]
    pub fn new(
        space: Option<&ColorSpace>,
        components: usize,
        bpc: u32,
        decode: Option<&Array>,
    ) -> Self {
        let max_data = if bpc >= 32 {
            f32::from(u16::MAX)
        } else {
            #[expect(
                clippy::cast_precision_loss,
                reason = "bit depths cap at 16, so the value is exact in f32"
            )]
            let v = ((1u64 << bpc.min(31)) - 1) as f32;
            v
        };
        let indexed = matches!(space, Some(ColorSpace::Indexed(_)));
        let mut base = Vec::with_capacity(components);
        let mut step = Vec::with_capacity(components);
        let mut default = true;
        for i in 0..components {
            let (def_min, mut def_max) = space.map_or((0.0, 1.0), |s| {
                let (_, lo, hi) = s.default_value(i);
                (lo, hi)
            });
            // An indexed sample is an index, so its high bound is the largest
            // raw value rather than the base space's maximum.
            if indexed {
                def_max = max_data;
            }
            let (lo, hi) = if let Some(a) = decode {
                // Out-of-range reads yield zero rather than failing.
                let stated = (a.number_at_or_zero(i * 2), a.number_at_or_zero(i * 2 + 1));
                #[expect(
                    clippy::float_cmp,
                    reason = "PDFium compares these exactly, and a hair's \
                              difference really does flip `default_decode`"
                )]
                if def_min != stated.0 || def_max != stated.1 {
                    default = false;
                }
                stated
            } else {
                (def_min, def_max)
            };
            base.push(lo);
            step.push(if max_data == 0.0 {
                0.0
            } else {
                (hi - lo) / max_data
            });
        }
        Self {
            base: base.into(),
            step: step.into(),
            default,
        }
    }

    /// Apply the mapping to one raw sample of component `index`.
    #[must_use]
    pub fn apply(&self, index: usize, raw: f32) -> f32 {
        self.base.get(index).copied().unwrap_or(0.0)
            + self.step.get(index).copied().unwrap_or(0.0) * raw
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
        clippy::cast_sign_loss,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::DecodeMap;
    use crate::color::{ColorSpace, Indexed};
    use pdfrum_object::{Array, Object};

    fn array(values: &[f32]) -> Array {
        Array::of(values.iter().copied().map(Object::Real))
    }

    #[test]
    fn the_default_mapping_spans_zero_to_one() {
        let m = DecodeMap::new(Some(&ColorSpace::DeviceRgb), 3, 8, None);
        assert!(m.default);
        assert!(m.apply(0, 0.0).abs() < 1e-6);
        assert!((m.apply(0, 255.0) - 1.0).abs() < 1e-6);
        assert!((m.apply(1, 128.0) - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn an_inverted_decode_reverses_the_ramp_and_is_not_default() {
        let m = DecodeMap::new(
            Some(&ColorSpace::DeviceGray),
            1,
            8,
            Some(&array(&[1.0, 0.0])),
        );
        assert!(!m.default);
        assert!((m.apply(0, 0.0) - 1.0).abs() < 1e-6);
        assert!(m.apply(0, 255.0).abs() < 1e-6);
    }

    #[test]
    fn an_explicit_identity_decode_still_counts_as_default() {
        let m = DecodeMap::new(
            Some(&ColorSpace::DeviceGray),
            1,
            8,
            Some(&array(&[0.0, 1.0])),
        );
        assert!(m.default);
    }

    #[test]
    fn a_short_array_makes_the_missing_component_a_constant_zero() {
        // Three components but only one pair stated.
        let m = DecodeMap::new(
            Some(&ColorSpace::DeviceRgb),
            3,
            8,
            Some(&array(&[0.0, 1.0])),
        );
        assert!(!m.default, "the missing pairs differ from the default");
        assert!((m.apply(0, 255.0) - 1.0).abs() < 1e-6);
        // Components one and two read `[0, 0]`, a constant zero.
        assert!(m.apply(1, 255.0).abs() < 1e-6);
        assert!(m.apply(2, 255.0).abs() < 1e-6);
    }

    #[test]
    fn an_indexed_space_maps_onto_the_raw_sample_range() {
        let indexed = ColorSpace::Indexed(Box::new(Indexed {
            base: Box::new(ColorSpace::DeviceRgb),
            max_index: 3,
            lookup: Box::from(&[0u8; 12][..]),
            component_ranges: Box::from(&[(0.0f32, 1.0f32); 3][..]),
        }));
        // At four bits the top raw sample is 15, and it must map to 15 rather
        // than to the base space's 1.0.
        let m = DecodeMap::new(Some(&indexed), 1, 4, None);
        assert!(m.default);
        assert!((m.apply(0, 15.0) - 15.0).abs() < 1e-5);
        assert!((m.apply(0, 3.0) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn a_zero_bit_depth_yields_a_flat_mapping_rather_than_a_division_by_zero() {
        let m = DecodeMap::new(Some(&ColorSpace::DeviceGray), 1, 0, None);
        assert!(m.apply(0, 99.0).is_finite());
    }

    /// The 16-bit ruling, pinned over the whole sample range.
    ///
    /// A 16-bit sample is a value in `[0, 65535]` mapped linearly onto the
    /// `/Decode` range (ISO 32000-1 §8.9.5) and then **rounded** onto a byte.
    /// pdf.js computes exactly that (`DeviceRgbCS.getRgbBuffer`,
    /// `src/core/colorspace.js`: `scale = 255 / ((1 << bits) - 1)` stored into
    /// a rounding `Uint8ClampedArray`); the oracle approximates it by dropping
    /// the low byte, which is within one count everywhere. We take the exact
    /// map.
    ///
    /// Two properties are asserted for all 65 536 samples, which is what makes
    /// this a pin and not a spot check:
    ///
    /// 1. the byte equals `round(sample * 255 / 65535)` computed in `f64`, so
    ///    the `f32` decode arithmetic is proven not to drift off it; and
    /// 2. it is within one count of PDFium's `sample >> 8`, so a divergence
    ///    from the oracle can never grow past the rounding difference it is.
    #[test]
    fn sixteen_bit_samples_round_onto_the_byte_and_stay_within_a_count_of_the_oracle() {
        let m = DecodeMap::new(Some(&ColorSpace::DeviceGray), 1, 16, None);
        let mut differ_from_oracle = 0u32;
        for raw in 0u32..=65535 {
            let byte = (m.apply(0, raw as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
            let exact = (f64::from(raw) * 255.0 / 65535.0).round() as u8;
            assert_eq!(byte, exact, "sample {raw} must be the exact rounded map");
            // PDFium's fast path drops the low byte instead of rounding.
            let oracle = (raw >> 8) as u8;
            let gap = i32::from(byte) - i32::from(oracle);
            assert!(gap.abs() <= 1, "sample {raw}: {byte} vs oracle {oracle}");
            if gap != 0 {
                differ_from_oracle += 1;
            }
        }
        // 16 256 of 65 536 — the rest of the range is where dropping the low
        // byte happens to land on the rounded answer anyway. Pinned so the
        // ruling's blast radius against the oracle is a number in the tree
        // rather than a claim in a doc.
        assert_eq!(differ_from_oracle, 16_256);
    }

    /// Below 16 bits the ruling changes nothing at all.
    ///
    /// `step` is exactly `1/max` and the products are small enough to be exact
    /// in `f32`, so rounding and truncating agree on every raw value at 1, 2, 4
    /// and 8 bits — and both agree with the oracle's integer `v * 255 / max`.
    #[test]
    fn below_sixteen_bits_rounding_truncating_and_the_oracles_integer_scale_agree() {
        for bpc in [1u32, 2, 4, 8] {
            let max = (1u32 << bpc) - 1;
            let m = DecodeMap::new(Some(&ColorSpace::DeviceGray), 1, bpc, None);
            for raw in 0..=max {
                let value = m.apply(0, raw as f32).clamp(0.0, 1.0) * 255.0;
                let integer = u8::try_from(raw * 255 / max).unwrap();
                assert_eq!(value.round() as u8, integer, "bpc {bpc} raw {raw}");
                assert_eq!(value as u8, integer, "bpc {bpc} raw {raw} truncated");
            }
        }
    }
}
