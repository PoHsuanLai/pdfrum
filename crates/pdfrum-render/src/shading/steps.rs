//! The 256-entry colour ramp axial and radial shadings index
//! (`GetShadingSteps`, `cpdf_rendershading.cpp:65-100`).
//!
//! Two arithmetic details are load-bearing and neither is what a
//! re-derivation would write. The LUT's divisor is `kShadingSteps` (256),
//! **not** `kShadingSteps - 1`, so the ramp never evaluates its functions at
//! `t_max` — the "shading LUT off-by-one" SPEC §7 already accepted porting
//! verbatim. And `ComponentToShadingIndex`, which maps a *mesh* vertex's
//! parametric value into the same ramp, scales by `255` instead — the two
//! sides of the same table disagree by design.

use pdfrum_page::Rgb;

use crate::color::Argb;

/// The ramp's length (`kShadingSteps`, `cpdf_rendershading.cpp:45`).
pub const STEPS: usize = 256;

/// A sampled colour ramp, one entry per step, at a fixed alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSteps {
    entries: Box<[Argb; STEPS]>,
}

impl ColorSteps {
    /// Sample a shading's functions across its domain.
    ///
    /// Returns `None` when the shading declares no outputs at all, which is
    /// upstream's `return false` and drops the whole shading.
    #[must_use]
    pub fn sample(
        shading: &pdfrum_page::Shading,
        t_min: f32,
        t_max: f32,
        alpha: u8,
    ) -> Option<Self> {
        if shading.functions.is_empty() && shading.space.n_components() == 0 {
            return None;
        }
        let diff = t_max - t_min;
        let mut entries = Box::new([Argb::TRANSPARENT; STEPS]);
        for (i, slot) in entries.iter_mut().enumerate() {
            // The divisor is STEPS, not STEPS - 1: `t_max` is never sampled.
            #[expect(clippy::cast_precision_loss, reason = "i < 256 is exact in f32")]
            let input = diff * (i as f32) / (STEPS as f32) + t_min;
            let rgb = shading.color_at(input);
            // The LUT rounds where the per-pixel function-based rasterizer
            // truncates — the same conversion written two ways upstream.
            let [r, g, b] = rgb.to_bytes();
            *slot = Argb { a: alpha, r, g, b };
        }
        Some(Self { entries })
    }

    /// Build a ramp from an explicit colour list, for tests and for callers
    /// that already hold the colours.
    #[must_use]
    pub fn from_colors(colors: [Rgb; STEPS], alpha: u8) -> Self {
        let mut entries = Box::new([Argb::TRANSPARENT; STEPS]);
        for (slot, rgb) in entries.iter_mut().zip(colors.iter()) {
            let [r, g, b] = rgb.to_bytes();
            *slot = Argb { a: alpha, r, g, b };
        }
        Self { entries }
    }

    /// Look up a parametric position, applying the `/Extend` rules.
    ///
    /// The index is `(s * 255) as i32` — a **truncation toward zero**, so a
    /// position of `0.999` lands on entry 254, not 255. Out of range with the
    /// matching extend flag clear returns `None`, and the caller must leave
    /// the destination pixel *untouched* rather than writing anything: the
    /// oracle's write is a plain assignment to the scanline word, so an
    /// unextended region keeps whatever `/Background` put there.
    #[must_use]
    pub fn lookup(&self, s: f32, extend_start: bool, extend_end: bool) -> Option<Argb> {
        if s.is_nan() {
            // In C++ `(int)NaN` is undefined and in practice yields `0` or
            // `INT_MIN`; the `< 0` test is what catches the latter, and the
            // radial rasterizer's unguarded `sqrt` is the only producer. We
            // pin the negative reading, which is the one the extend flags
            // give meaning to — a NaN position is *outside* the geometry, so
            // treating it as "before the start" is what leaves an unextended
            // shading's uncovered region untouched.
            return extend_start
                .then(|| self.entries.first().copied())
                .flatten();
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the clamp below bounds the index; the truncation is the ported behaviour"
        )]
        let index = (s * 255.0) as i32;
        let index = if index < 0 {
            if !extend_start {
                return None;
            }
            0
        } else if index >= STEPS as i32 {
            if !extend_end {
                return None;
            }
            STEPS - 1
        } else {
            index as usize
        };
        self.entries.get(index).copied()
    }

    /// One entry, by index.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<Argb> {
        self.entries.get(index).copied()
    }
}

/// `ComponentToShadingIndex` (`cpdf_rendershading.cpp:102-107`): map a mesh
/// vertex's parametric component into the ramp's index space.
///
/// Note the `255` here against the LUT's `256` — the ramp is built with one
/// divisor and addressed with another, and both spellings are ported as
/// written.
#[must_use]
pub fn component_to_shading_index(c: f32, c_min: f32, c_max: f32) -> f32 {
    if c_min == c_max {
        0.0
    } else {
        ((c - c_min) / (c_max - c_min)) * 255.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> ColorSteps {
        let mut colors = [Rgb::BLACK; STEPS];
        for (i, c) in colors.iter_mut().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "i < 256 is exact")]
            let v = i as f32 / 255.0;
            *c = Rgb { r: v, g: v, b: v };
        }
        ColorSteps::from_colors(colors, 255)
    }

    #[test]
    fn lut_divisor_is_256_so_t_max_is_never_sampled() {
        // With the divisor 256, the last input is 255/256 of the way across
        // the domain — not the domain's end. A `STEPS - 1` divisor would put
        // it exactly at t_max, and every shading's last stop would differ.
        let last_input: f64 = 255.0 / 256.0;
        assert!(last_input < 1.0);
        assert!((last_input - 0.996_093_75).abs() < 1e-6);
    }

    #[test]
    fn axial_index_truncates() {
        let r = ramp();
        // 0.999 * 255 = 254.7 -> 254, not 255.
        assert_eq!(r.lookup(0.999, true, true).map(|c| c.r), Some(254));
        assert_eq!(r.lookup(1.0, true, true).map(|c| c.r), Some(255));
        assert_eq!(r.lookup(0.0, true, true).map(|c| c.r), Some(0));
    }

    #[test]
    fn extend_skips_leave_the_pixel_untouched() {
        let r = ramp();
        assert_eq!(r.lookup(-0.5, false, true), None);
        assert_eq!(r.lookup(1.5, true, false), None);
        // With the flag set the value clamps to the ramp's end instead.
        assert_eq!(r.lookup(-0.5, true, true).map(|c| c.r), Some(0));
        assert_eq!(r.lookup(1.5, true, true).map(|c| c.r), Some(255));
    }

    #[test]
    fn nan_takes_the_start_extend_arm() {
        let r = ramp();
        assert_eq!(r.lookup(f32::NAN, false, true), None);
        assert_eq!(r.lookup(f32::NAN, true, false).map(|c| c.r), Some(0));
    }

    #[test]
    fn component_index_scales_by_255_not_256() {
        assert!((component_to_shading_index(1.0, 0.0, 1.0) - 255.0).abs() < 1e-6);
        assert!((component_to_shading_index(0.5, 0.0, 1.0) - 127.5).abs() < 1e-6);
        // A degenerate range is zero rather than a division by zero.
        assert_eq!(component_to_shading_index(7.0, 3.0, 3.0), 0.0);
    }
}
