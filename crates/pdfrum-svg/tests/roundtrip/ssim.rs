//! The board's SSIM, so the round trip's numbers mean the same thing the
//! conformance scoreboard's do.
//!
//! Transcribed from `conformance/src/ssim.rs` rather than called: the
//! conformance harness is a binary with no library target, and the metric is
//! deliberately hand-rolled in both places so a dependency bump can never
//! silently move a published number. The formula is Wang et al. 2004 over a
//! sliding 8x8 box window with `K1 = 0.01`, `K2 = 0.03`, `L = 255`, sample
//! (`N - 1`) normalisation, on a Rec. 601 luma projection composited over
//! white.

/// Window edge length in pixels.
const WINDOW: usize = 8;
/// Stabilizer constants, on the standard 8-bit dynamic range `L = 255`.
const K1: f64 = 0.01;
const K2: f64 = 0.03;
const L: f64 = 255.0;

/// A decoded 8-bit RGBA image.
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Exactly `width * height * 4` bytes, RGBA order, unpremultiplied.
    pub rgba: Vec<u8>,
}

/// Decode a PNG into RGBA8, or `None` when it will not decode.
///
/// `resvg`'s own `tiny-skia`, which is the decoder the round trip already
/// depends on. It premultiplies, so the alpha is divided back out — the
/// goldens are opaque page renders, where that is the identity.
pub fn decode_png(bytes: &[u8]) -> Option<Image> {
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(bytes).ok()?;
    let (width, height) = (pixmap.width(), pixmap.height());
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let a = px.alpha();
        let un = |c: u8| {
            if a == 0 {
                0
            } else {
                u8::try_from((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).unwrap_or(255)
            }
        };
        rgba.extend_from_slice(&[un(px.red()), un(px.green()), un(px.blue()), a]);
    }
    Some(Image {
        width,
        height,
        rgba,
    })
}

/// Mean SSIM between two same-sized images, or `None` when they differ in
/// size or either buffer is malformed.
pub fn compare(a: &Image, b: &Image) -> Option<f64> {
    let expected = (a.width as usize) * (a.height as usize) * 4;
    if a.width != b.width
        || a.height != b.height
        || a.rgba.len() != expected
        || b.rgba.len() != expected
    {
        return None;
    }
    if a.rgba == b.rgba {
        return Some(1.0);
    }
    Some(mean_ssim(
        &to_gray(a),
        &to_gray(b),
        a.width as usize,
        a.height as usize,
    ))
}

/// Rec. 601 luma over a white background, matching the board's projection.
fn to_gray(image: &Image) -> Vec<f64> {
    image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[r, g, b, a]| {
            let alpha = f64::from(a) / 255.0;
            let over_white = |c: u8| f64::from(c) * alpha + 255.0 * (1.0 - alpha);
            0.299 * over_white(r) + 0.587 * over_white(g) + 0.114 * over_white(b)
        })
        .collect()
}

/// Mean SSIM over every full window position.
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

#[allow(clippy::too_many_arguments)]
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
    // A window is at most 8x8, so the count is exact in `f64`.
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
