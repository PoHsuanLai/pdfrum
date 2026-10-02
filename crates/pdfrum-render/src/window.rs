//! A backend whose root target is a window onto a larger frame.
//!
//! A region render keeps every decision the engine makes — which pixels an
//! object covers, how a glyph snaps, where a group's buffer starts — in the
//! frame a whole-page render uses, so that each is computed from the same
//! numbers and a tile's pixel is the whole page's. Only the target is
//! smaller. This wrapper is where the two meet: it takes calls in the whole
//! frame and hands the real backend the same calls moved by the window's
//! corner, which is a whole number of pixels.
//!
//! Targets made by [`RasterBackend::new_target`] have no offset: the engine
//! makes them in their own local frame, as in a whole render, and only the
//! root target made by [`WindowBackend::new_window`] is shifted.

use kurbo::{Affine, BezPath, Rect, Stroke, Vec2};
use pdfrum_page::BlendMode;

use crate::device::{AntiAlias, Brush, FillRule, ImageQuality, RasterBackend, RenderDevice};
use crate::pixmap::{AlphaMask, Pixmap};

/// `B`, with a root target that is a window onto the whole page's frame.
pub(crate) struct WindowBackend<'a, B>(pub(crate) &'a B);

/// A target of `B`, whose calls arrive in a frame `offset` pixels away.
pub(crate) struct WindowDevice<D> {
    pub(crate) inner: D,
    /// Where the target's corner is in the frame its calls are made in.
    offset: Vec2,
}

impl<B: RasterBackend> WindowBackend<'_, B> {
    /// A `w` x `h` target whose top-left pixel is (`x`, `y`) in the frame the
    /// engine draws in.
    pub(crate) fn new_window(
        &self,
        w: u32,
        h: u32,
        clear: peniko::Color,
        origin: (u32, u32),
    ) -> WindowDevice<B::Device> {
        WindowDevice {
            inner: self.0.new_target(w, h, clear),
            offset: Vec2::new(f64::from(origin.0), f64::from(origin.1)),
        }
    }
}

impl<D> WindowDevice<D> {
    fn to_local(&self) -> Affine {
        Affine::translate(-self.offset)
    }

    fn is_shifted(&self) -> bool {
        self.offset != Vec2::ZERO
    }
}

impl<D: RenderDevice> RenderDevice for WindowDevice<D> {
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        let shift = self.to_local();
        self.inner.fill_path(path, shift * t, brush, rule, aa);
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        let shift = self.to_local();
        self.inner.stroke_path(path, shift * t, brush, stroke, aa);
    }

    fn draw_image(&mut self, img: &Pixmap, t: Affine, quality: ImageQuality, alpha: f32) {
        let shift = self.to_local();
        self.inner.draw_image(img, shift * t, quality, alpha);
    }

    fn draw_glyph_lcd(
        &mut self,
        glyph: &crate::glyph::SubpixelBitmap,
        origin: (f64, f64),
        colour: peniko::Color,
    ) {
        self.inner.draw_glyph_lcd(
            glyph,
            (origin.0 - self.offset.x, origin.1 - self.offset.y),
            colour,
        );
    }

    fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        if self.is_shifted() {
            self.inner
                .push_clip(&(self.to_local() * path.clone()), rule);
        } else {
            self.inner.push_clip(path, rule);
        }
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        self.inner.push_clip_rect(rect - self.offset);
    }

    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
        self.inner.push_layer(blend, alpha, mask);
    }

    fn pop(&mut self) {
        self.inner.pop();
    }
}

impl<B: RasterBackend> RasterBackend for WindowBackend<'_, B> {
    type Device = WindowDevice<B::Device>;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        self.new_window(w, h, clear, (0, 0))
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        WindowDevice {
            inner: self.0.new_target_with_backdrop(base),
            offset: Vec2::ZERO,
        }
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        self.0.snapshot(&d.inner)
    }

    fn finish(&self, d: Self::Device) -> Pixmap {
        self.0.finish(d.inner)
    }

    fn snapshot_rect(
        &self,
        d: &Self::Device,
        origin_x: u32,
        origin_y: u32,
        width: u32,
        height: u32,
    ) -> Pixmap {
        // A rectangle in the whole frame, inside the window by construction:
        // every group's rectangle is clipped to the window's box.
        let local_x = f64::from(origin_x) - d.offset.x;
        let local_y = f64::from(origin_y) - d.offset.y;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the rectangle lies inside the window, so both are whole \
                      and non-negative; a violation saturates to the corner \
                      rather than wrapping"
        )]
        self.0
            .snapshot_rect(&d.inner, local_x as u32, local_y as u32, width, height)
    }

    fn composite_isolated_groups_as_layers(&self) -> bool {
        self.0.composite_isolated_groups_as_layers()
    }
}
