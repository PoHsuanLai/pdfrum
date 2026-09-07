//! `vello_cpu` implementation of `pdfrum-render`'s `RenderDevice` and
//! `RasterBackend` traits — the primary rasterizer. Fully wraps the backend so
//! its still-moving API never leaks into the engine.
//!
//! The SIMD feature level and render mode are pinned rather than detected
//! ([`vello_cpu::Level::baseline`], the f32 [`RenderMode::OptimizeQuality`]
//! pipeline), so output does not vary between machines: `vello_cpu` is
//! bit-exact across thread counts but not across feature levels.
//! [`VelloCpuDevice::draw_image`] clamps its `alpha` to `0.0..=1.0` and, below
//! one, wraps the draw in an opacity layer with the sampler at exactly `1.0`,
//! because `vello_cpu 0.2.0` panics on any other sampler alpha.
//!
//! ```
//! use kurbo::Affine;
//! use pdfrum_render::{ImageQuality, Pixmap, RasterBackend, RenderDevice};
//! use pdfrum_raster_vello_cpu::VelloCpuBackend;
//!
//! let backend = VelloCpuBackend::new();
//! let mut device = backend.new_target(4, 4, peniko::Color::WHITE);
//!
//! // A translucent image draw: the sampler alpha vello_cpu 0.2.0 refuses,
//! // routed through an opacity layer instead of panicking.
//! let image = Pixmap::filled(4, 4, peniko::Color::from_rgba8(255, 0, 0, 255));
//! device.draw_image(&image, Affine::IDENTITY, ImageQuality::Nearest, 0.5);
//!
//! let pixmap = backend.finish(device);
//! let [r, g, b, a] = pixmap.pixel(1, 1).expect("in bounds");
//! assert_eq!(a, 255, "over an opaque page");
//! assert!(r > g && g == b, "half red over white: {r},{g},{b}");
//! ```

// Determinism, pinned. Measured, a baseline-vs-AVX2 pair differs by one count
// on a handful of pixels, reproducibly, in the shared flattening stage. That is
// inside the cross-backend rounding budget, but it would make our own output
// differ between developer machines and CI, so neither the level nor the
// pipeline is detected. Conformance runs are reproducible as a result, which is
// the property that actually matters.
//
// The image-alpha panic: `vello_common/src/encode.rs` reaches an
// `unimplemented!("Applying opacity to image commands")` for a non-1.0
// `ImageSampler::alpha`, and every image drawn with a `ca`/`CA` below one takes
// that path, which is common in the corpus. The doctest above asserts the
// wrapped call does not panic, so a future `vello_cpu` that implements the
// field cannot silently change our rounding.
//
// The crate names follow vello's own: upstream ships `vello` (GPU, on `wgpu`),
// `vello_cpu` and `vello_hybrid`, so the bare name is the GPU backend and this
// one, which wraps `vello_cpu`, says so.

#![forbid(unsafe_code)]

mod convert;

use kurbo::{Affine, BezPath, Rect, Stroke};
use pdfrum_page::BlendMode;
use pdfrum_render::{
    AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, MAX_TARGET_DIMENSION, Pixmap,
    RasterBackend, RasterImage, RenderDevice,
};
use vello_cpu::color::palette::css::TRANSPARENT;
use vello_cpu::peniko::{ImageBrush, ImageSampler};
use vello_cpu::{
    CompositeMode, Level, Mask, PixelFormat, PixmapMut, RasterizerSettings, RenderContext,
    RenderMode, RenderSettings, Resources,
};

pub use convert::{to_blend_mode, to_mix};

/// The SIMD level every target is rasterized at.
///
/// Pinned rather than detected: `Level::try_detect()` would make output
/// machine-dependent by one count on a few pixels, and reproducibility across
/// machines is worth more than the throughput.
#[must_use]
pub fn pinned_level() -> Level {
    Level::baseline()
}

/// The rasterization mode every target uses: the f32 pipeline, which
/// `vello_cpu`'s own documentation recommends for test snapshots.
pub const PINNED_RENDER_MODE: RenderMode = RenderMode::OptimizeQuality;

/// The `vello_cpu` backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct VelloCpuBackend;

impl VelloCpuBackend {
    /// A backend handle. Stateless: every target is independent.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// What kind of thing the innermost `pop` will undo.
///
/// `vello_cpu` keeps clips and layers on **different** stacks — `render_with`
/// asserts every layer is popped but explicitly permits pending clips — so
/// the device must remember which one each `pop` belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Clip,
    Layer,
}

/// A `vello_cpu` render target.
///
/// `vello_cpu` is a *retained scene*: drawing records into a dispatcher and
/// pixels appear only at `render()`. That suits the engine, which is already
/// a one-pass emitter, but it means a snapshot costs a full rasterization of
/// the scene so far — which is why the engine renders each transparency group
/// into its own target rather than snapshotting sub-rectangles of a shared
/// one.
#[derive(Debug)]
pub struct VelloCpuDevice {
    ctx: RenderContext,
    /// The pixels the scene is composited over.
    seed: Seed,
    frames: Vec<Frame>,
    width: u32,
    height: u32,
}

/// What a target's pixels start out as, before the scene composites over them.
///
/// A uniform clear colour is not a pixmap: it is four bytes plus the rule for
/// writing them, and a target-sized buffer holding one repeated pixel is that
/// value paid for at the target's whole area. Only a non-isolated group's
/// backdrop is genuinely per-pixel, so only that variant carries a buffer.
/// The distinction is worth a type because most targets are the uniform case
/// — every transparency group, soft mask, pattern cell and the page itself —
/// and on a 4473 pt square page at 150 DPI the buffer that variant no longer
/// allocates is 331 MiB, written once, copied once and dropped.
#[derive(Debug)]
enum Seed {
    /// Every pixel the same premultiplied colour.
    Solid(peniko::Color),
    /// A non-isolated group's backdrop, pixel for pixel.
    Backdrop(Pixmap),
}

impl VelloCpuDevice {
    fn new(width: u32, height: u32, seed: Seed) -> Self {
        let (w, h) = (
            u16::try_from(width.min(MAX_TARGET_DIMENSION)).unwrap_or(u16::MAX),
            u16::try_from(height.min(MAX_TARGET_DIMENSION)).unwrap_or(u16::MAX),
        );
        let settings = RenderSettings {
            level: pinned_level(),
            num_threads: 0,
        };
        let ctx = RenderContext::new_with(w.max(1), h.max(1), settings);
        Self {
            ctx,
            seed,
            frames: Vec::new(),
            width,
            height,
        }
    }

    fn apply_brush(&mut self, brush: &Brush<'_>) -> bool {
        match brush {
            Brush::Solid(c) => {
                self.ctx.set_paint(to_vello_color(*c));
                true
            }
            // An image brush reaches the device through `draw_image`; there
            // is no second path for it here.
            Brush::Image(_) => false,
        }
    }

    /// Rasterize the recorded scene over the seed.
    fn rasterize(&self) -> Pixmap {
        let (Ok(w), Ok(h)) = (u16::try_from(self.width), u16::try_from(self.height)) else {
            return Pixmap::new(self.width, self.height);
        };
        if w == 0 || h == 0 {
            return Pixmap::new(self.width, self.height);
        }
        // The result buffer is the render target: `vello_cpu` rasterizes
        // through a `PixmapMut` borrowed over any `&mut [u8]`, so a
        // `vello_cpu::Pixmap` of its own and the copy back out of it are both
        // avoidable. The two buffers are the same premultiplied RGBA8 bytes in
        // the same order, and only one of them now exists at a time — 331 MiB
        // apiece on the 4473 pt square page.
        let mut out = match &self.seed {
            // A transparent seed is `Pixmap::new`'s zeroing and no more.
            Seed::Solid(color) if *color == peniko::Color::TRANSPARENT => {
                Pixmap::new(self.width, self.height)
            }
            Seed::Solid(color) => Pixmap::filled(self.width, self.height, *color),
            Seed::Backdrop(base) => base.clone(),
        };
        let settings = RasterizerSettings {
            render_mode: PINNED_RENDER_MODE,
            composite_mode: CompositeMode::SrcOver,
            pixel_format: PixelFormat::Rgba8,
            offset: (0, 0),
        };
        let mut resources = Resources::new();
        // Seeded above, then composited over: `CompositeMode::Replace` would
        // discard the seed.
        let Some(target) = PixmapMut::new(w, h, out.data_mut()) else {
            return out;
        };
        self.ctx.render_with(target, &mut resources, settings);
        out
    }
}

#[expect(
    clippy::many_single_char_names,
    reason = "r/g/b/a are the colour channels"
)]
fn to_vello_color(c: peniko::Color) -> vello_cpu::color::AlphaColor<vello_cpu::color::Srgb> {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    vello_cpu::color::AlphaColor::from_rgba8(r, g, b, a)
}

impl RenderDevice for VelloCpuDevice {
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        if !self.apply_brush(brush) {
            return;
        }
        self.ctx.set_transform(t);
        self.ctx.set_fill_rule(convert::to_fill(rule));
        self.ctx
            .set_aliasing_threshold(convert::to_aliasing_threshold(aa));
        self.ctx.fill_path(path);
        self.ctx.set_aliasing_threshold(None);
        self.ctx.reset_transform();
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        if !self.apply_brush(brush) {
            return;
        }
        self.ctx.set_transform(t);
        self.ctx.set_stroke(stroke.clone());
        self.ctx
            .set_aliasing_threshold(convert::to_aliasing_threshold(aa));
        self.ctx.stroke_path(path);
        self.ctx.set_aliasing_threshold(None);
        self.ctx.reset_transform();
    }

    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32) {
        let Some(pixmap) = convert::to_vello_pixmap(img) else {
            return;
        };
        let opacity = alpha.clamp(0.0, 1.0);
        // B1: `vello_cpu 0.2.0` panics on a sampler alpha below 1.0, so a
        // translucent image is wrapped in an opacity layer instead and the
        // sampler always sees exactly 1.0.
        let layered = opacity < 1.0;
        if layered {
            self.ctx.push_opacity_layer(opacity);
        }
        let source = vello_cpu::ImageSource::Pixmap(std::sync::Arc::new(pixmap));
        let brush = ImageBrush {
            image: source,
            sampler: ImageSampler {
                alpha: 1.0,
                quality: convert::to_image_quality(quality),
                ..ImageSampler::default()
            },
        };
        self.ctx.set_paint(brush);
        self.ctx.set_transform(t);
        // The unit square the trait specifies, scaled to the image's own
        // pixel grid, is exactly the rect the caller's transform maps.
        self.ctx.fill_rect(&Rect::new(
            0.0,
            0.0,
            f64::from(img.width()),
            f64::from(img.height()),
        ));
        self.ctx.reset_transform();
        if layered {
            self.ctx.pop_layer();
        }
    }

    fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        self.ctx.set_fill_rule(convert::to_fill(rule));
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.push_clip_path(path);
        self.frames.push(Frame::Clip);
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        let mut path = BezPath::new();
        path.move_to((rect.x0, rect.y0));
        path.line_to((rect.x1, rect.y0));
        path.line_to((rect.x1, rect.y1));
        path.line_to((rect.x0, rect.y1));
        path.close_path();
        self.ctx.set_fill_rule(convert::to_fill(FillRule::Winding));
        self.ctx.set_transform(Affine::IDENTITY);
        // Hard-edged, so a `re W n` clip does not soften every edge it
        // touches — which is most clipped edges in the corpus.
        self.ctx.set_aliasing_threshold(Some(128));
        self.ctx.push_clip_path(&path);
        self.ctx.set_aliasing_threshold(None);
        self.frames.push(Frame::Clip);
    }

    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
        // E8: a device-sized, device-aligned mask is an invariant, not a
        // preference — `vello_cpu` maps a mismatched one to `None` silently,
        // which produces *unmasked* output, the worst possible failure.
        let converted = mask.and_then(|m| {
            debug_assert_eq!(
                (m.width(), m.height()),
                (self.width, self.height),
                "an AlphaMask must be device-sized and device-aligned"
            );
            if m.width() != self.width || m.height() != self.height {
                return None;
            }
            let (w, h) = (
                u16::try_from(m.width()).ok()?,
                u16::try_from(m.height()).ok()?,
            );
            // Built from raw bytes: `Mask::new_luminance` uses BT.709
            // coefficients and the oracle's soft masks use NTSC ones, so the
            // convenience constructors are the obvious wrong choice.
            Some(Mask::from_parts(m.data().to_vec(), w, h))
        });
        self.ctx.push_layer(
            None,
            Some(convert::to_blend_mode(blend)),
            Some(alpha.clamp(0.0, 1.0)),
            converted,
            None,
        );
        self.frames.push(Frame::Layer);
    }

    fn pop(&mut self) {
        match self.frames.pop() {
            Some(Frame::Clip) => self.ctx.pop_clip_path(),
            Some(Frame::Layer) => self.ctx.pop_layer(),
            None => {}
        }
    }
}

impl RasterBackend for VelloCpuBackend {
    type Device = VelloCpuDevice;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        let (w, h) = (w.min(MAX_TARGET_DIMENSION), h.min(MAX_TARGET_DIMENSION));
        VelloCpuDevice::new(w, h, Seed::Solid(clear))
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        VelloCpuDevice::new(base.width(), base.height(), Seed::Backdrop(base.clone()))
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        // `render_with` asserts every layer is popped; pending clips are
        // explicitly permitted, so only layers are the precondition.
        debug_assert!(
            !d.frames.contains(&Frame::Layer),
            "snapshot requires every layer popped"
        );
        d.rasterize()
    }

    fn finish(&self, mut d: Self::Device) -> Pixmap {
        // Close anything the engine left open, so `render_with`'s assert
        // cannot fire on a damaged page.
        while !d.frames.is_empty() {
            d.pop();
        }
        d.rasterize()
    }
}

/// Whether a colour is fully transparent, so the caller can skip a clear.
#[must_use]
pub fn is_transparent(c: peniko::Color) -> bool {
    c == peniko::Color::TRANSPARENT || to_vello_color(c) == TRANSPARENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    #[test]
    fn a_cleared_target_keeps_its_colour() {
        let backend = VelloCpuBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::WHITE);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn a_solid_fill_paints() {
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        device.fill_path(
            &square(0.0, 0.0, 4.0, 4.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(1, 1), Some([255, 0, 0, 255]));
    }

    #[test]
    fn a_translucent_image_draw_does_not_panic() {
        // B1's regression guard: `ImageSampler::alpha != 1.0` is an
        // `unimplemented!()` in vello_cpu 0.2.0, so this must go through an
        // opacity layer. If a future version implements the field and this
        // wrapper is removed, the rounding changes — the test is here so
        // that removal is deliberate.
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(4, 4, peniko::Color::from_rgba8(255, 0, 0, 255));
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 0.5);
        let out = backend.finish(device);
        let px = out.pixel(1, 1).expect("in bounds");
        assert!(px[3] > 100 && px[3] < 160, "half-opacity image: {}", px[3]);
    }

    #[test]
    fn an_opaque_image_draw_needs_no_layer() {
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(4, 4, peniko::Color::from_rgba8(0, 0, 255, 255));
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 1.0);
        assert!(device.frames.is_empty(), "an opaque draw pushes no frame");
        let out = backend.finish(device);
        assert_eq!(out.pixel(1, 1), Some([0, 0, 255, 255]));
    }

    #[test]
    fn a_hard_edged_rect_clip_has_no_soft_pixels() {
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(8, 1, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.5, 0.0, 4.5, 1.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        let out = backend.finish(device);
        for col in 0..8 {
            let a = out.pixel(col, 0).map_or(0, |px| px[3]);
            assert!(a == 0 || a == 255, "column {col} has soft alpha {a}");
        }
    }

    #[test]
    fn a_layer_composites_with_its_alpha() {
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(2, 2, peniko::Color::from_rgba8(0, 255, 0, 255));
        device.push_layer(BlendMode::Normal, 0.5, None);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 2.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        let out = backend.finish(device);
        let px = out.pixel(0, 0).expect("in bounds");
        assert!(px[0] > 100 && px[0] < 160, "red {}", px[0]);
        assert!(px[1] > 100 && px[1] < 160, "green {}", px[1]);
    }

    #[test]
    fn a_device_sized_mask_is_honoured() {
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let mask = AlphaMask::filled(4, 4, 128);
        device.push_layer(BlendMode::Normal, 1.0, Some(&mask));
        device.fill_path(
            &square(0.0, 0.0, 4.0, 4.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 255, 255, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        let out = backend.finish(device);
        let px = out.pixel(1, 1).expect("in bounds");
        assert!(
            px[3] > 100 && px[3] < 160,
            "the mask halved the alpha: {}",
            px[3]
        );
    }

    #[test]
    fn snapshot_then_backdrop_round_trips() {
        let backend = VelloCpuBackend::new();
        let device = backend.new_target(3, 3, peniko::Color::from_rgba8(1, 2, 3, 255));
        let snap = backend.snapshot(&device);
        assert_eq!(snap.pixel(1, 1), Some([1, 2, 3, 255]));
        let seeded = backend.new_target_with_backdrop(&snap);
        let out = backend.finish(seeded);
        assert_eq!(out.pixel(1, 1), Some([1, 2, 3, 255]));
    }

    #[test]
    fn clips_and_layers_use_separate_stacks() {
        // vello_cpu keeps them apart and asserts only that layers are
        // popped, so the device must pop the right one each time.
        let backend = VelloCpuBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 2.0, 4.0));
        device.push_layer(BlendMode::Multiply, 1.0, None);
        device.pop(); // the layer
        device.pop(); // the clip
        assert!(device.frames.is_empty());
        let _ = backend.finish(device);
    }

    #[test]
    fn the_simd_level_is_pinned_not_detected() {
        // Determinism across machines is the point; a detected level would
        // differ by one count on a handful of pixels between CI and a laptop.
        // `Level` is not `PartialEq`, so compare its debug spelling — which
        // is enough to catch a swap to `try_detect()`.
        assert_eq!(
            format!("{:?}", pinned_level()),
            format!("{:?}", Level::baseline())
        );
        assert_eq!(PINNED_RENDER_MODE, RenderMode::OptimizeQuality);
    }
}
