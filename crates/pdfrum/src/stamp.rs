//! A mark on every page: text or an image drawn over the content.
//!
//! A stamp is page content, not an annotation. It goes out as one more
//! content stream appended after the page's own, so it paints over what was
//! there and under any annotation a viewer draws; the page's existing streams
//! are not rewritten. The writer frames the appended stream with the inverse
//! of whatever transform the page's content left behind, so the stamp lands
//! in page space whatever came before it.
//!
//! Placement is in the page *as displayed*: a corner on a page with
//! `/Rotate 90` is that corner on the screen, and the stamp reads upright
//! there.

use kurbo::{Affine, Point, Rect};
use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_object::{Name, Resolve};
use pdfrum_page::{BuildContext, PageObject};
use pdfrum_parser::PageDict;

use crate::edit::transform_object;
use crate::page::build_graph;
use crate::{
    Color, DocEdit, EmbeddedImage, ImageBuilder, PageEdit, Result, StandardFont, TextBuilder,
};

/// Where a stamp sits on the page, as the page is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StampPosition {
    /// Centred on the crop box.
    #[default]
    Center,
    /// The top-left corner, inset by [`StampOptions::margin`].
    TopLeft,
    /// The top-right corner, inset by [`StampOptions::margin`].
    TopRight,
    /// The bottom-left corner, inset by [`StampOptions::margin`].
    BottomLeft,
    /// The bottom-right corner, inset by [`StampOptions::margin`].
    BottomRight,
}

/// The error [`StampPosition`]'s [`FromStr`](std::str::FromStr) returns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a stamp position: {0}")]
pub struct UnknownStampPosition(String);

impl std::fmt::Display for StampPosition {
    /// The kebab-case corner name, which round-trips through
    /// [`FromStr`](std::str::FromStr).
    ///
    /// ```
    /// assert_eq!(pdfrum::StampPosition::TopLeft.to_string(), "top-left");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StampPosition::Center => "center",
            StampPosition::TopLeft => "top-left",
            StampPosition::TopRight => "top-right",
            StampPosition::BottomLeft => "bottom-left",
            StampPosition::BottomRight => "bottom-right",
        })
    }
}

impl std::str::FromStr for StampPosition {
    type Err = UnknownStampPosition;

    /// The inverse of [`Display`](std::fmt::Display) — what a `--position`
    /// flag parses.
    ///
    /// # Errors
    ///
    /// [`UnknownStampPosition`] when the string names no corner.
    fn from_str(s: &str) -> core::result::Result<StampPosition, UnknownStampPosition> {
        match s {
            "center" => Ok(StampPosition::Center),
            "top-left" => Ok(StampPosition::TopLeft),
            "top-right" => Ok(StampPosition::TopRight),
            "bottom-left" => Ok(StampPosition::BottomLeft),
            "bottom-right" => Ok(StampPosition::BottomRight),
            other => Err(UnknownStampPosition(other.to_owned())),
        }
    }
}

/// How a stamp is drawn.
///
/// A config struct with [`Default`], filled in with struct-update syntax.
/// The three font fields are read by [`DocEdit::stamp_text`] only; the
/// rest apply to an image stamp too.
///
/// ```
/// use pdfrum::{Color, StampOptions, StampPosition};
///
/// let draft = StampOptions {
///     position: StampPosition::Center,
///     angle: 45.0,
///     opacity: 0.3,
///     font_size: 96.0,
///     color: Color::from_rgb8(200, 0, 0),
///     ..StampOptions::default()
/// };
/// assert_eq!(draft.margin, 36.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StampOptions {
    /// Where the stamp's box sits. [`StampPosition::Center`] by default.
    pub position: StampPosition,
    /// Points between a corner-placed stamp and the crop box's edges. 36 —
    /// half an inch — by default.
    pub margin: f64,
    /// Degrees counter-clockwise, turned about the stamp's own centre after
    /// it is placed. 0 by default.
    pub angle: f64,
    /// Constant alpha, `0.0` (invisible) to `1.0` (opaque, the default),
    /// written as an `/ExtGState`. Multiplied into [`StampOptions::color`]'s
    /// own alpha for text.
    pub opacity: f32,
    /// The face for a text stamp, one of the standard 14 — Helvetica by
    /// default. Text is encoded as WinAnsi, so a character outside Latin-1
    /// draws as nothing.
    pub font: StandardFont,
    /// The text size in points. 36 by default.
    pub font_size: f32,
    /// The text colour. Black by default.
    pub color: Color,
}

impl Default for StampOptions {
    fn default() -> Self {
        Self {
            position: StampPosition::Center,
            margin: 36.0,
            angle: 0.0,
            opacity: 1.0,
            font: StandardFont::Helvetica,
            font_size: 36.0,
            color: Color::BLACK,
        }
    }
}

/// Builds a [`StampOptions`] a setting at a time.
///
/// Sugar over the struct-update syntax, which still works. Every method
/// consumes and returns the builder; [`build`](Self::build) hands back the
/// options.
///
/// ```
/// use pdfrum::{Color, StampOptions, StampPosition};
///
/// let draft = StampOptions::builder()
///     .position(StampPosition::Center)
///     .angle(45.0)
///     .opacity(0.3)
///     .font_size(96.0)
///     .color(Color::from_rgb8(200, 0, 0))
///     .build();
///
/// assert_eq!(draft.margin, 36.0);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
#[must_use]
pub struct StampOptionsBuilder(StampOptions);

impl StampOptionsBuilder {
    /// Where the stamp's box sits — [`StampOptions::position`].
    ///
    /// ```
    /// use pdfrum::{StampOptions, StampPosition};
    ///
    /// let options = StampOptions::builder().position(StampPosition::TopRight).build();
    /// ```
    pub fn position(mut self, position: StampPosition) -> Self {
        self.0.position = position;
        self
    }

    /// Points between a corner-placed stamp and the crop box —
    /// [`StampOptions::margin`].
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder().margin(18.0).build();
    /// assert_eq!(options.margin, 18.0);
    /// ```
    pub fn margin(mut self, margin: f64) -> Self {
        self.0.margin = margin;
        self
    }

    /// Degrees counter-clockwise — [`StampOptions::angle`].
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder().angle(45.0).build();
    /// assert_eq!(options.angle, 45.0);
    /// ```
    pub fn angle(mut self, angle: f64) -> Self {
        self.0.angle = angle;
        self
    }

    /// Constant alpha, `0.0` to `1.0` — [`StampOptions::opacity`].
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder().opacity(0.3).build();
    /// assert_eq!(options.opacity, 0.3);
    /// ```
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.0.opacity = opacity;
        self
    }

    /// The face for a text stamp — [`StampOptions::font`].
    ///
    /// ```
    /// use pdfrum::{StampOptions, StandardFont};
    ///
    /// let options = StampOptions::builder().font(StandardFont::Courier).build();
    /// ```
    pub fn font(mut self, font: StandardFont) -> Self {
        self.0.font = font;
        self
    }

    /// The text size in points — [`StampOptions::font_size`].
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder().font_size(96.0).build();
    /// assert_eq!(options.font_size, 96.0);
    /// ```
    pub fn font_size(mut self, size: f32) -> Self {
        self.0.font_size = size;
        self
    }

    /// The text colour — [`StampOptions::color`].
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder()
    ///     .color(pdfrum::Color::from_rgb8(200, 0, 0))
    ///     .build();
    /// ```
    pub fn color(mut self, color: Color) -> Self {
        self.0.color = color;
        self
    }

    /// The options as built.
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder().build();
    /// assert_eq!(options, pdfrum::StampOptions::default());
    /// ```
    #[must_use]
    pub fn build(self) -> StampOptions {
        self.0
    }
}

impl StampOptions {
    /// A builder starting from the defaults.
    ///
    /// ```
    /// let options = pdfrum::StampOptions::builder().angle(45.0).build();
    /// ```
    pub fn builder() -> StampOptionsBuilder {
        StampOptionsBuilder::default()
    }
}

/// Where one page's stamp goes, in that page's own space.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    /// The centre of the stamp's box.
    center: Point,
    /// Degrees counter-clockwise in page space — the caller's angle plus the
    /// page's own `/Rotate`, so the stamp reads at the caller's angle on
    /// screen.
    angle: f64,
}

/// One page as the session's edits leave it: its graph, opened for the
/// stamp to be pushed onto, and the geometry the placement needs.
struct SessionPage {
    edit: PageEdit,
    crop: Rect,
    /// `/Rotate`, normalized to 0, 90, 180 or 270.
    rotate: u32,
}

impl Placement {
    /// Place a `width` by `height` box on a page as displayed — `crop` turned
    /// by `rotate` degrees clockwise — then map the result back into page
    /// space.
    fn of(crop: Rect, rotate: u32, width: f64, height: f64, options: &StampOptions) -> Self {
        // The page as displayed: a quarter turn swaps its sides.
        let (shown_width, shown_height) = if rotate.is_multiple_of(180) {
            (crop.width(), crop.height())
        } else {
            (crop.height(), crop.width())
        };
        let margin = options.margin;
        let shown = match options.position {
            StampPosition::Center => Point::new(shown_width / 2.0, shown_height / 2.0),
            StampPosition::TopLeft => {
                Point::new(margin + width / 2.0, shown_height - margin - height / 2.0)
            }
            StampPosition::TopRight => Point::new(
                shown_width - margin - width / 2.0,
                shown_height - margin - height / 2.0,
            ),
            StampPosition::BottomLeft => Point::new(margin + width / 2.0, margin + height / 2.0),
            StampPosition::BottomRight => {
                Point::new(shown_width - margin - width / 2.0, margin + height / 2.0)
            }
        };
        // Display rotates the page clockwise by `rotate`; this is the inverse,
        // from the displayed point back to the page's own coordinates.
        let center = match rotate {
            90 => Point::new(crop.x1 - shown.y, crop.y0 + shown.x),
            180 => Point::new(crop.x1 - shown.x, crop.y1 - shown.y),
            270 => Point::new(crop.x0 + shown.y, crop.y1 - shown.x),
            _ => Point::new(crop.x0 + shown.x, crop.y0 + shown.y),
        };
        Self {
            center,
            angle: options.angle + f64::from(rotate),
        }
    }

    /// The turn about the box's centre, or identity for no turn.
    fn rotation(&self) -> Affine {
        if self.angle == 0.0 {
            return Affine::IDENTITY;
        }
        let about = self.center.to_vec2();
        Affine::translate(about)
            * Affine::rotate(self.angle.to_radians())
            * Affine::translate(-about)
    }
}

/// `color` with `opacity` folded into its alpha.
fn with_opacity(color: Color, opacity: f32) -> Color {
    let [r, g, b, a] = color.components;
    Color::new([r, g, b, a * opacity.clamp(0.0, 1.0)])
}

/// The extent of a run of text in a standard font: (width, ascent, descent)
/// in points at `size`, descent negative.
struct TextExtent {
    width: f64,
    ascent: f64,
    descent: f64,
}

impl DocEdit<'_> {
    /// Draw `text` over every page.
    ///
    /// The text is set in [`StampOptions::font`] at [`StampOptions::font_size`],
    /// its box placed by [`StampOptions::position`] on the page as displayed,
    /// then turned by [`StampOptions::angle`] about the box's centre. One
    /// `/Font` object serves every page. Each page's existing content is left
    /// as it was; the stamp is one more stream after it, so it paints on top.
    ///
    /// ```
    /// use pdfrum::{Document, SaveOptions, StampOptions, StampPosition};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world_2_pages.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.stamp_text(
    ///     "DRAFT",
    ///     &StampOptions {
    ///         position: StampPosition::BottomRight,
    ///         opacity: 0.5,
    ///         ..StampOptions::default()
    ///     },
    /// )?;
    /// let mut bytes = Vec::new();
    /// edit.write_to(&mut bytes, &SaveOptions::default())?;
    ///
    /// let stamped = pdfrum::Document::from_bytes(bytes.into())?;
    /// assert!(stamped.page(1)?.text().to_string().contains("DRAFT"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the font cannot be added, and
    /// [`Error::Read`](crate::Error::Read) when a page cannot be opened.
    pub fn stamp_text(&mut self, text: &str, options: &StampOptions) -> Result<()> {
        let font = self.inner.standard_font(options.font)?;
        let codes = font.encode(text);
        let extent = self.text_extent(font.object(), &codes, options);
        let height = extent.ascent - extent.descent;
        let shared = pdfrum_edit::shared_objects(&self.inner);
        for index in 0..self.doc.page_count() {
            let Some(mut page) = self.session_page(index.into())? else {
                continue;
            };
            let place = Placement::of(page.crop, page.rotate, extent.width, height, options);
            let baseline = Point::new(
                place.center.x - extent.width / 2.0,
                place.center.y - height / 2.0 - extent.descent,
            );
            let mut object = TextBuilder {
                position: baseline,
                fill: with_opacity(options.color, options.opacity),
                ..TextBuilder::new(codes.clone(), font.object(), options.font_size)
            }
            .build();
            transform_object(&mut object, place.rotation());
            page.edit.push(object);
            self.apply_page(&page.edit, &shared)?;
        }
        Ok(())
    }

    /// Draw `image` over every page, `width` points wide with its aspect
    /// ratio kept.
    ///
    /// Placed and turned as [`DocEdit::stamp_text`] places text, at
    /// [`StampOptions::opacity`]; the font fields are not read. The image
    /// is one this session embedded through [`DocEdit::embed_jpeg`] or
    /// [`DocEdit::embed_image`], and one `XObject` serves every page.
    ///
    /// ```
    /// use pdfrum::{Document, PixelFormat, SaveOptions, StampOptions};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// // A two-by-one image: red, then blue.
    /// let image = edit.embed_image(&[255, 0, 0, 0, 0, 255], 2, 1, PixelFormat::Rgb8)?;
    /// edit.stamp_image(&image, 200.0, &StampOptions { opacity: 0.5, ..StampOptions::default() })?;
    /// let mut bytes = Vec::new();
    /// edit.write_to(&mut bytes, &SaveOptions::default())?;
    /// assert!(bytes.starts_with(b"%PDF-"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when `width` is not positive, and
    /// [`Error::Read`](crate::Error::Read) when a page cannot be opened.
    pub fn stamp_image(
        &mut self,
        image: &EmbeddedImage,
        width: f64,
        options: &StampOptions,
    ) -> Result<()> {
        if !width.is_finite() || width <= 0.0 || image.width() == 0 {
            return Err(pdfrum_edit::Error::EmptyImage.into());
        }
        let height = width * f64::from(image.height()) / f64::from(image.width());
        let shared = pdfrum_edit::shared_objects(&self.inner);
        for index in 0..self.doc.page_count() {
            let Some(mut page) = self.session_page(index.into())? else {
                continue;
            };
            let place = Placement::of(page.crop, page.rotate, width, height, options);
            let rect = Rect::from_center_size(place.center, (width, height));
            let mut object = ImageBuilder::at(image.object(), rect).build();
            if let PageObject::Image(content) = &mut object {
                content.state.general.fill_alpha = options.opacity.clamp(0.0, 1.0);
            }
            transform_object(&mut object, place.rotation());
            page.edit.push(object);
            self.apply_page(&page.edit, &shared)?;
        }
        Ok(())
    }

    /// Page `index` as this session's edits leave it — its dictionary,
    /// resources and content read through the overlay, so a rotation set or
    /// a stamp drawn earlier in the session is what this one builds on.
    /// `None` for a page written inline in its parent's `/Kids`.
    fn session_page(&self, index: PageIndex) -> Result<Option<SessionPage>> {
        let Some((reference, dict, _)) = self.page_state(index)? else {
            return Ok(None);
        };
        let page = PageDict {
            dict,
            reference: Some(reference),
        };
        let mut diags = Diagnostics::default();
        let (_, crop) = pdfrum_page::derive_boxes(
            &page.dict,
            |key| page.inherited(key, &self.inner),
            &self.inner,
            &mut diags,
        );
        let rotate = pdfrum_page::Rotation::from_degrees(
            page.inherited(&Name::from("Rotate"), &self.inner)
                .as_ref()
                .and_then(pdfrum_object::Object::as_int)
                .unwrap_or(0),
        );
        self.doc.note(&diags);
        let graph = build_graph(self.doc, &page, &self.inner, &mut BuildContext::new());
        Ok(Some(SessionPage {
            edit: PageEdit { index, page: graph },
            crop,
            rotate: rotate.degrees(),
        }))
    }

    /// Measure `codes` in the font `font` names, through the font crate's
    /// reading of the dictionary this session wrote for it.
    fn text_extent(
        &self,
        font: pdfrum_object::ObjRef,
        codes: &[u8],
        options: &StampOptions,
    ) -> TextExtent {
        let size = f64::from(options.font_size);
        let mut diags = Diagnostics::default();
        let loaded = self
            .inner
            .fetch(font)
            .ok()
            .as_deref()
            .and_then(pdfrum_object::Object::as_dict)
            .and_then(|dict| {
                pdfrum_font::load(
                    dict,
                    &self.inner,
                    &pdfrum_font::FontCache::new(),
                    &self.doc.limits,
                    &mut diags,
                )
            });
        let per_em = |units: f32| f64::from(units) / 1000.0 * size;
        match loaded {
            Some(metrics) if metrics.ascent() > 0.0 => TextExtent {
                width: per_em(metrics.string_width(codes)),
                ascent: per_em(metrics.ascent()),
                descent: per_em(metrics.descent()),
            },
            // A face with no metrics: the proportions of a typical Latin
            // face, so the box is at least the right order of size.
            _ => TextExtent {
                width: 0.5 * size * f64::from(u32::try_from(codes.len()).unwrap_or(u32::MAX)),
                ascent: 0.75 * size,
                descent: -0.25 * size,
            },
        }
    }
}
