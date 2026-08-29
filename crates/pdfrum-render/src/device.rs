//! The only seam between the engine and a rasterizer (SPEC.md §8).
//!
//! Two traits and a handful of vocabulary types, all spoken in kurbo/peniko.
//! Nothing PDF-specific crosses this boundary: every decision that both
//! backends must agree on — degeneracy filters, integer rect snapping, the
//! stroke matrix split, dash normalisation, the ±32000 coordinate clamp — is
//! made in the engine *before* a device call, so a backend is free to be dumb
//! and Tier C can demand that the two receive identical geometry.

use kurbo::{Affine, BezPath, Rect, Stroke};
use pdfrum_page::BlendMode;

use crate::pixmap::{AlphaMask, Pixmap};

/// Whether a primitive's edges are antialiased.
///
/// `Off` is the oracle's `aliased_path`, which thresholds coverage at
/// `> 127 -> 255` rather than turning the rasterizer off. It is what the
/// engine asks for on the never-antialiased axis-aligned rect fast path and
/// on Coons patch cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AntiAlias {
    /// Antialiased, the default for every ordinary fill and stroke.
    #[default]
    On,
    /// Hard-edged.
    Off,
}

/// Which winding rule decides a path's interior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    /// Non-zero winding.
    #[default]
    Winding,
    /// Even-odd.
    EvenOdd,
}

/// How an image's samples are reconstructed when it is scaled.
///
/// The *selection* is the engine's — it ports PDFium's `/Interpolate`, its
/// `bNoSmoothing`/`bHalftone` flags, the `kHugeImageSize` forcing rule and
/// the `UseInterpolateBilinear` heuristic — and the kernels are each
/// backend's own. `Nearest` is passed explicitly for an integer-only
/// translation, because `vello_cpu` silently downgrades bilinear to nearest
/// in exactly that case and `tiny-skia` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageQuality {
    /// Nearest neighbour.
    #[default]
    Nearest,
    /// Bilinear.
    Bilinear,
}

/// A premultiplied RGBA8 image ready to be drawn, with its own dimensions.
pub type RasterImage = Pixmap;

/// What a fill or stroke paints with.
#[derive(Debug, Clone, Copy)]
pub enum Brush<'a> {
    /// A single colour.
    Solid(peniko::Color),
    /// An image, mapped onto the primitive by the draw's own transform.
    Image(&'a RasterImage),
}

/// The device a page is drawn into: six primitives plus a hard-edged rect
/// clip, all object-safe.
///
/// The engine holds one of these per render target — the page, each
/// transparency group, each soft mask, each pattern cell — and never learns
/// which rasterizer is behind it.
pub trait RenderDevice {
    /// Fill `path` (in user space) transformed by `t`.
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    );

    /// Stroke `path` (in user space) transformed by `t`.
    ///
    /// The engine has already resolved the stroke's device width — including
    /// the one-device-pixel minimum — and normalised its dash array to an
    /// even-length, all-positive list, so `stroke` is always directly usable.
    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    );

    /// Draw `img` with its own **pixel grid** mapped through `t`, at a
    /// constant `alpha`.
    ///
    /// `t` maps image pixel `(0, 0)`'s corner to its device position, so an
    /// identity transform is a texel-for-pixel blit at the origin and a
    /// translation moves it whole pixels — *not* a unit-square mapping. The
    /// engine has already resampled an image to its device size before it
    /// arrives here (`image::resample`), so in practice every call site passes
    /// a plain translation. Reading `t` the other way collapses a whole-page
    /// image onto a single pixel, which is silent and total.
    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32);

    /// Intersect the clip with `path` (already in device space).
    fn push_clip(&mut self, path: &BezPath, rule: FillRule);

    /// Intersect the clip with an axis-aligned rectangle, **hard-edged**.
    ///
    /// PDFium's `SetClip_PathFill` takes a rect fast path that snaps to the
    /// outer integer rect and applies it with no antialiasing at all; `re W n`
    /// is the commonest clip in the corpus, so routing it through
    /// [`RenderDevice::push_clip`] would add a soft pixel along every clipped
    /// edge on a large fraction of the corpus.
    fn push_clip_rect(&mut self, rect: Rect);

    /// Begin a layer that will be composited back with `blend`, scaled by
    /// `alpha` and masked by `mask`.
    ///
    /// `mask`, when present, is device-sized and device-aligned. That is an
    /// invariant, not a convention: a mismatched mask is silently ignored by
    /// `vello_cpu` and merely warned about by `tiny-skia`, so violating it
    /// fails *open*, producing unmasked output. Backends assert it.
    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>);

    /// End the innermost clip or layer.
    fn pop(&mut self);
}

/// The factory that lets the engine rasterize offscreen: soft masks,
/// transparency groups, pattern cells and the fill+stroke knockout buffer all
/// need a second target.
pub trait RasterBackend {
    /// The device this backend produces.
    type Device: RenderDevice;

    /// A fresh target of the given size, every pixel set to `clear`.
    ///
    /// Both axes must be at most [`MAX_TARGET_DIMENSION`]: `vello_cpu` sizes
    /// its scenes, pixmaps and masks with `u16`. The engine enforces the
    /// bound before calling.
    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device;

    /// A fresh target seeded with `base`'s pixels.
    ///
    /// This is a non-isolated transparency group's backdrop (ISO 32000
    /// §11.4.6) and the fill+stroke knockout buffer's saved destination.
    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device;

    /// The device's current pixels, without consuming it.
    ///
    /// Only valid with every [`RenderDevice::push_layer`] matched by a
    /// [`RenderDevice::pop`] — `vello_cpu` asserts it. On a retained-scene
    /// backend this costs a full rasterization of the scene so far, not a
    /// rectangle copy, which is why the engine renders each group into its
    /// own target rather than snapshotting sub-rectangles of a shared one.
    fn snapshot(&self, d: &Self::Device) -> Pixmap;

    /// Consume the device, yielding its premultiplied RGBA8 pixels.
    fn finish(&self, d: Self::Device) -> Pixmap;
}

/// The largest render target either backend accepts in one axis.
///
/// `vello_cpu`'s `RenderContext::new`, `Pixmap::new` and `Mask` are all
/// `u16`-dimensioned; `tiny-skia` is `u32` but must agree for Tier C.
pub const MAX_TARGET_DIMENSION: u32 = u16::MAX as u32;

impl From<pdfrum_page::FillRule> for FillRule {
    /// `kWinding` maps to non-zero and **everything else, `kNoFill`
    /// included, maps to even-odd** (`cfx_agg_devicedriver.cpp:395-400`).
    fn from(value: pdfrum_page::FillRule) -> Self {
        match value {
            pdfrum_page::FillRule::Winding => Self::Winding,
            pdfrum_page::FillRule::EvenOdd | pdfrum_page::FillRule::None => Self::EvenOdd,
        }
    }
}

/// The `peniko` mix mode a PDF blend mode maps to.
#[must_use]
pub fn peniko_mix(mode: BlendMode) -> peniko::Mix {
    match mode {
        BlendMode::Normal | BlendMode::Compatible => peniko::Mix::Normal,
        BlendMode::Multiply => peniko::Mix::Multiply,
        BlendMode::Screen => peniko::Mix::Screen,
        BlendMode::Overlay => peniko::Mix::Overlay,
        BlendMode::Darken => peniko::Mix::Darken,
        BlendMode::Lighten => peniko::Mix::Lighten,
        BlendMode::ColorDodge => peniko::Mix::ColorDodge,
        BlendMode::ColorBurn => peniko::Mix::ColorBurn,
        BlendMode::HardLight => peniko::Mix::HardLight,
        BlendMode::SoftLight => peniko::Mix::SoftLight,
        BlendMode::Difference => peniko::Mix::Difference,
        BlendMode::Exclusion => peniko::Mix::Exclusion,
        BlendMode::Hue => peniko::Mix::Hue,
        BlendMode::Saturation => peniko::Mix::Saturation,
        BlendMode::Color => peniko::Mix::Color,
        BlendMode::Luminosity => peniko::Mix::Luminosity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fill_rule_reads_as_even_odd() {
        // cfx_agg_devicedriver.cpp:395-400: only kWinding is non-zero.
        assert_eq!(
            FillRule::from(pdfrum_page::FillRule::None),
            FillRule::EvenOdd
        );
        assert_eq!(
            FillRule::from(pdfrum_page::FillRule::EvenOdd),
            FillRule::EvenOdd
        );
        assert_eq!(
            FillRule::from(pdfrum_page::FillRule::Winding),
            FillRule::Winding
        );
    }

    #[test]
    fn every_blend_mode_has_a_mix() {
        // Compatible is Normal; the other fifteen are distinct.
        assert_eq!(peniko_mix(BlendMode::Compatible), peniko::Mix::Normal);
        assert_eq!(peniko_mix(BlendMode::SoftLight), peniko::Mix::SoftLight);
        assert_eq!(peniko_mix(BlendMode::Luminosity), peniko::Mix::Luminosity);
    }
}
