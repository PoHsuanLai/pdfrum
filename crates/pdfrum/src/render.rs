//! What a render is parameterised by. *Which* rasterizer performs it is an
//! argument, not an option — see [`Page::render_on`](crate::Page::render_on).

use kurbo::Affine;

pub use pdfrum_render::{ColorMode, ColorScheme, Pixmap, TextAa};

/// Everything a render is parameterised by.
///
/// A config struct with [`Default`]. The defaults render a page at one
/// pixel per PDF point, in colour, with antialiased text — what a viewer
/// shows. `#[non_exhaustive]` so a field added later is not a major break;
/// fill one in with [`RenderOptions::builder`] (struct-update syntax from
/// another crate cannot name every field).
///
/// It says nothing about *which* rasterizer draws the page: that is an
/// argument to [`Page::render_on`](crate::Page::render_on). Nor about how
/// much a render may cost: the pixel cap and the deadline are the
/// document's, set once in [`OpenOptions::limits`](crate::OpenOptions::limits)
/// and applied to every render of it.
///
/// `pdfrum_render::RenderOptions` is a **different type**, and the engine's
/// own. This one's flags are positive and default to the common case, so
/// `smooth_paths` here is the engine's `no_path_smooth` inverted.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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

/// Builds a [`RenderOptions`] a setting at a time.
///
/// The way to change one field from outside this crate: the type is
/// `#[non_exhaustive]`, so struct-update syntax is a same-crate spelling.
/// This also reads better when several settings are chosen, when they are
/// chosen conditionally, or from a language binding.
///
/// Every method consumes and returns the builder; [`build`](Self::build)
/// hands back the options.
///
/// ```
/// use pdfrum::{ColorMode, RenderOptions};
///
/// let options = RenderOptions::builder()
///     .scale(2.0)
///     .grayscale()
///     .annotations(false)
///     .build();
///
/// assert_eq!(options.color_mode, ColorMode::Gray);
/// assert!(!options.annotations);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
#[must_use]
pub struct RenderOptionsBuilder(RenderOptions);

impl RenderOptionsBuilder {
    /// Page space to device space — [`RenderOptions::transform`].
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder()
    ///     .transform(kurbo::Affine::scale(1.5))
    ///     .build();
    /// ```
    pub fn transform(mut self, transform: Affine) -> Self {
        self.0.transform = transform;
        self
    }

    /// Render at `scale` pixels per PDF point.
    ///
    /// Shorthand for [`transform`](Self::transform) with a uniform scale, and
    /// the same thing [`RenderOptions::scaled`] constructs.
    ///
    /// ```
    /// // 300 DPI.
    /// let options = pdfrum::RenderOptions::builder().scale(300.0 / 72.0).build();
    /// ```
    pub fn scale(self, scale: f64) -> Self {
        self.transform(Affine::scale(scale))
    }

    /// The colour mode — [`RenderOptions::color_mode`].
    ///
    /// ```
    /// use pdfrum::{ColorMode, RenderOptions};
    ///
    /// let options = RenderOptions::builder().color_mode(ColorMode::Gray).build();
    /// ```
    pub fn color_mode(mut self, color_mode: ColorMode) -> Self {
        self.0.color_mode = color_mode;
        self
    }

    /// Render greyscale — [`ColorMode::Gray`].
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder().grayscale().build();
    /// assert_eq!(options.color_mode, pdfrum::ColorMode::Gray);
    /// ```
    pub fn grayscale(self) -> Self {
        self.color_mode(ColorMode::Gray)
    }

    /// How glyphs are antialiased — [`RenderOptions::text_aa`].
    ///
    /// ```
    /// use pdfrum::{RenderOptions, TextAa};
    ///
    /// let options = RenderOptions::builder().text_aa(TextAa::None).build();
    /// ```
    pub fn text_aa(mut self, text_aa: TextAa) -> Self {
        self.0.text_aa = text_aa;
        self
    }

    /// Antialias path fills and strokes — [`RenderOptions::smooth_paths`].
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder().smooth_paths(false).build();
    /// assert!(!options.smooth_paths);
    /// ```
    pub fn smooth_paths(mut self, smooth: bool) -> Self {
        self.0.smooth_paths = smooth;
        self
    }

    /// Interpolate a scaled image where the file asks for it —
    /// [`RenderOptions::interpolate_images`].
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder().interpolate_images(false).build();
    /// assert!(!options.interpolate_images);
    /// ```
    pub fn interpolate_images(mut self, interpolate: bool) -> Self {
        self.0.interpolate_images = interpolate;
        self
    }

    /// Force a page background — [`RenderOptions::background`].
    ///
    /// Unset, the default follows the file. This sets it; pass `None` to
    /// [`RenderOptions::background`] directly to unset it again.
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder()
    ///     .background(pdfrum::Color::WHITE)
    ///     .build();
    /// assert!(options.background.is_some());
    /// ```
    pub fn background(mut self, background: peniko::Color) -> Self {
        self.0.background = Some(background);
        self
    }

    /// Draw the page's annotations over its content —
    /// [`RenderOptions::annotations`].
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder().annotations(false).build();
    /// assert!(!options.annotations);
    /// ```
    pub fn annotations(mut self, annotations: bool) -> Self {
        self.0.annotations = annotations;
        self
    }

    /// The options as built.
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder().build();
    /// assert_eq!(options, pdfrum::RenderOptions::default());
    /// ```
    #[must_use]
    pub fn build(self) -> RenderOptions {
        self.0
    }
}

impl RenderOptions {
    /// A builder starting from the defaults.
    ///
    /// ```
    /// let options = pdfrum::RenderOptions::builder().scale(2.0).build();
    /// ```
    pub fn builder() -> RenderOptionsBuilder {
        RenderOptionsBuilder::default()
    }

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
    /// let pixmap = page.render(&pdfrum::VelloCpuBackend::new(), &opts)?;
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
        pdfrum_render::RenderOptions::from(self)
    }
}

/// This crate's options as the engine's own.
///
/// Public because the engine crates take *their* type: a caller who reaches
/// past the facade -- to `pdfrum-svg`, or to `pdfrum_render` directly -- needs
/// to build one from the options they already have, and the flags do not line
/// up field for field. The engine's are negative and this crate's are
/// positive, so `smooth_paths` here is `no_path_smooth` there.
///
/// The engine's remaining flags keep their defaults; this crate does not
/// expose them.
///
/// ```
/// let engine = pdfrum_render::RenderOptions::from(&pdfrum::RenderOptions::default());
/// assert!(!engine.no_path_smooth, "smooth_paths defaults on, so its inverse is off");
/// ```
impl From<&RenderOptions> for pdfrum_render::RenderOptions {
    fn from(options: &RenderOptions) -> Self {
        Self {
            transform: options.transform,
            color_mode: options.color_mode,
            text_aa: options.text_aa,
            no_path_smooth: !options.smooth_paths,
            no_image_smooth: !options.interpolate_images,
            background: options.background,
            ..Self::default()
        }
    }
}
