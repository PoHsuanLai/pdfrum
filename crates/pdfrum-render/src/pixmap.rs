//! The two raster buffers the engine and the backends share: [`Pixmap`], a
//! premultiplied RGBA8 image, and [`AlphaMask`], an 8-bit coverage plane.
//!
//! **Part of the backend seam.** Both are in
//! [`RasterBackend`](crate::RasterBackend)'s own signatures; the alpha
//! arithmetic beside them — [`mul255`], [`alpha_merge`],
//! [`alpha_byte_truncating`] — is here because a backend must round its
//! alpha the way the oracle rounds it. The oracle's own buffers are
//! *straight* (non-premultiplied) and ours are premultiplied, and the
//! conversions that forces are owned here rather than in a backend, so the
//! engine's arithmetic stays bit-identical whichever rasterizer is in use.

use crate::color::rgb_to_gray;

/// A premultiplied RGBA8 image: four bytes per pixel, row-major, no padding.
///
/// Premultiplied means each colour byte has already been scaled by the alpha
/// byte, which is what both rasterizers produce and what makes a source-over
/// blit a plain weighted sum. [`Pixmap::to_straight_bgra`] undoes it at the
/// output boundary, because the oracle's PNGs and MD5s are taken over a
/// straight-alpha BGRA buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Pixmap {
    /// A fully transparent pixmap of the given size.
    ///
    /// A zero in either axis is legal and yields an empty buffer, because the
    /// engine reaches this with clipped-away geometry all the time.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    /// A pixmap of the given size, every pixel set to `color`.
    #[must_use]
    pub fn filled(width: u32, height: u32, color: peniko::Color) -> Self {
        let mut this = Self::new(width, height);
        this.fill(color);
        this
    }

    /// Wrap an existing premultiplied RGBA8 buffer.
    ///
    /// Returns `None` when `data` is not exactly `width * height * 4` bytes.
    #[must_use]
    pub fn from_vec(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (data.len() == len).then_some(Self {
            width,
            height,
            data,
        })
    }

    /// The pixmap's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The pixmap's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The premultiplied RGBA8 bytes, row-major.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The premultiplied RGBA8 bytes, mutably.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Consume the pixmap, yielding its premultiplied RGBA8 bytes.
    #[must_use]
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// The premultiplied RGBA bytes of one pixel, or `None` when out of range.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let i = self.index(x, y)?;
        let px = self.data.get(i..i + 4)?;
        Some([*px.first()?, *px.get(1)?, *px.get(2)?, *px.get(3)?])
    }

    /// Overwrite one pixel with premultiplied RGBA bytes. Out of range is a
    /// no-op — the shading rasterizers clip by construction and a stray write
    /// must not panic on a crafted file.
    pub fn set_pixel(&mut self, x: u32, y: u32, px: [u8; 4]) {
        let Some(i) = self.index(x, y) else { return };
        if let Some(slot) = self.data.get_mut(i..i + 4) {
            slot.copy_from_slice(&px);
        }
    }

    /// Set every pixel to `color`.
    pub fn fill(&mut self, color: peniko::Color) {
        let px = premultiply(color);
        for chunk in self.data.chunks_exact_mut(4) {
            chunk.copy_from_slice(&px);
        }
    }

    /// Scale every channel by `alpha`.
    ///
    /// The scalar is **truncated** to a byte (`0.5` becomes `127`, not `128`)
    /// and the per-pixel product truncates too. On a premultiplied buffer all
    /// four channels scale, where the oracle scales only the alpha byte of a
    /// straight one — the same image either way.
    pub fn multiply_alpha(&mut self, alpha: f32) {
        if alpha >= 1.0 {
            return;
        }
        let a = alpha_byte_truncating(alpha);
        for b in &mut self.data {
            *b = mul255(*b, a);
        }
    }

    /// `[oracle-bug]` Remove a non-isolated group's initial backdrop from its
    /// finished pixels (ISO 32000 §11.4.6, §11.6.6).
    ///
    /// The group's buffer starts as a copy of the page beneath it, so that
    /// copy would be counted a **second** time when the group is composited
    /// back. The spec's formula, per channel and premultiplied:
    ///
    /// ```text
    /// C = Cn + (Cn - C0) * (a0 / agn - a0)
    /// ```
    ///
    /// `C0`/`a0` are the backdrop's and `Cn`/`agn` the group's; at
    /// `agn == a0` this returns a transparent pixel, the same image once
    /// composited back. `backdrop` must match this pixmap's dimensions; a
    /// mismatch is a no-op, as for [`Self::multiply_alpha_mask`].
    pub fn remove_backdrop(&mut self, backdrop: &Self) {
        // The oracle never removes the backdrop: it copies the page beneath
        // into the group buffer, then composites that buffer back with no
        // removal step anywhere between, so the backdrop is counted twice.
        // The error is invisible at group alpha 1 under Normal blending and
        // grows with both.
        if backdrop.width != self.width || backdrop.height != self.height {
            return;
        }
        for (chunk, base) in self
            .data
            .chunks_exact_mut(4)
            .zip(backdrop.data.chunks_exact(4))
        {
            let (Some(&agn), Some(&a0)) = (chunk.get(3), base.get(3)) else {
                continue;
            };
            // Nothing was added over this pixel: it is pure backdrop, and the
            // group contributes nothing there.
            if agn <= a0 {
                for b in chunk.iter_mut() {
                    *b = 0;
                }
                continue;
            }
            // Un-premultiply, apply the formula, re-premultiply. The result's
            // alpha is the group's own contribution, `(agn - a0) / (1 - a0)`.
            let (fa0, fagn) = (f32::from(a0) / 255.0, f32::from(agn) / 255.0);
            let out_alpha = if fa0 >= 1.0 {
                0.0
            } else {
                ((fagn - fa0) / (1.0 - fa0)).clamp(0.0, 1.0)
            };
            for index in 0..3 {
                let (Some(&cn), Some(&c0)) = (chunk.get(index), base.get(index)) else {
                    continue;
                };
                let un = |v: u8, a: f32| {
                    if a > 0.0 {
                        f32::from(v) / 255.0 / a
                    } else {
                        0.0
                    }
                };
                let (ucn, uc0) = (un(cn, fagn), un(c0, fa0));
                let colour = uc0.mul_add(-(fa0 / fagn - fa0), ucn.mul_add(fa0 / fagn - fa0, ucn));
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the value to 0.0..=255.0"
                )]
                let byte = (colour.clamp(0.0, 1.0) * out_alpha * 255.0).round() as u8;
                if let Some(slot) = chunk.get_mut(index) {
                    *slot = byte;
                }
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the clamp bounds the value to 0.0..=255.0"
            )]
            let alpha_byte = (out_alpha * 255.0).round() as u8;
            if let Some(slot) = chunk.get_mut(3) {
                *slot = alpha_byte;
            }
        }
    }

    /// `[oracle-bug]` Lay `next` over this pixmap under **knockout**
    /// composition (ISO 32000 §11.6.6): where `next` has any coverage it
    /// *replaces* what is here, rather than blending over it.
    ///
    /// That is the whole of the knockout rule stated pixel-wise. Each object
    /// in a knockout group composites against the group's initial backdrop,
    /// so an earlier object's contribution at a pixel a later one also covers
    /// never reaches the result; only the coverage-weighted mix at the edges
    /// of the later object's own antialiasing keeps any of it.
    ///
    /// A mismatch in dimensions is a no-op, on the same invariant as
    /// [`Self::multiply_alpha_mask`].
    pub fn knockout_over(&mut self, next: &Self) {
        if next.width != self.width || next.height != self.height {
            return;
        }
        for (chunk, over) in self.data.chunks_exact_mut(4).zip(next.data.chunks_exact(4)) {
            let Some(&a) = over.get(3) else { continue };
            if a == 0 {
                continue;
            }
            if a == u8::MAX {
                chunk.copy_from_slice(over);
                continue;
            }
            // Partial coverage at the new object's own edge: mix toward it by
            // its alpha, which is `replace` weighted by coverage.
            for (slot, &value) in chunk.iter_mut().zip(over.iter()) {
                *slot = value.saturating_add(mul255(*slot, 255 - a));
            }
        }
    }

    /// Multiply every channel by a coverage mask, PDFium's
    /// `MultiplyAlphaMask`. The mask must match the pixmap's dimensions
    /// exactly — an invariant, not a preference: a mismatched mask silently
    /// disables masking on both backends, so the engine never emits one.
    pub fn multiply_alpha_mask(&mut self, mask: &AlphaMask) {
        if mask.width() != self.width || mask.height() != self.height {
            return;
        }
        for (chunk, &m) in self.data.chunks_exact_mut(4).zip(mask.data()) {
            for b in chunk {
                *b = mul255(*b, m);
            }
        }
    }

    /// The 8-bit luminosity of every pixel under the oracle's gray weights,
    /// for a soft mask's luminosity readback (ISO 32000 §11.6.5.2).
    ///
    /// Deliberately *not* either backend's luminance helper: both use BT.709
    /// coefficients and the oracle uses NTSC ones on a 0..100 integer scale.
    #[must_use]
    pub fn luminosity_mask(&self) -> AlphaMask {
        let mut out = Vec::with_capacity(self.data.len() / 4);
        for chunk in self.data.chunks_exact(4) {
            let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                (chunk.first(), chunk.get(1), chunk.get(2), chunk.get(3))
            else {
                continue;
            };
            // The oracle's luminosity buffer is opaque (24bpp BGR cleared to
            // the /BC backdrop), so un-premultiplying is the identity there.
            // Ours can carry alpha where a group did not paint over the
            // backdrop clear, so undo the premultiplication first.
            let [r, g, b] = unpremultiply_rgb(r, g, b, a);
            out.push(rgb_to_gray(r, g, b));
        }
        AlphaMask::from_vec(self.width, self.height, out)
            .unwrap_or_else(|| AlphaMask::new(self.width, self.height))
    }

    /// The alpha channel on its own, for an alpha-type soft mask.
    #[must_use]
    pub fn alpha_mask(&self) -> AlphaMask {
        let out: Vec<u8> = self
            .data
            .chunks_exact(4)
            .filter_map(|c| c.get(3).copied())
            .collect();
        AlphaMask::from_vec(self.width, self.height, out)
            .unwrap_or_else(|| AlphaMask::new(self.width, self.height))
    }

    /// The straight-alpha BGRA bytes the oracle hashes and encodes.
    ///
    /// `opaque` forces the alpha byte to `0xFF` without touching the colours,
    /// which is what `FPDFBitmap_BGRx` does: a page with no transparency is
    /// rendered into a 32bpp buffer whose fourth byte is padding, and the
    /// golden MD5 is taken over that padding as `0xFF`.
    #[must_use]
    pub fn to_straight_bgra(&self, opaque: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len());
        for chunk in self.data.chunks_exact(4) {
            let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                (chunk.first(), chunk.get(1), chunk.get(2), chunk.get(3))
            else {
                continue;
            };
            let [r, g, b] = unpremultiply_rgb(r, g, b, a);
            out.extend_from_slice(&[b, g, r, if opaque { 0xFF } else { a }]);
        }
        out
    }

    /// The straight-alpha RGB bytes, three per pixel — the shape the oracle's
    /// PNG encoder writes for a page with no transparency.
    #[must_use]
    pub fn to_straight_rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() / 4 * 3);
        for chunk in self.data.chunks_exact(4) {
            let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                (chunk.first(), chunk.get(1), chunk.get(2), chunk.get(3))
            else {
                continue;
            };
            out.extend_from_slice(&unpremultiply_rgb(r, g, b, a));
        }
        out
    }

    /// The straight-alpha RGBA bytes, four per pixel — the shape the oracle's
    /// PNG encoder writes for a page that has transparency.
    #[must_use]
    pub fn to_straight_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len());
        for chunk in self.data.chunks_exact(4) {
            let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                (chunk.first(), chunk.get(1), chunk.get(2), chunk.get(3))
            else {
                continue;
            };
            let [r, g, b] = unpremultiply_rgb(r, g, b, a);
            out.extend_from_slice(&[r, g, b, a]);
        }
        out
    }

    fn index(&self, x: u32, y: u32) -> Option<usize> {
        (x < self.width && y < self.height)
            .then(|| (y as usize).checked_mul(self.width as usize))
            .flatten()?
            .checked_add(x as usize)?
            .checked_mul(4)
    }
}

/// An 8-bit coverage plane: one byte per pixel, `255` fully opaque.
///
/// This is both a soft mask (ISO 32000 §11.6.5) and a clip mask. It is always
/// sized and aligned to the device it applies to, because a mismatched mask
/// is silently ignored by `vello_cpu` and merely warned about by
/// `tiny-skia` — it fails *open*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlphaMask {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl AlphaMask {
    /// A fully transparent (all-zero) mask.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    /// A mask with every byte set to `value`.
    #[must_use]
    pub fn filled(width: u32, height: u32, value: u8) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            data: vec![value; len],
        }
    }

    /// Wrap an existing coverage plane, or `None` on a size mismatch.
    #[must_use]
    pub fn from_vec(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        let len = (width as usize).checked_mul(height as usize)?;
        (data.len() == len).then_some(Self {
            width,
            height,
            data,
        })
    }

    /// The mask's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The mask's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The coverage bytes, row-major.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The coverage bytes, mutably.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Consume the mask, yielding its coverage bytes.
    #[must_use]
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Intersect with another mask of the same size: the truncating integer
    /// product `old * new / 255`.
    pub fn intersect(&mut self, other: &Self) {
        if other.width != self.width || other.height != self.height {
            return;
        }
        for (a, &b) in self.data.iter_mut().zip(&other.data) {
            *a = mul255(*a, b);
        }
    }

    /// Map every byte through a 256-entry lookup — a soft mask's `/TR`.
    pub fn apply_transfer(&mut self, table: &[u8; 256]) {
        for b in &mut self.data {
            *b = *table.get(*b as usize).unwrap_or(b);
        }
    }

    /// Place `src` at `(x, y)` in a device-sized mask, zero everywhere else.
    ///
    /// This is E8's padding: a soft mask is produced at the clip rect's size
    /// and must be handed to `push_layer` device-sized.
    #[must_use]
    pub fn placed_in(&self, width: u32, height: u32, x: i32, y: i32) -> Self {
        let mut out = Self::new(width, height);
        // `0 <= v < limit` with `limit: u32` puts the value inside `usize` on
        // every target, so `try_into` cannot fail once the guard has passed.
        let in_range = |v: i64, limit: u32| -> Option<usize> {
            (v >= 0 && v < i64::from(limit))
                .then(|| usize::try_from(v).ok())
                .flatten()
        };
        for row in 0..self.height {
            let Some(dy) = in_range(i64::from(row) + i64::from(y), height) else {
                continue;
            };
            for col in 0..self.width {
                let Some(dx) = in_range(i64::from(col) + i64::from(x), width) else {
                    continue;
                };
                let src = self
                    .data
                    .get((row as usize * self.width as usize) + col as usize);
                let dst = out.data.get_mut((dy * width as usize) + dx);
                if let (Some(&src), Some(dst)) = (src, dst) {
                    *dst = src;
                }
            }
        }
        out
    }
}

/// `alpha * 255` cast to an integer: **truncating**, so `ca 0.5` is `127`.
///
/// `alpha_byte_rounding` is the other conversion; the two differ on a large
/// fraction of real `ca` values.
#[must_use]
pub fn alpha_byte_truncating(alpha: f32) -> u8 {
    if alpha.is_nan() {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the NaN guard and the clamp bound the product to 0.0..=255.0; \
                  the truncation *is* the ported behaviour, not an accident"
    )]
    let byte = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    byte
}

/// `round(255 * alpha)` — the *other* alpha conversion, used where a shading
/// pattern resolves its painting object's alpha. Half-away-from-zero, unlike
/// [`alpha_byte_truncating`]; the two differ on a large fraction of real `ca`
/// values.
#[must_use]
pub(crate) fn alpha_byte_rounding(alpha: f32) -> u8 {
    if alpha.is_nan() {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the NaN guard and the clamp bound the rounded product to 0..=255"
    )]
    let byte = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    byte
}

/// `a * b / 255`, truncating — the oracle's ubiquitous 8-bit product.
#[must_use]
pub fn mul255(a: u8, b: u8) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "255*255/255 == 255 is the maximum, so the quotient always fits u8"
    )]
    let byte = ((u32::from(a) * u32::from(b)) / 255) as u8;
    byte
}

/// `AlphaMerge(d, s, a) = (d*(255-a) + s*a) / 255`, truncating and unclamped.
#[must_use]
pub fn alpha_merge(dest: u8, src: u8, alpha: u8) -> u8 {
    let a = u32::from(alpha);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the numerator is a convex combination of two 0..=255 bytes \
                  scaled by 255, so the quotient is itself 0..=255"
    )]
    let byte = ((u32::from(dest) * (255 - a) + u32::from(src) * a) / 255) as u8;
    byte
}

/// `AlphaUnion(d, s) = d + s - d*s/255`.
///
/// The truncating product makes this *not* the exact `1-(1-d)(1-s)` a float
/// composite would give, and it never quite reaches 255 from two partial
/// alphas — `AlphaUnion(128, 128) == 192`, where the float form gives 191.75.
#[must_use]
pub(crate) fn alpha_union(dest: u8, src: u8) -> u8 {
    let merged = u32::from(dest) + u32::from(src) - (u32::from(dest) * u32::from(src)) / 255;
    // Bounded by 255 for every byte pair, but the C++'s `uint8_t` return would
    // wrap rather than saturate if it were not, so say so rather than cast.
    u8::try_from(merged).unwrap_or(u8::MAX)
}

/// A `peniko::Color` as premultiplied RGBA8.
#[must_use]
pub(crate) fn premultiply(color: peniko::Color) -> [u8; 4] {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    [mul255(r, a), mul255(g, a), mul255(b, a), a]
}

/// Undo premultiplication on one pixel's colour channels.
///
/// Rounds the quotient rather than truncating (`+ alpha / 2`), which keeps
/// the round trip through [`premultiply`] stable for opaque pixels.
#[must_use]
pub(crate) fn unpremultiply_rgb(r: u8, g: u8, b: u8, a: u8) -> [u8; 3] {
    if a == 0 {
        return [0, 0, 0];
    }
    if a == 255 {
        return [r, g, b];
    }
    let a32 = u32::from(a);
    let up = |c: u8| -> u8 { ((u32::from(c) * 255 + a32 / 2) / a32).min(255) as u8 };
    [up(r), up(g), up(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit item **A13**. Knockout composition, stated pixel-wise: where a
    /// later object has coverage it *replaces* the earlier one rather than
    /// blending over it. That is the whole of §11.6.6's rule, and it is what
    /// `RenderDeviceDriverIface::SetGroupKnockout` (an empty body at
    /// `renderdevicedriver_iface.cpp:132` the AGG driver never overrides)
    /// does not do.
    #[test]
    fn knockout_replaces_rather_than_blending() {
        let red = peniko::Color::from_rgba8(255, 0, 0, 255);
        let blue = peniko::Color::from_rgba8(0, 0, 255, 255);

        // Full coverage replaces outright — under Normal compositing a
        // translucent blue over red would mix; here an opaque one wins whole.
        let mut base = Pixmap::filled(1, 1, red);
        base.knockout_over(&Pixmap::filled(1, 1, blue));
        assert_eq!(base.pixel(0, 0), Some([0, 0, 255, 255]));

        // No coverage leaves the earlier object alone: knockout replaces
        // where the later object *is*, not everywhere.
        let mut base = Pixmap::filled(1, 1, red);
        base.knockout_over(&Pixmap::new(1, 1));
        assert_eq!(base.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    /// Audit item **A13**. A mismatched overlay is a no-op, on the same
    /// invariant `multiply_alpha_mask` keeps.
    #[test]
    fn a_mismatched_knockout_overlay_changes_nothing() {
        let mut base = Pixmap::filled(2, 2, peniko::Color::from_rgba8(7, 8, 9, 255));
        let before = base.clone();
        base.knockout_over(&Pixmap::filled(3, 3, peniko::Color::BLACK));
        assert_eq!(base, before);
    }

    /// Audit item **A12**. The two properties the formula has to have, and
    /// which `cpdf_renderstatus.cpp` never gives it because it does not
    /// implement removal at all.
    #[test]
    fn removing_the_backdrop_leaves_only_the_groups_own_contribution() {
        // 1. A pixel nothing was drawn over is pure backdrop, so the group
        //    contributes nothing there and must come back transparent —
        //    otherwise compositing draws the page over itself.
        let backdrop = Pixmap::filled(1, 1, peniko::Color::from_rgba8(90, 120, 150, 255));
        let mut group = backdrop.clone();
        group.remove_backdrop(&backdrop);
        assert_eq!(group.pixel(0, 0), Some([0, 0, 0, 0]));

        // 2. Over a *translucent* backdrop the group's own contribution is
        //    recoverable, and comes back at the alpha the group added. Here
        //    the backdrop is half-opaque and the group finished opaque, so
        //    the group's own alpha is `(1 - 0.5) / (1 - 0.5) = 1`.
        let half = Pixmap::filled(1, 1, peniko::Color::from_rgba8(45, 60, 75, 128));
        let mut group = Pixmap::filled(1, 1, peniko::Color::from_rgba8(200, 40, 10, 255));
        group.remove_backdrop(&half);
        let after = group.pixel(0, 0).expect("one pixel");
        assert_eq!(after[3], 255, "the group finished opaque over the backdrop");
        // And it is not the backdrop: the removal changed the colour.
        assert_ne!(after, [45, 60, 75, 128]);

        // 3. `[oracle-bug]` note, recorded rather than hidden: over a fully
        //    **opaque** backdrop a non-isolated group's alpha cannot rise
        //    above 255, so the alpha channel carries no record of what the
        //    group painted and the removal yields a transparent pixel. That
        //    is the correct composite — the page beneath is already those
        //    pixels — but it means this formula recovers nothing extra there,
        //    which is why the two rows it turns byte-exact are the evidence
        //    that matters and not this unit test.
        let mut group = Pixmap::filled(1, 1, peniko::Color::from_rgba8(200, 40, 10, 255));
        group.remove_backdrop(&backdrop);
        assert_eq!(group.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    /// Audit item **A12**. A mismatched backdrop is a no-op, on the same
    /// invariant `multiply_alpha_mask` keeps.
    #[test]
    fn removing_a_mismatched_backdrop_changes_nothing() {
        let mut group = Pixmap::filled(2, 2, peniko::Color::from_rgba8(1, 2, 3, 200));
        let before = group.clone();
        group.remove_backdrop(&Pixmap::filled(3, 3, peniko::Color::BLACK));
        assert_eq!(group, before);
    }

    #[test]
    fn multiply_alpha_truncates() {
        // cfx_dibitmap.cpp:382-410 casts, it does not round: 0.5 -> 127.
        assert_eq!(alpha_byte_truncating(0.5), 127);
        assert_eq!(alpha_byte_rounding(0.5), 128);

        let mut p = Pixmap::filled(1, 1, peniko::Color::from_rgba8(255, 255, 255, 255));
        p.multiply_alpha(0.5);
        assert_eq!(p.pixel(0, 0), Some([127, 127, 127, 127]));
    }

    #[test]
    fn multiply_alpha_one_is_exact_early_return() {
        let mut p = Pixmap::filled(2, 2, peniko::Color::from_rgba8(10, 20, 30, 200));
        let before = p.clone();
        p.multiply_alpha(1.0);
        assert_eq!(p, before);
    }

    #[test]
    fn premul_roundtrip() {
        // Ports CFXDIBitmapTest.{Pre,Un}Multiply*: a straight colour through
        // premultiply and back is itself for every alpha that can represent it.
        for a in [0u8, 1, 64, 128, 254, 255] {
            for c in [0u8, 1, 127, 128, 254, 255] {
                let color = peniko::Color::from_rgba8(c, c, c, a);
                let [pr, pg, pb, pa] = premultiply(color);
                assert_eq!(pa, a);
                let [ur, _, _] = unpremultiply_rgb(pr, pg, pb, pa);
                if a == 0 {
                    assert_eq!(ur, 0);
                } else {
                    // Premultiplication is lossy below full alpha; the round
                    // trip must stay within the quantisation step.
                    let step = 255_u32.div_ceil(u32::from(a).max(1));
                    assert!(
                        u32::from(ur).abs_diff(u32::from(c)) <= step,
                        "a={a} c={c} ur={ur}"
                    );
                }
            }
        }
    }

    #[test]
    fn alpha_merge_truncates_and_does_not_clamp() {
        assert_eq!(alpha_merge(0, 255, 128), 128);
        assert_eq!(alpha_merge(255, 0, 128), 127);
        assert_eq!(alpha_merge(100, 200, 0), 100);
        assert_eq!(alpha_merge(100, 200, 255), 200);
    }

    #[test]
    fn mask_intersect_is_truncating_product() {
        let mut a = AlphaMask::filled(2, 1, 128);
        let b = AlphaMask::filled(2, 1, 128);
        a.intersect(&b);
        // 128*128/255 = 64.25 -> 64
        assert_eq!(a.data(), &[64, 64]);
    }

    #[test]
    fn mask_placed_in_pads_with_zero() {
        let m = AlphaMask::filled(2, 2, 200);
        let placed = m.placed_in(4, 4, 1, 1);
        assert_eq!(placed.data().first().copied(), Some(0));
        assert_eq!(placed.data().get(5).copied(), Some(200));
        assert_eq!(placed.data().get(10).copied(), Some(200));
        assert_eq!(placed.data().get(15).copied(), Some(0));
    }

    #[test]
    fn opaque_output_forces_alpha_ff() {
        // pdfium_test asks for FPDFBitmap_BGRx on a page with no transparency
        // and the golden MD5 is over that buffer, padding byte included.
        let p = Pixmap::filled(1, 1, peniko::Color::from_rgba8(1, 2, 3, 255));
        assert_eq!(p.to_straight_bgra(true), vec![3, 2, 1, 0xFF]);
    }

    #[test]
    fn luminosity_uses_ntsc_not_bt709() {
        // Pure blue: FXRGB2GRAY gives 255*11/100 = 28; BT.709 would give 18.
        let p = Pixmap::filled(1, 1, peniko::Color::from_rgba8(0, 0, 255, 255));
        assert_eq!(p.luminosity_mask().data(), &[28]);
    }
}
