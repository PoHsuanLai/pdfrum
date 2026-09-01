//! An analytic scanline rasterizer implementing `pdfrum-render`'s
//! `RenderDevice` and `RasterBackend` traits, written for parity with the
//! oracle's own scan converter (SPEC.md §8).
//!
//! # Why a third backend
//!
//! The two existing backends are third-party rasterizers wrapped behind the
//! trait, and each brings its own idea of what a partially covered pixel is
//! worth. `tiny-skia` supersamples at four subsamples per axis: a diagonal
//! edge therefore has **seventeen** distinct coverage levels, and a
//! half-covered pixel quantises to 8/16 of the range rather than to a half.
//! The oracle integrates the covered area analytically and writes the exact
//! value. Over the corpus that difference is a persistent few-count spread
//! along every non-axis-aligned edge — measured, in
//! `docs/status/pdfrum-render.md`, as the population sitting between SSIM 0.95
//! and 0.99 with no image, shading, pattern or soft mask on the page.
//!
//! This backend closes that by construction rather than by correction: it
//! computes the same integral the oracle computes, on the same 256ths-of-a-
//! pixel grid, and maps coverage to alpha through the same measured formula.
//! It exists for parity, and the two wrapped backends stay exactly where they
//! are — `vello_cpu` remains the facade's default for API users, who want a
//! fast production rasterizer rather than a byte-comparable one.
//!
//! # What "exact" means here, and what it does not
//!
//! Exact refers to the *coverage integral* and the arithmetic downstream of
//! it: a pixel's alpha is `min(255, floor(coverage * 256))` of the true
//! geometric coverage, every intermediate value is an integer, and no step
//! samples. It does not mean the whole page matches the oracle byte for byte —
//! glyph rendering, image resampling kernels and the engine's own decisions
//! are all upstream of this crate and unchanged by it.
//!
//! # The design in one paragraph
//!
//! [`pdfrum_render::scanline`] holds the rasterizer: kurbo flattens curves,
//! each segment is integrated into per-pixel `(cover, area)` cells, and a sweep
//! turns the sorted cells into spans of constant coverage. It lives in the
//! engine rather than here because the engine's own glyph path needs the same
//! integrator, and a glyph bitmap must not depend on which rasterizer draws the
//! page. [`target`] holds the pixel
//! buffer, the clip — a coverage plane multiplied in per pixel, which is how
//! the oracle's clip region works too — and the span blitter. [`image`] holds
//! the inverse-mapped image sampler. The compositing arithmetic itself is
//! **`pdfrum-render`'s**, not this crate's: `blend::composite_premultiplied`
//! is the one authority, so a pixel this backend blends and a pixel the engine
//! blends in its own offscreen buffers agree by construction.
//!
//! ```
//! use kurbo::{Affine, Rect};
//! use pdfrum_render::{AntiAlias, Brush, FillRule, RasterBackend, RenderDevice};
//! use pdfrum_raster_exact::ExactBackend;
//!
//! let backend = ExactBackend::new();
//! let mut device = backend.new_target(4, 1, peniko::Color::TRANSPARENT);
//!
//! // A rectangle covering exactly half of column 0 and all of column 1.
//! let mut half = kurbo::BezPath::new();
//! half.move_to((0.5, 0.0));
//! half.line_to((2.0, 0.0));
//! half.line_to((2.0, 1.0));
//! half.line_to((0.5, 1.0));
//! half.close_path();
//! device.fill_path(
//!     &half,
//!     Affine::IDENTITY,
//!     &Brush::Solid(peniko::Color::BLACK),
//!     FillRule::Winding,
//!     AntiAlias::On,
//! );
//!
//! let pixmap = backend.finish(device);
//! // Exactly half, not the nearest of seventeen supersampled levels.
//! assert_eq!(pixmap.pixel(0, 0).map(|px| px[3]), Some(128));
//! assert_eq!(pixmap.pixel(1, 0).map(|px| px[3]), Some(255));
//! ```

#![forbid(unsafe_code)]
// Every coordinate reaching this crate came from an untrusted file by way of
// the engine: index with `get()` and do arithmetic with `checked_*`.
#![warn(clippy::indexing_slicing)]

pub mod image;
pub mod target;

use kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use pdfrum_page::BlendMode;
use pdfrum_render::{
    AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, MAX_TARGET_DIMENSION, Pixmap,
    RasterBackend, RasterImage, RenderDevice, pixmap,
};

use pdfrum_render::scanline::{self, Rasterizer};
use target::{Source, Target};

/// The flattening tolerance, in device pixels.
///
/// The oracle's own curve subdivider stops when a control point lies within
/// **half a device pixel** of the chord (`agg_curves.cpp:36`'s
/// `m_distance_tolerance_square = 1/4`, which is a squared distance, against a
/// squared chord length). Matching that number exactly would reproduce its
/// *vertices*, but not its subdivision — AGG bisects recursively and kurbo
/// places points adaptively, so the two agree on the curve rather than on the
/// polyline.
///
/// A tenth of a pixel is used instead, and it is the safer direction: it is
/// five times finer than the oracle's, so the flattening error is well below
/// the coverage quantisation the output byte imposes, and no edge can land a
/// count away because a chord cut a corner. It is also the tolerance the
/// engine's own stroke outlining already uses, so a stroke expanded for a clip
/// and a stroke expanded for a fill are the same polygon.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// The analytic backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExactBackend;

impl ExactBackend {
    /// A backend handle. Stateless: every target is independent.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// One layer: its own target plus how it composites back.
#[derive(Debug)]
struct Layer {
    target: Target,
    blend: BlendMode,
    alpha: f32,
    mask: Option<AlphaMask>,
}

/// What kind of thing the innermost `pop` will undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Clip,
    Layer,
}

/// A render target for the analytic rasterizer.
///
/// Both stacks are explicit here, as they must be for any backend without
/// native ones: a layer is an offscreen target composited on `pop`, and a clip
/// is a coverage plane pushed onto a stack so `pop` restores the previous one
/// rather than recomputing an intersection.
#[derive(Debug)]
pub struct ExactDevice {
    base: Target,
    layers: Vec<Layer>,
    /// The clip stack, innermost last. `None` is "everything visible".
    clips: Vec<Option<AlphaMask>>,
    frames: Vec<Frame>,
    /// Reused across draws so a page of paths costs one allocation, not one
    /// per fill.
    raster: Rasterizer,
}

impl ExactDevice {
    fn new(target: Target) -> Self {
        Self {
            base: target,
            layers: Vec::new(),
            clips: vec![None],
            frames: Vec::new(),
            raster: Rasterizer::new(),
        }
    }

    /// The target draws currently land in: the innermost layer, or the base.
    fn target(&mut self) -> &mut Target {
        match self.layers.last_mut() {
            Some(layer) => &mut layer.target,
            None => &mut self.base,
        }
    }

    /// The device's size, which every layer and mask shares.
    fn size(&self) -> (u32, u32) {
        (self.base.width(), self.base.height())
    }

    /// Rasterize a device-space path and hand its spans to `paint`.
    ///
    /// The rasterizer is reset rather than rebuilt, and the borrow is split so
    /// `paint` can hold the target mutably while the spans stream out.
    fn scan(
        &mut self,
        path: &BezPath,
        rule: FillRule,
        aa: AntiAlias,
        mut paint: impl FnMut(&mut Target, i32, i32, i32, u8),
    ) {
        self.raster.reset();
        self.raster.add_path(path, FLATTEN_TOLERANCE);
        let rule = to_scanline_rule(rule);
        let coverage = to_coverage(aa);
        // Split the borrow: `raster` and the target live in the same struct,
        // and the sweep needs one while the paint closure needs the other.
        let (raster, layers, base) = (&mut self.raster, &mut self.layers, &mut self.base);
        let target = match layers.last_mut() {
            Some(layer) => &mut layer.target,
            None => base,
        };
        raster.sweep(rule, coverage, |x, len, y, alpha| {
            paint(target, x, len, y, alpha);
        });
    }

    /// Composite an image through the inverse-mapped sampler: the general
    /// case, for any transform that is not a whole-pixel translation.
    ///
    /// `t` maps the image's own **pixel grid** onto the device, not its unit
    /// square: every engine call site passes a plain translation for a
    /// device-sized buffer, and the engine has already folded any scale into
    /// the samples by resampling them. Reading `t` as a unit-square map instead
    /// collapses a whole-page image onto one pixel.
    ///
    /// The image's footprint is its pixel rectangle through `t`, and it is
    /// filled **antialiased**. Hard-edging it instead thresholds the silhouette
    /// at half a pixel, which silently deletes any image thin enough to cover
    /// less than that — a sheared type-3 glyph drawn as an inline image mask is
    /// exactly such a case, and the oracle paints it at its true partial
    /// coverage rather than dropping it.
    ///
    /// The border is not darkened twice by this. The sampler carries the
    /// image's *own* alpha and the footprint carries the silhouette's, and the
    /// two are different quantities: outside the last texel the sampler returns
    /// nothing at all, so the only pixels the silhouette modulates are the
    /// boundary ones, where a partial coverage is what the geometry says.
    fn draw_image_sampled(
        &mut self,
        img: &RasterImage,
        t: Affine,
        quality: ImageQuality,
        alpha: u8,
    ) {
        let Some(sampler) = image::Sampler::new(img, t, quality) else {
            return;
        };
        let footprint = t * rect_path(Rect::new(
            0.0,
            0.0,
            f64::from(img.width()),
            f64::from(img.height()),
        ));
        self.scan(
            &footprint,
            FillRule::Winding,
            AntiAlias::On,
            move |target, x, len, y, cov| {
                target.blend_span_with(x, len, y, cov, BlendMode::Normal, |col, row| {
                    sampler
                        .sample(col, row)
                        .map(|px| image::scale_alpha(px, alpha))
                });
            },
        );
    }

    /// Composite an image texel-for-pixel at a whole-pixel offset.
    ///
    /// The blit `draw_image` degenerates to when its transform is a whole-pixel
    /// translation. Every row of the image is one span of full coverage, so
    /// there is nothing to rasterize and nothing to invert: the source column
    /// is the destination column minus the offset.
    ///
    /// The clip still applies, through the same `blend_span_with` the general
    /// path uses, so a clipped blit lands on exactly the pixels a clipped image
    /// draw would.
    fn blit(&mut self, img: &RasterImage, dx: i32, dy: i32, alpha: u8) {
        let target = match self.layers.last_mut() {
            Some(layer) => &mut layer.target,
            None => &mut self.base,
        };
        for row in 0..img.height() {
            let Ok(row_i32) = i32::try_from(row) else {
                continue;
            };
            let Some(y) = row_i32.checked_add(dy) else {
                continue;
            };
            let Ok(width) = i32::try_from(img.width()) else {
                continue;
            };
            target.blend_span_with(dx, width, y, 255, BlendMode::Normal, |col, _| {
                let src_col = u32::try_from(i64::from(col) - i64::from(dx)).ok()?;
                img.pixel(src_col, row)
                    .map(|px| image::scale_alpha(px, alpha))
            });
        }
    }

    /// The coverage plane a path fills, device-sized — a clip.
    fn coverage_of(&mut self, path: &BezPath, rule: FillRule, aa: AntiAlias) -> AlphaMask {
        let (w, h) = self.size();
        let mut mask = AlphaMask::new(w, h);
        self.raster.reset();
        self.raster.add_path(path, FLATTEN_TOLERANCE);
        let width = w as usize;
        self.raster.sweep(
            to_scanline_rule(rule),
            to_coverage(aa),
            |x, len, y, alpha| {
                let (Ok(row), Ok(w_i32)) = (u32::try_from(y), i32::try_from(w)) else {
                    return;
                };
                if row >= h {
                    return;
                }
                let x0 = x.max(0);
                let x1 = x.saturating_add(len).min(w_i32);
                for col in x0..x1 {
                    let Ok(col) = usize::try_from(col) else {
                        continue;
                    };
                    if let Some(slot) = mask.data_mut().get_mut(row as usize * width + col) {
                        *slot = alpha;
                    }
                }
            },
        );
        mask
    }

    /// Push a clip that is the intersection of the current one with `mask`.
    fn push_clip_mask(&mut self, mut mask: AlphaMask) {
        if let Some(current) = self.clips.last().and_then(Option::as_ref) {
            mask.intersect(current);
        }
        self.clips.push(Some(mask));
        self.frames.push(Frame::Clip);
        self.sync_clip();
    }

    /// Point the active target at the innermost clip.
    fn sync_clip(&mut self) {
        let clip = self.clips.last().and_then(Option::as_ref).cloned();
        self.target().set_clip(clip);
    }

    /// The solid source colour a brush paints, if it has one.
    ///
    /// **Straight**, not premultiplied. Premultiplying here and
    /// un-premultiplying inside the composite would quantise the colour to the
    /// `alpha + 1` values a premultiplied byte can hold — one count of red off
    /// the form-field tint, at the highlight's alpha of 100 — for no gain: the
    /// oracle's own render targets are straight-alpha and the composite takes a
    /// straight source directly.
    fn solid(brush: &Brush<'_>) -> Option<Source> {
        match brush {
            Brush::Solid(color) => {
                let [r, g, b, a] = color.to_rgba8().to_u8_array();
                Some(Source::Straight([r, g, b], a))
            }
            // An image brush reaches the device through `draw_image`; the
            // engine never fills a path with one.
            Brush::Image(_) => None,
        }
    }
}

/// The whole-pixel `(dx, dy)` a transform amounts to, or `None` when it is
/// anything else.
///
/// Exact equality on the four coefficients, not a tolerance: the point is to
/// take the fast path only where it is provably the *same answer*, and a matrix
/// a hair off the identity really does resample. The offsets are likewise
/// required to be exact integers, because half a pixel of translation is a
/// genuine resample and the general path is the one that performs it.
fn whole_pixel_offset(transform: Affine) -> Option<(i32, i32)> {
    let [xx, yx, xy, yy, tx, ty] = transform.as_coeffs();
    #[expect(
        clippy::float_cmp,
        reason = "the fast path must be taken only where the two paths agree \
                  exactly; a tolerance here would silently skip a resample"
    )]
    let unrotated_unscaled = xx == 1.0 && yx == 0.0 && xy == 0.0 && yy == 1.0;
    if !unrotated_unscaled {
        return None;
    }
    Some((whole(tx)?, whole(ty)?))
}

/// A float that is exactly an integer, as an `i32`.
///
/// Exactly, not nearly: half a pixel of translation is a genuine resample and
/// belongs on the sampled path.
fn whole(value: f64) -> Option<i32> {
    (value.fract() == 0.0 && value.abs() < f64::from(i32::MAX)).then(|| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "guarded above: integral and within i32"
        )]
        let n = value as i32;
        n
    })
}

/// The rasterizer's fill rule for the trait's.
fn to_scanline_rule(rule: FillRule) -> scanline::FillRule {
    match rule {
        FillRule::Winding => scanline::FillRule::NonZero,
        FillRule::EvenOdd => scanline::FillRule::EvenOdd,
    }
}

/// The integrator's coverage mode for the trait's antialiasing mode.
fn to_coverage(aa: AntiAlias) -> scanline::Coverage {
    match aa {
        AntiAlias::On => scanline::Coverage::Exact,
        AntiAlias::Off => scanline::Coverage::Thresholded,
        AntiAlias::FullCover => scanline::Coverage::Full,
    }
}

/// A rectangle as a closed path.
fn rect_path(rect: Rect) -> BezPath {
    rect.to_path(FLATTEN_TOLERANCE)
}

impl RenderDevice for ExactDevice {
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        let Some(src) = Self::solid(brush) else {
            return;
        };
        let device_path = t * path.clone();
        self.scan(&device_path, rule, aa, move |target, x, len, y, alpha| {
            target.blend_span(x, len, y, alpha, src, BlendMode::Normal);
        });
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        let Some(src) = Self::solid(brush) else {
            return;
        };
        // The stroke is expanded to an outline and *filled*, never stroked by
        // a stroker of this crate's own. That is not a shortcut: it makes the
        // stroke's geometry the engine's under every backend, which is the
        // same argument `pdfrum_render::stroke::outline` makes for a stroke
        // used as a clip, and it is why the two wrapped backends do it too.
        // A stroker of our own would be a third geometry to keep in step.
        let outline = kurbo::stroke(
            path.path_elements(FLATTEN_TOLERANCE),
            stroke,
            &kurbo::StrokeOpts::default(),
            FLATTEN_TOLERANCE,
        );
        let device_path = t * outline;
        // A stroke outline self-overlaps at joins and caps, so it must be
        // filled non-zero: even-odd would punch the overlaps back out.
        self.scan(
            &device_path,
            FillRule::Winding,
            aa,
            move |target, x, len, y, alpha| {
                target.blend_span(x, len, y, alpha, src, BlendMode::Normal);
            },
        );
    }

    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32) {
        let constant = pixmap::alpha_byte_truncating(alpha);
        if constant == 0 {
            return;
        }
        // A whole-pixel translation is a *blit*: every device pixel takes one
        // texel, the footprint's edges land exactly on pixel boundaries, and
        // the general path computes both facts the expensive way — an inverse
        // transform and two floors per pixel to recover an index that is a
        // subtraction, and an antialiased rasterization of a rectangle whose
        // coverage is everywhere 0 or 255.
        //
        // It is a fast path rather than a different answer, and
        // `an_integer_blit_agrees_with_the_general_path` pins that. It matters
        // because the glyph path blits one small image per glyph occurrence,
        // tens of thousands of times on a dense page, where the general path
        // costs more than filling the outline it replaced.
        if let Some((dx, dy)) = whole_pixel_offset(t) {
            self.blit(img, dx, dy, constant);
            return;
        }
        self.draw_image_sampled(img, t, quality, constant);
    }

    fn draw_glyph_lcd(
        &mut self,
        glyph: &pdfrum_render::glyph::SubpixelBitmap,
        origin: (f64, f64),
        colour: peniko::Color,
    ) {
        let [red, green, blue, alpha] = colour.to_rgba8().to_u8_array();
        if alpha == 0 || glyph.is_empty() {
            return;
        }
        // The origin is whole by construction — a snapped glyph origin plus a
        // box computed in whole pixels — so this is a blit, not a resample, and
        // a non-integer origin would mean the caller broke that invariant
        // rather than that this should interpolate.
        let (Some(dx), Some(dy)) = (whole(origin.0), whole(origin.1)) else {
            return;
        };
        let target = match self.layers.last_mut() {
            Some(layer) => &mut layer.target,
            None => &mut self.base,
        };
        for row in 0..glyph.height {
            let Some(y) = row.checked_add(dy) else {
                continue;
            };
            for col in 0..glyph.width {
                let Some(x) = col.checked_add(dx) else {
                    continue;
                };
                let coverage = glyph.at(col, row);
                if coverage == [0; 3] {
                    continue;
                }
                target.merge_lcd_pixel(x, y, [red, green, blue], alpha, coverage);
            }
        }
    }

    fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        let mask = self.coverage_of(path, rule, AntiAlias::On);
        self.push_clip_mask(mask);
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        // Hard-edged, per E2: PDFium's rect clips are integer-snapped and
        // aliased, and softening them would add a partial pixel along every
        // `re W n` edge in the corpus. The same integrator runs — only the
        // coverage-to-alpha step thresholds instead of scaling.
        let mask = self.coverage_of(&rect_path(rect), FillRule::Winding, AntiAlias::Off);
        self.push_clip_mask(mask);
    }

    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
        let (w, h) = self.size();
        // E8: a soft mask is device-sized and device-aligned. Both wrapped
        // backends fail *open* on a mismatch — one silently ignores it, the
        // other only warns — so the invariant is asserted here rather than
        // deferred, and a wrong size drops the mask rather than producing
        // silently unmasked output.
        let mask = mask.and_then(|m| {
            debug_assert_eq!(
                (m.width(), m.height()),
                (w, h),
                "an AlphaMask must be device-sized and device-aligned (SPEC §8)"
            );
            (m.width() == w && m.height() == h).then(|| m.clone())
        });
        let mut target = Target::new(w, h, peniko::Color::TRANSPARENT);
        // A layer inherits the clip in force, so geometry drawn inside it is
        // clipped once, on the way in, rather than again on the way out.
        target.set_clip(self.clips.last().and_then(Option::as_ref).cloned());
        self.layers.push(Layer {
            target,
            blend,
            alpha,
            mask,
        });
        self.frames.push(Frame::Layer);
    }

    fn pop(&mut self) {
        match self.frames.pop() {
            Some(Frame::Clip) => {
                if self.clips.len() > 1 {
                    self.clips.pop();
                }
                self.sync_clip();
            }
            Some(Frame::Layer) => {
                let Some(layer) = self.layers.pop() else {
                    return;
                };
                let mut pixels = layer.target.into_pixmap();
                // Alpha then mask, in that order: the group's constant alpha
                // scales what it painted, and the soft mask then selects from
                // the scaled result (ISO 32000 §11.6.4.4).
                pixels.multiply_alpha(layer.alpha);
                if let Some(mask) = &layer.mask {
                    pixels.multiply_alpha_mask(mask);
                }
                // The layer already carries the clip, applied on the way in.
                // Compositing it back through the clip a second time would
                // darken every clipped edge by the clip's own coverage
                // squared, so this blit runs unclipped.
                let (w, h) = (pixels.width(), pixels.height());
                let target = self.target();
                let saved = target.clip().cloned();
                target.set_clip(None);
                for y in 0..h {
                    for x in 0..w {
                        let Some(src) = pixels.pixel(x, y) else {
                            continue;
                        };
                        if src[3] == 0 {
                            continue;
                        }
                        let Ok(col) = i32::try_from(x) else { continue };
                        let Ok(row) = i32::try_from(y) else { continue };
                        target.blend_span(
                            col,
                            1,
                            row,
                            255,
                            Source::Premultiplied(src),
                            layer.blend,
                        );
                    }
                }
                target.set_clip(saved);
            }
            None => {}
        }
    }
}

impl RasterBackend for ExactBackend {
    type Device = ExactDevice;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        // The bound is the trait's, and it exists because `vello_cpu` sizes
        // its scenes with `u16`. This backend has no such limit, but Tier C
        // compares the three and a target one backend cannot allocate is not
        // comparable.
        let (w, h) = (w.min(MAX_TARGET_DIMENSION), h.min(MAX_TARGET_DIMENSION));
        ExactDevice::new(Target::new(w, h, clear))
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        ExactDevice::new(Target::from_pixmap(base.clone()))
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        debug_assert!(
            d.layers.is_empty(),
            "snapshot requires every layer popped (SPEC §8)"
        );
        d.base.pixels().clone()
    }

    fn finish(&self, mut d: Self::Device) -> Pixmap {
        // Flatten anything the engine left open rather than losing it.
        while !d.frames.is_empty() {
            d.pop();
        }
        d.base.into_pixmap()
    }
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

    const RED: peniko::Color = peniko::Color::from_rgba8(255, 0, 0, 255);

    #[test]
    fn a_cleared_target_keeps_its_colour() {
        let backend = ExactBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::WHITE);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn a_transparent_target_starts_empty() {
        let backend = ExactBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_half_covered_edge_is_exactly_half() {
        // The reason this crate exists: a supersampler quantises this to one
        // of seventeen levels, and the oracle writes 128.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 1, peniko::Color::TRANSPARENT);
        device.fill_path(
            &square(0.0, 0.0, 0.5, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::BLACK),
            FillRule::Winding,
            AntiAlias::On,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0).map(|px| px[3]), Some(128));
    }

    #[test]
    fn a_stroke_covers_both_sides_of_its_centre_line_equally() {
        // AGG writes 128 into both columns a unit-wide stroke on an integer x
        // half-covers. Expanding the outline and filling it is what keeps the
        // two sides symmetric.
        let backend = ExactBackend::new();
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
        // which AGG paints at alpha 64.
        let backend = ExactBackend::new();
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
    fn a_hard_edged_rect_clip_has_no_soft_pixels() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(8, 1, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.5, 0.0, 4.5, 1.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
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
    fn clips_nest_and_unwind() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
        device.push_clip_rect(Rect::new(2.0, 0.0, 8.0, 8.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        device.pop();
        // Only the 2..4 intersection is painted.
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0).map(|px| px[3]), Some(0));
        assert_eq!(out.pixel(3, 0).map(|px| px[3]), Some(255));
        assert_eq!(out.pixel(5, 0).map(|px| px[3]), Some(0));
    }

    #[test]
    fn popping_a_clip_restores_the_previous_one() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(8, 1, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 1.0));
        device.push_clip_rect(Rect::new(0.0, 0.0, 2.0, 1.0));
        device.pop();
        // Back to the outer clip: column 3 is visible again, column 5 is not.
        device.fill_path(
            &square(0.0, 0.0, 8.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        let out = backend.finish(device);
        assert_eq!(out.pixel(3, 0).map(|px| px[3]), Some(255));
        assert_eq!(out.pixel(5, 0).map(|px| px[3]), Some(0));
    }

    #[test]
    fn an_antialiased_path_clip_keeps_partial_coverage() {
        // `push_clip` is the soft one, unlike `push_clip_rect`: a clip edge
        // at a half pixel leaves a half-covered column.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 1, peniko::Color::TRANSPARENT);
        device.push_clip(&square(0.0, 0.0, 1.5, 1.0), FillRule::Winding);
        device.fill_path(
            &square(0.0, 0.0, 4.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::BLACK),
            FillRule::Winding,
            AntiAlias::On,
        );
        device.pop();
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0).map(|px| px[3]), Some(255));
        assert_eq!(out.pixel(1, 0).map(|px| px[3]), Some(128), "half clipped");
        assert_eq!(out.pixel(2, 0).map(|px| px[3]), Some(0));
    }

    #[test]
    fn a_layer_composites_with_its_blend_and_alpha() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(2, 2, peniko::Color::from_rgba8(0, 255, 0, 255));
        device.push_layer(BlendMode::Normal, 0.5, None);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 2.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
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
    fn a_layer_mask_must_be_device_sized() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let mask = AlphaMask::filled(4, 4, 128);
        device.push_layer(BlendMode::Normal, 1.0, Some(&mask));
        device.fill_path(
            &square(0.0, 0.0, 4.0, 4.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::WHITE),
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
    fn a_layer_inherits_the_clip_and_does_not_apply_it_twice() {
        // Applying the clip on the way in *and* on the way out would square a
        // partial clip's coverage: a half clip would come back as a quarter.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(2, 1, peniko::Color::TRANSPARENT);
        let mut half = AlphaMask::new(2, 1);
        half.data_mut().fill(128);
        device.push_clip(&square(0.0, 0.0, 2.0, 1.0), FillRule::Winding);
        device.push_layer(BlendMode::Normal, 1.0, None);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::BLACK),
            FillRule::Winding,
            AntiAlias::On,
        );
        device.pop();
        device.pop();
        let out = backend.finish(device);
        assert_eq!(
            out.pixel(0, 0).map(|px| px[3]),
            Some(255),
            "a full clip must leave the layer fully opaque"
        );
    }

    #[test]
    fn snapshot_then_backdrop_round_trips() {
        let backend = ExactBackend::new();
        let device = backend.new_target(3, 3, peniko::Color::from_rgba8(1, 2, 3, 255));
        let snap = backend.snapshot(&device);
        let seeded = backend.new_target_with_backdrop(&snap);
        let out = backend.finish(seeded);
        assert_eq!(out.pixel(1, 1), Some([1, 2, 3, 255]));
    }

    #[test]
    fn an_image_draws_at_its_pixel_grid() {
        // `t` maps the image's pixel grid, so an identity transform is a
        // texel-for-pixel blit at the origin -- which is what every engine
        // call site relies on, all of them passing a plain translation.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(2, 2, RED);
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 1.0);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 0, 0, 255]), "inside the image");
        assert_eq!(out.pixel(1, 1), Some([255, 0, 0, 255]), "inside the image");
        assert_eq!(
            out.pixel(2, 2).map(|px| px[3]),
            Some(0),
            "past its 2x2 grid"
        );
    }

    #[test]
    fn an_image_translates_by_whole_pixels() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(2, 2, RED);
        device.draw_image(
            &img,
            Affine::translate((2.0, 2.0)),
            ImageQuality::Nearest,
            1.0,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0).map(|px| px[3]), Some(0));
        assert_eq!(out.pixel(2, 2), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(3, 3), Some([255, 0, 0, 255]));
    }

    #[test]
    fn an_image_draw_with_alpha_does_not_panic() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(4, 4, RED);
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 0.5);
        let out = backend.finish(device);
        let px = out.pixel(0, 0).expect("in bounds");
        assert!(px[3] > 100 && px[3] < 160, "half-opacity image: {}", px[3]);
    }

    #[test]
    fn an_image_is_clipped_like_any_other_primitive() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 1, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(4, 1, RED);
        device.push_clip_rect(Rect::new(0.0, 0.0, 2.0, 1.0));
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 1.0);
        device.pop();
        let out = backend.finish(device);
        assert_eq!(out.pixel(1, 0).map(|px| px[3]), Some(255));
        assert_eq!(out.pixel(3, 0).map(|px| px[3]), Some(0));
    }

    #[test]
    fn finish_flattens_an_unpopped_layer() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(2, 2, peniko::Color::TRANSPARENT);
        device.push_layer(BlendMode::Normal, 1.0, None);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 2.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0).map(|px| px[0]), Some(255));
    }

    #[test]
    fn an_image_brush_on_a_fill_paints_nothing_rather_than_panicking() {
        // The engine routes image brushes through `draw_image`; a fill with
        // one is out of contract, and dropping it is the safe reading.
        let backend = ExactBackend::new();
        let img = Pixmap::filled(2, 2, RED);
        let mut device = backend.new_target(2, 2, peniko::Color::TRANSPARENT);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 2.0),
            Affine::IDENTITY,
            &Brush::Image(&img),
            FillRule::Winding,
            AntiAlias::Off,
        );
        let out = backend.finish(device);
        assert!(out.data().iter().all(|&b| b == 0));
    }

    #[test]
    fn a_zero_sized_target_survives_every_operation() {
        // The engine reaches this with clipped-away geometry; nothing here may
        // panic or allocate unboundedly.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(0, 0, peniko::Color::WHITE);
        device.push_clip_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        device.fill_path(
            &square(0.0, 0.0, 1.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::On,
        );
        device.pop();
        let out = backend.finish(device);
        assert_eq!((out.width(), out.height()), (0, 0));
    }

    #[test]
    fn a_transform_applies_to_a_filled_path() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.fill_path(
            &square(0.0, 0.0, 2.0, 2.0),
            Affine::translate((4.0, 4.0)),
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        let out = backend.finish(device);
        assert_eq!(
            out.pixel(0, 0).map(|px| px[3]),
            Some(0),
            "untranslated spot"
        );
        assert_eq!(
            out.pixel(5, 5).map(|px| px[3]),
            Some(255),
            "translated spot"
        );
    }

    #[test]
    fn a_degenerate_thin_fill_is_not_dropped() {
        // The failure mode the verification document records for tiny-skia:
        // a fill thinner than 1/4096 becomes a silent no-op there. An
        // analytic integrator has no such threshold — it paints the coverage
        // the geometry implies, however small.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 1, peniko::Color::TRANSPARENT);
        device.fill_path(
            &square(0.0, 0.0, 4.0, 0.02),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::BLACK),
            FillRule::Winding,
            AntiAlias::On,
        );
        let out = backend.finish(device);
        let alpha = out.pixel(0, 0).map_or(0, |px| px[3]);
        assert_eq!(alpha, 5, "0.02 coverage is floor(0.02 * 256) == 5");
    }

    /// A small image whose every texel differs from its neighbours, so a blit
    /// that shifted or repeated one would be visible.
    fn checkerboard(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let on = (x + y) % 2 == 0;
                let a = if on { 200 } else { 90 };
                p.set_pixel(x, y, [a / 2, a / 3, a / 4, a]);
            }
        }
        p
    }

    #[test]
    fn a_whole_pixel_transform_is_recognised_and_nothing_else_is() {
        assert_eq!(whole_pixel_offset(Affine::IDENTITY), Some((0, 0)));
        assert_eq!(
            whole_pixel_offset(Affine::translate((3.0, -7.0))),
            Some((3, -7))
        );
        // Half a pixel is a resample, not a blit.
        assert_eq!(whole_pixel_offset(Affine::translate((3.5, 0.0))), None);
        // So is any scale, however close to one.
        assert_eq!(whole_pixel_offset(Affine::scale(1.000_001)), None);
        assert_eq!(whole_pixel_offset(Affine::rotate(0.001)), None);
        // And a non-finite offset is not an offset.
        assert_eq!(whole_pixel_offset(Affine::translate((f64::NAN, 0.0))), None);
    }

    #[test]
    fn an_integer_blit_agrees_with_the_general_path() {
        // The property the fast path is allowed to exist on: it is faster and
        // it is not different. Both are run over the same image at the same
        // whole-pixel offset, and the general one is reached by asking for it
        // rather than by perturbing the transform, because a perturbed
        // transform would legitimately differ.
        let image = checkerboard(5, 4);
        let backend = ExactBackend::new();

        let mut fast = backend.new_target(12, 10, peniko::Color::WHITE);
        fast.draw_image(
            &image,
            Affine::translate((3.0, 2.0)),
            ImageQuality::Nearest,
            1.0,
        );
        let fast = backend.finish(fast);

        let mut slow_device = backend.new_target(12, 10, peniko::Color::WHITE);
        slow_device.draw_image_sampled(
            &image,
            Affine::translate((3.0, 2.0)),
            ImageQuality::Nearest,
            255,
        );
        let slow = backend.finish(slow_device);

        assert_eq!(fast.data(), slow.data(), "the blit is the same answer");
    }

    #[test]
    fn a_blit_respects_the_clip_and_the_edges_of_the_target() {
        let image = checkerboard(6, 6);
        let backend = ExactBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::WHITE);
        device.push_clip_rect(Rect::new(2.0, 2.0, 5.0, 5.0));
        // Deliberately hangs off the left and top, to exercise the clamp.
        device.draw_image(
            &image,
            Affine::translate((-1.0, -1.0)),
            ImageQuality::Nearest,
            1.0,
        );
        device.pop();
        let out = backend.finish(device);
        assert_eq!(
            out.pixel(1, 1),
            Some([255, 255, 255, 255]),
            "outside the clip"
        );
        assert_eq!(
            out.pixel(6, 6),
            Some([255, 255, 255, 255]),
            "outside the clip"
        );
        assert_ne!(out.pixel(3, 3), Some([255, 255, 255, 255]), "inside it");
    }

    #[test]
    fn a_partial_alpha_blit_scales_the_source_as_the_general_path_does() {
        let image = checkerboard(4, 4);
        let backend = ExactBackend::new();
        let mut fast = backend.new_target(6, 6, peniko::Color::WHITE);
        fast.draw_image(
            &image,
            Affine::translate((1.0, 1.0)),
            ImageQuality::Nearest,
            0.5,
        );
        let fast = backend.finish(fast);
        let mut slow = backend.new_target(6, 6, peniko::Color::WHITE);
        slow.draw_image_sampled(
            &image,
            Affine::translate((1.0, 1.0)),
            ImageQuality::Nearest,
            pdfrum_render::pixmap::alpha_byte_truncating(0.5),
        );
        let slow = backend.finish(slow);
        assert_eq!(fast.data(), slow.data());
    }

    /// One glyph pixel per column, with the stripes ramping across.
    fn lcd_strip(width: usize, stripes: &[[u8; 3]]) -> pdfrum_render::glyph::SubpixelBitmap {
        let mut channels = Vec::with_capacity(width * 3);
        for x in 0..width {
            let px = stripes.get(x % stripes.len()).copied().unwrap_or([0; 3]);
            channels.extend_from_slice(&px);
        }
        let width = i32::try_from(width).unwrap_or(0);
        pdfrum_render::glyph::SubpixelBitmap {
            left: 0,
            top: 0,
            width,
            height: 1,
            channels,
        }
    }

    #[test]
    fn a_clear_type_glyph_reaches_the_pixels_with_its_fringes_intact() {
        // The whole of M14 OWED item 2's pixel claim, through the trait rather
        // than through `Target`: a black glyph whose three stripes differ comes
        // out a *coloured* pixel, which no single-alpha image draw can produce.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(3, 1, peniko::Color::WHITE);
        device.draw_glyph_lcd(
            &lcd_strip(3, &[[255, 128, 0], [255, 255, 255], [0, 128, 255]]),
            (0.0, 0.0),
            peniko::Color::BLACK,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([0, 127, 255, 255]));
        assert_eq!(out.pixel(1, 0), Some([0, 0, 0, 255]), "no stripe survives");
        assert_eq!(out.pixel(2, 0), Some([255, 127, 0, 255]));
    }

    #[test]
    fn a_clear_type_glyph_lands_where_its_origin_says() {
        // The origin is the bitmap's top-left corner in whole device pixels,
        // the same convention `draw_image` uses for a glyph's gray blit — so a
        // run that switches between the two spellings must not shift.
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 2, peniko::Color::WHITE);
        device.draw_glyph_lcd(
            &lcd_strip(2, &[[255, 255, 255]]),
            (2.0, 1.0),
            peniko::Color::BLACK,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(out.pixel(2, 1), Some([0, 0, 0, 255]));
        assert_eq!(out.pixel(3, 1), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_clear_type_glyph_is_clipped_like_any_other_primitive() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(4, 1, peniko::Color::WHITE);
        device.push_clip_rect(Rect::new(0.0, 0.0, 2.0, 1.0));
        device.draw_glyph_lcd(
            &lcd_strip(4, &[[255, 255, 255]]),
            (0.0, 0.0),
            peniko::Color::BLACK,
        );
        device.pop();
        let out = backend.finish(device);
        assert_eq!(out.pixel(1, 0), Some([0, 0, 0, 255]), "inside the clip");
        assert_eq!(
            out.pixel(2, 0),
            Some([255, 255, 255, 255]),
            "outside the clip, untouched"
        );
    }

    #[test]
    fn an_invisible_clear_type_glyph_paints_nothing() {
        let backend = ExactBackend::new();
        let mut device = backend.new_target(2, 1, peniko::Color::WHITE);
        device.draw_glyph_lcd(
            &lcd_strip(2, &[[255, 255, 255]]),
            (0.0, 0.0),
            peniko::Color::TRANSPARENT,
        );
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
    }
}
