#![doc = include_str!("../README.md")]
// Why a third backend. The two existing backends are third-party rasterizers
// wrapped behind the trait, and each brings its own idea of what a partially
// covered pixel is worth. `tiny-skia` supersamples at four subsamples per axis,
// so a diagonal edge has seventeen distinct coverage levels and a half-covered
// pixel quantises to 8/16 of the range rather than to a half. Over the corpus
// that difference is a persistent few-count spread along every non-axis-aligned
// edge — measured as the population sitting between SSIM 0.95 and 0.99 with no
// image, shading, pattern or soft mask on the page. This backend closes it by
// construction: the same integral on the same 256ths-of-a-pixel grid, mapped to
// alpha through the same measured formula. `vello_cpu` remains the facade's
// default for API users, who want a fast production rasterizer rather than a
// byte-comparable one.
//
// Parity here means AGG parity — Anti-Grain Geometry, the scan converter PDFium
// draws with — over the coverage integral and the arithmetic downstream of it,
// and nothing wider.
//
// The design in one paragraph. `pdfrum_render::scanline` holds the rasterizer:
// kurbo flattens curves, each segment is integrated into per-pixel
// `(cover, area)` cells, and a sweep turns the sorted cells into spans of
// constant coverage. It lives in the engine rather than here because the
// engine's own glyph path needs the same integrator, and a glyph bitmap must
// not depend on which rasterizer draws the page. This crate's own two modules
// are private: `target` holds the pixel buffer, the clip — a coverage plane
// multiplied in per pixel, which is how the oracle's clip region works too —
// and the span blitter, and `image` holds the inverse-mapped image sampler. The
// compositing arithmetic itself is `pdfrum-render`'s, not this crate's:
// `blend::composite_premultiplied` is the one authority, so a pixel this
// backend blends and a pixel the engine blends in its own offscreen buffers
// agree by construction.
#![forbid(unsafe_code)]
// Every coordinate reaching this crate came from an untrusted file by way of
// the engine: index with `get()` and do arithmetic with `checked_*`.
#![warn(clippy::indexing_slicing)]

mod image;
mod target;

use std::sync::Arc;

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
/// A tenth of a pixel, which is five times finer than the half-pixel a
/// recursive bisecting subdivider would stop at. Matching such a subdivider's
/// number exactly would reproduce its *vertices* and still not its
/// subdivision, because it bisects recursively where kurbo places points
/// adaptively: the two agree on the curve rather than on the polyline, so
/// there is nothing to gain by matching the tolerance and something to lose.
///
/// Finer is the safer direction: the flattening error stays well below the
/// coverage quantisation the output byte imposes, so no edge can land a count
/// away because a chord cut a corner. It is also the tolerance the engine's
/// own stroke outlining already uses, so a stroke expanded for a clip and a
/// stroke expanded for a fill are the same polygon.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// Intersect `mask` with `other` over the half-open row range `rows`, as the
/// truncating integer product `old * new / 255` restricted to a band.
///
/// [`AlphaMask::intersect`] over the whole buffer is what this replaces, and
/// the two agree byte for byte whenever every row outside `rows` is zero in
/// `mask`: `mul255(0, b) == 0` for every `b`, so those rows are already the
/// product. [`AggDevice::coverage_of`] is the only producer and it reports the
/// band it wrote, so the precondition holds by construction. A mismatched size
/// declines, exactly as `intersect` does.
fn intersect_rows(mask: &mut AlphaMask, other: &AlphaMask, rows: core::ops::Range<u32>) {
    if other.width() != mask.width() || other.height() != mask.height() {
        return;
    }
    let width = mask.width() as usize;
    let Some(start) = (rows.start as usize).checked_mul(width) else {
        return;
    };
    let Some(end) = (rows.end as usize).checked_mul(width) else {
        return;
    };
    let Some(src) = other.data().get(start..end) else {
        return;
    };
    let Some(dest) = mask.data_mut().get_mut(start..end) else {
        return;
    };
    for (a, &b) in dest.iter_mut().zip(src) {
        *a = pixmap::mul255(*a, b);
    }
}

/// The analytic backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct AggBackend;

impl AggBackend {
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
pub struct AggDevice {
    base: Target,
    layers: Vec<Layer>,
    /// The clip stack, innermost last. `None` is "everything visible".
    ///
    /// Each plane is behind an `Arc` because the active target holds the same
    /// one — see `Target`'s `clip` field for why that share replaced a
    /// device-sized copy on every push and pop.
    clips: Vec<Option<Arc<AlphaMask>>>,
    /// The band of rows each entry of `clips` has non-zero coverage in, so a
    /// popped plane can be returned to `planes` by clearing that band alone.
    ///
    /// Parallel to `clips` and pushed and popped with it. The bottom entry is
    /// the `None` clip and its band is empty. An entry is an *upper bound* on
    /// the plane's non-zero rows, never an exact one — see
    /// [`AggDevice::coverage_of`].
    bands: Vec<core::ops::Range<u32>>,
    /// Device-sized coverage planes, every byte zero, waiting to be swept
    /// into by [`AggDevice::coverage_of`].
    ///
    /// A clip plane is the size of the device — half a megabyte on a letter
    /// page — and allocating one is `vec![0; len]`, which is a half-megabyte
    /// `memset` the allocator cannot hand over already zeroed once the page's
    /// first few are in flight. A page of annotation appearances pushes one
    /// clip per widget's `/BBox` plus whatever the appearance's own content
    /// pushes inside it, so `forms_combo_box` allocates two hundred and
    /// seventy-six of them per render for clips thirty rows tall.
    ///
    /// The pool holds the ones a `pop` reclaimed. The invariant that makes a
    /// reused plane indistinguishable from a fresh one is that **every byte in
    /// here is zero**: `recycle` clears the popped plane's band, which is an
    /// upper bound on where it is non-zero, before the plane lands here.
    planes: Vec<AlphaMask>,
    frames: Vec<Frame>,
    /// Reused across draws so a page of paths costs one allocation, not one
    /// per fill.
    raster: Rasterizer,
}

impl AggDevice {
    fn new(target: Target) -> Self {
        Self {
            base: target,
            layers: Vec::new(),
            clips: vec![None],
            // One entry, matching `clips`'s `None` bottom. Its band is
            // empty because that entry has no plane to reclaim. Spelt through
            // `from_iter` because `vec![0..0]` reads to clippy as a `Vec`
            // built *from* a range rather than a `Vec` holding one.
            bands: core::iter::once(0..0).collect(),
            planes: Vec::new(),
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
        // Every consumer below drops a span whose row is off the target, so
        // the rasterizer is told the range and never records those cells: a
        // clip path tens of thousands of rows tall otherwise sorts and sweeps
        // every one of them to have the answer thrown away.
        self.raster.keep_rows(0..target_rows(self.base.height()));
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
    /// Picking the layer is all this does; the walk itself is the target's,
    /// because the whole point is that a blit's column range, source offset and
    /// row band are the same on every row and are derived once rather than per
    /// row. The clip applies there, as the same coverage product every other
    /// primitive folds it in with, so a clipped blit lands on exactly the
    /// pixels a clipped image draw would.
    fn blit(&mut self, img: &RasterImage, dx: i32, dy: i32, alpha: u8) {
        let target = match self.layers.last_mut() {
            Some(layer) => &mut layer.target,
            None => &mut self.base,
        };
        target.blit_image(img, dx, dy, alpha);
    }

    /// A device-sized coverage plane with every byte zero, from the pool if
    /// one is waiting there and freshly allocated otherwise.
    ///
    /// The pool's invariant is that its planes are all-zero, so this is
    /// [`AlphaMask::new`] without the `memset` — see the `planes` field. They
    /// are also device-sized, because `size()` is fixed for the life of an
    /// `AggDevice` and every plane in the pool came from a `coverage_of` on
    /// this one; the check is what says so out loud, and a plane that somehow
    /// failed it is dropped rather than handed back at the wrong stride.
    fn blank_plane(&mut self, w: u32, h: u32) -> AlphaMask {
        match self.planes.pop() {
            Some(plane) if plane.width() == w && plane.height() == h => plane,
            _ => AlphaMask::new(w, h),
        }
    }

    /// Return a popped clip plane to the pool, zeroed.
    ///
    /// `band` is the range the entry's plane was recorded with, and it bounds
    /// the plane's non-zero rows above: `coverage_of` wrote only inside its own
    /// band and `push_clip_mask`'s intersection only narrowed that, so clearing
    /// the band restores the pool's all-zero invariant. Clearing thirty rows is
    /// what makes the reuse worth having; clearing the whole plane would be the
    /// `memset` this exists to avoid.
    ///
    /// The plane is reclaimed only when this stack entry held the **last**
    /// share of it. A layer pushed while the clip was in force holds one of its
    /// own (`push_layer`), and the active target holds one until `sync_clip`
    /// has run — so this is called after that, and `Arc::try_unwrap` declining
    /// simply means the plane is still somebody's and the pool does without it.
    fn recycle(&mut self, plane: Option<Arc<AlphaMask>>, band: core::ops::Range<u32>) {
        let Some(plane) = plane else {
            return;
        };
        let Ok(mut plane) = Arc::try_unwrap(plane) else {
            return;
        };
        let width = plane.width() as usize;
        let (Some(start), Some(end)) = (
            (band.start as usize).checked_mul(width),
            (band.end as usize).checked_mul(width),
        ) else {
            return;
        };
        let Some(rows) = plane.data_mut().get_mut(start..end) else {
            return;
        };
        rows.fill(0);
        self.planes.push(plane);
    }

    /// The coverage plane a path fills, device-sized — a clip — and the half-
    /// open band of rows the sweep actually wrote to.
    ///
    /// The plane is device-sized because that is what a clip is: [`Target`]
    /// indexes it by absolute device row and column. The *band* is what makes
    /// intersecting one cheap. Every row outside it is untouched, which for a
    /// zeroed mask means it is all zero, and `mul255(0, b)` is `0` for every
    /// `b` — so an intersection restricted to the band produces byte-for-byte
    /// the plane a whole-buffer one would. See [`AggDevice::push_clip_mask`].
    ///
    /// The plane comes from [`AggDevice::blank_plane`], which is the pool, and
    /// the pool's planes are zero for the same reason a fresh one is. The
    /// sweep *assigns* rather than accumulates — `*slot = alpha` — so a
    /// recycled plane's spans are the new path's, not a mixture; what the pool
    /// has to guarantee is only that the bytes the sweep does **not** touch are
    /// zero, which is exactly `recycle`'s postcondition.
    fn coverage_of(
        &mut self,
        path: &BezPath,
        rule: FillRule,
        aa: AntiAlias,
    ) -> (AlphaMask, core::ops::Range<u32>) {
        let (w, h) = self.size();
        let mut mask = self.blank_plane(w, h);
        self.raster.reset();
        self.raster.keep_rows(0..target_rows(h));
        self.raster.add_path(path, FLATTEN_TOLERANCE);
        let width = w as usize;
        // The sweep visits rows in order, but the band is folded rather than
        // read off the first and last call: a span whose columns all fall
        // outside the buffer writes nothing, and counting it would widen the
        // band past what was written. Widening is harmless to correctness —
        // the band is an upper bound on the non-zero rows — but the fold costs
        // nothing and keeps it tight.
        let mut first = h;
        let mut last = 0_u32;
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
                if x1 > x0 {
                    first = first.min(row);
                    last = last.max(row.saturating_add(1));
                }
                // A span is a run of *constant* alpha, so it is one `fill`
                // over the row's slice rather than a write per column. The
                // per-column spelling asked `data_mut()` for the whole buffer
                // and re-derived `row * width + col` on every pixel, inside a
                // bounds check the row's own slice already answers.
                let (Ok(x0u), Ok(x1u)) = (usize::try_from(x0), usize::try_from(x1)) else {
                    return;
                };
                let Some(start) = (row as usize).checked_mul(width) else {
                    return;
                };
                let (Some(lo), Some(hi)) = (start.checked_add(x0u), start.checked_add(x1u)) else {
                    return;
                };
                if let Some(span) = mask.data_mut().get_mut(lo..hi) {
                    span.fill(alpha);
                }
            },
        );
        (mask, first..last.max(first))
    }

    /// Push a clip that is the intersection of the current one with `mask`.
    ///
    /// `band` is the row range `coverage_of` wrote; every row outside it is
    /// zero, and zero survives the product. So the intersection runs over the
    /// band alone and the result is the same plane the whole-buffer spelling
    /// produced — on an annotation appearance's `/BBox`, thirty rows of a
    /// letter page's eight hundred.
    fn push_clip_mask(&mut self, mut mask: AlphaMask, band: core::ops::Range<u32>) {
        if let Some(current) = self.clips.last().and_then(Option::as_ref) {
            intersect_rows(&mut mask, current, band.clone());
        }
        self.clips.push(Some(Arc::new(mask)));
        // The intersection only narrows what the sweep wrote, so the sweep's
        // band still bounds this plane's non-zero rows — which is what
        // `recycle` needs when this entry is popped.
        self.bands.push(band);
        self.frames.push(Frame::Clip);
        self.sync_clip();
    }

    /// Point the active target at the innermost clip.
    ///
    /// A refcount bump, not a copy: see [`Target`]'s `clip` field for why the
    /// plane is shared. This runs on every clip push and on every pop, and a
    /// page of annotation appearances performs hundreds of them.
    fn sync_clip(&mut self) {
        let clip = self.clips.last().and_then(Option::as_ref).map(Arc::clone);
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

/// A device height as the exclusive row bound the rasterizer takes.
///
/// A target taller than `i32::MAX` rows cannot exist — its pixels would not
/// be addressable — so the saturating conversion is a total function rather
/// than a case anything reaches.
fn target_rows(height: u32) -> i32 {
    i32::try_from(height).unwrap_or(i32::MAX)
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

impl RenderDevice for AggDevice {
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
        let (mask, band) = self.coverage_of(path, rule, AntiAlias::On);
        self.push_clip_mask(mask, band);
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        // Hard-edged, per E2: PDFium's rect clips are integer-snapped and
        // aliased, and softening them would add a partial pixel along every
        // `re W n` edge in the corpus. The same integrator runs — only the
        // coverage-to-alpha step thresholds instead of scaling.
        let (mask, band) = self.coverage_of(&rect_path(rect), FillRule::Winding, AntiAlias::Off);
        self.push_clip_mask(mask, band);
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
                "an AlphaMask must be device-sized and device-aligned"
            );
            (m.width() == w && m.height() == h).then(|| m.clone())
        });
        let mut target = Target::new(w, h, peniko::Color::TRANSPARENT);
        // A layer inherits the clip in force, so geometry drawn inside it is
        // clipped once, on the way in, rather than again on the way out.
        target.set_clip(self.clips.last().and_then(Option::as_ref).map(Arc::clone));
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
                let popped = if self.clips.len() > 1 {
                    Some((self.clips.pop(), self.bands.pop()))
                } else {
                    None
                };
                // Before the reclaim, not after: the target holds a share of
                // the popped plane until this repoints it one level out, and
                // `recycle` takes the plane only when nothing else does.
                self.sync_clip();
                if let Some((Some(plane), Some(band))) = popped {
                    self.recycle(plane, band);
                }
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
                // squared, so this runs unclipped — which `composite_layer`
                // is, rather than by saving and restoring the target's clip
                // around a loop that would then have to re-read it per pixel.
                self.target().composite_layer(&pixels, layer.blend);
            }
            None => {}
        }
    }
}

impl RasterBackend for AggBackend {
    type Device = AggDevice;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        // The bound is the trait's, and it exists because `vello_cpu` sizes
        // its scenes with `u16`. This backend has no such limit, but Tier C
        // compares the three and a target one backend cannot allocate is not
        // comparable.
        let (w, h) = (w.min(MAX_TARGET_DIMENSION), h.min(MAX_TARGET_DIMENSION));
        AggDevice::new(Target::new(w, h, clear))
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        AggDevice::new(Target::from_pixmap(base.clone()))
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        debug_assert!(d.layers.is_empty(), "snapshot requires every layer popped");
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
        let backend = AggBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::WHITE);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn a_transparent_target_starts_empty() {
        let backend = AggBackend::new();
        let device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let out = backend.finish(device);
        assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_half_covered_edge_is_exactly_half() {
        // The reason this crate exists: a supersampler quantises this to one
        // of seventeen levels, and the oracle writes 128.
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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

    /// The row-`fill` spelling of [`AggDevice::coverage_of`]'s sweep writes
    /// exactly the plane the per-column one wrote.
    ///
    /// A span is a run of constant alpha, so filling the row's slice is the
    /// same bytes as assigning each column of it — but only if the slice is the
    /// right one, and the two spellings clamp differently on an out-of-range
    /// span: the per-column one wrote the in-range prefix of a row that
    /// overruns the buffer, and a row `fill` writes nothing at all. That case
    /// is unreachable — `x1` is already clamped to the width and `row` is
    /// rejected past the height, so `row * width + x1` never leaves the buffer
    /// — and this is what says the clamps really do bound it, over clips that
    /// leave the target on every side and a target whose width is not a
    /// multiple of anything.
    ///
    /// The comparison is against the plane a whole-device fill produces
    /// through the same push, so a spelling that silently dropped a row or a
    /// column would differ from itself under a clip that does not.
    #[test]
    fn a_clip_planes_spans_are_filled_over_exactly_their_own_rows() {
        let backend = AggBackend::new();
        for (x0, y0, x1, y1) in [
            (0.0, 0.0, 7.0, 5.0),
            (-4.0, -3.0, 3.0, 2.0),
            (3.0, 2.0, 40.0, 30.0),
            (-9.0, -9.0, 40.0, 30.0),
            (2.0, 1.0, 2.0, 1.0),
        ] {
            // 7x5 rather than a power of two, so a row's end and the buffer's
            // are not the same arithmetic.
            let mut device = backend.new_target(7, 5, peniko::Color::TRANSPARENT);
            device.push_clip_rect(Rect::new(x0, y0, x1, y1));
            device.fill_path(
                &square(-20.0, -20.0, 40.0, 40.0),
                Affine::IDENTITY,
                &Brush::Solid(RED),
                FillRule::Winding,
                AntiAlias::Off,
            );
            device.pop();
            let out = backend.finish(device);
            for row in 0..5 {
                for col in 0..7 {
                    let inside = f64::from(col) >= x0.max(0.0)
                        && f64::from(col) + 1.0 <= x1.min(7.0)
                        && f64::from(row) >= y0.max(0.0)
                        && f64::from(row) + 1.0 <= y1.min(5.0);
                    let a = out.pixel(col, row).map_or(0, |px| px[3]);
                    assert_eq!(
                        a,
                        u8::from(inside) * 255,
                        "({x0},{y0},{x1},{y1}) at ({col},{row})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_hard_edged_rect_clip_has_no_soft_pixels() {
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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

    /// The band `push_clip_mask` intersects over is an optimisation, and this
    /// is the case that would catch it being wrong: two clips whose row ranges
    /// **do not overlap at all**, so the intersection is empty and every byte
    /// of the answer lies outside the inner clip's band.
    ///
    /// A whole-buffer `intersect` produces an all-zero plane here. The banded
    /// one writes nothing outside rows 4..8 — and the plane is already zero
    /// there, because `coverage_of` allocates it zero and the sweep touched
    /// only that band. So the two agree, and nothing paints.
    #[test]
    fn a_clip_outside_the_previous_ones_rows_paints_nothing() {
        let backend = AggBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        // Rows 0..2, then rows 4..8: disjoint.
        device.push_clip_rect(Rect::new(0.0, 0.0, 8.0, 2.0));
        device.push_clip_rect(Rect::new(0.0, 4.0, 8.0, 8.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        device.pop();
        let out = backend.finish(device);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    out.pixel(x, y).map(|px| px[3]),
                    Some(0),
                    "({x}, {y}) painted through two disjoint clips"
                );
            }
        }
    }

    /// The banded intersection must agree with the whole-buffer one *inside*
    /// the overlap too, byte for byte — the outer clip's coverage has to reach
    /// the inner plane's rows.
    ///
    /// Two rectangles overlapping in rows 2..4 and in columns 2..4. Only that
    /// square may paint: a band that lost the outer clip's columns would leave
    /// the whole of rows 2..4 visible.
    #[test]
    fn a_banded_intersection_still_carries_the_outer_clips_columns() {
        let backend = AggBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 4.0));
        device.push_clip_rect(Rect::new(2.0, 2.0, 8.0, 8.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        device.pop();
        let out = backend.finish(device);
        for y in 0..8 {
            for x in 0..8 {
                let inside = (2..4).contains(&x) && (2..4).contains(&y);
                assert_eq!(
                    out.pixel(x, y).map(|px| px[3]),
                    Some(if inside { 255 } else { 0 }),
                    "({x}, {y}) disagrees with the two clips' intersection"
                );
            }
        }
    }

    /// Popping a clip must hand the target the *previous* plane, not a stale
    /// share of the one just popped. The `Arc` turned `sync_clip` from a
    /// device-sized copy into a refcount bump, and this is what says the bump
    /// points at the right plane.
    ///
    /// `popping_a_clip_restores_the_previous_one` covers the same unwind on
    /// one level; this one nests four deep and checks every level on the way
    /// out, because a share that lagged by one would still pass a single pop.
    #[test]
    fn each_pop_hands_the_target_the_plane_one_level_out() {
        let backend = AggBackend::new();
        let widths = [8.0_f64, 6.0, 4.0, 2.0];
        // At each depth, everything left of `widths[depth]` paints and
        // everything from it rightwards does not.
        for (depth, &edge) in widths.iter().enumerate() {
            let mut device = backend.new_target(8, 1, peniko::Color::TRANSPARENT);
            for w in &widths {
                device.push_clip_rect(Rect::new(0.0, 0.0, *w, 1.0));
            }
            for _ in 0..(widths.len() - 1 - depth) {
                device.pop();
            }
            device.fill_path(
                &square(0.0, 0.0, 8.0, 1.0),
                Affine::IDENTITY,
                &Brush::Solid(RED),
                FillRule::Winding,
                AntiAlias::Off,
            );
            let out = backend.finish(device);
            for x in 0..8 {
                let inside = f64::from(x) < edge;
                assert_eq!(
                    out.pixel(x, 0).map(|px| px[3]),
                    Some(if inside { 255 } else { 0 }),
                    "column {x} at depth {depth} (clip edge {edge})"
                );
            }
        }
    }

    /// A recycled plane must be indistinguishable from a fresh one, and the
    /// only way it is not is if `recycle` failed to clear a byte the next
    /// sweep does not overwrite.
    ///
    /// Push a clip over rows 0..4, pop it, then push one over rows 4..8. The
    /// second push takes the first's plane out of the pool. If its rows 0..4
    /// were still carrying the first clip's coverage, the fill below would
    /// paint through them — the second clip does not sweep there, so nothing
    /// would ever clear them.
    #[test]
    fn a_recycled_plane_carries_none_of_the_clip_it_held() {
        let backend = AggBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 8.0, 4.0));
        device.pop();
        device.push_clip_rect(Rect::new(0.0, 4.0, 8.0, 8.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        let out = backend.finish(device);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    out.pixel(x, y).map(|px| px[3]),
                    Some(if y >= 4 { 255 } else { 0 }),
                    "({x}, {y}) disagrees with the second clip alone"
                );
            }
        }
    }

    /// The plane a *nested* clip recycles carries the intersection, not the
    /// sweep — narrower than its band, and still entirely inside it.
    ///
    /// Rows 2..6 intersected with rows 0..4 leaves 2..4 non-zero, recorded
    /// under the band 2..6. Clearing the band clears the intersection with
    /// room to spare; clearing anything narrower would not. The third clip
    /// then reuses that plane over rows 0..2, which touches none of 2..6.
    #[test]
    fn a_recycled_nested_plane_is_cleared_over_its_whole_band() {
        let backend = AggBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 8.0, 4.0));
        device.push_clip_rect(Rect::new(0.0, 2.0, 8.0, 6.0));
        device.pop();
        device.push_clip_rect(Rect::new(0.0, 0.0, 8.0, 2.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        device.pop();
        let out = backend.finish(device);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    out.pixel(x, y).map(|px| px[3]),
                    Some(if y < 2 { 255 } else { 0 }),
                    "({x}, {y}) disagrees with rows 0..4 and rows 0..2"
                );
            }
        }
    }

    /// A layer opened over a clip, and a clip opened inside a layer, both
    /// unwind with the right plane in force at every step.
    ///
    /// `push_layer` clones the clip `Arc` into the layer's target, so the two
    /// hold the same plane for as long as the layer is open. `frames` is one
    /// LIFO, so a layer opened after a clip is always popped before it and the
    /// share is gone by the time `recycle` looks — which is why the reclaim's
    /// `Arc::try_unwrap` never declines on the corpus (measured: zero declines
    /// over all forty-four `benches/corpus` documents). It is kept anyway,
    /// because `recycle` must not be the thing that makes a future share
    /// unsound, and because it costs one already-loaded refcount. This test is
    /// the shape it guards: nested clips and layers, checked on the way out.
    #[test]
    fn a_clip_under_a_layer_unwinds_with_the_layer_between_them() {
        let backend = AggBackend::new();
        let mut device = backend.new_target(8, 8, peniko::Color::TRANSPARENT);
        device.push_clip_rect(Rect::new(0.0, 0.0, 8.0, 4.0));
        device.push_layer(BlendMode::Normal, 1.0, None);
        device.push_clip_rect(Rect::new(0.0, 2.0, 8.0, 8.0));
        device.fill_path(
            &square(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        device.pop();
        // Back under the outer clip alone, and drawing again must reach rows
        // 0..2 — which the inner clip excluded and which a recycled plane
        // would have carried its coverage into.
        device.fill_path(
            &square(0.0, 0.0, 8.0, 1.0),
            Affine::IDENTITY,
            &Brush::Solid(RED),
            FillRule::Winding,
            AntiAlias::Off,
        );
        device.pop();
        let out = backend.finish(device);
        for y in 0..8 {
            for x in 0..8 {
                let inside = (2..4).contains(&y) || y < 1;
                assert_eq!(
                    out.pixel(x, y).map(|px| px[3]),
                    Some(if inside { 255 } else { 0 }),
                    "({x}, {y}) disagrees with the clips the two fills ran under"
                );
            }
        }
    }

    #[test]
    fn popping_a_clip_restores_the_previous_one() {
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
        let mut device = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        let img = Pixmap::filled(4, 4, RED);
        device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 0.5);
        let out = backend.finish(device);
        let px = out.pixel(0, 0).expect("in bounds");
        assert!(px[3] > 100 && px[3] < 160, "half-opacity image: {}", px[3]);
    }

    #[test]
    fn an_image_is_clipped_like_any_other_primitive() {
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();

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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        // Pixel claim, through the trait rather
        // than through `Target`: a black glyph whose three stripes differ comes
        // out a *coloured* pixel, which no single-alpha image draw can produce.
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
        let backend = AggBackend::new();
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
