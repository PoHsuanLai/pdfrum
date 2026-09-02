//! What a render is parameterised by. *Which* rasterizer performs it is an
//! argument, not an option — see [`Page::render_on`](crate::Page::render_on).
//!
//! # Where `Backend` went
//!
//! *Withdrawn 2026-09-02.* This module used to own
//! `pub enum Backend { VelloCpu, TinySkia, Agg }` and a
//! `RenderOptions::backend` field, and [`Page::render`](crate::Page::render)
//! dispatched on it through a three-arm match. That flattened a seam the
//! engine below already had: `pdfrum_render::RasterBackend` is a trait with
//! an associated `Device`, and `render_page_with_caches` is generic over it.
//! The facade's enum could only ever name the rasterizers the *facade*
//! depended on — so every caller of `cargo add pdfrum` compiled three of
//! them, and no caller could ever pass a fourth.
//!
//! The backend is now a **parameter**: [`Page::render_on`](crate::Page::render_on),
//! [`Page::render_with_on`](crate::Page::render_with_on) and
//! [`Page::render_session_on`](crate::Page::render_session_on) each take
//! `&B where B: RasterBackend`. [`Page::render`](crate::Page::render) and its
//! two siblings keep their signatures and mean
//! [`VelloCpuBackend`](crate::VelloCpuBackend), the facade's default and its
//! one rasterizer dependency. `tiny-skia` and the AGG-parity backend are now
//! the caller's own dependency, named directly.
//!
//! **Const generics were considered and rejected:** a `const` parameter
//! cannot carry a `wgpu` device, so a const-selected backend could never
//! name the GPU one — which is precisely the fourth backend the enum could
//! not name either.
//!
//! [`RasterBackend`](crate::RasterBackend) and
//! [`RenderDevice`](crate::RenderDevice) are re-exported from this crate, so
//! a caller can write the bound without adding `pdfrum-render` to their
//! manifest.

use kurbo::Affine;

pub use pdfrum_render::{ColorMode, ColorScheme, Pixmap, TextAa};

/// Everything a render is parameterised by.
///
/// A plain config struct with [`Default`], filled in with struct-update
/// syntax (STYLE.md §4). The defaults render a page at one pixel per PDF
/// point, in colour, with antialiased text — what a viewer shows.
///
/// It says nothing about *which* rasterizer draws the page: that is an
/// argument to [`Page::render_on`](crate::Page::render_on) rather than a
/// field here — `RenderOptions::backend`, and the `Backend` enum it selected
/// from, were withdrawn 2026-09-02.
///
/// ```
/// use pdfrum::RenderOptions;
/// use pdfrum::kurbo::Affine;
///
/// // 150 DPI: PDF points are 1/72 inch, so the scale is 150/72.
/// let opts = RenderOptions {
///     transform: Affine::scale(150.0 / 72.0),
///     ..RenderOptions::default()
/// };
/// assert_eq!(opts.transform, Affine::scale(150.0 / 72.0));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Page space to device space — what sizes the output.
    ///
    /// The identity transform gives one pixel per PDF point, so a US Letter
    /// page renders 612x792. `Affine::scale(2.0)` doubles that. The page's
    /// own rotation and crop box are composed in for you, so this is a
    /// scale-and-place transform rather than a full page matrix.
    pub transform: Affine,
    /// The colour mode. [`ColorMode::Gray`] renders greyscale;
    /// [`ColorMode::Forced`] substitutes a fixed palette, for a
    /// high-contrast or dark-mode view.
    pub color_mode: ColorMode,
    /// How glyphs are antialiased.
    pub text_aa: TextAa,
    /// Hard-edge every path fill and stroke instead of antialiasing them.
    pub no_path_smooth: bool,
    /// Never interpolate an image when scaling it, whatever the file asks.
    pub no_image_smooth: bool,
    /// The page background.
    ///
    /// `None` — the default — follows the file: opaque white for a page
    /// without transparency, fully transparent for one with it. Set it to
    /// force one or the other.
    pub background: Option<peniko::Color>,
    /// Draw the page's annotations over its content.
    ///
    /// On by default, which is what a viewer does and what makes a form's
    /// filled-in values visible. Turning it off renders the page's own
    /// content stream alone.
    pub annotations: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            transform: Affine::IDENTITY,
            color_mode: ColorMode::default(),
            text_aa: TextAa::default(),
            no_path_smooth: false,
            no_image_smooth: false,
            background: None,
            annotations: true,
        }
    }
}

impl RenderOptions {
    /// Options rendering at `scale` pixels per PDF point, everything else
    /// left at its default.
    ///
    /// ```
    /// // 300 DPI.
    /// let opts = pdfrum::RenderOptions::scaled(300.0 / 72.0);
    /// ```
    #[must_use]
    pub fn scaled(scale: f64) -> RenderOptions {
        RenderOptions {
            transform: Affine::scale(scale),
            ..RenderOptions::default()
        }
    }

    /// Options sized so a page of `width` x `height` points fits in a box of
    /// `max_width` x `max_height` pixels, keeping its aspect ratio.
    ///
    /// The thumbnail case, which otherwise every caller writes out by hand.
    ///
    /// ```
    /// # let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let page = doc.page(0)?;
    /// let opts = pdfrum::RenderOptions::fit(page.width(), page.height(), 100, 100);
    /// let pixmap = page.render(&opts)?;
    /// assert!(pixmap.width() <= 100 && pixmap.height() <= 100);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn fit(width: f64, height: f64, max_width: u32, max_height: u32) -> RenderOptions {
        if width <= 0.0 || height <= 0.0 {
            return RenderOptions::default();
        }
        let scale = (f64::from(max_width) / width).min(f64::from(max_height) / height);
        RenderOptions::scaled(scale)
    }

    /// The options the engine below actually receives.
    pub(crate) fn to_inner(&self) -> pdfrum_render::RenderOptions {
        pdfrum_render::RenderOptions {
            transform: self.transform,
            color_mode: self.color_mode,
            text_aa: self.text_aa,
            no_path_smooth: self.no_path_smooth,
            no_image_smooth: self.no_image_smooth,
            background: self.background,
            ..pdfrum_render::RenderOptions::default()
        }
    }
}
