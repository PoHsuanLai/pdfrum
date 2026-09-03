//! `tiny-skia` implementation of `pdfrum-render`'s `RenderDevice` and
//! `RasterBackend` traits — the cross-check rasterizer and determinism
//! baseline that conformance diffs against `vello_cpu`.
//!
//! tiny-skia has neither a layer stack nor a clip stack, so both are emulated
//! here rather than in the engine: the engine must not know which rasterizer
//! it has. A layer is an offscreen pixmap composited on `pop`; a clip is a
//! coverage `Mask`, and pushing one intersects a clone of the current mask —
//! which is literally `a * b / 255`, the same truncating product PDFium's own
//! `CFX_AggClipRgn::IntersectMask` uses.
//!
//! ## What this backend deliberately does not do
//!
//! tiny-skia silently drops any path fill or clip whose bounding box is
//! thinner than `1/4096` in either axis, turning a degenerate *clip* into a
//! no-op clip rather than an empty one — a correctness inversion. Fixing that
//! here, per call, would make this backend's geometry differ from
//! `vello_cpu`'s and break the cross-backend contract's premise. The engine
//! ports PDFium's rectangle snapping and zero-area detection instead, which
//! removes the bulk of the cases, and the harness turns a residual drop into
//! a hard failure rather than a quiet drift.
//!
//! ```
//! use kurbo::{Affine, Rect};
//! use pdfrum_render::{AntiAlias, Brush, FillRule, RasterBackend, RenderDevice};
//! use pdfrum_raster_tinyskia::TinySkiaBackend;
//!
//! let backend = TinySkiaBackend::new();
//! let mut device = backend.new_target(8, 8, peniko::Color::WHITE);
//!
//! // A hard-edged rect clip, which is what an axis-aligned `re W n` becomes.
//! device.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
//! let mut square = kurbo::BezPath::new();
//! square.move_to((0.0, 0.0));
//! square.line_to((8.0, 0.0));
//! square.line_to((8.0, 8.0));
//! square.line_to((0.0, 8.0));
//! square.close_path();
//! device.fill_path(
//!     &square,
//!     Affine::IDENTITY,
//!     &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
//!     FillRule::Winding,
//!     AntiAlias::Off,
//! );
//! device.pop();
//!
//! let pixmap = backend.finish(device);
//! assert_eq!(pixmap.pixel(1, 1), Some([255, 0, 0, 255]), "inside the clip");
//! assert_eq!(pixmap.pixel(6, 1), Some([255, 255, 255, 255]), "outside it");
//! ```

#![forbid(unsafe_code)]

mod convert;

use kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use pdfrum_page::BlendMode;
use pdfrum_render::{
    AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, MAX_TARGET_DIMENSION, Pixmap,
    RasterBackend, RasterImage, RenderDevice,
};
use tiny_skia::{Mask, Paint, PixmapPaint, PixmapRef, Shader, Transform};

pub use convert::{to_blend_mode, to_color, to_path, to_transform};

/// The flattening tolerance the stroke expansion in `stroke_path` uses, in
/// device pixels.
///
/// The same tenth of a pixel `pdfrum_render::stroke::outline` expands a
/// clip-shaped stroke at, for the same reason: fine enough that the filled
/// outline lands on the pixels the stroke covers, coarse enough not to emit a
/// segment per pixel along a long curve.
const STROKE_TOLERANCE: f64 = 0.1;

/// The `tiny-skia` backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct TinySkiaBackend;

impl TinySkiaBackend {
    /// A backend handle. Stateless: every target is independent.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// One emulated layer: its own pixmap plus how it composites back.
#[derive(Debug)]
struct Layer {
    pixmap: tiny_skia::Pixmap,
    blend: BlendMode,
    alpha: f32,
    mask: Option<Mask>,
    /// The clip in force when the layer was pushed, cloned because tiny-skia
    /// takes a mask per draw call rather than as device state.
    clip: Option<Mask>,
}

/// What kind of thing the innermost `pop` will undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Clip,
    Layer,
}

/// A tiny-skia render target.
///
/// `base` is `None` only for a size tiny-skia refuses to allocate, which the
/// engine's own bounds already exclude; in that state every draw is a no-op
/// and `finish` yields an empty pixmap.
#[derive(Debug)]
pub struct TinySkiaDevice {
    base: Option<tiny_skia::Pixmap>,
    layers: Vec<Layer>,
    clips: Vec<Option<Mask>>,
    frames: Vec<Frame>,
    width: u32,
    height: u32,
}

impl TinySkiaDevice {
    fn empty(width: u32, height: u32) -> Self {
        Self {
            base: try_pixmap(width, height),
            layers: Vec::new(),
            clips: vec![None],
            frames: Vec::new(),
            width,
            height,
        }
    }

    /// The pixmap draws currently land in: the innermost layer, or the base.
    fn target(&mut self) -> Option<&mut tiny_skia::Pixmap> {
        match self.layers.last_mut() {
            Some(layer) => Some(&mut layer.pixmap),
            None => self.base.as_mut(),
        }
    }

    fn clip(&self) -> Option<&Mask> {
        self.clips.last().and_then(Option::as_ref)
    }

    /// The target and the clip in force, borrowed together.
    ///
    /// `target` and `clip` cannot both be called on `&mut self`, and a draw
    /// needs both — so every draw used to hand `tiny_skia` a **clone** of the
    /// clip mask. That clone is device-sized: a letter page's is half a
    /// megabyte, and a page of annotation appearances draws hundreds of
    /// objects under one, so it is a `memcpy` per fill, per stroke and per
    /// image rather than per clip push. `clips` and `layers`/`base` are
    /// disjoint fields, so splitting the borrow by hand is all that was ever
    /// needed; nothing about the drawing changes, and `tiny_skia` takes the
    /// mask by reference either way.
    fn target_and_clip(&mut self) -> Option<(&mut tiny_skia::Pixmap, Option<&Mask>)> {
        let clip = self.clips.last().and_then(Option::as_ref);
        let target = match self.layers.last_mut() {
            Some(layer) => &mut layer.pixmap,
            None => self.base.as_mut()?,
        };
        Some((target, clip))
    }

    /// The device's current base pixels.
    fn snapshot_pixels(&self) -> Pixmap {
        self.base
            .as_ref()
            .and_then(|p| Pixmap::from_vec(self.width, self.height, p.data().to_vec()))
            .unwrap_or_else(|| Pixmap::new(self.width, self.height))
    }
}

/// The size tiny-skia will accept for a target, or `None` when it will not.
///
/// `Pixmap::new` refuses a zero dimension or an allocation that overflows
/// `isize`. The engine already clamps to [`MAX_TARGET_DIMENSION`] and never
/// asks for a zero axis, so a `None` here means a pathological file reached
/// us; every draw then becomes a no-op rather than a panic.
fn try_pixmap(width: u32, height: u32) -> Option<tiny_skia::Pixmap> {
    tiny_skia::Pixmap::new(width.max(1), height.max(1))
}

/// A brush as a tiny-skia `Paint`, or `None` for a brush this backend paints
/// through another entry point.
fn to_paint<'a>(brush: &Brush<'a>, aa: AntiAlias) -> Option<Paint<'a>> {
    let shader = match brush {
        Brush::Solid(c) => Shader::SolidColor(convert::to_color(*c)),
        // An image brush is only used by the shading blits, which go
        // through `draw_image` instead; a solid fallback keeps the trait
        // total without a second code path.
        Brush::Image(_) => return None,
    };
    Some(Paint {
        shader,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        anti_alias: convert::to_anti_alias(aa),
        // The high-precision pipeline: tiny-skia's u16 lanes and
        // vello_cpu's f32 ones must land within the cross-backend
        // rounding budget, and forcing f32 here narrows the gap for free.
        force_hq_pipeline: true,
        colorspace: tiny_skia::ColorSpace::default(),
    })
}

impl RenderDevice for TinySkiaDevice {
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        let Some(p) = convert::to_path(path) else {
            return;
        };
        let Some(paint) = to_paint(brush, aa) else {
            return;
        };
        let transform = convert::to_transform(t);
        let fill_rule = convert::to_fill_rule(rule);
        let Some((target, clip)) = self.target_and_clip() else {
            return;
        };
        target.fill_path(&p, &paint, fill_rule, transform, clip);
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        // The stroke is expanded to its outline here and *filled*, rather than
        // handed to `Pixmap::stroke_path`. tiny-skia's own stroker places the
        // two sides of a stroke asymmetrically: a one-pixel-wide vertical
        // stroke centred on an integer x covers each neighbouring column by a
        // half, and AGG — like `vello_cpu`, and like tiny-skia's own *fill* of
        // the identical rectangle — writes both columns at 128, while its
        // stroker writes 128 and 127. The bias is a half-count of geometry, so
        // it lands on every antialiased stroke edge in the corpus and on the
        // outer tip of every miter corner, where it is large enough to lose the
        // corner pixel entirely.
        //
        // Expanding through `kurbo` removes it and, more importantly, makes the
        // stroke outline the *engine's* geometry under both rasterizers rather
        // than each stroker's own — the same argument
        // `pdfrum_render::stroke::outline` already makes for a stroke used as a
        // clip (design brief §6.1).
        let Some(paint) = to_paint(brush, aa) else {
            return;
        };
        let outline = kurbo::stroke(
            path.path_elements(STROKE_TOLERANCE),
            stroke,
            &kurbo::StrokeOpts::default(),
            STROKE_TOLERANCE,
        );
        let Some(p) = convert::to_path(&outline) else {
            return;
        };
        let transform = convert::to_transform(t);
        let Some((target, clip)) = self.target_and_clip() else {
            return;
        };
        // A stroke outline self-overlaps at joins and caps, so it must be
        // filled non-zero: even-odd would punch the overlaps back out.
        target.fill_path(&p, &paint, tiny_skia::FillRule::Winding, transform, clip);
    }

    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32) {
        let Some(src) = PixmapRef::from_bytes(img.data(), img.width(), img.height()) else {
            return;
        };
        let paint = PixmapPaint {
            opacity: alpha.clamp(0.0, 1.0),
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: convert::to_filter_quality(quality),
        };
        // `draw_pixmap` places the image's own pixel grid at (0, 0) and then
        // applies the transform, which is exactly the unit-square mapping the
        // trait specifies once the caller has folded in the image's size.
        let transform = convert::to_transform(t);
        let Some((target, clip)) = self.target_and_clip() else {
            return;
        };
        target.draw_pixmap(0, 0, src, &paint, transform, clip);
    }

    fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        let mut mask = if let Some(m) = self.clip() {
            m.clone()
        } else {
            let Some(mut m) = Mask::new(self.width.max(1), self.height.max(1)) else {
                return;
            };
            m.data_mut().fill(255);
            m
        };
        if let Some(p) = convert::to_path(path) {
            mask.intersect_path(&p, convert::to_fill_rule(rule), true, Transform::identity());
        } else {
            // An unbuildable path clips everything away, matching the
            // engine's rect(-1,-1,0,0) "clip it all out" convention.
            mask.data_mut().fill(0);
        }
        self.clips.push(Some(mask));
        self.frames.push(Frame::Clip);
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        let mut mask = if let Some(m) = self.clip() {
            m.clone()
        } else {
            let Some(mut m) = Mask::new(self.width.max(1), self.height.max(1)) else {
                return;
            };
            m.data_mut().fill(255);
            m
        };
        match convert::rect_path(rect) {
            // `anti_alias = false` is the whole point: a rect clip in PDFium
            // is hard-edged, and routing it through the antialiased path
            // would soften every `re W n` edge in the corpus.
            Some(p) => mask.intersect_path(
                &p,
                tiny_skia::FillRule::Winding,
                false,
                Transform::identity(),
            ),
            None => mask.data_mut().fill(0),
        }
        self.clips.push(Some(mask));
        self.frames.push(Frame::Clip);
    }

    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
        // E8: a soft mask is device-sized and device-aligned. Both backends
        // fail *open* on a mismatch — vello silently ignores it, tiny-skia
        // only warns — so the engine's invariant is checked here rather than
        // deferred to the crate.
        let converted = mask.and_then(|m| {
            debug_assert_eq!(
                (m.width(), m.height()),
                (self.width, self.height),
                "an AlphaMask must be device-sized and device-aligned (SPEC §8)"
            );
            (m.width() == self.width && m.height() == self.height)
                .then(|| {
                    tiny_skia::IntSize::from_wh(self.width.max(1), self.height.max(1))
                        .and_then(|size| Mask::from_vec(m.data().to_vec(), size))
                })
                .flatten()
        });
        let Some(pixmap) = try_pixmap(self.width, self.height) else {
            return;
        };
        let clip = self.clip().cloned();
        self.layers.push(Layer {
            pixmap,
            blend,
            alpha,
            mask: converted,
            clip,
        });
        self.frames.push(Frame::Layer);
    }

    fn pop(&mut self) {
        match self.frames.pop() {
            Some(Frame::Clip) => {
                if self.clips.len() > 1 {
                    self.clips.pop();
                }
            }
            Some(Frame::Layer) => {
                let Some(mut layer) = self.layers.pop() else {
                    return;
                };
                if layer.alpha < 1.0 {
                    let a = layer.alpha.clamp(0.0, 1.0);
                    for b in layer.pixmap.data_mut() {
                        #[expect(
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss,
                            reason = "the clamp bounds the product to 0..=255"
                        )]
                        let scaled = (f32::from(*b) * a) as u8;
                        *b = scaled;
                    }
                }
                if let Some(m) = &layer.mask {
                    layer.pixmap.apply_mask(m);
                }
                let paint = PixmapPaint {
                    opacity: 1.0,
                    blend_mode: convert::to_blend_mode(layer.blend),
                    quality: tiny_skia::FilterQuality::Nearest,
                };
                let src = layer.pixmap.as_ref();
                let clip = layer.clip.clone();
                let Some(target) = self.target() else { return };
                target.draw_pixmap(0, 0, src, &paint, Transform::identity(), clip.as_ref());
            }
            None => {}
        }
    }
}

impl RasterBackend for TinySkiaBackend {
    type Device = TinySkiaDevice;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        let (w, h) = (w.min(MAX_TARGET_DIMENSION), h.min(MAX_TARGET_DIMENSION));
        let mut device = TinySkiaDevice::empty(w, h);
        if clear != peniko::Color::TRANSPARENT
            && let Some(base) = device.base.as_mut()
        {
            base.fill(convert::to_color(clear));
        }
        device
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        let mut device = TinySkiaDevice::empty(base.width(), base.height());
        if let Some(target) = device.base.as_mut() {
            let dst = target.data_mut();
            let src = base.data();
            let n = dst.len().min(src.len());
            if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
                d.copy_from_slice(s);
            }
        }
        device
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        debug_assert!(
            d.layers.is_empty(),
            "snapshot requires every layer popped (SPEC §8)"
        );
        d.snapshot_pixels()
    }

    fn finish(&self, mut d: Self::Device) -> Pixmap {
        // Flatten anything the engine left open rather than losing it.
        while !d.frames.is_empty() {
            d.pop();
        }
        d.snapshot_pixels()
    }
}

#[cfg(test)]
mod tests {
    use pdfrum_render::AlphaMask;

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
        let backend = TinySkiaBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::WHITE);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn a_transparent_target_starts_empty() {
        let backend = TinySkiaBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_hard_edged_rect_clip_has_no_soft_pixels() {
        let backend = TinySkiaBackend::new();
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
        // Every pixel is either fully painted or fully clear: the clip's
        // edge contributed no partial coverage.
        for col in 0..8 {
            let a = out.pixel(col, 0).map_or(0, |px| px[3]);
            assert!(a == 0 || a == 255, "column {col} has soft alpha {a}");
        }
    }

    #[test]
    fn a_stroke_covers_both_sides_of_its_centre_line_equally() {
        // AGG writes 128 into both columns a unit-wide stroke on an integer x
        // half-covers; tiny-skia's own stroker writes 128 and 127. Expanding
        // the outline and filling it removes the bias, so this is the pixel
        // form of the reason `stroke_path` does not call `Pixmap::stroke_path`.
        let backend = TinySkiaBackend::new();
        let mut device = backend.new_target(8, 4, peniko::Color::TRANSPARENT);
        let mut line = BezPath::new();
        line.move_to((4.0, 0.0));
        line.line_to((4.0, 4.0));
        device.stroke_path(
            &line,
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::BLACK),
            &Stroke::new(1.0),
            AntiAlias::On,
        );
        let out = backend.finish(device);
        let left = out.pixel(3, 2).map_or(0, |px| px[3]);
        let right = out.pixel(4, 2).map_or(0, |px| px[3]);
        assert_eq!(left, right, "the two half-covered columns must agree");
        assert_eq!(left, 128, "AGG's coverage for a half-covered pixel");
    }

    #[test]
    fn a_mitred_corner_paints_its_outer_tip() {
        // The outer tip of a right-angle miter covers a quarter of its pixel,
        // which AGG paints at alpha 64. The stroker's half-count bias was
        // enough to lose it entirely, and it recurs at every corner of every
        // stroked rectangle in the corpus.
        let backend = TinySkiaBackend::new();
        let mut device = backend.new_target(16, 16, peniko::Color::TRANSPARENT);
        let mut corner = BezPath::new();
        corner.move_to((4.0, 12.0));
        corner.line_to((4.0, 4.0));
        corner.line_to((12.0, 4.0));
        device.stroke_path(
            &corner,
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::BLACK),
            &Stroke::new(1.0).with_join(kurbo::Join::Miter),
            AntiAlias::On,
        );
        let out = backend.finish(device);
        let tip = out.pixel(3, 3).map_or(0, |px| px[3]);
        assert_eq!(tip, 64, "the miter's outer tip is a quarter-covered pixel");
    }

    #[test]
    fn a_layer_composites_with_its_blend_and_alpha() {
        let backend = TinySkiaBackend::new();
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
        // Half red over green: both channels present, neither saturated.
        assert!(px[0] > 100 && px[0] < 160, "red {}", px[0]);
        assert!(px[1] > 100 && px[1] < 160, "green {}", px[1]);
    }

    #[test]
    fn a_layer_mask_must_be_device_sized() {
        let backend = TinySkiaBackend::new();
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
        let px = out.pixel(0, 0).expect("in bounds");
        assert!(
            px[3] > 100 && px[3] < 160,
            "the mask halved the alpha: {}",
            px[3]
        );
    }

    #[test]
    fn snapshot_then_backdrop_round_trips() {
        let backend = TinySkiaBackend::new();
        let device = backend.new_target(3, 3, peniko::Color::from_rgba8(1, 2, 3, 255));
        let snap = backend.snapshot(&device);
        let seeded = backend.new_target_with_backdrop(&snap);
        let out = backend.finish(seeded);
        assert_eq!(out.pixel(1, 1), Some([1, 2, 3, 255]));
    }

    #[test]
    fn clips_nest_and_unwind() {
        let backend = TinySkiaBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
        device.push_clip_rect(Rect::new(2.0, 0.0, 8.0, 8.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        device.pop();
        let out = backend.finish(device);
        // Only the 2..4 intersection is painted.
        assert_eq!(out.pixel(0, 0).map(|px| px[3]), Some(0));
        assert_eq!(out.pixel(3, 0).map(|px| px[3]), Some(255));
        assert_eq!(out.pixel(5, 0).map(|px| px[3]), Some(0));
    }

    #[test]
    fn an_image_draw_with_alpha_does_not_panic() {
        // The vello backend has to route this through an opacity layer;
        // tiny-skia honours it directly. Both must survive it.
        let backend = TinySkiaBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(4, 4, peniko::Color::from_rgba8(255, 0, 0, 255));
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 0.5);
        let out = backend.finish(device);
        let px = out.pixel(0, 0).expect("in bounds");
        assert!(px[3] > 100 && px[3] < 160, "half-opacity image: {}", px[3]);
    }

    #[test]
    fn finish_flattens_an_unpopped_layer() {
        let backend = TinySkiaBackend::new();
        let mut device = backend.new_target(2, 2, peniko::Color::TRANSPARENT);
        device.push_layer(BlendMode::Normal, 1.0, None);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 2.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0).map(|px| px[0]), Some(255));
    }
}
