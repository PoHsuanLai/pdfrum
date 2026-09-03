//! The three device families and the Adobe CMYK table
//! (ISO 32000-1 §8.6.4).
//!
//! `DeviceGray` and `DeviceRGB` are clamps. `DeviceCMYK` has **two**
//! conversions and picks between them with a flag that is on in exactly one
//! situation: while decoding an image whose caller asked for it. Vector fills
//! always take the Adobe-table path; some image paths take the naive
//! `1 - min(1, c + k)` one, which is visibly different. That flag is threaded
//! here as a plain `bool` rather than the C++'s cascading refcount
//! (design brief D9).

// `c`, `m`, `y` and `k` are the specification's names for these four
// quantities; spelling them out would make the formulas harder to check
// against ISO 32000-1 §8.6.4, not easier.
#![expect(
    clippy::many_single_char_names,
    reason = "cyan, magenta, yellow and black are single-letter by convention"
)]

use super::Rgb;
use super::cmyk_table::{AXIS, CMYK};

/// The value `(v * 255)` is offset by before truncating, chosen so the result
/// matches `roundf` on every float in `0..=1`.
///
/// The obvious `0.5` differs for `0.0019607842`, so PDFium uses the float
/// immediately below it and so do we.
const ROUNDING_OFFSET: f32 = 0.499_999_97;

/// `DeviceGray`: component 0, clamped, replicated.
///
/// Components past the first are ignored, which is why
/// `[0.43, 0.11, 0.34]` converts to `0.43` three times.
#[must_use]
pub fn gray_to_rgb(comps: &[f32]) -> Rgb {
    let g = comps.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
    Rgb { r: g, g, b: g }
}

/// `DeviceRGB`: three components, each clamped.
#[must_use]
pub fn rgb_to_rgb(comps: &[f32]) -> Rgb {
    let at = |i: usize| comps.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    Rgb {
        r: at(0),
        g: at(1),
        b: at(2),
    }
}

/// `DeviceCMYK`: the Adobe sample table, after clamping each component.
///
/// # PDFium's second formula, and why it is not here
///
/// The oracle carries an alternative — the naive subtractive
/// `1 - min(1, x + k)`, run *without* clamping its inputs — behind a flag it
/// calls *standard conversion*. **That formula can never reach a pixel**, so
/// this port does not implement it and no signature here carries the switch:
/// this function is the only `DeviceCMYK` conversion a colour goes through.
/// The proof is a `//` note on the body, and
/// `std_conversion_would_have_computed` pins what the formula would have
/// produced so the claim stays falsifiable.
// The proof that PDFium's standard-conversion formula is unreachable:
//
// - The only consumer in the whole C++ tree is `CPDF_DeviceCS`'s
//   `kDeviceCMYK` arm — `GetRGB` (`cpdf_devicecs.cpp:65`) and
//   `TranslateImageLine` (`cpdf_devicecs.cpp:119`). The flag itself is
//   `CPDF_ColorSpace::EnableStdConversion` (`cpdf_colorspace.cpp:663`), read
//   back through `IsStdConversionEnabled()`. `ICCBased`, `Lab` and `CalRGB`
//   never consult it; `CPDF_BasedCS` only forwards the counter to a base
//   space (`cpdf_basedcs.cpp:13`).
// - `CPDF_DIB` raises the counter *after* the image's own colour work is done
//   and lowers it before returning. `StartLoadDIBBase` (`cpdf_dib.cpp:199`)
//   runs `LoadPalette` (`:184`) and `CreateDecoder` **before**
//   `ContinueToLoadMask` raises it at `:153`; `:244` lowers it.
// - The image body is never translated inside that bracket.
//   `TranslateImageLine` is reached only from `TranslateScanline24bpp`
//   (`:1007`), called only from `CPDF_DIB::GetScanline` (`:1129`) — a `const`
//   accessor the rasterizer pulls during compositing, long after the counter
//   is back to zero. So `cpdf_devicecs.cpp:119`'s `IsStdConversionEnabled()`
//   is always false.
//
// What is left inside the bracket is one conversion, the `/Matte` colour at
// `cpdf_dib.cpp:839` — so the flag could only move a `DeviceCMYK` image
// carrying an `/SMask` with a `/Matte` array *and* drawn offscreen, a
// conjunction no file in `testing/corpus` or `testing/resources` contains,
// and the mask's own DIB (which `StartLoadMaskDIB` (`:832`) hands
// `bStdCS=true` unconditionally) is always `DeviceGray`, which ignores it.
// The naive formula is also the *less* correct of the two, so nothing of
// value is lost.
#[must_use]
pub fn cmyk_to_rgb(comps: &[f32]) -> Rgb {
    let at = |i: usize| comps.get(i).copied().unwrap_or(0.0);
    let clamp = |i: usize| at(i).clamp(0.0, 1.0);
    adobe_cmyk_to_srgb_f(clamp(0), clamp(1), clamp(2), clamp(3))
}

/// The float wrapper over [`adobe_cmyk_to_srgb`]: round each component to a
/// byte, look the colour up, and scale back.
///
/// Every result is therefore an exact multiple of `1/255`, which is what the
/// C++ unittest's vectors demonstrate.
#[must_use]
pub fn adobe_cmyk_to_srgb_f(c: f32, m: f32, y: f32, k: f32) -> Rgb {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "callers clamp to 0..=1, so the offset product is within a byte"
    )]
    let byte = |v: f32| (v * 255.0 + ROUNDING_OFFSET) as u8;
    let [r, g, b] = adobe_cmyk_to_srgb(byte(c), byte(m), byte(y), byte(k));
    Rgb {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
    }
}

/// The index of a grid cell.
fn index_from_cmyk(c: usize, m: usize, y: usize, k: usize) -> usize {
    AXIS * AXIS * AXIS * c + AXIS * AXIS * m + AXIS * y + k
}

fn cell(c: usize, m: usize, y: usize, k: usize) -> [i32; 3] {
    let rgb = CMYK
        .get(index_from_cmyk(c, m, y, k))
        .copied()
        .unwrap_or([0; 3]);
    [i32::from(rgb[0]), i32::from(rgb[1]), i32::from(rgb[2])]
}

/// The Adobe CMYK→sRGB table with per-axis linear interpolation in 8.8 fixed
/// point.
///
/// Each axis contributes a gradient **against the same base cell** rather
/// than a proper quadrilinear blend, and the result is clamped only from
/// below. Both are reproduced: they are what the corpus was rendered with.
#[must_use]
pub fn adobe_cmyk_to_srgb(c: u8, m: u8, y: u8, k: u8) -> [u8; 3] {
    let fix = |v: u8| i32::from(v) << 8;
    let (fix_c, fix_m, fix_y, fix_k) = (fix(c), fix(m), fix(y), fix(k));
    // The nearest grid index, rounding at the half-cell.
    let idx = |f: i32| usize::try_from((f + 4096) >> 13).unwrap_or(0).min(AXIS - 1);
    let (ci, mi, yi, ki) = (idx(fix_c), idx(fix_m), idx(fix_y), idx(fix_k));
    // The neighbour index, nudged away when it coincides with the base.
    let neighbour = |f: i32, base: usize| -> usize {
        let n = usize::try_from(f >> 13).unwrap_or(0).min(AXIS - 1);
        if n == base {
            if n == AXIS - 1 { n - 1 } else { n + 1 }
        } else {
            n
        }
    };
    let c1 = neighbour(fix_c, ci);
    let m1 = neighbour(fix_m, mi);
    let y1 = neighbour(fix_y, yi);
    let k1 = neighbour(fix_k, ki);

    let start = cell(ci, mi, yi, ki);
    let mut out = [start[0] << 8, start[1] << 8, start[2] << 8];

    // One axis's contribution: the gradient from the base cell towards its
    // neighbour, weighted by how far into the cell the input sits.
    let mut axis = |other: [i32; 3], fixed: i32, base: usize, alt: usize| {
        let base_i = i32::try_from(base).unwrap_or(0);
        let alt_i = i32::try_from(alt).unwrap_or(0);
        let rate = (fixed - (base_i << 13)) * (base_i - alt_i);
        for (out_c, (start_c, other_c)) in out.iter_mut().zip(start.iter().zip(other.iter())) {
            *out_c = out_c.saturating_add((start_c - other_c).saturating_mul(rate) / 32);
        }
    };
    axis(cell(c1, mi, yi, ki), fix_c, ci, c1);
    axis(cell(ci, m1, yi, ki), fix_m, mi, m1);
    axis(cell(ci, mi, y1, ki), fix_y, yi, y1);
    axis(cell(ci, mi, yi, k1), fix_k, ki, k1);

    // Clamped from below only: the C++ has no high clamp, and the byte cast
    // that follows wraps. Reproduce the wrap with a mask.
    let done = |v: i32| u8::try_from((v.max(0) >> 8) & 0xFF).unwrap_or(0);
    [done(out[0]), done(out[1]), done(out[2])]
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

    use super::{cmyk_to_rgb, gray_to_rgb, rgb_to_rgb};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn device_gray_reads_only_component_zero_and_clamps() {
        let rgb = gray_to_rgb(&[0.43, 0.11, 0.34]);
        assert!(close(rgb.r, 0.43) && close(rgb.g, 0.43) && close(rgb.b, 0.43));
        let rgb = gray_to_rgb(&[0.872]);
        assert!(close(rgb.r, 0.872));
        assert!(close(gray_to_rgb(&[0.0]).r, 0.0));
        assert!(close(gray_to_rgb(&[1.0]).r, 1.0));
        assert!(close(gray_to_rgb(&[-0.01]).r, 0.0));
        assert!(close(gray_to_rgb(&[12.5]).r, 1.0));
    }

    #[test]
    fn device_rgb_is_a_clamp() {
        let rgb = rgb_to_rgb(&[0.13, 1.0, 0.652]);
        assert!(close(rgb.r, 0.13) && close(rgb.g, 1.0) && close(rgb.b, 0.652));
        let rgb = rgb_to_rgb(&[0.0, 0.52, 0.78]);
        assert!(close(rgb.r, 0.0) && close(rgb.g, 0.52) && close(rgb.b, 0.78));
        let rgb = rgb_to_rgb(&[-10.5, 100.0, 0.78]);
        assert!(close(rgb.r, 0.0) && close(rgb.g, 1.0) && close(rgb.b, 0.78));
    }

    #[test]
    fn device_cmyk_matches_the_oracle_vectors() {
        // The four vectors from `cpdf_devicecs_unittest.cpp` — the best
        // available check that the 6561-entry table transcribed correctly.
        let cases: [([f32; 4], [f32; 3]); 4] = [
            ([0.6, 0.5, 0.3, 0.9], [0.0627451, 0.0627451, 0.10588236]),
            ([0.15, 0.5, 0.0, 0.9], [0.2, 0.0862745, 0.16470589]),
            ([0.15, 0.5, 1.0, 0.0], [0.85098046, 0.552941, 0.15686275]),
            // Out-of-range components clamp, giving the previous result.
            ([0.15, 0.5, 1.5, -0.6], [0.85098046, 0.552941, 0.15686275]),
        ];
        for (input, want) in cases {
            let got = cmyk_to_rgb(&input);
            assert!(
                close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2]),
                "cmyk {input:?}: got {got:?}, want {want:?}"
            );
        }
    }

    /// The record of PDFium's inert second formula, kept because the reason
    /// it is absent is a claim about the oracle and should stay falsifiable.
    ///
    /// This is **not** a test of `cmyk_to_rgb`: it recomputes the naive
    /// subtractive formula `1 - min(1, x + k)` here, unclamped, and pins what
    /// the oracle's standard-conversion path *would* have produced had it
    /// ever run. See `cmyk_to_rgb` for the proof that it cannot.
    #[test]
    fn std_conversion_would_have_computed() {
        let naive = |comps: &[f32; 4]| {
            let at = |i: usize| comps.get(i).copied().unwrap_or(0.0);
            let (c, m, y, k) = (at(0), at(1), at(2), at(3));
            [
                1.0 - (c + k).min(1.0),
                1.0 - (m + k).min(1.0),
                1.0 - (y + k).min(1.0),
            ]
        };
        let got = naive(&[0.5, 0.25, 0.0, 0.25]);
        assert!(close(got[0], 0.25) && close(got[1], 0.5) && close(got[2], 0.75));
        // The naive path saturates at 1 through `min`, not through a clamp of
        // the inputs — and the managed path this crate does run disagrees.
        assert!(close(naive(&[2.0, 0.0, 0.0, 0.0])[0], 0.0));
        assert!(close(cmyk_to_rgb(&[2.0, 0.0, 0.0, 0.0]).r, 0.0));
    }

    #[test]
    fn every_table_entry_is_reachable() {
        // A cheap transcription check: the corners must be white and black.
        let white = super::adobe_cmyk_to_srgb(0, 0, 0, 0);
        assert_eq!(white, [255, 255, 255]);
        let black = super::adobe_cmyk_to_srgb(255, 255, 255, 255);
        assert!(black.iter().all(|v| *v < 16), "got {black:?}");
        assert_eq!(super::CMYK.len(), 6561);
    }
}
