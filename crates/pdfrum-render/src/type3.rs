//! Uncoloured Type 3 glyphs whose procedure is a single stencil image.
//!
//! Those glyphs are a coverage mask, not a form: stretch the 1-bit source to
//! an integer device size, then blit it 1:1 in the outer fill. The source is
//! already y-down (row 0 is the top of the image). A negative dest height
//! flips that axis because the mapped box is upside down, not because PDF
//! image space needs a second conversion.
//!
//! `[oracle-bug]` The screen blit scales coverage by the fill's alpha when
//! merging glyphs, then again when tinting the merged mask. ISO is one
//! factor; the goldens are two.

use std::collections::HashMap;

use kurbo::Affine;
use pdfrum_font::FontId;
use pdfrum_page::{ImageData, ImageObject, Pixels, Samples};

use crate::color::Argb;
use crate::device::{ImageQuality, MAX_TARGET_DIMENSION, RasterBackend, RenderDevice};
use crate::image::{mask_pixmap, to_pixmap, use_interpolate_bilinear};
use crate::pixmap::{AlphaMask, Pixmap, alpha_union, mul255};

/// Type 3 glyphs snap their top and bottom to previously used scanlines
/// within this many pixels, so a run at one size shares a baseline.
const BLUE_SNAP: f64 = 0.8;

/// Cap on remembered baselines per size. Further glyphs still snap to the
/// ones already kept; they just do not add new ones.
const MAX_BLUES: usize = 16;

/// A scan with no byte above this is empty. Faint coverage does not count
/// as ink, so a padded stencil does not take the integer-stretch path.
const INK: u8 = 0x40;

/// Baselines remembered per font and linear device matrix.
#[derive(Debug, Default)]
pub(crate) struct BlueCache {
    sizes: HashMap<(FontId, [i32; 4]), Blues>,
}

impl BlueCache {
    pub(crate) fn for_font_matrix(&mut self, font: FontId, matrix: Affine) -> &mut Blues {
        self.sizes.entry((font, size_key(matrix))).or_default()
    }
}

/// One size's top and bottom baselines.
#[derive(Debug, Default)]
pub(crate) struct Blues {
    top: Vec<i32>,
    bottom: Vec<i32>,
}

impl Blues {
    fn snap(&mut self, top: f64, bottom: f64) -> (i32, i32) {
        (
            snap_one(top, &mut self.top),
            snap_one(bottom, &mut self.bottom),
        )
    }
}

/// A stencil stretched to an integer device size, offset from the char origin.
#[derive(Debug, Clone)]
pub(crate) struct Stretched {
    mask: AlphaMask,
    dx: i32,
    dy: i32,
}

impl Stretched {
    /// Place at the char's rounded translation.
    #[must_use]
    pub(crate) fn place(self, char_to_device: Affine) -> Option<PlacedMask> {
        let [_, _, _, _, e, f] = char_to_device.as_coeffs();
        Some(PlacedMask {
            mask: self.mask,
            x: round_i32(e)?.checked_add(self.dx)?,
            y: round_i32(f)?.checked_add(self.dy)?,
        })
    }
}

/// A glyph mask whose top-left is already in device pixels.
#[derive(Debug, Clone)]
pub(crate) struct PlacedMask {
    mask: AlphaMask,
    x: i32,
    y: i32,
}

/// Stretch a sole-image stencil to an integer device size.
///
/// `None` when this path does not apply: the image is sheared, the source
/// has empty padding rows, or a destination dimension is zero or overflows.
/// The caller then draws the image as an image.
#[must_use]
pub(crate) fn try_stretch<B: RasterBackend>(
    backend: &B,
    image: &ImageObject,
    char_to_device: Affine,
    blues: &mut Blues,
) -> Option<Stretched> {
    let [a, b, c, d, _, _] = char_to_device.as_coeffs();
    // Drop the char translation so dest size comes from the image's own box.
    // kurbo applies the right-hand matrix first.
    let image_matrix = Affine::new([a, b, c, d, 0.0, 0.0]) * image.matrix;
    if !nearly_axis_aligned(image_matrix) {
        return None;
    }
    let coverage = coverage_of(&image.image);
    if coverage.width() == 0 || coverage.height() == 0 {
        return None;
    }
    let (first, last) = ink_range(&coverage)?;
    if first != 0 || last != coverage.height().saturating_sub(1) {
        return None;
    }
    let dest = dest_rect(image_matrix, blues)?;
    let mask = stretch_mask(backend, &coverage, dest.width, dest.height)?;
    Some(Stretched {
        mask,
        dx: dest.dx,
        dy: dest.dy,
    })
}

/// Merge `glyphs` into one mask and blit it in `fill`.
///
/// `[oracle-bug]` Coverage is scaled by `fill.a` on the merge and again on
/// the tint. ISO would tint once.
pub(crate) fn blit_batch<D: RenderDevice>(device: &mut D, glyphs: &[PlacedMask], fill: Argb) {
    if glyphs.is_empty() || fill.is_invisible() {
        return;
    }
    let Some(bounds) = bounds_of(glyphs) else {
        return;
    };
    let mut combined = AlphaMask::new(bounds.width, bounds.height);
    for glyph in glyphs {
        let dx = glyph.x.saturating_sub(bounds.left);
        let dy = glyph.y.saturating_sub(bounds.top);
        merge_coverage(&mut combined, &glyph.mask, dx, dy, fill.a);
    }
    let tinted = tint(&combined, fill);
    if tinted.width() == 0 || tinted.height() == 0 {
        return;
    }
    device.draw_image(
        &tinted,
        Affine::translate((f64::from(bounds.left), f64::from(bounds.top))),
        ImageQuality::Nearest,
        1.0,
    );
}

struct DestRect {
    width: i32,
    height: i32,
    dx: i32,
    dy: i32,
}

struct Bounds {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

/// Integer dest size and offset from the char origin.
///
/// Width is the truncated x-scale. Height is the snapped distance between
/// the image's top and bottom in device space; a negative value flips the
/// samples. The offset is that top-left, not a second y-up/y-down conversion.
fn dest_rect(image_matrix: Affine, blues: &mut Blues) -> Option<DestRect> {
    let [a, _, _, d, e, f] = image_matrix.as_coeffs();
    let mut top = d + f;
    let mut bottom = f;
    let flipped = top > bottom;
    if flipped {
        std::mem::swap(&mut top, &mut bottom);
    }
    let (top_line, bottom_line) = blues.snap(top, bottom);
    let height = if flipped {
        top_line.checked_sub(bottom_line)?
    } else {
        bottom_line.checked_sub(top_line)?
    };
    let width = trunc_i32(a)?;
    if width == 0 || height == 0 {
        return None;
    }
    let dx = if a < 0.0 {
        round_i32(e + a)?
    } else {
        round_i32(e)?
    };
    Some(DestRect {
        width,
        height,
        dx,
        dy: top_line,
    })
}

fn nearly_axis_aligned(matrix: Affine) -> bool {
    let [sx, kx, ky, sy, _, _] = matrix.as_coeffs();
    kx.abs() < sx.abs() / 100.0 && ky.abs() < sy.abs() / 100.0
}

fn coverage_of(image: &ImageData) -> AlphaMask {
    if let Samples::Whole(Pixels::Stencil(bits)) = &image.samples {
        let len = (bits.width as usize).saturating_mul(bits.height as usize);
        let mut data = vec![0_u8; len];
        for y in 0..bits.height {
            for x in 0..bits.width {
                if !bits.pixel(x, y) {
                    continue;
                }
                let index = (y as usize)
                    .saturating_mul(bits.width as usize)
                    .saturating_add(x as usize);
                if let Some(slot) = data.get_mut(index) {
                    *slot = 255;
                }
            }
        }
        return AlphaMask::from_vec(bits.width, bits.height, data)
            .unwrap_or_else(|| AlphaMask::new(0, 0));
    }
    to_pixmap(image, Argb::opaque(255, 255, 255), None).alpha_mask()
}

fn ink_range(mask: &AlphaMask) -> Option<(u32, u32)> {
    let width = mask.width() as usize;
    if width == 0 || mask.height() == 0 {
        return None;
    }
    let mut first = None;
    let mut last = None;
    for (y, row) in mask.data().chunks(width).enumerate() {
        if row.iter().any(|&b| b > INK) {
            let y = u32::try_from(y).ok()?;
            if first.is_none() {
                first = Some(y);
            }
            last = Some(y);
        }
    }
    Some((first?, last?))
}

/// Stretch coverage to `width` × `height`. A negative axis flips it.
fn stretch_mask<B: RasterBackend>(
    backend: &B,
    src: &AlphaMask,
    width: i32,
    height: i32,
) -> Option<AlphaMask> {
    let src_w = src.width();
    let src_h = src.height();
    if width == i32::try_from(src_w).ok()? && height == i32::try_from(src_h).ok()? {
        return Some(src.clone());
    }
    let dw = width.unsigned_abs();
    let dh = height.unsigned_abs();
    if dw == 0 || dh == 0 || dw > MAX_TARGET_DIMENSION || dh > MAX_TARGET_DIMENSION {
        return None;
    }
    let quality = if use_interpolate_bilinear(
        false,
        false,
        src_w,
        src_h,
        i64::from(width),
        i64::from(height),
    ) {
        ImageQuality::Bilinear
    } else {
        ImageQuality::Nearest
    };
    let pixmap = mask_pixmap(src.data(), src_w, src_h);
    let sx = f64::from(width) / f64::from(src_w);
    let sy = f64::from(height) / f64::from(src_h);
    let tx = if width < 0 { f64::from(dw) } else { 0.0 };
    let ty = if height < 0 { f64::from(dh) } else { 0.0 };
    let mut target = backend.new_target(dw, dh, peniko::Color::TRANSPARENT);
    target.draw_image(
        &pixmap,
        Affine::new([sx, 0.0, 0.0, sy, tx, ty]),
        quality,
        1.0,
    );
    Some(backend.finish(target).alpha_mask())
}

/// Merge `src` into `dest` at `(dx, dy)`: coverage scaled by `fill_a`, then
/// unioned with what is already there.
fn merge_coverage(dest: &mut AlphaMask, src: &AlphaMask, dx: i32, dy: i32, fill_a: u8) {
    let dw = dest.width();
    let dh = dest.height();
    let sw = src.width();
    for row in 0..src.height() {
        let Some(out_y) = in_range(i64::from(row) + i64::from(dy), dh) else {
            continue;
        };
        for col in 0..src.width() {
            let Some(out_x) = in_range(i64::from(col) + i64::from(dx), dw) else {
                continue;
            };
            let src_i = (row as usize)
                .saturating_mul(sw as usize)
                .saturating_add(col as usize);
            let dst_i = out_y.saturating_mul(dw as usize).saturating_add(out_x);
            let (Some(&coverage), Some(slot)) =
                (src.data().get(src_i), dest.data_mut().get_mut(dst_i))
            else {
                continue;
            };
            let src_alpha = mul255(coverage, fill_a);
            if src_alpha == 0 {
                continue;
            }
            *slot = if *slot == 0 {
                src_alpha
            } else {
                alpha_union(*slot, src_alpha)
            };
        }
    }
}

fn tint(mask: &AlphaMask, fill: Argb) -> Pixmap {
    let mut out = Pixmap::new(mask.width(), mask.height());
    for (slot, &coverage) in out
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(mask.data())
    {
        let a = mul255(coverage, fill.a);
        *slot = [mul255(fill.r, a), mul255(fill.g, a), mul255(fill.b, a), a];
    }
    out
}

fn bounds_of(glyphs: &[PlacedMask]) -> Option<Bounds> {
    let mut left = i32::MAX;
    let mut top = i32::MAX;
    let mut right = i32::MIN;
    let mut bottom = i32::MIN;
    for glyph in glyphs {
        let w = i32::try_from(glyph.mask.width()).ok()?;
        let h = i32::try_from(glyph.mask.height()).ok()?;
        left = left.min(glyph.x);
        top = top.min(glyph.y);
        right = right.max(glyph.x.checked_add(w)?);
        bottom = bottom.max(glyph.y.checked_add(h)?);
    }
    let width = u32::try_from(right.checked_sub(left)?).ok()?;
    let height = u32::try_from(bottom.checked_sub(top)?).ok()?;
    if width == 0 || height == 0 || width > MAX_TARGET_DIMENSION || height > MAX_TARGET_DIMENSION {
        return None;
    }
    Some(Bounds {
        left,
        top,
        width,
        height,
    })
}

fn size_key(matrix: Affine) -> [i32; 4] {
    let [a, b, c, d, _, _] = matrix.as_coeffs();
    [
        round_i32(a * 10_000.0).unwrap_or(0),
        round_i32(b * 10_000.0).unwrap_or(0),
        round_i32(c * 10_000.0).unwrap_or(0),
        round_i32(d * 10_000.0).unwrap_or(0),
    ]
}

fn snap_one(pos: f64, blues: &mut Vec<i32>) -> i32 {
    let mut closest = None;
    let mut min_distance = f64::INFINITY;
    for (i, &blue) in blues.iter().enumerate() {
        let distance = (pos - f64::from(blue)).abs();
        if distance < min_distance && distance < BLUE_SNAP {
            min_distance = distance;
            closest = Some(i);
        }
    }
    if let Some(i) = closest {
        return blues.get(i).copied().unwrap_or(0);
    }
    let new_pos = round_i32(pos).unwrap_or(0);
    if blues.len() < MAX_BLUES {
        blues.push(new_pos);
    }
    new_pos
}

fn in_range(v: i64, limit: u32) -> Option<usize> {
    (v >= 0 && v < i64::from(limit))
        .then(|| usize::try_from(v).ok())
        .flatten()
}

fn round_i32(v: f64) -> Option<i32> {
    f64_to_i32(v, v.round())
}

fn trunc_i32(v: f64) -> Option<i32> {
    f64_to_i32(v, v.trunc())
}

fn f64_to_i32(src: f64, converted: f64) -> Option<i32> {
    if !src.is_finite() {
        return None;
    }
    if converted < f64::from(i32::MIN) || converted > f64::from(i32::MAX) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the range check above keeps the value inside i32"
    )]
    Some(converted as i32)
}

#[cfg(test)]
mod tests {
    use pdfrum_page::BitImage;

    use super::*;

    fn stencil(width: u32, height: u32, bits: Vec<u8>) -> ImageData {
        ImageData {
            width,
            height,
            samples: Samples::Whole(Pixels::Stencil(BitImage {
                width,
                height,
                row_bytes: width.div_ceil(8) as usize,
                bits,
            })),
            mask: None,
            matte: None,
            interpolate: false,
            family: pdfrum_page::Family::Unknown,
        }
    }

    #[test]
    fn a_set_bit_is_full_coverage() {
        let img = stencil(8, 1, vec![0b1000_0000]);
        assert_eq!(coverage_of(&img).data(), &[255, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn baselines_snap_within_eight_tenths() {
        let mut blues = Blues::default();
        assert_eq!(blues.snap(10.4, 0.0), (10, 0));
        // 10.7 is 0.7 from 10, inside 0.8, so it snaps rather than becoming 11.
        assert_eq!(blues.snap(10.7, 0.2), (10, 0));
        assert_eq!(blues.snap(12.0, 0.0).0, 12);
    }

    #[test]
    fn the_batch_applies_fill_alpha_twice() {
        let mask = AlphaMask::filled(1, 1, 255);
        let mut combined = AlphaMask::new(1, 1);
        merge_coverage(&mut combined, &mask, 0, 0, 128);
        assert_eq!(combined.data(), &[128]);
        let tinted = tint(
            &combined,
            Argb {
                a: 128,
                r: 255,
                g: 0,
                b: 0,
            },
        );
        assert_eq!(tinted.pixel(0, 0), Some([64, 0, 0, 64]));
    }

    #[test]
    fn place_adds_the_offset_to_the_rounded_origin() {
        let glyph = Stretched {
            mask: AlphaMask::filled(2, 2, 255),
            dx: 3,
            dy: 5,
        };
        let placed = glyph
            .place(Affine::translate((10.4, 20.4)))
            .expect("in range");
        assert_eq!((placed.x, placed.y), (13, 25));
    }

    #[test]
    fn dest_width_truncates_toward_zero() {
        let mut blues = Blues::default();
        // y-down: top = d+f = -8, bottom = 0, not flipped.
        let m = Affine::new([12.9, 0.0, 0.0, -8.0, 0.0, 0.0]);
        let dest = dest_rect(m, &mut blues).expect("axis-aligned");
        assert_eq!(dest.width, 12);
        assert_eq!(dest.height, 8);
        assert_eq!(dest.dx, 0);
        assert_eq!(dest.dy, -8);
    }

    #[test]
    fn a_negative_x_scale_offsets_left_by_that_scale() {
        let mut blues = Blues::default();
        let m = Affine::new([-10.0, 0.0, 0.0, 10.0, 3.0, 0.0]);
        let dest = dest_rect(m, &mut blues).expect("axis-aligned");
        assert_eq!(dest.dx, -7);
        assert_eq!(dest.width, -10);
    }

    #[test]
    fn shear_does_not_take_the_integer_stretch() {
        assert!(!nearly_axis_aligned(Affine::new([
            10.0, 1.0, 0.0, 10.0, 0.0, 0.0
        ])));
        assert!(nearly_axis_aligned(Affine::new([
            10.0, 0.0, 0.0, -8.0, 0.0, 0.0
        ])));
    }

    #[test]
    fn overlapping_glyphs_union_coverage() {
        let mut dest = AlphaMask::new(2, 1);
        let src = AlphaMask::filled(1, 1, 255);
        merge_coverage(&mut dest, &src, 0, 0, 255);
        merge_coverage(&mut dest, &src, 1, 0, 255);
        assert_eq!(dest.data(), &[255, 255]);
        merge_coverage(&mut dest, &src, 0, 0, 128);
        assert_eq!(dest.data().first().copied(), Some(alpha_union(255, 128)));
    }
}
