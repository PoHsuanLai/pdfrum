//! Pixel comparison: PNG in, one shape out, then the same grayscale SSIM the
//! conformance harness ratchets on (`conformance/src/ssim.rs`, copied rather
//! than depended on so this crate stays outside the workspace — and so the
//! metric here can never drift from the one there without a visible diff).
//!
//! Every image is composited over opaque white before anything is compared.
//! The oracle writes RGB for an opaque page and RGBA for a transparent one;
//! the engines hand back premultiplied or straight alpha; a page "over
//! white" is the one thing all of them mean the same way.

use serde::{Deserialize, Serialize};

use crate::model::Raster;

/// Window edge length in pixels.
const WINDOW: usize = 8;
const K1: f64 = 0.01;
const K2: f64 = 0.03;
const L: f64 = 255.0;

/// Sizes may differ by this many pixels per axis and still be compared over
/// their common area: engines round `points * scale` differently
/// (`pdfium_test` truncates, some peers round or floor after an f32 pass).
pub const SIZE_SLACK: u32 = 2;

/// An opaque 8-bit RGB image, stored as RGBA with alpha 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One page's verdict against the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RenderDiff {
    /// Mean SSIM over the grayscale projection; 1.0 is identical.
    pub ssim: f64,
    /// Every byte equal over the compared area, and the sizes equal too.
    pub exact: bool,
    /// Largest per-channel difference over the compared area.
    pub max_channel_diff: u8,
    /// Share of pixels (0..=100) where some channel differs by more than 8.
    pub differing_pct: f64,
    /// The engine's output size.
    pub size: (u32, u32),
    /// The oracle's output size.
    pub oracle_size: (u32, u32),
}

/// Why two images could not be compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelError {
    /// Sizes differ by more than [`SIZE_SLACK`] on some axis.
    SizeMismatch {
        oracle: (u32, u32),
        candidate: (u32, u32),
    },
    /// A buffer's length does not match its declared size.
    BadBuffer,
    /// The PNG would not decode.
    Png(String),
}

impl std::fmt::Display for PixelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelError::SizeMismatch { oracle, candidate } => {
                write!(
                    f,
                    "size mismatch: oracle {oracle:?}, candidate {candidate:?}"
                )
            }
            PixelError::BadBuffer => write!(f, "buffer length does not match its size"),
            PixelError::Png(err) => write!(f, "png: {err}"),
        }
    }
}

impl std::error::Error for PixelError {}

/// Composites a raster over white into an opaque image.
pub fn over_white(raster: &Raster) -> Result<Image, PixelError> {
    let expected = (raster.width as usize)
        .saturating_mul(raster.height as usize)
        .saturating_mul(4);
    if raster.rgba.len() != expected {
        return Err(PixelError::BadBuffer);
    }
    let mut rgba = Vec::with_capacity(expected);
    for px in raster.rgba.chunks_exact(4) {
        let a = u32::from(px[3]);
        let blend = |c: u8| -> u8 {
            let c = u32::from(c);
            let over = if raster.premultiplied {
                // Premultiplied: colour already carries alpha; add the white
                // that shows through.
                c + (255 - a)
            } else {
                (c * a + 255 * (255 - a)) / 255
            };
            over.min(255) as u8
        };
        rgba.extend_from_slice(&[blend(px[0]), blend(px[1]), blend(px[2]), 255]);
    }
    Ok(Image {
        width: raster.width,
        height: raster.height,
        rgba,
    })
}

/// Decodes a PNG and composites it over white.
pub fn decode_png(bytes: &[u8]) -> Result<Image, PixelError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|err| PixelError::Png(err.to_string()))?;
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| PixelError::Png(err.to_string()))?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        png::ColorType::Indexed => {
            return Err(PixelError::Png(
                "indexed PNG after normalization".to_owned(),
            ));
        }
    };
    over_white(&Raster {
        width: info.width,
        height: info.height,
        rgba,
        premultiplied: false,
    })
}

/// Encodes an opaque image as an RGB PNG.
pub fn encode_png(image: &Image) -> Result<Vec<u8>, PixelError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| PixelError::Png(err.to_string()))?;
        let rgb: Vec<u8> = image
            .rgba
            .chunks_exact(4)
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect();
        writer
            .write_image_data(&rgb)
            .map_err(|err| PixelError::Png(err.to_string()))?;
    }
    Ok(out)
}

/// Crops an image to its top-left `width` x `height`.
fn crop(image: &Image, width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    let row_bytes = (image.width * 4) as usize;
    for row in 0..height as usize {
        let start = row * row_bytes;
        out.extend_from_slice(&image.rgba[start..start + (width * 4) as usize]);
    }
    out
}

/// Compares a candidate against the oracle over their common area.
pub fn compare(oracle: &Image, candidate: &Image) -> Result<RenderDiff, PixelError> {
    let dx = oracle.width.abs_diff(candidate.width);
    let dy = oracle.height.abs_diff(candidate.height);
    if dx > SIZE_SLACK || dy > SIZE_SLACK {
        return Err(PixelError::SizeMismatch {
            oracle: (oracle.width, oracle.height),
            candidate: (candidate.width, candidate.height),
        });
    }
    let width = oracle.width.min(candidate.width);
    let height = oracle.height.min(candidate.height);
    let a = crop(oracle, width, height);
    let b = crop(candidate, width, height);

    let same_size = dx == 0 && dy == 0;
    let exact = same_size && a == b;
    let mut max_channel_diff = 0u8;
    let mut differing = 0usize;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let d = pa[0]
            .abs_diff(pb[0])
            .max(pa[1].abs_diff(pb[1]))
            .max(pa[2].abs_diff(pb[2]));
        max_channel_diff = max_channel_diff.max(d);
        differing += usize::from(d > 8);
    }
    let pixels = (width as usize) * (height as usize);
    let differing_pct = if pixels == 0 {
        0.0
    } else {
        100.0 * differing as f64 / pixels as f64
    };
    let ssim = if exact {
        1.0
    } else {
        mean_ssim(&to_gray(&a), &to_gray(&b), width as usize, height as usize)
    };
    Ok(RenderDiff {
        ssim,
        exact,
        max_channel_diff,
        differing_pct,
        size: (candidate.width, candidate.height),
        oracle_size: (oracle.width, oracle.height),
    })
}

/// Rec. 601 luma of an already-opaque RGBA buffer.
fn to_gray(rgba: &[u8]) -> Vec<f64> {
    rgba.chunks_exact(4)
        .map(|px| 0.299 * f64::from(px[0]) + 0.587 * f64::from(px[1]) + 0.114 * f64::from(px[2]))
        .collect()
}

/// Mean SSIM over every full 8x8 window position (`conformance/src/ssim.rs`).
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
    reason = "the formula's inputs, as in the conformance harness"
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
    let n = (win_w * win_h) as f64;
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

    #[test]
    fn identical_images_are_exact_and_score_one() {
        let a = solid(16, 16, 40);
        let diff = compare(&a, &a).unwrap_or_else(|err| panic!("{err}"));
        assert!(diff.exact);
        assert!((diff.ssim - 1.0).abs() < f64::EPSILON);
        assert_eq!(diff.max_channel_diff, 0);
    }

    #[test]
    fn a_one_pixel_size_difference_is_compared_over_the_common_area() {
        let a = solid(16, 16, 40);
        let b = solid(17, 16, 40);
        let diff = compare(&a, &b).unwrap_or_else(|err| panic!("{err}"));
        assert!(!diff.exact);
        assert_eq!(diff.max_channel_diff, 0);
        assert_eq!(diff.size, (17, 16));
    }

    #[test]
    fn a_large_size_difference_is_a_mismatch() {
        let a = solid(16, 16, 40);
        let b = solid(32, 16, 40);
        assert!(matches!(
            compare(&a, &b),
            Err(PixelError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn premultiplied_transparent_pixels_become_white() {
        let raster = Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
            premultiplied: true,
        };
        let image = over_white(&raster).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(image.rgba, [255, 255, 255, 255]);
    }

    #[test]
    fn straight_half_alpha_black_is_mid_gray() {
        let raster = Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 128],
            premultiplied: false,
        };
        let image = over_white(&raster).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(image.rgba[0], 127);
    }
}
