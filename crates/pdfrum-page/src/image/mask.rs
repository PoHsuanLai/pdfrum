//! Image masking: stencils, colour keys, soft masks and `/Matte`
//! (ISO 32000-1 §8.9.6).
//!
//! **`/SMask` wins over `/Mask` at every level.** When both are present, the
//! colour-key array is not merely overridden — it is never read at all,
//! because the code returns before reaching it.
//!
//! Two smaller rules are easy to get wrong:
//!
//! - A colour-key array shorter than two entries per component still turns
//!   masking **on**; the C++ then reads uninitialized ranges. We default them
//!   to `0/0`, so only exactly-zero samples become transparent
//!   (design brief D19).
//! - `/Matte` needs **four** conditions at once — an array of exactly the
//!   image's component count, a colour space that is not a pattern, and that
//!   space needing no more components than the image has.
//! - A mask that fails to load **does not fail its base image**: the mask is
//!   dropped and the image is kept. A mask is also never resolution-reduced
//!   and never carries a mask of its own.

use crate::color::{ColorSpace, Rgb};
use pdfrum_object::Array;

/// A colour-key mask: per component, the closed range of raw sample values
/// that is transparent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColorKey {
    /// Per component, `(min, max)` inclusive.
    pub ranges: Box<[(u32, u32)]>,
}

impl ColorKey {
    /// Read a `/Mask` array.
    ///
    /// Returns a mask even when the array is too short, with the missing
    /// ranges defaulting to `0..=0` — which makes only exactly-zero samples
    /// transparent. The C++ leaves them uninitialized here; matching
    /// *undefined* behaviour is not possible, and this is the closest
    /// defensible reading.
    #[must_use]
    pub fn from_array(array: &Array, components: usize, max_data: u32) -> Self {
        let complete = array.len() >= components * 2;
        let ranges = (0..components)
            .map(|i| {
                if !complete {
                    return (0u32, 0u32);
                }
                let lo = array.int_at(i * 2).unwrap_or(0).max(0);
                let hi = array
                    .int_at(i * 2 + 1)
                    .unwrap_or(0)
                    .clamp(0, i64::from(max_data));
                (
                    u32::try_from(lo).unwrap_or(0),
                    u32::try_from(hi).unwrap_or(0),
                )
            })
            .collect();
        Self { ranges }
    }

    /// Whether the array stated a full set of ranges.
    #[must_use]
    pub fn is_complete(array: &Array, components: usize) -> bool {
        array.len() >= components * 2
    }

    /// Whether a pixel is transparent: **every** component must fall inside
    /// its range.
    #[must_use]
    pub fn is_transparent(&self, samples: &[u32]) -> bool {
        if self.ranges.is_empty() {
            return false;
        }
        self.ranges.iter().enumerate().all(|(i, (lo, hi))| {
            let v = samples.get(i).copied().unwrap_or(0);
            v >= *lo && v <= *hi
        })
    }
}

/// An image's alpha, however it was expressed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ImageMask {
    /// A `/Mask` array naming transparent sample values.
    ColorKey(ColorKey),
    /// A separate grayscale image, `/SMask` or a `/Mask` stream, at its own
    /// resolution — masks are **never** resolution-reduced, so a 400×400 mask
    /// stays 400×400 even beside a 50×50 base image.
    Alpha {
        /// Mask width in samples.
        width: u32,
        /// Mask height in samples.
        height: u32,
        /// One byte per sample; 255 is opaque.
        alpha: Box<[u8]>,
        /// Whether the mask is a `/Mask` stencil rather than an `/SMask`,
        /// which inverts its sense.
        stencil: bool,
    },
}

impl ImageMask {
    /// The alpha at `(x, y)`, or 255 when the mask does not cover it.
    #[must_use]
    pub fn alpha_at(&self, x: u32, y: u32) -> u8 {
        match self {
            Self::ColorKey(_) => 255,
            Self::Alpha {
                width,
                height,
                alpha,
                stencil,
            } => {
                if x >= *width || y >= *height {
                    return 255;
                }
                let index = usize::try_from(y)
                    .ok()
                    .and_then(|row| row.checked_mul(usize::try_from(*width).ok()?))
                    .and_then(|base| base.checked_add(usize::try_from(x).ok()?));
                let v = index.and_then(|i| alpha.get(i)).copied().unwrap_or(255);
                if *stencil { 255 - v } else { v }
            }
        }
    }
}

/// The `/Matte` colour a pre-blended soft-masked image was composed against.
///
/// All four preconditions must hold at once; any one failing means no matte,
/// which the compositor treats as "not pre-blended".
#[must_use]
pub fn matte_color(
    matte: Option<&Array>,
    space: Option<&ColorSpace>,
    components: usize,
) -> Option<Rgb> {
    let matte = matte?;
    let space = space?;
    if matches!(space, ColorSpace::Pattern(_)) {
        return None;
    }
    // Exactly the image's component count, not merely enough.
    if matte.len() != components {
        return None;
    }
    if space.n_components() > components {
        return None;
    }
    let comps: Vec<f32> = (0..components)
        .map(|i| matte.number_at_or_zero(i))
        .collect();
    Some(space.to_rgb(&comps))
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

    use super::{ColorKey, ImageMask, matte_color};
    use crate::color::ColorSpace;
    use pdfrum_object::{Array, Object};

    fn array(values: &[i64]) -> Array {
        Array::of(values.iter().copied().map(Object::Int))
    }

    #[test]
    fn a_colour_key_needs_every_component_inside_its_range() {
        let key = ColorKey::from_array(&array(&[10, 20, 30, 40, 50, 60]), 3, 255);
        assert!(key.is_transparent(&[15, 35, 55]));
        // One component outside is enough to make it opaque.
        assert!(!key.is_transparent(&[15, 35, 61]));
        assert!(!key.is_transparent(&[9, 35, 55]));
        // The bounds are inclusive.
        assert!(key.is_transparent(&[10, 30, 50]));
        assert!(key.is_transparent(&[20, 40, 60]));
    }

    #[test]
    fn a_short_array_still_masks_but_only_exactly_zero_samples() {
        let short = array(&[10, 20]);
        assert!(!ColorKey::is_complete(&short, 3));
        let key = ColorKey::from_array(&short, 3, 255);
        assert!(key.is_transparent(&[0, 0, 0]));
        assert!(!key.is_transparent(&[15, 0, 0]));
    }

    #[test]
    fn colour_key_bounds_are_clamped_to_the_sample_range() {
        let key = ColorKey::from_array(&array(&[-5, 999]), 1, 255);
        assert_eq!(key.ranges.first(), Some(&(0, 255)));
    }

    #[test]
    fn an_alpha_mask_reads_out_of_bounds_as_opaque() {
        let mask = ImageMask::Alpha {
            width: 2,
            height: 2,
            alpha: Box::from(&[0u8, 64, 128, 255][..]),
            stencil: false,
        };
        assert_eq!(mask.alpha_at(0, 0), 0);
        assert_eq!(mask.alpha_at(1, 1), 255);
        assert_eq!(mask.alpha_at(5, 5), 255);
    }

    #[test]
    fn a_stencil_mask_inverts_its_sense() {
        let mask = ImageMask::Alpha {
            width: 1,
            height: 1,
            alpha: Box::from(&[0u8][..]),
            stencil: true,
        };
        assert_eq!(mask.alpha_at(0, 0), 255);
    }

    #[test]
    fn matte_needs_all_four_preconditions() {
        let rgb = ColorSpace::DeviceRgb;
        let three = array(&[0, 0, 0]);
        assert!(matte_color(Some(&three), Some(&rgb), 3).is_some());
        // Wrong length, exactly rather than merely enough.
        assert!(matte_color(Some(&array(&[0, 0])), Some(&rgb), 3).is_none());
        assert!(matte_color(Some(&array(&[0, 0, 0, 0])), Some(&rgb), 3).is_none());
        // No space at all.
        assert!(matte_color(Some(&three), None, 3).is_none());
        // A pattern space.
        let pattern = ColorSpace::Pattern(Box::default());
        assert!(matte_color(Some(&three), Some(&pattern), 3).is_none());
        // A space wanting more components than the image has.
        assert!(matte_color(Some(&array(&[0])), Some(&rgb), 1).is_none());
        // No array.
        assert!(matte_color(None, Some(&rgb), 3).is_none());
    }
}
