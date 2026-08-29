//! `--png` and `--md5`: rendering a page to the file and the stdout line the
//! oracle produces.
//!
//! Three conventions have to match exactly for the harness to diff
//! like-for-like, and none of them is the obvious choice:
//!
//! - **The PNG is RGB, not RGBA**, for a page without transparency. The
//!   oracle asks `png_codec::EncodeBGRA` to discard transparency whenever the
//!   page reports none, and the encoder then drops the fourth channel
//!   (`png_codec_libpng.cpp:620-626`).
//! - **The MD5 is over the raw bitmap buffer, not the PNG.** It hashes
//!   `stride * height` bytes straight out of `FPDFBitmap_GetBuffer`
//!   (`pdfium_test.cc:1024-1038`), which is 32-bit **BGRA** — and for an
//!   opaque page that is `FPDFBitmap_BGRx`, whose fourth byte is padding the
//!   `FillRect(0xFFFFFFFF)` clear leaves at `0xFF`.
//! - **`--reverse-byte-order` is off by default**, so the buffer really is
//!   BGRA rather than RGBA.
//!
//! The stdout line is `MD5:<path>:<hex>` (`pdfium_test.cc:311`), where the
//! path is the file just written, and the file name is
//! `<input>.<page>.png`.

use std::path::{Path, PathBuf};

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::Resolve;
use pdfrum_page::BuildContext;
use pdfrum_parser::PageDict;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_raster_vello::VelloBackend;
use pdfrum_render::{Pixmap, RenderOptions, needs_alpha_background, render_page};

/// The default rendering scale, in device pixels per PDF point.
///
/// `pdfium_test` renders at `scale = 1.0` unless `--scale` says otherwise
/// (`pdfium_test.cc`'s `static_cast<int>(FPDF_GetPageWidth(page) * scale)`),
/// so a 612x792 page is a 612x792 bitmap.
pub const DEFAULT_SCALE: f64 = 1.0;

/// The file a page's PNG is written to: `<input>.<page>.png`, beside the
/// input, which is where the harness harvests it from.
#[must_use]
pub fn output_path(input: &Path, index: u32) -> Option<PathBuf> {
    let name = input.file_name()?.to_str()?;
    let file = format!("{name}.{index}.png");
    // The oracle refuses a name of 256 bytes or more rather than truncating.
    if file.len() >= 256 {
        return None;
    }
    Some(input.with_file_name(file))
}

/// What one rendered page produced.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The PNG bytes, ready to write.
    pub png: Vec<u8>,
    /// The MD5 of the *raw bitmap buffer*, hex-encoded lower case.
    pub digest: String,
}

/// The environment variable that selects a rasterizer.
///
/// Deliberately **not** a command-line flag: the tool's flag surface is
/// compared against the oracle's, and a flag `pdfium_test` does not have
/// would be one more thing to keep in step for no benefit. Tier C is our own
/// concern, so it travels out of band.
pub const BACKEND_ENV: &str = "PDFRUM_BACKEND";

/// Which rasterizer a render uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// `tiny-skia` — the default, and the determinism baseline.
    #[default]
    TinySkia,
    /// `vello_cpu`, at its pinned SIMD level and render mode.
    Vello,
}

impl Backend {
    /// The backend `PDFRUM_BACKEND` selects, defaulting to tiny-skia.
    ///
    /// An unrecognised value is the default rather than an error: this is a
    /// developer-facing knob, and a typo should not change what a
    /// conformance run means.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var(BACKEND_ENV).as_deref() {
            Ok("vello" | "vello_cpu") => Self::Vello,
            _ => Self::TinySkia,
        }
    }
}

/// Render one page and encode it the way the oracle does.
///
/// Returns `None` when the page has no renderable size, which the oracle
/// also skips — a page still counts as processed either way.
#[must_use]
pub fn render<R: Resolve>(
    page: &PageDict,
    r: &R,
    scale: f64,
    ctx: &mut BuildContext,
) -> Option<Rendered> {
    let limits = Limits::default();
    let mut build_diags = Diagnostics::default();
    let page = crate::content::build(page, r, ctx, &limits, &mut build_diags);
    let opts = RenderOptions {
        transform: kurbo::Affine::scale(scale),
        ..RenderOptions::default()
    };
    let mut diags = Diagnostics::default();
    // tiny-skia is the default: it is the determinism baseline, and a tool
    // whose output moved with the host's SIMD level would make every
    // conformance comparison unreproducible. `vello_cpu` pins its own level
    // so it is reproducible too, and Tier C diffs the two.
    let pixmap = match Backend::from_env() {
        Backend::TinySkia => render_page(&page, &opts, &TinySkiaBackend::new(), &mut diags).ok()?,
        Backend::Vello => render_page(&page, &opts, &VelloBackend::new(), &mut diags).ok()?,
    };
    // `FPDFPage_HasTransparency` is not the page's `/Group`: it is set only
    // by a blend mode above Multiply. The engine exposes the same predicate
    // so the output encoding and the background clear cannot disagree.
    Some(encode(&pixmap, needs_alpha_background(&page)))
}

/// Encode a rendered pixmap as the oracle's PNG and MD5.
#[must_use]
pub fn encode(pixmap: &Pixmap, has_transparency: bool) -> Rendered {
    // The hash is over the 32-bit BGRA buffer — BGRx for an opaque page,
    // whose padding byte the white clear leaves at 0xFF.
    let buffer = pixmap.to_straight_bgra(!has_transparency);
    let digest = md5_hex(&buffer);
    let png = if has_transparency {
        encode_png(
            pixmap.width(),
            pixmap.height(),
            &pixmap.to_straight_rgba(),
            png::ColorType::Rgba,
        )
    } else {
        encode_png(
            pixmap.width(),
            pixmap.height(),
            &pixmap.to_straight_rgb(),
            png::ColorType::Rgb,
        )
    };
    Rendered { png, digest }
}

fn encode_png(width: u32, height: u32, data: &[u8], color: png::ColorType) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let Ok(mut writer) = encoder.write_header() else {
            return Vec::new();
        };
        if writer.write_image_data(data).is_err() {
            return Vec::new();
        }
    }
    out
}

fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(32), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

/// The stdout line `--md5` prints for one written file
/// (`pdfium_test.cc:311`).
#[must_use]
pub fn md5_line(path: &Path, digest: &str) -> String {
    format!("MD5:{}:{digest}\n", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_paths_follow_the_oracle_naming() {
        let path = output_path(Path::new("/tmp/input.pdf"), 3).expect("named");
        assert_eq!(path, PathBuf::from("/tmp/input.pdf.3.png"));
    }

    #[test]
    fn an_over_long_name_is_refused_rather_than_truncated() {
        let long = "x".repeat(300);
        assert!(output_path(Path::new(&long), 0).is_none());
    }

    #[test]
    fn the_md5_is_over_the_raw_bgra_buffer_not_the_png() {
        // A 2x1 opaque white pixmap hashes as eight 0xFF bytes: four per
        // pixel, the fourth being BGRx's padding, which the white clear
        // leaves at 0xFF.
        let pixmap = Pixmap::filled(2, 1, peniko::Color::WHITE);
        let out = encode(&pixmap, false);
        assert_eq!(out.digest, md5_hex(&[0xFF; 8]));
        // And it is *not* the hash of the PNG file.
        assert_ne!(out.digest, md5_hex(&out.png));
    }

    #[test]
    fn an_opaque_page_encodes_as_rgb() {
        let pixmap = Pixmap::filled(4, 3, peniko::Color::from_rgba8(1, 2, 3, 255));
        let out = encode(&pixmap, false);
        // IHDR colour type 2 is RGB; 6 would be RGBA.
        assert_eq!(
            out.png.get(25).copied(),
            Some(2),
            "opaque pages drop the alpha channel"
        );
    }

    #[test]
    fn a_transparent_page_encodes_as_rgba() {
        let pixmap = Pixmap::new(4, 3);
        let out = encode(&pixmap, true);
        assert_eq!(out.png.get(25).copied(), Some(6));
    }

    #[test]
    fn the_md5_line_is_the_oracle_format() {
        assert_eq!(
            md5_line(Path::new("input.pdf.0.png"), "deadbeef"),
            "MD5:input.pdf.0.png:deadbeef\n"
        );
    }

    #[test]
    fn md5_hex_is_lower_case_and_32_wide() {
        let digest = md5_hex(b"");
        assert_eq!(digest, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(digest.len(), 32);
    }
}
