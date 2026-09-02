//! `Indexed` spaces: a palette over some base space
//! (ISO 32000-1 §8.6.6.3).
//!
//! Two details drive everything here. First, `/hival` is **clamped** to
//! `0..=255` rather than rejected, so an out-of-range palette still loads —
//! PDFium's comment says so explicitly. Second, the per-component ranges are
//! snapshotted at load time and the field named `max` is overwritten with the
//! *range* (`max - min`), which is what the lookup formula multiplies by.
//! Getting that wrong silently shifts every indexed colour.

use super::{ColorSpace, Rgb};

/// An `Indexed` colorspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Indexed {
    /// The space the palette entries live in.
    pub base: Box<ColorSpace>,
    /// The largest legal index, clamped into `0..=255` at load.
    pub max_index: u8,
    /// The palette, one base component per byte. May be shorter than
    /// `(max_index + 1) * base.n_components()`, in which case high indices
    /// simply have no colour.
    pub lookup: Box<[u8]>,
    /// Per base component, `(min, max - min)` snapshotted from the base's
    /// default value range.
    pub component_ranges: Box<[(f32, f32)]>,
}

impl Indexed {
    /// Resolve one index to a colour, or `None` when the index is out of
    /// range or the palette is too short to hold it.
    ///
    /// A `None` here is *no colour*: the caller paints black in bulk
    /// conversion and skips the object in scalar use.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32]) -> Option<Rgb> {
        let raw = comps.first().copied().unwrap_or(0.0);
        if raw.is_nan() {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "truncation toward zero is exactly the C++'s int cast"
        )]
        let index = raw as i64;
        if index < 0 || index > i64::from(self.max_index) {
            return None;
        }
        let n = self.base.n_components();
        let index = usize::try_from(index).ok()?;
        let needed = index.checked_add(1)?.checked_mul(n)?;
        if needed > self.lookup.len() {
            return None;
        }
        let offset = index.checked_mul(n)?;
        let mut base_comps = Vec::with_capacity(n);
        for i in 0..n {
            let byte = self.lookup.get(offset + i).copied().unwrap_or(0);
            let (min, range) = self.component_ranges.get(i).copied().unwrap_or((0.0, 1.0));
            base_comps.push(min + range * f32::from(byte) / 255.0);
        }
        Some(self.base.to_rgb(&base_comps))
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

    use super::Indexed;
    use crate::color::ColorSpace;

    fn gray_palette() -> Indexed {
        Indexed {
            base: Box::new(ColorSpace::DeviceGray),
            max_index: 3,
            lookup: Box::from(&[0u8, 85, 170, 255][..]),
            component_ranges: Box::from(&[(0.0f32, 1.0f32)][..]),
        }
    }

    #[test]
    fn indices_map_through_the_palette() {
        let cs = gray_palette();
        let rgb = cs.to_rgb(&[0.0]).expect("index 0");
        assert!(rgb.r.abs() < 1e-6);
        let rgb = cs.to_rgb(&[3.0]).expect("index 3");
        assert!((rgb.r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn out_of_range_indices_yield_no_colour() {
        let cs = gray_palette();
        assert!(cs.to_rgb(&[-1.0]).is_none());
        assert!(cs.to_rgb(&[4.0]).is_none());
        assert!(cs.to_rgb(&[f32::NAN]).is_none());
    }

    #[test]
    fn a_short_palette_yields_no_colour_rather_than_reading_past_it() {
        let cs = Indexed {
            base: Box::new(ColorSpace::DeviceRgb),
            max_index: 255,
            // Room for exactly one RGB entry.
            lookup: Box::from(&[10u8, 20, 30][..]),
            component_ranges: Box::from(&[(0.0f32, 1.0f32); 3][..]),
        };
        assert!(cs.to_rgb(&[0.0]).is_some());
        assert!(cs.to_rgb(&[1.0]).is_none());
    }

    #[test]
    fn indices_truncate_toward_zero() {
        let cs = gray_palette();
        // 2.9 selects entry 2, not entry 3.
        let a = cs.to_rgb(&[2.9]).expect("2.9");
        let b = cs.to_rgb(&[2.0]).expect("2.0");
        assert!((a.r - b.r).abs() < 1e-6);
    }
}
