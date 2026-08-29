//! What a render is parameterised by, and which rasterizer performs it.

use kurbo::Affine;

pub use pdfrum_render::{ColorMode, ColorScheme, Pixmap, TextAa};

/// Which rasterizer draws a page.
///
/// All three are pure Rust, all three are deterministic, and all three
/// produce a [`Pixmap`] — the choice is a trade between speed and matching a
/// reference renderer's edges. The engine above them is the same either way:
/// a backend rasterizes paths and images, it does not interpret PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// `vello_cpu` — the default. A modern sparse-strip rasterizer with SIMD
    /// throughout, pinned to a fixed instruction level so its output does not
    /// change with the host CPU.
    #[default]
    Vello,
    /// `tiny-skia` — a mature Skia CPU port, and this project's determinism
    /// baseline. Kept as a cross-check: where the two disagree, the bug is in
    /// a backend rather than in the engine.
    TinySkia,
    /// The analytic rasterizer — pick this when you want edges that match
    /// PDFium's rather than the fastest render.
    ///
    /// It integrates each pixel's covered area exactly, on PDFium's own
    /// 256ths-of-a-pixel grid, where the other two sample or approximate: a
    /// half-covered pixel comes out at exactly half, not at the nearest of
    /// seventeen supersampled levels. That is what the project's conformance
    /// runs use, and it is worth reaching for when you are diffing output
    /// against another PDF renderer. It has no SIMD, so it is the slower
    /// choice for bulk rendering.
    Exact,
}

/// Everything a render is parameterised by.
///
/// A plain config struct with [`Default`], filled in with struct-update
/// syntax (STYLE.md §4). The defaults render a page at one pixel per PDF
/// point, in colour, with antialiased text — what a viewer shows.
///
/// ```
/// use pdfrum::{Backend, RenderOptions};
/// use pdfrum::kurbo::Affine;
///
/// // 150 DPI: PDF points are 1/72 inch, so the scale is 150/72.
/// let opts = RenderOptions {
///     transform: Affine::scale(150.0 / 72.0),
///     backend: Backend::TinySkia,
///     ..RenderOptions::default()
/// };
/// assert_eq!(opts.backend, Backend::TinySkia);
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
    /// Which rasterizer draws it.
    pub backend: Backend,
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
            backend: Backend::default(),
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
