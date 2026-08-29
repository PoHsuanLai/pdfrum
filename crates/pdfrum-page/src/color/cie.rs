//! The CIE-based families and the XYZ→sRGB machinery they share
//! (ISO 32000-1 §8.6.5).
//!
//! Three of PDFium's asymmetries live here and are all deliberate:
//!
//! - **`CalGray` ignores everything it parses.** Gamma, white point and black
//!   point are read, validated, and then never consulted: the conversion is
//!   `(g, g, g)` with no clamping, making it a *worse* `DeviceGray`. The
//!   fields are kept so dumps can report them.
//! - **`CalRGB` honours gamma, matrix and white point in the scalar path but
//!   not the bulk one**, where it degrades to a red↔blue swap. The C++
//!   unittest pins exactly that, so both halves are ported.
//! - **`Lab`'s constants are not the textbook CIE ones.** The threshold
//!   `0.2069`, the linear slope `0.12842` and the white point `(0.957, 1.0,
//!   1.0889)` are PDFium's, and reproducing them is what makes Lab images
//!   match.

// A three-by-three matrix's entries are `a` through `i` in every textbook;
// naming them `row_zero_column_one` would obscure the arithmetic.
#![expect(
    clippy::many_single_char_names,
    reason = "matrix entries are single-letter by convention"
)]

use super::srgb_table::{SAMPLES1_LEN, SRGB_SAMPLES1, SRGB_SAMPLES2};
use super::{Rgb, load::CsArray};
use crate::names;
use pdfrum_object::{Array, Dict, Resolve};

/// The default `/Gamma` when the key is absent or reads as zero.
const DEFAULT_GAMMA: f32 = 1.0;

/// `/Range`'s default, which applies **only** when the key is absent
/// entirely — a present-but-short array yields zeros for the missing entries.
const DEFAULT_LAB_RANGES: [f32; 4] = [-100.0, 100.0, -100.0, 100.0];

/// A `CalGray` space (ISO 32000-1 §8.6.5.2).
///
/// Every field is parsed and validated, and none of them affects
/// [`cal_gray_to_rgb`]. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct CalGray {
    /// The diffuse white point, `[Xw, Yw, Zw]`.
    pub white_point: [f32; 3],
    /// The diffuse black point, `[Xb, Yb, Zb]`.
    pub black_point: [f32; 3],
    /// The `/Gamma` exponent, defaulting to 1.0.
    pub gamma: f32,
}

/// A `CalRGB` space (ISO 32000-1 §8.6.5.3).
#[derive(Debug, Clone, PartialEq)]
pub struct CalRgb {
    /// The diffuse white point.
    pub white_point: [f32; 3],
    /// The diffuse black point. Parsed and then ignored.
    pub black_point: [f32; 3],
    /// The per-component gammas, when `/Gamma` was present at all.
    pub gamma: Option<[f32; 3]>,
    /// The XYZ mixing matrix, when `/Matrix` was present at all.
    pub matrix: Option<[f32; 9]>,
}

/// A `Lab` space (ISO 32000-1 §8.6.5.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Lab {
    /// The diffuse white point.
    pub white_point: [f32; 3],
    /// The diffuse black point. Parsed and then ignored.
    pub black_point: [f32; 3],
    /// The `a*` and `b*` component ranges, `[amin, amax, bmin, bmax]`.
    pub ranges: [f32; 4],
}

/// Read `/WhitePoint`, which **must** have exactly three elements and satisfy
/// `Xw > 0 && Yw == 1.0 && Zw > 0`.
///
/// The equality on `Yw` is exact, so `[0.9505 0.99999 1.089]` fails the whole
/// space's load — a real and deliberate strictness.
fn white_point(dict: &Dict, r: &impl Resolve) -> Option<[f32; 3]> {
    let array = dict.array(names::WHITE_POINT, r)?;
    if array.len() != 3 {
        return None;
    }
    let wp = [
        array.number_at_or_zero(0),
        array.number_at_or_zero(1),
        array.number_at_or_zero(2),
    ];
    let [xw, yw, zw] = wp;
    // The equality on Yw is **exact**, deliberately: `0.99999` fails.
    #[expect(
        clippy::float_cmp,
        reason = "the exact comparison is the behaviour being ported"
    )]
    let valid = xw > 0.0 && yw == 1.0 && zw > 0.0;
    valid.then_some(wp)
}

/// Read `/BlackPoint`, which never fails: a missing or wrong-sized array, or
/// any negative component, yields all zeros.
fn black_point(dict: &Dict, r: &impl Resolve) -> [f32; 3] {
    let Some(array) = dict.array(names::BLACK_POINT, r) else {
        return [0.0; 3];
    };
    if array.len() != 3 {
        return [0.0; 3];
    }
    let bp = [
        array.number_at_or_zero(0),
        array.number_at_or_zero(1),
        array.number_at_or_zero(2),
    ];
    if bp.iter().any(|v| *v < 0.0) {
        return [0.0; 3];
    }
    bp
}

impl CalGray {
    /// Load from a `[/CalGray << … >>]` array. Element 1 must be a dictionary
    /// and its `/WhitePoint` must validate.
    pub(super) fn load(array: &CsArray, r: &impl Resolve) -> Option<Self> {
        let dict = array.dict_at(1, r)?;
        let white_point = white_point(&dict, r)?;
        let gamma = dict.number(names::GAMMA, r).unwrap_or(0.0);
        Some(Self {
            white_point,
            black_point: black_point(&dict, r),
            // A `/Gamma` of zero, including an absent key, means 1.0.
            gamma: if gamma == 0.0 { DEFAULT_GAMMA } else { gamma },
        })
    }
}

impl CalRgb {
    /// Load from a `[/CalRGB << … >>]` array.
    ///
    /// `/Gamma` and `/Matrix` are read positionally **without any length
    /// check** when the key exists at all, so `/Gamma [2.2]` yields
    /// `{2.2, 0, 0}`.
    pub(super) fn load(array: &CsArray, r: &impl Resolve) -> Option<Self> {
        let dict = array.dict_at(1, r)?;
        let white_point = white_point(&dict, r)?;
        let read =
            |a: &Array, n: usize| -> Vec<f32> { (0..n).map(|i| a.number_at_or_zero(i)).collect() };
        let gamma = dict.array(names::GAMMA, r).map(|a| {
            let v = read(&a, 3);
            [
                v.first().copied().unwrap_or(0.0),
                v.get(1).copied().unwrap_or(0.0),
                v.get(2).copied().unwrap_or(0.0),
            ]
        });
        let matrix = dict.array(names::MATRIX, r).map(|a| {
            let v = read(&a, 9);
            std::array::from_fn(|i| v.get(i).copied().unwrap_or(0.0))
        });
        Some(Self {
            white_point,
            black_point: black_point(&dict, r),
            gamma,
            matrix,
        })
    }
}

impl Lab {
    /// Load from a `[/Lab << … >>]` array.
    pub(super) fn load(array: &CsArray, r: &impl Resolve) -> Option<Self> {
        let dict = array.dict_at(1, r)?;
        let white_point = white_point(&dict, r)?;
        let ranges = match dict.array(names::RANGE, r) {
            // Present: read positionally, so a short array yields zeros.
            Some(a) => std::array::from_fn(|i| a.number_at_or_zero(i)),
            // Absent: the documented defaults.
            None => DEFAULT_LAB_RANGES,
        };
        Some(Self {
            white_point,
            black_point: black_point(&dict, r),
            ranges,
        })
    }

    /// The initial value and legal interval of component `index`.
    ///
    /// `L*` is always `0..=100`. For `a*` and `b*`, an interval whose low
    /// bound exceeds its high bound falls back to `0..=100` — **not** to
    /// ±100.
    #[must_use]
    pub fn default_value(&self, index: usize) -> (f32, f32, f32) {
        if index == 0 {
            return (0.0, 0.0, 100.0);
        }
        let lo = self.ranges.get((index - 1) * 2).copied().unwrap_or(0.0);
        let hi = self.ranges.get((index - 1) * 2 + 1).copied().unwrap_or(0.0);
        if lo <= hi {
            (0.0f32.clamp(lo, hi), lo, hi)
        } else {
            (0.0, 0.0, 100.0)
        }
    }
}

/// PDFium's sRGB gamma encoding: a 1024-step table lookup, not the analytic
/// transfer function.
///
/// The input is clamped, scaled by **1023**, and split at index 192 between a
/// dense table and a quarter-resolution one. The maximum second-table index
/// is `1023 / 4 - 48 = 207`, which is exactly its length minus one.
#[must_use]
pub fn rgb_conversion(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the clamp above bounds the product to 0..=1023"
    )]
    let idx = (c * 1023.0).max(0.0) as usize;
    let sample = if idx < SAMPLES1_LEN {
        SRGB_SAMPLES1.get(idx).copied()
    } else {
        SRGB_SAMPLES2.get(idx / 4 - 48).copied()
    };
    f32::from(sample.unwrap_or(0)) / 255.0
}

/// The fixed sRGB primaries matrix, applied when no white-point adaptation is
/// wanted.
#[must_use]
pub fn xyz_to_srgb(x: f32, y: f32, z: f32) -> Rgb {
    Rgb {
        r: rgb_conversion(3.2410 * x - 1.5374 * y - 0.4986 * z),
        g: rgb_conversion(-0.9692 * x + 1.8760 * y + 0.0416 * z),
        b: rgb_conversion(0.0556 * x - 0.2040 * y + 1.0570 * z),
    }
}

/// A 3×3 matrix in row-major order, with the inverse PDFium computes.
#[derive(Debug, Clone, Copy)]
struct Matrix3([f32; 9]);

impl Matrix3 {
    fn at(self, i: usize) -> f32 {
        self.0.get(i).copied().unwrap_or(0.0)
    }

    /// The inverse, or the **all-zero matrix** when the determinant is
    /// smaller than one float epsilon — PDFium silently produces black rather
    /// than reporting a singular matrix.
    fn inverse(self) -> Self {
        let (a, b, c) = (self.at(0), self.at(1), self.at(2));
        let (d, e, f) = (self.at(3), self.at(4), self.at(5));
        let (g, h, i) = (self.at(6), self.at(7), self.at(8));
        let det = a * (e * i - f * h) - b * (i * d - f * g) + c * (d * h - e * g);
        if det.abs() < f32::EPSILON {
            return Self([0.0; 9]);
        }
        Self([
            (e * i - f * h) / det,
            -(b * i - c * h) / det,
            (b * f - c * e) / det,
            -(d * i - f * g) / det,
            (a * i - c * g) / det,
            -(a * f - c * d) / det,
            (d * h - e * g) / det,
            -(a * h - b * g) / det,
            (a * e - b * d) / det,
        ])
    }

    fn mul_vec(self, v: [f32; 3]) -> [f32; 3] {
        let (x, y, z) = (v[0], v[1], v[2]);
        [
            self.at(0) * x + self.at(1) * y + self.at(2) * z,
            self.at(3) * x + self.at(4) * y + self.at(5) * z,
            self.at(6) * x + self.at(7) * y + self.at(8) * z,
        ]
    }

    fn mul_diag(self, d: [f32; 3]) -> Self {
        Self([
            self.at(0) * d[0],
            self.at(1) * d[1],
            self.at(2) * d[2],
            self.at(3) * d[0],
            self.at(4) * d[1],
            self.at(5) * d[2],
            self.at(6) * d[0],
            self.at(7) * d[1],
            self.at(8) * d[2],
        ])
    }
}

/// XYZ to sRGB with a Bradford-free white-point adaptation built from the
/// sRGB chromaticities.
#[must_use]
pub fn xyz_to_srgb_white_point(xyz: [f32; 3], white: [f32; 3]) -> Rgb {
    // The sRGB primaries, as PDFium spells them.
    const RX: f32 = 0.64;
    const RY: f32 = 0.33;
    const GX: f32 = 0.30;
    const GY: f32 = 0.60;
    const BX: f32 = 0.15;
    const BY: f32 = 0.06;
    let rgb_xyz = Matrix3([
        RX,
        GX,
        BX,
        RY,
        GY,
        BY,
        1.0 - RX - RY,
        1.0 - GX - GY,
        1.0 - BX - BY,
    ]);
    let s = rgb_xyz.inverse().mul_vec(white);
    let m = rgb_xyz.mul_diag(s);
    let out = m.inverse().mul_vec(xyz);
    Rgb {
        r: rgb_conversion(out[0]),
        g: rgb_conversion(out[1]),
        b: rgb_conversion(out[2]),
    }
}

/// `CalGray`'s conversion: the level replicated, unclamped, with every parsed
/// parameter ignored.
#[must_use]
pub fn cal_gray_to_rgb(comps: &[f32]) -> Rgb {
    let g = comps.first().copied().unwrap_or(0.0);
    Rgb { r: g, g, b: g }
}

/// `CalRGB`'s scalar conversion, which does honour gamma, matrix and white
/// point. Inputs are not clamped, so a negative component raised to a
/// non-integer gamma yields NaN, which the sRGB table's own clamp absorbs.
#[must_use]
pub fn cal_rgb_to_rgb(space: &CalRgb, comps: &[f32]) -> Rgb {
    let mut a = comps.first().copied().unwrap_or(0.0);
    let mut b = comps.get(1).copied().unwrap_or(0.0);
    let mut c = comps.get(2).copied().unwrap_or(0.0);
    if let Some(g) = space.gamma {
        a = a.powf(g[0]);
        b = b.powf(g[1]);
        c = c.powf(g[2]);
    }
    let xyz = match space.matrix {
        Some(m) => [
            m[0] * a + m[3] * b + m[6] * c,
            m[1] * a + m[4] * b + m[7] * c,
            m[2] * a + m[5] * b + m[8] * c,
        ],
        None => [a, b, c],
    };
    xyz_to_srgb_white_point(xyz, space.white_point)
}

/// `Lab`'s conversion. Note the constants are PDFium's, not the textbook CIE
/// ones, and that `/Range` is **not** applied here — there is no clamping at
/// conversion time.
#[must_use]
pub fn lab_to_rgb(comps: &[f32]) -> Rgb {
    /// The cube-root threshold below which the transfer function is linear.
    const THRESHOLD: f32 = 0.2069;
    /// The slope of that linear segment.
    const SLOPE: f32 = 0.12842;
    /// Its offset.
    const OFFSET: f32 = 0.1379;
    /// The white point's X, with Y implicitly 1.0.
    const XN: f32 = 0.957;
    /// The white point's Z.
    const ZN: f32 = 1.0889;

    let l_star = comps.first().copied().unwrap_or(0.0);
    let a_star = comps.get(1).copied().unwrap_or(0.0);
    let b_star = comps.get(2).copied().unwrap_or(0.0);

    let m = (l_star + 16.0) / 116.0;
    let l = m + a_star / 500.0;
    let n = m - b_star / 200.0;

    let f = |v: f32| {
        if v < THRESHOLD {
            SLOPE * (v - OFFSET)
        } else {
            v * v * v
        }
    };
    xyz_to_srgb(XN * f(l), f(m), ZN * f(n))
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

    use super::{Lab, cal_gray_to_rgb, lab_to_rgb, rgb_conversion};

    #[test]
    fn srgb_table_endpoints_and_branch() {
        assert!(rgb_conversion(0.0).abs() < 1e-6);
        assert!((rgb_conversion(1.0) - 1.0).abs() < 1e-6);
        // Clamping, not extrapolation.
        assert!(rgb_conversion(-5.0).abs() < 1e-6);
        assert!((rgb_conversion(10.0) - 1.0).abs() < 1e-6);
        // Just below and just above the 192-entry branch.
        let low = rgb_conversion(191.0 / 1023.0);
        let high = rgb_conversion(192.0 / 1023.0);
        assert!(high >= low, "the table must be monotonic across its seam");
    }

    #[test]
    fn cal_gray_is_an_unclamped_passthrough() {
        let rgb = cal_gray_to_rgb(&[0.25, 9.0, -9.0]);
        assert!((rgb.r - 0.25).abs() < 1e-6);
        assert!((rgb.g - 0.25).abs() < 1e-6);
        assert!((rgb.b - 0.25).abs() < 1e-6);
        // Unclamped, unlike DeviceGray.
        let rgb = cal_gray_to_rgb(&[5.0]);
        assert!((rgb.r - 5.0).abs() < 1e-6);
    }

    #[test]
    fn lab_black_and_white() {
        let black = lab_to_rgb(&[0.0, 0.0, 0.0]);
        assert!(black.r < 0.1 && black.g < 0.1 && black.b < 0.1);
        let white = lab_to_rgb(&[100.0, 0.0, 0.0]);
        assert!(white.r > 0.9 && white.g > 0.9 && white.b > 0.9);
    }

    #[test]
    fn lab_default_values_fall_back_to_zero_hundred_when_inverted() {
        let lab = Lab {
            white_point: [0.9505, 1.0, 1.089],
            black_point: [0.0; 3],
            ranges: [50.0, -50.0, -100.0, 100.0],
        };
        // L* is always 0..=100.
        assert_eq!(lab.default_value(0), (0.0, 0.0, 100.0));
        // An inverted a* range falls back to 0..=100, not ±100.
        assert_eq!(lab.default_value(1), (0.0, 0.0, 100.0));
        // A sane b* range is used as given, with the value clamped into it.
        assert_eq!(lab.default_value(2), (0.0, -100.0, 100.0));
    }
}
