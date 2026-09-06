//! Drawing on an existing page without writing content-stream operators.
//!
//! A [`Canvas`] is a retained drawing surface over one page. A caller places
//! fills, strokes, text and images in the page's *displayed* coordinate space
//! — y-up, in PDF points, with the crop box and `/Rotate` already composed in
//! — and the canvas emits one content stream that is **appended** to the
//! page's `/Contents`. The page's own streams are never rewritten, so nothing
//! a save would otherwise lose (`pdfrum_edit`'s regeneration losses) applies
//! to a page that is only drawn on.
//!
//! `docs/design/canvas.md` states the coordinate space and the
//! resource-merging rule in full; the two are summarised on [`Canvas`] and
//! [`DocEdit::draw_page`] respectively.
//!
//! # There is no layout here, deliberately
//!
//! [`Canvas::text`] draws one string at one point. There is no line breaking,
//! no wrapping and no paragraph model, and the only measurement is
//! [`Canvas::text_width`], a single string's advance. A caller who needs
//! layout has a typesetting problem and brings their own layout to `text`.

use std::fmt::Write as _;

use kurbo::{Affine, BezPath, PathEl, Point, Rect, RoundedRect, Shape};
use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_edit::{
    ContentsShape, EmbeddedFont, EmbeddedImage, write_float, write_matrix, write_point,
};
use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, Resolve, Stream, names as pdf_names,
};

use crate::{Color, DocEdit, Error, Result};

/// How a shape is painted.
///
/// An enum rather than two `Option<Color>` fields because the three states
/// are what the PDF paint operators actually offer, and "neither" is not one
/// of them: a caller who wants to paint nothing does not call the method.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    /// Filled only. Written as `f` or `f*`.
    Fill(Color),
    /// Stroked only, at [`Stroke::width`]. Written as `S`.
    Stroke(Stroke),
    /// Filled and stroked, the fill first. Written as `B` or `B*`.
    FillStroke(Color, Stroke),
}

impl Paint {
    /// The fill colour, if this paint fills.
    ///
    /// ```
    /// use pdfrum::{Color, Paint};
    ///
    /// assert_eq!(Paint::Fill(Color::BLACK).fill(), Some(Color::BLACK));
    /// ```
    #[must_use]
    pub fn fill(self) -> Option<Color> {
        match self {
            Self::Fill(color) | Self::FillStroke(color, _) => Some(color),
            Self::Stroke(_) => None,
        }
    }

    /// The stroke, if this paint strokes.
    ///
    /// ```
    /// use pdfrum::{Color, Paint, Stroke};
    ///
    /// assert!(Paint::Fill(Color::BLACK).stroke().is_none());
    /// assert!(Paint::Stroke(Stroke::new(Color::BLACK, 2.0)).stroke().is_some());
    /// ```
    #[must_use]
    pub fn stroke(self) -> Option<Stroke> {
        match self {
            Self::Stroke(stroke) | Self::FillStroke(_, stroke) => Some(stroke),
            Self::Fill(_) => None,
        }
    }
}

/// A stroke's colour and width, in canvas units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    /// The colour. Its alpha is honoured, as an `/ExtGState` `/CA`.
    pub color: Color,
    /// The line width in canvas units — page points.
    pub width: f64,
}

impl Stroke {
    /// A stroke of `color` at `width` points.
    ///
    /// ```
    /// let hairline = pdfrum::Stroke::new(pdfrum::Color::BLACK, 0.5);
    /// assert_eq!(hairline.width, 0.5);
    /// ```
    #[must_use]
    pub fn new(color: Color, width: f64) -> Self {
        Self { color, width }
    }
}

/// Which points a fill considers inside.
///
/// The `pdfrum-page` reader's `FillRule` carries a third `None` case for a
/// path that is only stroked; here that case is spelled by [`Paint::Stroke`]
/// instead, so this enum has exactly the two rules ISO 32000-1 §8.5.3.3
/// defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fill {
    /// The nonzero winding rule — `f`, `B`. The default.
    #[default]
    NonZero,
    /// The even-odd rule — `f*`, `B*`.
    EvenOdd,
}

/// One page's drawing surface.
///
/// Handed to the closure of [`DocEdit::draw_page`] and
/// [`DocEdit::draw_pages`]; it cannot be constructed otherwise, because a
/// canvas is only meaningful against the page whose space it maps and the
/// session whose resources it merges into.
///
/// # The coordinate space
///
/// Canvas coordinates are the page **as displayed**, in points:
///
/// - the origin is the lower-left corner of the crop box after `/Rotate`;
/// - x runs right and **y runs up**, as PDF page space does and unlike a
///   raster;
/// - the extent is [`Canvas::size`], whose sides are the crop box's *swapped*
///   on a quarter or three-quarter turn.
///
/// So a caller places things where they see them: on a page with
/// `/Rotate 90`, `Point::new(0.0, 0.0)` is the bottom-left corner on screen,
/// and text drawn along +x reads upright there. The composition is exactly
/// the inverse of [`pdfrum_page::Rotation::display_matrix`] over the crop
/// box, which is the same matrix the renderer uses, so what a caller places
/// and what a viewer shows cannot drift apart.
///
/// Every method takes canvas coordinates. [`Canvas::transform`] composes a
/// further transform *inside* that space, so a rotation about a point is
/// written in the coordinates the caller is already using.
pub struct Canvas<'a, 'b> {
    /// Where the operators accumulate. Not yet wrapped in `q`/`Q`.
    out: String,
    /// The session the resources are merged into and new objects allocated
    /// from.
    edit: &'a mut DocEdit<'b>,
    /// The resources this drawing needs, by category, under names already
    /// checked against the page's own.
    added: Vec<(&'static Name, Name, Object)>,
    /// Names already taken: the page's own, plus every name this drawing has
    /// allocated. Fresh names are chosen against this set, per category.
    taken: Vec<(&'static Name, Name)>,
    /// The displayed size, in points.
    size: kurbo::Size,
    /// The page being drawn on.
    index: PageIndex,
    /// The first error a drawing method hit. Reported once, from
    /// [`DocEdit::draw_page`], rather than at every call.
    failed: Option<Error>,
}

impl Canvas<'_, '_> {
    /// The page's displayed size in points — the crop box's, with its sides
    /// swapped on a quarter turn.
    ///
    /// The canvas's own extent: `Rect::from_origin_size(Point::ZERO, size)`
    /// is the whole visible page.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     assert!(c.size().width > 0.0);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn size(&self) -> kurbo::Size {
        self.size
    }

    /// The whole visible page, as a rectangle in canvas coordinates.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     assert_eq!(c.bounds().origin(), pdfrum::Point::ZERO);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn bounds(&self) -> Rect {
        Rect::from_origin_size(Point::ZERO, self.size)
    }

    /// The page this canvas draws on.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world_2_pages.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_pages(|c| assert!(u32::from(c.page()) < 2))?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn page(&self) -> PageIndex {
        self.index
    }

    /// Draw inside a saved graphics state, restored when `body` returns.
    ///
    /// This is the *only* spelling of `q`/`Q`: there is no bare `save` a
    /// caller could leave unmatched, and no `restore` that could pop a state
    /// the caller did not push. Nesting is the closure nesting, so an
    /// unbalanced stream is not expressible.
    ///
    /// ```
    /// use pdfrum::{Color, Paint, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.saved(|c| {
    ///         c.clip(Rect::new(0.0, 0.0, 100.0, 100.0), pdfrum::Fill::NonZero);
    ///         c.fill_rect(Rect::new(0.0, 0.0, 500.0, 500.0), Color::from_rgb8(200, 0, 0));
    ///     });
    ///     // The clip is gone here.
    ///     c.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::BLACK);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn saved(&mut self, body: impl FnOnce(&mut Self)) {
        self.out.push_str("q\n");
        body(self);
        self.out.push_str("Q\n");
    }

    /// Compose `transform` into the canvas space, for everything drawn after
    /// it.
    ///
    /// Scoped by [`Canvas::saved`], like every other graphics-state change; a
    /// transform outside one lasts for the rest of the drawing.
    ///
    /// ```
    /// use pdfrum::{Affine, Color, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.saved(|c| {
    ///         c.transform(Affine::rotate_about(0.5, c.bounds().center()));
    ///         c.fill_rect(Rect::new(0.0, 0.0, 100.0, 20.0), Color::BLACK);
    ///     });
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn transform(&mut self, transform: Affine) {
        write_matrix(&mut self.out, transform);
        self.out.push_str(" cm\n");
    }

    /// Intersect the clip with `shape`, for everything drawn after it.
    ///
    /// Scoped by [`Canvas::saved`]: a PDF clip can only ever be narrowed, so
    /// a `q`/`Q` is the only way back.
    ///
    /// ```
    /// use pdfrum::{Fill, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.saved(|c| c.clip(Rect::new(10.0, 10.0, 90.0, 90.0), Fill::NonZero));
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn clip(&mut self, shape: impl Shape, rule: Fill) {
        self.write_path(&shape.into_path(0.1));
        self.out.push_str(match rule {
            Fill::NonZero => " W n\n",
            Fill::EvenOdd => " W* n\n",
        });
    }

    /// Fill `shape` with `color`, by the nonzero rule.
    ///
    /// ```
    /// use pdfrum::{Color, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.fill(Rect::new(0.0, 0.0, 50.0, 50.0), Color::from_rgb8(0, 0, 255));
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn fill(&mut self, shape: impl Shape, color: Color) {
        self.draw(shape, Paint::Fill(color), Fill::NonZero);
    }

    /// Fill `rect` with `color` — [`Canvas::fill`] on the commonest shape.
    ///
    /// ```
    /// use pdfrum::{Color, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.fill_rect(Rect::new(0.0, 0.0, 50.0, 50.0), Color::BLACK);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.fill(rect, color);
    }

    /// Fill a rectangle with `radius`-point rounded corners.
    ///
    /// ```
    /// use pdfrum::{Color, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.fill_rounded_rect(Rect::new(0.0, 0.0, 80.0, 30.0), 6.0, Color::BLACK);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f64, color: Color) {
        self.fill(RoundedRect::from_rect(rect, radius), color);
    }

    /// Stroke `shape`.
    ///
    /// ```
    /// use pdfrum::{Color, Rect, Stroke};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.stroke(Rect::new(0.0, 0.0, 50.0, 50.0), Stroke::new(Color::BLACK, 1.0));
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn stroke(&mut self, shape: impl Shape, stroke: Stroke) {
        self.draw(shape, Paint::Stroke(stroke), Fill::NonZero);
    }

    /// Stroke the straight segment from `from` to `to` — a header rule, a
    /// divider.
    ///
    /// ```
    /// use pdfrum::{Color, Point, Stroke};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     let y = c.size().height - 50.0;
    ///     c.line(Point::new(50.0, y), Point::new(c.size().width - 50.0, y),
    ///            Stroke::new(Color::BLACK, 0.75));
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn line(&mut self, from: Point, to: Point, stroke: Stroke) {
        self.stroke(kurbo::Line::new(from, to), stroke);
    }

    /// Paint `shape` with `paint`, filling by `rule`.
    ///
    /// The general case the other shape methods narrow: [`Canvas::fill`] is
    /// `Paint::Fill` with [`Fill::NonZero`], [`Canvas::stroke`] is
    /// `Paint::Stroke`.
    ///
    /// ```
    /// use pdfrum::{Color, Fill, Paint, Rect, Stroke};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.draw(
    ///         Rect::new(0.0, 0.0, 40.0, 40.0),
    ///         Paint::FillStroke(Color::from_rgb8(255, 255, 0), Stroke::new(Color::BLACK, 2.0)),
    ///         Fill::EvenOdd,
    ///     );
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn draw(&mut self, shape: impl Shape, paint: Paint, rule: Fill) {
        let path = shape.into_path(0.1);
        if path.elements().is_empty() {
            return;
        }
        self.out.push_str("q\n");
        self.set_paint(paint);
        self.write_path(&path);
        self.out.push_str(paint_operator(paint, rule));
        self.out.push_str("\nQ\n");
    }

    /// Draw `text` in `font` at `size`, with its baseline starting at `at`.
    ///
    /// `font` is one this session loaded through [`DocEdit::embed_font`] or
    /// [`DocEdit::standard_font`], so its glyphs are subset and embedded by
    /// the machinery that already does that for a saved font. A base-14 face
    /// from `standard_font` needs no embedded program.
    ///
    /// One string, one point, one line: see the module documentation for why
    /// there is no wrapping.
    ///
    /// # Errors
    ///
    /// A character `font` has no glyph for is an error, not a blank — the
    /// canvas records it and [`DocEdit::draw_page`] returns it. Nothing of
    /// this call is written when it fails, so a refused string leaves no
    /// half-drawn run behind.
    ///
    /// ```
    /// use pdfrum::{Color, Point, StandardFont};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let font = edit.standard_font(StandardFont::Helvetica)?;
    /// edit.draw_page(0, |c| {
    ///     c.text("Page 1", &font, 10.0, Point::new(72.0, 72.0), Color::BLACK);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn text(&mut self, text: &str, font: &EmbeddedFont, size: f64, at: Point, color: Color) {
        let codes = match font.encode_checked(text) {
            Ok(codes) => codes,
            Err(missing) => return self.fail(pdfrum_edit::Error::from(missing).into()),
        };
        if codes.is_empty() {
            return;
        }
        let name = self.realize(pdf_names::FONT, Object::Ref(font.object()));
        self.out.push_str("q\n");
        self.set_paint(Paint::Fill(color));
        self.out.push_str("BT\n/");
        self.push_name(&name);
        self.out.push(' ');
        write_f64(&mut self.out, size);
        self.out.push_str(" Tf 1 0 0 1 ");
        write_point(&mut self.out, at);
        self.out.push_str(" Tm ");
        write_hex_string(&mut self.out, &codes);
        self.out.push_str(" Tj\nET\nQ\n");
    }

    /// The advance of `text` in `font` at `size`, in canvas units.
    ///
    /// The **only** measurement this API offers, and it is what centring a
    /// single string needs. It is not a layout engine and does not claim to
    /// be: no line breaking, no kerning beyond the font's own advances, and
    /// no vertical metrics.
    ///
    /// `0.0` for a string `font` cannot encode.
    ///
    /// ```
    /// use pdfrum::StandardFont;
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let font = edit.standard_font(StandardFont::Helvetica)?;
    /// edit.draw_page(0, |c| {
    ///     assert!(c.text_width("Hello", &font, 12.0) > 0.0);
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn text_width(&self, text: &str, font: &EmbeddedFont, size: f64) -> f64 {
        let Ok(codes) = font.encode_checked(text) else {
            return 0.0;
        };
        self.edit.string_width(font.object(), &codes) * size / 1000.0
    }

    /// Draw `image` stretched onto `rect`.
    ///
    /// `image` is one this session embedded through [`DocEdit::embed_jpeg`]
    /// or [`DocEdit::embed_image`]. Nothing preserves the aspect ratio: a
    /// caller who wants it kept sizes `rect` from
    /// [`EmbeddedImage::width`](pdfrum_edit::EmbeddedImage::width) and
    /// [`EmbeddedImage::height`](pdfrum_edit::EmbeddedImage::height).
    ///
    /// ```
    /// use pdfrum::Rect;
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let logo = edit.embed_jpeg(include_bytes!("../tests/fixtures/mona_lisa.jpg"))?;
    /// edit.draw_page(0, |c| {
    ///     c.image(&logo, Rect::new(20.0, 20.0, 80.0, 80.0));
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn image(&mut self, image: &EmbeddedImage, rect: Rect) {
        if rect.width() == 0.0 || rect.height() == 0.0 {
            return;
        }
        let name = self.realize(pdf_names::XOBJECT, Object::Ref(image.object()));
        self.out.push_str("q\n");
        write_matrix(
            &mut self.out,
            Affine::new([rect.width(), 0.0, 0.0, rect.height(), rect.x0, rect.y0]),
        );
        self.out.push_str(" cm /");
        self.push_name(&name);
        self.out.push_str(" Do\nQ\n");
    }

    /// Set the constant alpha for everything drawn after it, as an
    /// `/ExtGState` naming `/ca` and `/CA`.
    ///
    /// Scoped by [`Canvas::saved`]. The alpha a [`Color`] already carries is
    /// applied on top of this, so a translucent colour under a 0.5 opacity is
    /// twice translucent.
    ///
    /// ```
    /// use pdfrum::{Color, Rect};
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     c.saved(|c| {
    ///         c.opacity(0.2);
    ///         c.fill_rect(Rect::new(0.0, 0.0, 100.0, 100.0), Color::from_rgb8(255, 0, 0));
    ///     });
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn opacity(&mut self, alpha: f64) {
        let alpha = alpha.clamp(0.0, 1.0);
        let state = Dict::from_pairs([
            (Name::from("ca"), Object::Real(as_f32(alpha))),
            (Name::from("CA"), Object::Real(as_f32(alpha))),
        ]);
        let name = self.realize(pdf_names::EXT_G_STATE, Object::Dict(state));
        self.out.push('/');
        self.push_name(&name);
        self.out.push_str(" gs\n");
    }

    /// Record the first failure; later ones are dropped, because the first is
    /// the one that explains the rest.
    fn fail(&mut self, error: Error) {
        if self.failed.is_none() {
            self.failed = Some(error);
        }
    }

    /// Write the colour and width operators `paint` asks for.
    fn set_paint(&mut self, paint: Paint) {
        // Alpha rides on an `/ExtGState`, since `rg`/`RG` carry none.
        let alpha = paint
            .fill()
            .map(alpha_of)
            .into_iter()
            .chain(paint.stroke().map(|s| alpha_of(s.color)))
            .fold(1.0_f64, f64::min);
        if alpha < 1.0 {
            self.opacity(alpha);
        }
        if let Some(color) = paint.fill() {
            self.write_rgb(color);
            self.out.push_str(" rg\n");
        }
        if let Some(stroke) = paint.stroke() {
            self.write_rgb(stroke.color);
            self.out.push_str(" RG\n");
            write_f64(&mut self.out, stroke.width.max(0.0));
            self.out.push_str(" w\n");
        }
    }

    /// Append a colour's three clamped components, space separated.
    fn write_rgb(&mut self, color: Color) {
        let [r, g, b, _] = color.components;
        for (i, component) in [r, g, b].into_iter().enumerate() {
            if i > 0 {
                self.out.push(' ');
            }
            write_float(&mut self.out, component.clamp(0.0, 1.0));
        }
    }

    /// Append a path's construction operators, with no trailing separator.
    ///
    /// A path that is exactly an axis-aligned rectangle is written as one
    /// `re`, which is both shorter and what `pdfrum-edit`'s own emitter
    /// writes, so the two producers spell the commonest shape the same way.
    ///
    /// Otherwise a running cursor tracks the current point, because a
    /// quadratic segment needs the point it starts from: PDF has no quadratic
    /// operator, so each one is raised to the cubic with the identical curve
    /// rather than flattened into lines.
    fn write_path(&mut self, path: &BezPath) {
        if let Some(rect) = axis_aligned_rect(path) {
            pdfrum_edit::write_rect(&mut self.out, rect);
            self.out.push_str(" re");
            return;
        }
        let mut at = Point::ZERO;
        let mut start = Point::ZERO;
        for (index, element) in path.elements().iter().enumerate() {
            if index > 0 {
                self.out.push(' ');
            }
            match *element {
                PathEl::MoveTo(p) => {
                    write_point(&mut self.out, p);
                    self.out.push_str(" m");
                    at = p;
                    start = p;
                }
                PathEl::LineTo(p) => {
                    write_point(&mut self.out, p);
                    self.out.push_str(" l");
                    at = p;
                }
                PathEl::QuadTo(c, p) => {
                    let (c1, c2) = quad_to_cubic(at, c, p);
                    self.write_cubic(c1, c2, p);
                    at = p;
                }
                PathEl::CurveTo(c1, c2, p) => {
                    self.write_cubic(c1, c2, p);
                    at = p;
                }
                PathEl::ClosePath => {
                    self.out.push('h');
                    at = start;
                }
            }
        }
    }

    /// Append one `c` operator: three points, space separated.
    fn write_cubic(&mut self, c1: Point, c2: Point, end: Point) {
        write_point(&mut self.out, c1);
        self.out.push(' ');
        write_point(&mut self.out, c2);
        self.out.push(' ');
        write_point(&mut self.out, end);
        self.out.push_str(" c");
    }

    /// Append a name's bytes, escaping what ISO 32000-1 §7.3.5 requires.
    ///
    /// Every name this canvas writes is one it minted, so nothing needs
    /// escaping in practice; the escape is here so that a name reaching it
    /// some other way still produces a stream our own lexer reads back.
    fn push_name(&mut self, name: &Name) {
        for byte in name.as_bytes() {
            if byte.is_ascii_alphanumeric() {
                self.out.push(char::from(*byte));
            } else {
                let _ = write!(self.out, "#{byte:02X}");
            }
        }
    }

    /// The name `value` is known by in `category`, allocating a fresh one
    /// that collides with neither the page's own resources nor anything this
    /// drawing already added.
    fn realize(&mut self, category: &'static Name, value: Object) -> Name {
        // The same font or image drawn twice takes one name, not two.
        if let Some((_, name, _)) = self
            .added
            .iter()
            .find(|(held_category, _, held)| *held_category == category && *held == value)
        {
            return name.clone();
        }
        let name = self.free_name(category);
        self.taken.push((category, name.clone()));
        self.added.push((category, name.clone(), value));
        name
    }

    /// The first `PdfrumC<n>` name free in `category`.
    ///
    /// The prefix is this crate's and nothing else in the workspace mints it:
    /// `pdfrum-edit`'s regeneration uses `FX*`, and a producer's own names are
    /// whatever the page already holds — which is exactly what `taken`
    /// carries, so a collision is checked rather than assumed away.
    fn free_name(&self, category: &Name) -> Name {
        for id in 1u32.. {
            let candidate = Name::from(format!("PdfrumC{id}").as_str());
            if !self
                .taken
                .iter()
                .any(|(cat, name)| *cat == category && *name == candidate)
            {
                return candidate;
            }
        }
        Name::from("PdfrumC1")
    }
}

/// The rectangle `path` draws, when it draws exactly one.
///
/// Four corners, axis-aligned, closed — which is what `Rect::into_path`
/// produces and what a caller's own rectangle almost always is. Anything else
/// returns `None` and is written segment by segment.
///
/// The coordinate comparisons are **exact**, deliberately. This is a
/// recognizer for a shape the caller built, not a geometric tolerance: a path
/// whose corners are a rounding error apart is not the rectangle the caller
/// asked for, and writing it as `re` would move an edge. `pdfrum-edit`'s own
/// emitter recognizes its rectangles the same way.
#[expect(
    clippy::float_cmp,
    reason = "exact recognition of a caller-built rectangle; a tolerance here would move an edge"
)]
fn axis_aligned_rect(path: &BezPath) -> Option<Rect> {
    let corners: [Point; 4] = match path.elements() {
        // A closed four-sided path, with or without the redundant final
        // `LineTo` back to the start that some shapes emit before `h`.
        [
            PathEl::MoveTo(first),
            PathEl::LineTo(second),
            PathEl::LineTo(third),
            PathEl::LineTo(fourth),
            PathEl::ClosePath,
        ] => [*first, *second, *third, *fourth],
        [
            PathEl::MoveTo(first),
            PathEl::LineTo(second),
            PathEl::LineTo(third),
            PathEl::LineTo(fourth),
            PathEl::LineTo(back),
            PathEl::ClosePath,
        ] if back == first => [*first, *second, *third, *fourth],
        _ => return None,
    };
    // Axis-aligned means each side shares one coordinate with the next.
    for index in 0..4 {
        let from = *corners.get(index)?;
        let to = *corners.get((index + 1) % 4)?;
        if from.x != to.x && from.y != to.y {
            return None;
        }
    }
    // And it must be a rectangle rather than a degenerate zig-zag: opposite
    // corners differ in both coordinates.
    let (origin, opposite) = (*corners.first()?, *corners.get(2)?);
    if origin.x == opposite.x || origin.y == opposite.y {
        return None;
    }
    Some(Rect::new(origin.x, origin.y, opposite.x, opposite.y))
}

/// A colour's alpha, clamped.
fn alpha_of(color: Color) -> f64 {
    f64::from(color.components[3]).clamp(0.0, 1.0)
}

/// An `f64` narrowed to the `f32` a PDF number is (SPEC §2).
#[expect(
    clippy::cast_possible_truncation,
    reason = "PDF numbers are f32 (SPEC §2); the geometry vocabulary is f64"
)]
fn as_f32(value: f64) -> f32 {
    value as f32
}

/// Append an `f64` through the crate-wide number spelling.
fn write_f64(out: &mut String, value: f64) {
    write_float(out, as_f32(value));
}

/// The cubic control points equal to the quadratic `previous`-`control`-`end`.
fn quad_to_cubic(previous: Point, control: Point, end: Point) -> (Point, Point) {
    let third = 2.0 / 3.0;
    (
        previous + (control - previous) * third,
        end + (control - end) * third,
    )
}

/// The paint operator for a paint and a fill rule (ISO 32000-1 table 60).
fn paint_operator(paint: Paint, rule: Fill) -> &'static str {
    match (paint, rule) {
        (Paint::Fill(_), Fill::NonZero) => " f",
        (Paint::Fill(_), Fill::EvenOdd) => " f*",
        (Paint::Stroke(_), _) => " S",
        (Paint::FillStroke(_, _), Fill::NonZero) => " B",
        (Paint::FillStroke(_, _), Fill::EvenOdd) => " B*",
    }
}

/// Append `codes` as a hexadecimal string, `<...>`.
///
/// Hex rather than a literal `(...)` so that no byte ever needs escaping: a
/// composite font's two-byte codes are full of parentheses and backslashes,
/// and getting that escaping subtly wrong is how a writer produces a stream
/// nothing can read.
fn write_hex_string(out: &mut String, codes: &[u8]) {
    out.push('<');
    for byte in codes {
        let _ = write!(out, "{byte:02X}");
    }
    out.push('>');
}

impl DocEdit<'_> {
    /// Draw on page `index`, appending what `body` draws as one new content
    /// stream.
    ///
    /// The canvas's coordinate space is the page as displayed — see
    /// [`Canvas`]. The stream is wrapped in `q`/`Q` and appended to the
    /// page's `/Contents` array, so the page's own graphics state cannot leak
    /// into the drawing and the drawing's cannot leak into the page. The
    /// page's existing streams are **not** rewritten, which is why drawing on
    /// a page costs none of the regeneration losses
    /// [`PageEdit`](crate::PageEdit) documents.
    ///
    /// # The resource-merging rule
    ///
    /// Fonts, images and graphics states the drawing used are merged into the
    /// page's `/Resources` under names of this crate's own `PdfrumC<n>`
    /// series, each checked against the names the page already holds, so a
    /// merged name can collide with neither the producer's nor
    /// `pdfrum-edit`'s `FX*`. A `/Resources` the page shares with another
    /// page is copied before it is written to, so drawing on one page cannot
    /// change another.
    ///
    /// ```
    /// use pdfrum::{Color, Document, Point, SaveOptions, StandardFont};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let font = edit.standard_font(StandardFont::Helvetica)?;
    /// edit.draw_page(0, |c| {
    ///     c.text("drawn", &font, 12.0, Point::new(40.0, 40.0), Color::BLACK);
    /// })?;
    ///
    /// let mut bytes = Vec::new();
    /// edit.write_to(&mut bytes, &SaveOptions::default())?;
    /// let saved = Document::from_bytes(bytes.into())?;
    /// assert!(saved.page(0)?.text().to_string().contains("drawn"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Whatever `body` refused to draw — a character the font has no glyph
    /// for, most often — and [`pdfrum_edit::Error::InlinePage`] for a page with no
    /// object of its own. Nothing is written when the drawing failed.
    pub fn draw_page(
        &mut self,
        index: impl Into<PageIndex>,
        body: impl FnOnce(&mut Canvas<'_, '_>),
    ) -> Result<()> {
        let index = index.into();
        let Some((reference, dict, resources)) = self.page_state(index)? else {
            return Err(pdfrum_edit::Error::InlinePage(index).into());
        };
        let (to_page, size) = self.canvas_space(reference, &dict);

        let taken = existing_names(&resources, &self.inner);
        let mut canvas = Canvas {
            out: String::new(),
            edit: self,
            added: Vec::new(),
            taken,
            size,
            index,
            failed: None,
        };
        body(&mut canvas);
        if let Some(error) = canvas.failed {
            return Err(error);
        }
        let Canvas { out, added, .. } = canvas;
        if out.is_empty() {
            return Ok(());
        }

        // `q` … `Q` around the whole drawing, with the canvas-to-page
        // transform inside it, so neither state escapes into the other.
        let mut bytes = String::with_capacity(out.len() + 64);
        bytes.push_str("q\n");
        write_matrix(&mut bytes, to_page);
        bytes.push_str(" cm\n");
        bytes.push_str(&out);
        bytes.push_str("Q\n");

        self.append_stream(reference, &dict, &resources, bytes.as_bytes(), &added);
        Ok(())
    }

    /// Draw on every page, one canvas each.
    ///
    /// The closure runs once per page in order and is handed that page's own
    /// canvas, so [`Canvas::size`] and [`Canvas::page`] are the page's. A page
    /// written inline in its parent's `/Kids` is skipped rather than refused:
    /// a whole-document watermark should not fail because one page of a
    /// thousand cannot carry it.
    ///
    /// ```
    /// use pdfrum::{Color, Document, SaveOptions, Stroke, Point};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world_2_pages.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_pages(|c| {
    ///     let y = c.size().height - 40.0;
    ///     c.line(Point::new(40.0, y), Point::new(c.size().width - 40.0, y),
    ///            Stroke::new(Color::BLACK, 0.5));
    /// })?;
    /// let mut bytes = Vec::new();
    /// edit.write_to(&mut bytes, &SaveOptions::default())?;
    /// assert!(bytes.starts_with(b"%PDF-"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`DocEdit::draw_page`], for the first page whose drawing failed.
    pub fn draw_pages(&mut self, mut body: impl FnMut(&mut Canvas<'_, '_>)) -> Result<()> {
        for index in 0..self.doc.page_count() {
            let index = PageIndex::from(index);
            if self.page_state(index)?.is_none() {
                continue;
            }
            self.draw_page(index, &mut body)?;
        }
        Ok(())
    }

    /// The canvas-to-page transform and the displayed size for page `index`.
    ///
    /// The transform is the inverse of the renderer's own display matrix over
    /// the crop box, which is the whole of the coordinate-space composition:
    /// the crop box's offset and the `/Rotate` quarter turn fall out of it
    /// together, and there is nothing else to get right.
    fn canvas_space(&self, reference: ObjRef, dict: &Dict) -> (Affine, kurbo::Size) {
        // The dictionary is the session's, read through the overlay, and the
        // inheritance walk goes through the overlay too — so a `/Rotate` or a
        // `/CropBox` this same session set is what the canvas is built on,
        // rather than the base document's stale one.
        let page = pdfrum_parser::PageDict {
            dict: dict.clone(),
            reference: Some(reference),
        };
        let mut diags = Diagnostics::default();
        let (_, crop) = pdfrum_page::derive_boxes(
            &page.dict,
            |key| page.inherited(key, &self.inner),
            &self.inner,
            &mut diags,
        );
        self.doc.note(&diags);
        let rotate_key = Name::from("Rotate");
        let rotate = pdfrum_page::Rotation::from_degrees(
            page.dict
                .raw(&rotate_key)
                .cloned()
                .or_else(|| page.inherited(&rotate_key, &self.inner))
                .and_then(|value| value.resolve(&self.inner).ok()?.get().as_int())
                .unwrap_or(0),
        );
        let size = if rotate.quarters().is_multiple_of(2) {
            kurbo::Size::new(crop.width(), crop.height())
        } else {
            kurbo::Size::new(crop.height(), crop.width())
        };
        (rotate.display_matrix(crop).inverse(), size)
    }

    /// The advance of `codes` in the font `font` names, in thousandths of an
    /// em — the font crate's reading of the dictionary this session wrote.
    pub(crate) fn string_width(&self, font: ObjRef, codes: &[u8]) -> f64 {
        let mut diags = Diagnostics::default();
        let loaded = self
            .inner
            .fetch(font)
            .ok()
            .as_deref()
            .and_then(Object::as_dict)
            .and_then(|dict| {
                pdfrum_font::load(
                    dict,
                    &self.inner,
                    &pdfrum_font::FontCache::new(),
                    &self.doc.limits,
                    &mut diags,
                )
            });
        match loaded {
            Some(metrics) => f64::from(metrics.string_width(codes)),
            // A face with no metrics: half an em a code, the proportions of a
            // typical Latin face, so a centred string is at least close.
            None => 500.0 * f64::from(u32::try_from(codes.len()).unwrap_or(u32::MAX)),
        }
    }

    /// Append `bytes` as one more content stream of the page `reference`
    /// names, merging `added` into its `/Resources`.
    fn append_stream(
        &mut self,
        reference: ObjRef,
        dict: &Dict,
        resources: &Dict,
        bytes: &[u8],
        added: &[(&'static Name, Name, Object)],
    ) {
        let stream = Stream::new(
            Dict::from_pairs([(
                pdf_names::LENGTH.clone(),
                Object::Int(i64::try_from(bytes.len()).unwrap_or(0)),
            )]),
            ByteSpan::from(bytes.to_vec()),
        );
        let fresh = self.inner.add(Object::Stream(Box::new(stream)));

        let shape = ContentsShape::read(dict, &self.inner);
        let (_, next) = shape.with_added(fresh);
        let shared = pdfrum_edit::shared_objects(&self.inner);

        let mut dict = dict.clone();
        // The `/Contents` array: reused when the page owns it outright,
        // otherwise a fresh one, exactly as `apply_rewrite` decides it.
        let elements = next.elements();
        let array = Object::Array(Array::of(elements.iter().map(|e| Object::Ref(*e))));
        let reusable = matches!(
            dict.raw(pdf_names::CONTENTS),
            Some(Object::Ref(r)) if !shared.contains(&r.num) && !elements.contains(r)
        );
        let contents = match dict.raw(pdf_names::CONTENTS) {
            Some(Object::Ref(existing)) if reusable => {
                let existing = *existing;
                self.inner.replace(existing, array);
                Object::Ref(existing)
            }
            _ => Object::Ref(self.inner.add(array)),
        };
        dict = with_key(&dict, pdf_names::CONTENTS, contents);

        let merged = merge_resources(resources, added);
        match dict.raw(pdf_names::RESOURCES) {
            // The page reaches its resources through an object it does not
            // share: write through it and leave the page's key alone.
            Some(Object::Ref(existing)) if !shared.contains(&existing.num) => {
                let existing = *existing;
                self.inner.replace(existing, Object::Dict(merged));
            }
            // Shared, inline or absent: the page gets its own copy, so
            // drawing on one page cannot change another.
            _ => dict = with_key(&dict, pdf_names::RESOURCES, Object::Dict(merged)),
        }

        self.inner.replace(reference, Object::Dict(dict));
    }
}

/// Every name the page's `/Resources` already uses, per category, so a fresh
/// one is chosen against them rather than merely hoped to differ.
fn existing_names(resources: &Dict, r: &impl Resolve) -> Vec<(&'static Name, Name)> {
    let mut taken = Vec::new();
    for category in [pdf_names::FONT, pdf_names::XOBJECT, pdf_names::EXT_G_STATE] {
        let Some(sub) = resources.dict(category, r) else {
            continue;
        };
        for (name, _) in sub.iter() {
            taken.push((category, name.clone()));
        }
    }
    taken
}

/// `resources` with `added` merged in, each under the category it belongs to.
///
/// Every other key — colour spaces, patterns, `/ProcSet` — is carried through
/// untouched: the canvas emits no operator that would name one.
fn merge_resources(resources: &Dict, added: &[(&'static Name, Name, Object)]) -> Dict {
    let mut out = Dict::new();
    for (key, value) in resources.iter() {
        let extra: Vec<_> = added
            .iter()
            .filter(|(category, _, _)| *category == key)
            .collect();
        if extra.is_empty() {
            out.push(key.clone(), value.clone());
            continue;
        }
        // A category the page already has: keep every entry and add ours.
        // The sub-dictionary may be indirect; it is inlined here rather than
        // written through, because the object could be shared with a page
        // this drawing is not touching.
        let mut sub = match value {
            Object::Dict(dict) => dict.clone(),
            _ => Dict::new(),
        };
        for (_, name, held) in extra {
            sub.push(name.clone(), held.clone());
        }
        out.push(key.clone(), Object::Dict(sub));
    }
    for category in [pdf_names::FONT, pdf_names::XOBJECT, pdf_names::EXT_G_STATE] {
        if out.contains_key(category) {
            continue;
        }
        let mut sub = Dict::new();
        for (_, name, held) in added.iter().filter(|(cat, _, _)| *cat == category) {
            sub.push(name.clone(), held.clone());
        }
        if !sub.is_empty() {
            out.push(category.clone(), Object::Dict(sub));
        }
    }
    out
}

/// A copy of `dict` with `key` set, keeping every other entry in its place.
fn with_key(dict: &Dict, key: &Name, value: Object) -> Dict {
    let mut out = Dict::new();
    let mut written = false;
    for (existing, held) in dict.iter() {
        if existing == key {
            if !written {
                out.push(existing.clone(), value.clone());
                written = true;
            }
        } else {
            out.push(existing.clone(), held.clone());
        }
    }
    if !written {
        out.push(key.clone(), value);
    }
    out
}
