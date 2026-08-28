//! Tier B pixel comparison: grayscale SSIM plus the cheap exact-match and
//! max-channel-diff signals.
//!
//! Hand-rolled on purpose (DEPS.md): SSIM is the ratchet metric, and a
//! threshold recorded in `thresholds.toml` must mean the same thing next year
//! as it does today. A dependency bump must never silently move a score.
//!
//! The formula is the standard one from Wang et al. 2004 over a sliding 8x8
//! window, with `K1 = 0.01`, `K2 = 0.03`, `L = 255`:
//!
//! ```text
//! SSIM = ((2·μx·μy + C1)(2·σxy + C2)) / ((μx² + μy² + C1)(σx² + σy² + C2))
//! ```
//!
//! The reported score is the mean over all window positions (the "mean SSIM"
//! of the paper). Windows are uniformly weighted rather than Gaussian: the
//! metric only has to be *stable* and monotone in visual difference, and a box
//! window is far easier to reason about when a threshold is disputed.

/// Window edge length in pixels.
const WINDOW: usize = 8;
/// Stabilizer constants, on the standard 8-bit dynamic range `L = 255`.
const K1: f64 = 0.01;
const K2: f64 = 0.03;
const L: f64 = 255.0;

/// A decoded 8-bit RGBA image, the shape both goldens and candidates arrive in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Exactly `width * height * 4` bytes, RGBA order.
    pub rgba: Vec<u8>,
}

/// The Tier B verdict for one page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelDiff {
    /// Mean SSIM over the grayscale projection, in `[-1, 1]`; 1.0 is identical.
    pub ssim: f64,
    /// True when every byte matches — the strongest signal, and free.
    pub exact: bool,
    /// Largest absolute per-channel difference across the image.
    pub max_channel_diff: u8,
}

/// What made two images incomparable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PixelError {
    #[error("image dimensions differ: golden {golden:?}, candidate {candidate:?}")]
    SizeMismatch {
        golden: (u32, u32),
        candidate: (u32, u32),
    },
    #[error("image buffer is {actual} bytes, expected {expected} for {width}x{height} RGBA")]
    BadBuffer {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
}

impl Image {
    /// Checks the buffer length matches the declared dimensions.
    pub fn validate(&self) -> Result<(), PixelError> {
        let expected = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        if self.rgba.len() == expected {
            Ok(())
        } else {
            Err(PixelError::BadBuffer {
                width: self.width,
                height: self.height,
                expected,
                actual: self.rgba.len(),
            })
        }
    }
}

/// Compares a candidate render against its golden.
///
/// Differing dimensions are a `SizeMismatch` rather than a zero score: a page
/// rendered at the wrong size is a Tier A-class bug, and averaging it into a
/// pixel score would hide that.
pub fn compare(golden: &Image, candidate: &Image) -> Result<PixelDiff, PixelError> {
    golden.validate()?;
    candidate.validate()?;
    if golden.width != candidate.width || golden.height != candidate.height {
        return Err(PixelError::SizeMismatch {
            golden: (golden.width, golden.height),
            candidate: (candidate.width, candidate.height),
        });
    }

    let exact = golden.rgba == candidate.rgba;
    let max_channel_diff = golden
        .rgba
        .iter()
        .zip(&candidate.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);

    let ssim = if exact {
        // Short-circuit: identical input is exactly 1.0 by definition, and
        // saying so avoids any floating-point drift near the top of the range.
        1.0
    } else {
        mean_ssim(
            &to_gray(golden),
            &to_gray(candidate),
            golden.width as usize,
            golden.height as usize,
        )
    };

    Ok(PixelDiff {
        ssim,
        exact,
        max_channel_diff,
    })
}

/// Projects RGBA to luma over a white background.
///
/// PDF pages are composited onto white by the oracle, so an alpha-aware
/// projection keeps a transparent candidate from scoring as "all black".
/// Coefficients are Rec. 601 luma, the conventional choice for SSIM.
fn to_gray(image: &Image) -> Vec<f64> {
    image
        .rgba
        .chunks_exact(4)
        .map(|px| {
            let alpha = f64::from(px[3]) / 255.0;
            let over_white = |c: u8| f64::from(c) * alpha + 255.0 * (1.0 - alpha);
            0.299 * over_white(px[0]) + 0.587 * over_white(px[1]) + 0.114 * over_white(px[2])
        })
        .collect()
}

/// Mean SSIM over every full 8x8 window position.
///
/// Images smaller than one window in either axis are scored as a single window
/// covering the whole image — the corpus does hold 1x1 and few-pixel pages.
fn mean_ssim(a: &[f64], b: &[f64], width: usize, height: usize) -> f64 {
    if width == 0 || height == 0 {
        return 1.0;
    }
    let win_w = WINDOW.min(width);
    let win_h = WINDOW.min(height);
    let c1 = (K1 * L) * (K1 * L);
    let c2 = (K2 * L) * (K2 * L);

    let mut total = 0.0;
    let mut windows = 0.0f64;
    for top in 0..=(height - win_h) {
        for left in 0..=(width - win_w) {
            total += window_ssim(a, b, width, left, top, win_w, win_h, c1, c2);
            windows += 1.0;
        }
    }
    if windows == 0.0 { 1.0 } else { total / windows }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a window is a position plus a size plus the two stabilizers; \
              bundling them into a struct would obscure the formula"
)]
fn window_ssim(
    a: &[f64],
    b: &[f64],
    stride: usize,
    left: usize,
    top: usize,
    win_w: usize,
    win_h: usize,
    c1: f64,
    c2: f64,
) -> f64 {
    // Window edges are at most 8, so the pixel count is tiny and exact in f64.
    let n = f64::from(u32::try_from(win_w * win_h).unwrap_or(u32::MAX));
    let (mut sum_a, mut sum_b) = (0.0, 0.0);
    for row in 0..win_h {
        let base = (top + row) * stride + left;
        for col in 0..win_w {
            sum_a += a.get(base + col).copied().unwrap_or(0.0);
            sum_b += b.get(base + col).copied().unwrap_or(0.0);
        }
    }
    let mean_a = sum_a / n;
    let mean_b = sum_b / n;

    let (mut var_a, mut var_b, mut cov) = (0.0, 0.0, 0.0);
    for row in 0..win_h {
        let base = (top + row) * stride + left;
        for col in 0..win_w {
            let da = a.get(base + col).copied().unwrap_or(0.0) - mean_a;
            let db = b.get(base + col).copied().unwrap_or(0.0) - mean_b;
            var_a += da * da;
            var_b += db * db;
            cov += da * db;
        }
    }
    // Sample (N-1) normalization, matching the reference implementation.
    let denom_n = if n > 1.0 { n - 1.0 } else { 1.0 };
    var_a /= denom_n;
    var_b /= denom_n;
    cov /= denom_n;

    let numerator = (2.0 * mean_a * mean_b + c1) * (2.0 * cov + c2);
    let denominator = (mean_a * mean_a + mean_b * mean_b + c1) * (var_a + var_b + c2);
    if denominator == 0.0 {
        1.0
    } else {
        numerator / denominator
    }
}

#[cfg(test)]
// Exact float equality is deliberate here: these assertions pin values that
// are exact by construction (SSIM of identical input is 1.0; a threshold
// parsed from text round-trips bit-for-bit). An epsilon would weaken them.
#[allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "exactly-representable assertions; fixture pixels are bounded by construction"
)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, value: u8) -> Image {
        Image {
            width,
            height,
            rgba: (0..width * height)
                .flat_map(|_| [value, value, value, 255])
                .collect(),
        }
    }

    fn from_gray(width: u32, height: u32, pixels: &[u8]) -> Image {
        Image {
            width,
            height,
            rgba: pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        }
    }

    #[test]
    fn identical_images_score_exactly_one() {
        let img = solid(32, 32, 128);
        let diff = compare(&img, &img).unwrap();
        assert_eq!(diff.ssim, 1.0);
        assert!(diff.exact);
        assert_eq!(diff.max_channel_diff, 0);
    }

    #[test]
    fn identical_noise_scores_exactly_one() {
        // A non-uniform image: catches a short-circuit that only works on flats.
        let pixels: Vec<u8> = (0..64 * 64).map(|i| ((i * 37) % 251) as u8).collect();
        let img = from_gray(64, 64, &pixels);
        assert_eq!(compare(&img, &img).unwrap().ssim, 1.0);
    }

    #[test]
    fn equal_but_distinct_buffers_still_score_one() {
        let a = from_gray(
            16,
            16,
            &(0..256).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        );
        let b = a.clone();
        let diff = compare(&a, &b).unwrap();
        assert!(diff.exact);
        assert_eq!(diff.ssim, 1.0);
    }

    #[test]
    fn black_against_white_scores_near_zero() {
        let diff = compare(&solid(16, 16, 0), &solid(16, 16, 255)).unwrap();
        assert!(!diff.exact);
        assert_eq!(diff.max_channel_diff, 255);
        assert!(diff.ssim < 0.01, "ssim was {}", diff.ssim);
    }

    #[test]
    fn a_known_flat_pair_matches_the_closed_form() {
        // Two flat fields have zero variance and zero covariance, so
        // SSIM reduces to the luminance term (2·μa·μb + C1)/(μa² + μb² + C1).
        let (a, b) = (100.0f64, 110.0f64);
        let c1 = (K1 * L) * (K1 * L);
        let expected = (2.0 * a * b + c1) / (a * a + b * b + c1);
        let diff = compare(&solid(16, 16, 100), &solid(16, 16, 110)).unwrap();
        assert!(
            (diff.ssim - expected).abs() < 1e-9,
            "ssim {} vs closed form {expected}",
            diff.ssim
        );
    }

    #[test]
    fn a_tiny_perturbation_stays_above_the_default_threshold() {
        let base: Vec<u8> = (0..64 * 64).map(|i| ((i * 17) % 251) as u8).collect();
        let nudged: Vec<u8> = base
            .iter()
            .enumerate()
            .map(|(i, &v)| if i % 997 == 0 { v.saturating_add(1) } else { v })
            .collect();
        let diff = compare(&from_gray(64, 64, &base), &from_gray(64, 64, &nudged)).unwrap();
        assert!(!diff.exact);
        assert_eq!(diff.max_channel_diff, 1);
        assert!(diff.ssim > 0.99, "ssim was {}", diff.ssim);
    }

    #[test]
    fn ssim_is_symmetric() {
        let a = from_gray(
            24,
            24,
            &(0..576).map(|i| (i % 200) as u8).collect::<Vec<_>>(),
        );
        let b = from_gray(
            24,
            24,
            &(0..576).map(|i| ((i * 3) % 200) as u8).collect::<Vec<_>>(),
        );
        let forward = compare(&a, &b).unwrap().ssim;
        let backward = compare(&b, &a).unwrap().ssim;
        assert!((forward - backward).abs() < 1e-12);
    }

    #[test]
    fn ssim_decreases_as_distortion_grows() {
        let base: Vec<u8> = (0..64 * 64).map(|i| ((i * 17) % 251) as u8).collect();
        let mut previous = 1.0;
        for step in [2u8, 8, 32, 96] {
            let noisy: Vec<u8> = base.iter().map(|&v| v.saturating_add(step)).collect();
            let ssim = compare(&from_gray(64, 64, &base), &from_gray(64, 64, &noisy))
                .unwrap()
                .ssim;
            assert!(ssim < previous, "step {step}: {ssim} not below {previous}");
            previous = ssim;
        }
    }

    #[test]
    fn images_smaller_than_a_window_still_score() {
        let one = from_gray(1, 1, &[42]);
        assert_eq!(compare(&one, &one).unwrap().ssim, 1.0);
        let small = from_gray(3, 2, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(compare(&small, &small).unwrap().ssim, 1.0);
        let other = from_gray(3, 2, &[9, 9, 9, 9, 9, 9]);
        assert!(compare(&small, &other).unwrap().ssim < 1.0);
    }

    #[test]
    fn size_mismatch_is_an_error() {
        assert_eq!(
            compare(&solid(8, 8, 0), &solid(9, 8, 0)),
            Err(PixelError::SizeMismatch {
                golden: (8, 8),
                candidate: (9, 8)
            })
        );
    }

    #[test]
    fn a_short_buffer_is_an_error() {
        let bad = Image {
            width: 4,
            height: 4,
            rgba: vec![0; 10],
        };
        assert!(matches!(
            compare(&bad, &solid(4, 4, 0)),
            Err(PixelError::BadBuffer { .. })
        ));
    }

    #[test]
    fn alpha_composites_over_white() {
        // Fully transparent must read as white, not as its RGB payload.
        let transparent = Image {
            width: 8,
            height: 8,
            rgba: (0..64).flat_map(|_| [0, 0, 0, 0]).collect(),
        };
        let white = solid(8, 8, 255);
        assert!(compare(&transparent, &white).unwrap().ssim > 0.999);
    }
}
