//! The [`RasterBackend`] that turns a page render into SVG.
//!
//! One backend wrapping another. Every call still reaches the wrapped
//! rasterizer, which is what keeps `snapshot`, `finish` and the pixel
//! arithmetic the engine does on their results working exactly as before —
//! `remove_backdrop`, `knockout_over`, `luminosity_mask` and the rest are
//! untouched, because the pixels they operate on are still real.
//!
//! What is added is a second recording of the *root* target's calls, as SVG,
//! and a note of every offscreen target's shape so a pixel region blitted
//! onto the root can say why it is pixels.

use std::cell::RefCell;
use std::rc::Rc;

use kurbo::{Affine, BezPath, Rect, Stroke};
use pdfrum_page::BlendMode;
use pdfrum_render::{
    AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, Pixmap, RasterBackend, RasterImage,
    RenderDevice,
};

use crate::doc::Svg;
use crate::evidence::{Fingerprint, Seed, Witness, classify};
use crate::report::{RasterRegion, RasterReport};

/// The document and the pending evidence, shared between the backend and the
/// devices it makes.
///
/// A `RefCell` because `RasterBackend::new_target` takes `&self` and
/// `finish` consumes a device without seeing the root — the two halves of the
/// recording live on opposite sides of the trait, and the trait is not
/// changing. Borrows are taken for the length of one statement and never
/// nest, so the cell cannot be re-entered. `pdfrum-raster-vello` holds its
/// renderer the same way and for the same reason.
#[derive(Debug)]
struct Shared {
    /// `None` until the engine asks for its first target, which is the page.
    svg: Option<Svg>,
    /// The report being accumulated.
    report: RasterReport,
    /// Offscreen targets that have closed since the last root draw.
    pending: Vec<Witness>,
}

/// A [`RasterBackend`] that records the page as SVG while `inner` rasterizes
/// it.
///
/// Rendering a page through this and then taking [`Self::into_svg`] is the
/// whole API; `pdfrum_svg::page_to_svg` wraps the two calls.
///
/// ```
/// use pdfrum_raster_tinyskia::TinySkiaBackend;
/// use pdfrum_svg::SvgBackend;
///
/// let tiny = TinySkiaBackend::new();
/// let backend = SvgBackend::new(&tiny);
/// // ... render a page on `&backend` ...
/// let (svg, report) = backend.into_svg();
/// assert!(report.is_empty(), "nothing was drawn, so nothing rasterized");
/// assert!(svg.is_empty(), "and no target was ever asked for");
/// ```
#[derive(Debug)]
pub struct SvgBackend<'a, B> {
    inner: &'a B,
    shared: Rc<RefCell<Shared>>,
}

impl<'a, B> SvgBackend<'a, B> {
    /// Wrap `inner`.
    ///
    /// ```
    /// use pdfrum_raster_tinyskia::TinySkiaBackend;
    /// use pdfrum_svg::SvgBackend;
    ///
    /// let tiny = TinySkiaBackend::new();
    /// let backend = SvgBackend::new(&tiny);
    /// let (svg, _) = backend.into_svg();
    /// assert_eq!(svg, "");
    /// ```
    pub fn new(inner: &'a B) -> Self {
        Self {
            inner,
            shared: Rc::new(RefCell::new(Shared {
                svg: None,
                report: RasterReport::default(),
                pending: Vec::new(),
            })),
        }
    }

    /// The finished document and its rasterized-region report.
    ///
    /// An empty string when no page was rendered — the backend never learned
    /// a size, so there is no document to write.
    ///
    /// ```
    /// use pdfrum_raster_tinyskia::TinySkiaBackend;
    /// use pdfrum_svg::SvgBackend;
    ///
    /// let tiny = TinySkiaBackend::new();
    /// let (svg, report) = SvgBackend::new(&tiny).into_svg();
    /// assert!(svg.is_empty() && report.is_empty());
    /// ```
    #[must_use]
    pub fn into_svg(self) -> (String, RasterReport) {
        let mut shared = self.shared.borrow_mut();
        let report = core::mem::take(&mut shared.report);
        let svg = shared.svg.take().map(Svg::finish).unwrap_or_default();
        (svg, report)
    }
}

/// Which of the two kinds of target a device is.
///
/// An enum rather than an `is_root` flag, because the root owns a document
/// and an offscreen owns a fingerprint and neither has any use for the
/// other's field. Private: it names a state a caller can
/// neither create nor act on.
#[derive(Debug)]
enum Role {
    /// The page. Every call is both rasterized and written to the document.
    Root,
    /// An offscreen buffer, rasterized as before and watched. Carries what
    /// has been drawn into it, which it contributes when it closes.
    Offscreen(Fingerprint),
}

/// A render target made by [`SvgBackend`].
///
/// Opaque: the engine holds it, calls [`RenderDevice`] methods on it and
/// hands it back to the backend, and never looks inside. Every call reaches
/// the wrapped rasterizer, so the pixels are exactly the ones the wrapped
/// backend would have produced on its own.
#[derive(Debug)]
pub struct SvgDevice<D> {
    /// The wrapped rasterizer's device, which still does all the pixels.
    inner: D,
    /// Whether this is the page or an offscreen buffer.
    role: Role,
    /// The shared document, report and pending-witness list.
    shared: Rc<RefCell<Shared>>,
}

impl<D> SvgDevice<D> {
    /// Run `f` on the document when this is the root, and do nothing
    /// otherwise.
    fn record(&mut self, f: impl FnOnce(&mut Svg)) {
        if matches!(self.role, Role::Root)
            && let Some(svg) = self.shared.borrow_mut().svg.as_mut()
        {
            f(svg);
        }
    }

    /// Note a draw against this target's fingerprint, when it has one.
    fn note(&mut self, f: impl FnOnce(&mut Fingerprint)) {
        if let Role::Offscreen(seen) = &mut self.role {
            seen.draws += 1;
            f(seen);
        }
    }
}

/// The colour a brush paints with, or `None` for an image brush.
///
/// An image brush reaches `fill_path` only from the tiling-pattern path,
/// which this crate rasterizes as a whole region, so the root document never
/// needs to express one — but the fill still has to be *drawn*, which the
/// wrapped rasterizer does either way.
fn brush_color(brush: &Brush<'_>) -> Option<peniko::Color> {
    match brush {
        Brush::Solid(c) => Some(*c),
        Brush::Image(_) => None,
    }
}

impl<D: RenderDevice> RenderDevice for SvgDevice<D> {
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        self.inner.fill_path(path, t, brush, rule, aa);
        self.note(|f| {
            f.drew.fills = true;
            f.full_cover_fill |= matches!(aa, AntiAlias::FullCover);
        });
        if let Some(color) = brush_color(brush) {
            self.record(|svg| svg.fill_path(path, t, color, rule, aa));
        }
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        self.inner.stroke_path(path, t, brush, stroke, aa);
        self.note(|f| f.drew.strokes = true);
        if let Some(color) = brush_color(brush) {
            self.record(|svg| svg.stroke_path(path, t, color, stroke, aa));
        }
    }

    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32) {
        self.inner.draw_image(img, t, quality, alpha);
        self.note(|f| f.drew.images = true);
        if !matches!(self.role, Role::Root) {
            return;
        }
        // The encoding is done before the borrow, because it is the expensive
        // part and holding the cell across it would serialise nothing useful.
        // A failure means the pixmap will not encode: the region is *still*
        // reported, because a region silently lost is exactly what the report
        // exists to prevent — it just has no element.
        //
        let png = img.encode_png().ok();

        // Classification and recording happen under one borrow: the pending
        // witnesses are this draw's evidence and are consumed by it, so the
        // next root draw starts from nothing.
        let mut shared = self.shared.borrow_mut();
        let cause = classify(&core::mem::take(&mut shared.pending));
        shared.report.push(RasterRegion {
            bounds: image_bounds(img, t),
            cause,
        });
        if let (Some(png), Some(svg)) = (png, shared.svg.as_mut()) {
            svg.draw_raster_region(&png, img.width(), img.height(), t, alpha, cause);
        }
    }

    fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        self.inner.push_clip(path, rule);
        self.record(|svg| svg.push_clip(path, rule));
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        self.inner.push_clip_rect(rect);
        self.record(|svg| svg.push_clip_rect(rect));
    }

    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
        self.inner.push_layer(blend, alpha, mask);
        // A mask on a layer is a device-sized coverage plane. The engine only
        // ever passes `None` here — every mask it has is multiplied into a
        // pixmap's alpha channel instead, which is why `SoftMask` is a raster
        // cause — so this arm is the contract rather than a fallback.
        self.record(|svg| svg.push_layer(blend, alpha));
    }

    fn pop(&mut self) {
        self.inner.pop();
        self.record(Svg::pop);
    }
}

/// The device-space rectangle `img` covers under `t`.
///
/// `t` maps the image's own pixel grid, so the extent is the image's pixel
/// rectangle transformed — the same reading `RenderDevice::draw_image`
/// documents.
fn image_bounds(img: &RasterImage, t: Affine) -> Rect {
    t.transform_rect_bbox(Rect::new(
        0.0,
        0.0,
        f64::from(img.width()),
        f64::from(img.height()),
    ))
}

impl<B: RasterBackend> RasterBackend for SvgBackend<'_, B> {
    type Device = SvgDevice<B::Device>;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        let inner = self.inner.new_target(w, h, clear);
        // The engine asks for the page before anything else — `render_page`
        // allocates the root target and only then walks the objects — so the
        // first target is the root and every later one is offscreen.
        let role = {
            let mut shared = self.shared.borrow_mut();
            if shared.svg.is_none() {
                shared.svg = Some(Svg::new(w, h));
                Role::Root
            } else {
                Role::Offscreen(Fingerprint::default())
            }
        };
        SvgDevice {
            inner,
            role,
            shared: Rc::clone(&self.shared),
        }
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        // Only a non-isolated transparency group asks for one of these, and
        // it is never the page, so this is always offscreen.
        SvgDevice {
            inner: self.inner.new_target_with_backdrop(base),
            role: Role::Offscreen(Fingerprint {
                seed: Seed::Backdrop,
                ..Fingerprint::default()
            }),
            shared: Rc::clone(&self.shared),
        }
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        self.inner.snapshot(&d.inner)
    }

    fn finish(&self, d: Self::Device) -> Pixmap {
        if let Role::Offscreen(seen) = d.role {
            d.shared.borrow_mut().pending.push(seen);
        }
        self.inner.finish(d.inner)
    }
}

#[cfg(test)]
mod tests {
    use pdfrum_raster_tinyskia::TinySkiaBackend;

    use super::*;

    fn square(size: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((size, 0.0));
        p.line_to((size, size));
        p.line_to((0.0, size));
        p.close_path();
        p
    }

    #[test]
    fn the_first_target_is_the_root_and_the_rest_are_offscreen() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let root = backend.new_target(8, 8, peniko::Color::WHITE);
        assert!(matches!(root.role, Role::Root));
        let sub = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        assert!(matches!(sub.role, Role::Offscreen(_)));
        drop(backend.finish(sub));
        drop(backend.finish(root));
    }

    #[test]
    fn a_root_fill_reaches_both_the_pixels_and_the_document() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let mut root = backend.new_target(8, 8, peniko::Color::WHITE);
        root.fill_path(
            &square(8.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
            FillRule::Winding,
            AntiAlias::Off,
        );
        let pixels = backend.finish(root);
        assert_eq!(
            pixels.pixel(4, 4),
            Some([255, 0, 0, 255]),
            "still rasterized"
        );
        let (svg, report) = backend.into_svg();
        assert!(svg.contains("fill=\"#ff0000\""));
        assert!(report.is_empty(), "a fill is not a raster region");
    }

    #[test]
    fn an_offscreen_fill_stays_out_of_the_document() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let root = backend.new_target(8, 8, peniko::Color::WHITE);
        let mut sub = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        sub.fill_path(
            &square(4.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(0, 255, 0, 255)),
            FillRule::Winding,
            AntiAlias::On,
        );
        drop(backend.finish(sub));
        drop(backend.finish(root));
        let (svg, _) = backend.into_svg();
        assert!(!svg.contains("#00ff00"), "the offscreen is not the page");
    }

    #[test]
    fn an_offscreen_subtree_blitted_back_is_reported_as_a_composite() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let mut root = backend.new_target(8, 8, peniko::Color::WHITE);
        let mut sub = backend.new_target(4, 4, peniko::Color::TRANSPARENT);
        sub.fill_path(
            &square(4.0),
            Affine::IDENTITY,
            &Brush::Solid(peniko::Color::from_rgba8(0, 0, 255, 255)),
            FillRule::Winding,
            AntiAlias::On,
        );
        let pixels = backend.finish(sub);
        root.draw_image(
            &pixels,
            Affine::translate((2.0, 2.0)),
            ImageQuality::Nearest,
            1.0,
        );
        drop(backend.finish(root));

        let (svg, report) = backend.into_svg();
        assert_eq!(
            report.counts(),
            vec![(crate::RasterCause::CompositedGroup, 1)]
        );
        assert_eq!(
            report.regions().first().map(|r| r.bounds),
            Some(Rect::new(2.0, 2.0, 6.0, 6.0))
        );
        assert!(svg.contains("data-cause=\"composited-group\""));
    }

    #[test]
    fn an_image_with_no_offscreen_behind_it_is_a_sampled_source() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let mut root = backend.new_target(8, 8, peniko::Color::WHITE);
        let img = Pixmap::filled(2, 2, peniko::Color::from_rgba8(1, 2, 3, 255));
        root.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 1.0);
        drop(backend.finish(root));
        let (_, report) = backend.into_svg();
        assert_eq!(
            report.counts(),
            vec![(crate::RasterCause::SampledSource, 1)]
        );
    }

    #[test]
    fn a_backdrop_target_reports_a_non_isolated_group() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let mut root = backend.new_target(8, 8, peniko::Color::WHITE);
        let base = backend.snapshot(&root);
        let sub = backend.new_target_with_backdrop(&base);
        let pixels = backend.finish(sub);
        root.draw_image(&pixels, Affine::IDENTITY, ImageQuality::Nearest, 1.0);
        drop(backend.finish(root));
        let (_, report) = backend.into_svg();
        assert_eq!(
            report.counts(),
            vec![(crate::RasterCause::NonIsolatedGroup, 1)]
        );
    }

    #[test]
    fn clips_and_layers_nest_and_close_on_the_root() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let mut root = backend.new_target(8, 8, peniko::Color::WHITE);
        root.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
        root.push_layer(BlendMode::Multiply, 0.5, None);
        root.pop();
        root.pop();
        drop(backend.finish(root));
        let (svg, _) = backend.into_svg();
        assert!(svg.contains("mix-blend-mode:multiply"));
        assert_eq!(svg.matches("</g>").count(), 2);
    }

    #[test]
    fn the_evidence_is_consumed_by_the_draw_it_explains() {
        let tiny = TinySkiaBackend::new();
        let backend = SvgBackend::new(&tiny);
        let mut root = backend.new_target(8, 8, peniko::Color::WHITE);
        let sub = backend.new_target(2, 2, peniko::Color::TRANSPARENT);
        let pixels = backend.finish(sub);
        root.draw_image(&pixels, Affine::IDENTITY, ImageQuality::Nearest, 1.0);
        // A second blit with no offscreen work of its own must not inherit
        // the first one's witnesses.
        root.draw_image(
            &pixels,
            Affine::translate((4.0, 0.0)),
            ImageQuality::Nearest,
            1.0,
        );
        drop(backend.finish(root));
        let (_, report) = backend.into_svg();
        assert_eq!(
            report.counts(),
            vec![
                (crate::RasterCause::CompositedGroup, 1),
                (crate::RasterCause::SampledSource, 1)
            ]
        );
    }
}
