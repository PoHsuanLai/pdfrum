//! Dest-space reverse-map of a sheared image.
//!
//! An axis-aligned draw stretches onto an integer rect. A sheared one cannot:
//! the dest pixels that land inside the parallelogram are the integer AABB of
//! the unit square, reverse-mapped into a hypot-sized stretch of the source
//! and reconstructed with bilinear taps. Scanning the quad instead misses the
//! dest pixels that sit in the AABB but outside the rasterizer's coverage.
//!
//! Complements are against 256, so a constant region is an identity. Taking
//! them against 255 would darken every sample by two levels.

use kurbo::{Affine, Rect};

use crate::device::{ImageQuality, MAX_TARGET_DIMENSION};
use crate::path::{IntRect, closest_rect};
use crate::pixmap::Pixmap;

/// A sheared image already mapped onto its dest integer rect.
#[derive(Debug, Clone)]
pub struct MappedImage {
    /// Premultiplied RGBA; written pixels are opaque, the rest stay clear.
    pub pixels: Pixmap,
    /// Device x of the pixmap's top-left.
    pub left: i32,
    /// Device y of the pixmap's top-left.
    pub top: i32,
}

/// Edge lengths of the hypot-sized axis-aligned stretch, or `None` when either
/// is degenerate.
#[must_use]
pub(crate) fn hypot_size(unit_matrix: Affine) -> Option<(u32, u32)> {
    let [a, b, c, d, _, _] = unit_matrix.as_coeffs();
    Some((hypot_len(a, b)?, hypot_len(c, d)?))
}

/// Map `src` through a sheared unit-square matrix onto the closest dest rect.
///
/// `unit_matrix` takes the image's unit square to the device, not the sample
/// grid. `clip` is the device box the draw may touch. `pass1` is the quality
/// of the axis-aligned hypot stretch; the dest reverse-map is always bilinear.
#[must_use]
pub fn map_sheared(
    src: &Pixmap,
    unit_matrix: Affine,
    clip: IntRect,
    pass1: ImageQuality,
) -> Option<MappedImage> {
    let setup = Setup::new(unit_matrix, clip)?;
    let stretched = stretch_pixmap(src, setup.stretch_w, setup.stretch_h, pass1)?;
    let (width, height) = dest_dims(setup.result)?;
    let mut pixels = Pixmap::new(width, height);
    if pixels.width() == 0 || pixels.height() == 0 {
        return None;
    }
    setup.paint_color(&stretched, &mut pixels);
    Some(MappedImage {
        pixels,
        left: setup.result.left,
        top: setup.result.top,
    })
}

/// Reverse-map an 8-bit coverage plane through the same dest-space loop.
#[must_use]
pub fn map_sheared_coverage(
    plane: &[u8],
    width: u32,
    height: u32,
    unit_matrix: Affine,
    clip: IntRect,
    pass1: ImageQuality,
) -> Option<(crate::pixmap::AlphaMask, i32, i32)> {
    let mut src = Pixmap::new(width, height);
    let w = width as usize;
    for (i, &coverage) in plane.iter().enumerate() {
        let x = u32::try_from(i % w).ok()?;
        let y = u32::try_from(i / w).ok()?;
        src.set_pixel(x, y, [coverage, coverage, coverage, coverage]);
    }
    let mapped = map_sheared(&src, unit_matrix, clip, pass1)?;
    Some((
        mapped.pixels.alpha_mask(),
        mapped.left,
        mapped.top,
    ))
}

struct Setup {
    result: IntRect,
    stretch_w: u32,
    stretch_h: u32,
    dest_to_stretch: Affine,
}

impl Setup {
    fn new(unit_matrix: Affine, clip: IntRect) -> Option<Self> {
        let [sx, kx, ky, sy, tx, ty] = unit_matrix.as_coeffs();
        if ![sx, kx, ky, sy, tx, ty].iter().all(|v| v.is_finite()) {
            return None;
        }
        let stretch_w = hypot_len(sx, kx)?;
        let stretch_h = hypot_len(ky, sy)?;
        if stretch_w == 0 || stretch_h == 0 {
            return None;
        }
        if stretch_w > MAX_TARGET_DIMENSION || stretch_h > MAX_TARGET_DIMENSION {
            return None;
        }
        let unit = unit_matrix.transform_rect_bbox(Rect::new(0.0, 0.0, 1.0, 1.0));
        let result = closest_rect(unit).intersect(clip);
        if !result.is_valid() {
            return None;
        }
        let (out_w, out_h) = dest_dims(result)?;
        if out_w > MAX_TARGET_DIMENSION || out_h > MAX_TARGET_DIMENSION {
            return None;
        }
        let sw = f64::from(stretch_w);
        let sh = f64::from(stretch_h);
        let y_flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, sh]);
        let edge = Affine::new([sx / sw, kx / sw, ky / sh, sy / sh, tx, ty]);
        let dest_to_stretch = (edge * y_flip).inverse();
        if dest_to_stretch
            .as_coeffs()
            .iter()
            .any(|v| !v.is_finite())
        {
            return None;
        }
        Some(Self {
            result,
            stretch_w,
            stretch_h,
            dest_to_stretch,
        })
    }

    fn sample(&self, col: i32, row: i32) -> Option<Tap> {
        // Dest-bitmap (col, row) through the 8.8 matrix that already includes
        // the result origin, plus half a stretched pixel.
        let origin = Affine::translate((
            f64::from(self.result.left),
            f64::from(self.result.top),
        ));
        let [sx, kx, ky, sy, tx, ty] = (self.dest_to_stretch * origin).as_coeffs();
        let coeff = |v: f64| trunc_i32((v * 256.0).round());
        let fx = i64::from(coeff(sx)?) * i64::from(col)
            + i64::from(coeff(ky)?) * i64::from(row)
            + i64::from(coeff(tx)?)
            + 128;
        let fy = i64::from(coeff(kx)?) * i64::from(col)
            + i64::from(coeff(sy)?) * i64::from(row)
            + i64::from(coeff(ty)?)
            + 128;
        let src_col = i32::try_from(fx / 256).ok()?;
        let src_row = i32::try_from(fy / 256).ok()?;
        let mut res_x = i32::try_from(fx % 256).ok()?;
        let mut res_y = i32::try_from(fy % 256).ok()?;
        if res_x < 0 && res_x > -256 {
            res_x += 256;
        }
        if res_y < 0 && res_y > -256 {
            res_y += 256;
        }
        let w = i32::try_from(self.stretch_w).ok()?;
        let h = i32::try_from(self.stretch_h).ok()?;
        if src_col < 0 || src_col > w || src_row < 0 || src_row > h {
            return None;
        }
        Some(Tap {
            col: clamp_edge(src_col, w),
            row: clamp_edge(src_row, h),
            col_r: clamp_edge(src_col.saturating_add(1), w),
            row_r: clamp_edge(src_row.saturating_add(1), h),
            res_x,
            res_y,
        })
    }

    fn paint_color(&self, src: &Pixmap, dest: &mut Pixmap) {
        let h = dest.height();
        let w = dest.width();
        for row in 0..h {
            for col in 0..w {
                let Some(col_i) = i32::try_from(col).ok() else {
                    continue;
                };
                let Some(row_i) = i32::try_from(row).ok() else {
                    continue;
                };
                let Some(tap) = self.sample(col_i, row_i) else {
                    continue;
                };
                let px = bilinear_rgba(src, tap);
                dest.set_pixel(col, row, px);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Tap {
    col: i32,
    row: i32,
    col_r: i32,
    row_r: i32,
    res_x: i32,
    res_y: i32,
}

fn hypot_len(x: f64, y: f64) -> Option<u32> {
    let h = x.hypot(y).ceil();
    if !h.is_finite() || h < 1.0 {
        return None;
    }
    if h > f64::from(MAX_TARGET_DIMENSION) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 1..=MAX_TARGET_DIMENSION"
    )]
    Some(h as u32)
}

fn trunc_i32(v: f64) -> Option<i32> {
    if !v.is_finite() {
        return None;
    }
    let t = v.trunc();
    if t < f64::from(i32::MIN) || t > f64::from(i32::MAX) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the range check keeps the value inside i32"
    )]
    Some(t as i32)
}

fn clamp_edge(v: i32, len: i32) -> i32 {
    if v == len {
        v.saturating_sub(1)
    } else {
        v.clamp(0, len.saturating_sub(1))
    }
}

fn bilinear_rgba(src: &Pixmap, tap: Tap) -> [u8; 4] {
    let p00 = texel(src, tap.col, tap.row);
    let p10 = texel(src, tap.col_r, tap.row);
    let p01 = texel(src, tap.col, tap.row_r);
    let p11 = texel(src, tap.col_r, tap.row_r);
    let inv_x = 256 - tap.res_x;
    let inv_y = 256 - tap.res_y;
    let mut out = [0_u8; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let h0 = (i32::from(p00.get(i).copied().unwrap_or(0)) * inv_x
            + i32::from(p10.get(i).copied().unwrap_or(0)) * tap.res_x)
            >> 8;
        let h1 = (i32::from(p01.get(i).copied().unwrap_or(0)) * inv_x
            + i32::from(p11.get(i).copied().unwrap_or(0)) * tap.res_x)
            >> 8;
        let v = (h0 * inv_y + h1 * tap.res_y) >> 8;
        *slot = u8::try_from(v.clamp(0, 255)).unwrap_or(0);
    }
    // An opaque source writes A = 255 wherever the inverse hit, matching a
    // dest-space loop that does not antialias the parallelogram.
    if p00[3] == 255 && p10[3] == 255 && p01[3] == 255 && p11[3] == 255 {
        out[3] = 255;
    }
    out
}

fn texel(src: &Pixmap, x: i32, y: i32) -> [u8; 4] {
    let x = u32::try_from(x.max(0)).unwrap_or(0).min(src.width().saturating_sub(1));
    let y = u32::try_from(y.max(0)).unwrap_or(0).min(src.height().saturating_sub(1));
    src.pixel(x, y).unwrap_or([0; 4])
}

fn stretch_pixmap(src: &Pixmap, dw: u32, dh: u32, quality: ImageQuality) -> Option<Pixmap> {
    if dw == 0 || dh == 0 {
        return None;
    }
    if dw == src.width() && dh == src.height() {
        return Some(src.clone());
    }
    if dw <= src.width() && dh <= src.height() {
        return Some(crate::stretch::reduce_to(src, dw, dh));
    }
    Some(resample_pixmap(src, dw, dh, quality))
}

fn resample_pixmap(src: &Pixmap, dw: u32, dh: u32, quality: ImageQuality) -> Pixmap {
    let mut out = Pixmap::new(dw, dh);
    let sw = src.width();
    let sh = src.height();
    if sw == 0 || sh == 0 {
        return out;
    }
    for y in 0..dh {
        for x in 0..dw {
            let px = match quality {
                ImageQuality::Nearest => {
                    let sx = scale_index(x, sw, dw);
                    let sy = scale_index(y, sh, dh);
                    src.pixel(sx, sy).unwrap_or([0; 4])
                }
                ImageQuality::Bilinear => {
                    let tap = enlarge_tap(x, y, sw, sh, dw, dh);
                    bilinear_rgba(src, tap)
                }
            };
            out.set_pixel(x, y, px);
        }
    }
    out
}

fn dest_dims(rect: IntRect) -> Option<(u32, u32)> {
    Some((
        u32::try_from(rect.width()).ok()?,
        u32::try_from(rect.height()).ok()?,
    ))
}

fn scale_index(pos: u32, src: u32, dest: u32) -> u32 {
    if dest == 0 || src == 0 {
        return 0;
    }
    let n = (u64::from(pos) * u64::from(src)) / u64::from(dest);
    u32::try_from(n).unwrap_or(u32::MAX).min(src.saturating_sub(1))
}

fn enlarge_tap(x: u32, y: u32, sw: u32, sh: u32, dw: u32, dh: u32) -> Tap {
    let sx = (f64::from(x) + 0.5) * f64::from(sw) / f64::from(dw) - 0.5;
    let sy = (f64::from(y) + 0.5) * f64::from(sh) / f64::from(dh) - 0.5;
    let col = trunc_i32(sx.floor()).unwrap_or(0);
    let row = trunc_i32(sy.floor()).unwrap_or(0);
    let res_x = trunc_i32((sx - sx.floor()) * 256.0).unwrap_or(0).clamp(0, 255);
    let res_y = trunc_i32((sy - sy.floor()) * 256.0).unwrap_or(0).clamp(0, 255);
    let w = i32::try_from(sw).unwrap_or(0);
    let h = i32::try_from(sh).unwrap_or(0);
    Tap {
        col: col.clamp(0, w.saturating_sub(1)),
        row: row.clamp(0, h.saturating_sub(1)),
        col_r: (col + 1).clamp(0, w.saturating_sub(1)),
        row_r: (row + 1).clamp(0, h.saturating_sub(1)),
        res_x: res_x.clamp(0, 255),
        res_y: res_y.clamp(0, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::takes_other_transform;
    use crate::path::outer_rect;

    fn solid(w: u32, h: u32, v: u8) -> Pixmap {
        let mut p = Pixmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                p.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        p
    }

    #[test]
    fn a_constant_region_is_an_identity() {
        let src = solid(4, 4, 153);
        let m = Affine::new([16.0, 64.0, 64.0, 16.0, 20.0, 120.0]);
        assert!(takes_other_transform(m));
        let mapped = map_sheared(
            &src,
            m,
            outer_rect(Rect::new(0.0, 0.0, 200.0, 200.0)),
            ImageQuality::Nearest,
        )
        .expect("mapped");
        // Interior of a constant block must be the source byte, not v-2.
        let mut saw = false;
        for y in 2..mapped.pixels.height().saturating_sub(2) {
            for x in 2..mapped.pixels.width().saturating_sub(2) {
                if let Some([r, g, b, a]) = mapped.pixels.pixel(x, y)
                    && a == 255
                {
                    assert_eq!([r, g, b], [153, 153, 153], "at {x},{y}");
                    saw = true;
                }
            }
        }
        assert!(saw, "expected opaque interior samples");
    }

    #[test]
    fn dest_pixels_outside_the_parallelogram_stay_clear() {
        let src = solid(2, 2, 200);
        let m = Affine::new([40.0, 5.0, 0.0, 60.0, 10.0, 10.0]);
        let mapped = map_sheared(
            &src,
            m,
            outer_rect(Rect::new(0.0, 0.0, 80.0, 80.0)),
            ImageQuality::Nearest,
        )
        .expect("mapped");
        let mut opaque = 0u32;
        let mut clear = 0u32;
        for y in 0..mapped.pixels.height() {
            for x in 0..mapped.pixels.width() {
                match mapped.pixels.pixel(x, y) {
                    Some([_, _, _, 0]) => clear += 1,
                    Some([_, _, _, 255]) => opaque += 1,
                    _ => {}
                }
            }
        }
        assert!(opaque > 0, "some dest pixels must hit the source");
        assert!(clear > 0, "AABB corners that miss the source stay clear");
    }

    #[test]
    fn a_quarter_turn_does_not_take_this_path() {
        assert!(!takes_other_transform(Affine::new([
            0.0, 10.0, -10.0, 0.0, 0.0, 0.0
        ])));
    }
}
