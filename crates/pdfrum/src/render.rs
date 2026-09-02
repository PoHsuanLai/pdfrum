//! What a render is parameterised by. *Which* rasterizer performs it is an
//! argument, not an option — see [`Page::render_on`](crate::Page::render_on).

use kurbo::Affine;

pub use pdfrum_render::{ColorMode, ColorScheme, Pixmap, TextAa};

/// Everything a render is parameterised by.
///
/// A config struct with [`Default`], filled in with struct-update syntax. The
/// defaults render a page at one pixel per PDF point, in colour, with
/// antialiased text — what a viewer shows.
///
/// It says nothing about *which* rasterizer draws the page: that is an
/// argument to [`Page::render_on`](crate::Page::render_on).
///
/// `pdfrum_render::RenderOptions` is a **different type**, and the engine's
/// own. This one's flags are positive and default to the common case, so
/// `smooth_paths` here is the engine's `no_path_smooth` inverted.
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
    /// Antialias path fills and strokes.
    ///
    /// On by default. Turning it off hard-edges every path, which is the
    /// engine's `no_path_smooth`.
    pub smooth_paths: bool,
    /// Interpolate an image when it is scaled, where the file asks for it.
    ///
    /// On by default. Turning it off uses the nearest sample whatever the
    /// image dictionary's `/Interpolate` says, which is the engine's
    /// `no_image_smooth`.
    pub interpolate_images: bool,
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
            smooth_paths: true,
            interpolate_images: true,
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
            no_path_smooth: !self.smooth_paths,
            no_image_smooth: !self.interpolate_images,
            background: self.background,
            ..pdfrum_render::RenderOptions::default()
        }
    }
}
