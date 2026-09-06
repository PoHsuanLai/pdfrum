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
#[derive(Debug, Clone, PartialEq)]
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
    pub fn fill(&self) -> Option<Color> {
        match self {
            Self::Fill(color) | Self::FillStroke(color, _) => Some(*color),
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
    pub fn stroke(&self) -> Option<&Stroke> {
        match self {
            Self::Stroke(stroke) | Self::FillStroke(_, stroke) => Some(stroke),
            Self::Fill(_) => None,
        }
    }
}

/// A stroke's colour, width and pen shape, in canvas units.
///
/// The fields stay public and every one but `color` and `width` has a
/// default, so `Stroke { cap: LineCap::Round, ..Stroke::new(color, 1.0) }`
/// works and the four settings ISO 32000-1 §8.4.3.3-§8.4.3.6 name are
/// reachable without builder ceremony. [`Stroke::new`] keeps meaning what it
/// always meant: PDF's own defaults — butt cap, miter join, miter limit 10,
/// no dash.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    /// The colour. Its alpha is honoured, as an `/ExtGState` `/CA`.
    pub color: Color,
    /// The line width in canvas units — page points.
    pub width: f64,
    /// How the open ends of a subpath are drawn. `J`.
    pub cap: LineCap,
    /// How two segments meet at a corner. `j`.
    pub join: LineJoin,
    /// Where a [`LineJoin::Miter`] corner becomes a bevel instead, as the
    /// ratio of miter length to line width. `M`.
    pub miter_limit: MiterLimit,
    /// The on/off pattern, or `None` for a solid line. `d`.
    pub dash: Option<Dash>,
}

impl Stroke {
    /// A solid stroke of `color` at `width` points, with PDF's default pen:
    /// butt cap, miter join, miter limit 10.
    ///
    /// ```
    /// let hairline = pdfrum::Stroke::new(pdfrum::Color::BLACK, 0.5);
    /// assert_eq!(hairline.width, 0.5);
    /// assert_eq!(hairline.cap, pdfrum::LineCap::Butt);
    /// assert!(hairline.dash.is_none());
    /// ```
    #[must_use]
    pub fn new(color: Color, width: f64) -> Self {
        Self {
            color,
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: MiterLimit::default(),
            dash: None,
        }
    }

    /// The same stroke with `cap` at its open ends.
    ///
    /// ```
    /// use pdfrum::{Color, LineCap, Stroke};
    ///
    /// let round = Stroke::new(Color::BLACK, 4.0).with_cap(LineCap::Round);
    /// assert_eq!(round.cap, LineCap::Round);
    /// ```
    #[must_use]
    pub fn with_cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    /// The same stroke with `join` at its corners.
    ///
    /// ```
    /// use pdfrum::{Color, LineJoin, Stroke};
    ///
    /// let soft = Stroke::new(Color::BLACK, 4.0).with_join(LineJoin::Round);
    /// assert_eq!(soft.join, LineJoin::Round);
    /// ```
    #[must_use]
    pub fn with_join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    /// The same stroke with `limit` on its miter joins.
    ///
    /// ```
    /// use pdfrum::{Color, MiterLimit, Stroke};
    ///
    /// let blunt = Stroke::new(Color::BLACK, 4.0).with_miter_limit(MiterLimit::new(2.0));
    /// assert_eq!(blunt.miter_limit.get(), 2.0);
    /// ```
    #[must_use]
    pub fn with_miter_limit(mut self, limit: MiterLimit) -> Self {
        self.miter_limit = limit;
        self
    }

    /// The same stroke dashed by `dash`.
    ///
    /// ```
    /// use pdfrum::{Color, Dash, Stroke};
    ///
    /// let dashed = Stroke::new(Color::BLACK, 1.0)
    ///     .with_dash(Dash::new(&[4.0, 2.0], 0.0).expect("a valid dash"));
    /// assert!(dashed.dash.is_some());
    /// ```
    #[must_use]
    pub fn with_dash(mut self, dash: Dash) -> Self {
        self.dash = Some(dash);
        self
    }
}

/// How the open ends of a stroked subpath are drawn — ISO 32000-1 §8.4.3.3's
/// line cap style, written as `J`.
///
/// An enum rather than the `0`/`1`/`2` the operator takes: the wire spelling
/// is an encoding detail (STYLE.md §2), and `LineCap::Round` says at a call
/// site what `1` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    /// Squared off exactly at the endpoint. PDF's default.
    #[default]
    Butt,
    /// A half-disc of the line's width centred on the endpoint.
    Round,
    /// A half-square projecting half a line width past the endpoint.
    Square,
}

impl LineCap {
    /// The operand `J` takes.
    fn operand(self) -> u8 {
        match self {
            Self::Butt => 0,
            Self::Round => 1,
            Self::Square => 2,
        }
    }
}

/// How two segments meet at a corner — ISO 32000-1 §8.4.3.4's line join
/// style, written as `j`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    /// Extended outer edges meeting in a point, beveled past
    /// [`Stroke::miter_limit`]. PDF's default.
    #[default]
    Miter,
    /// An arc of the line's width around the corner point.
    Round,
    /// The notch between the two segments filled with a triangle.
    Bevel,
}

impl LineJoin {
    /// The operand `j` takes.
    fn operand(self) -> u8 {
        match self {
            Self::Miter => 0,
            Self::Round => 1,
            Self::Bevel => 2,
        }
    }
}

/// The ratio of miter length to line width past which a [`LineJoin::Miter`]
/// corner is drawn beveled instead — ISO 32000-1 §8.4.3.5's `M`.
///
/// A newtype rather than a bare `f64` because the value has a floor: the
/// miter length is never shorter than the line width, so a ratio below 1
/// asks for something that cannot happen. It clamps rather than refusing,
/// unlike [`Dash`], because every out-of-range ratio has one obviously
/// intended reading and none of them makes a reader reject the stream.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct MiterLimit(f64);

impl MiterLimit {
    /// A miter limit of `ratio`, clamped up to 1; a non-finite ratio gives
    /// the default.
    ///
    /// ```
    /// use pdfrum::MiterLimit;
    ///
    /// assert_eq!(MiterLimit::new(4.0).get(), 4.0);
    /// assert_eq!(MiterLimit::new(0.5).get(), 1.0);
    /// assert_eq!(MiterLimit::new(f64::NAN).get(), 10.0);
    /// ```
    #[must_use]
    pub fn new(ratio: f64) -> Self {
        if ratio.is_finite() {
            Self(ratio.max(1.0))
        } else {
            Self::default()
        }
    }

    /// The ratio.
    ///
    /// ```
    /// assert_eq!(pdfrum::MiterLimit::default().get(), 10.0);
    /// ```
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl Default for MiterLimit {
    /// PDF's own initial value, 10 (ISO 32000-1 table 52).
    fn default() -> Self {
        Self(10.0)
    }
}

/// A dash pattern — ISO 32000-1 §8.4.3.6's dash array and phase, written as
/// `d`.
///
/// # Why construction is fallible
///
/// `d` is one of the few graphics-state operators a reader may reject
/// outright: a negative length, or an array summing to zero, is not a
/// degenerate dash but an *invalid* one, and a viewer that refuses it refuses
/// the whole content stream — every later operator with it. So an invalid
/// array is caught here, at construction, rather than normalized into a
/// pattern the caller did not ask for and cannot see. [`Dash::new`] returns
/// `None` and nothing reaches the stream.
///
/// A solid line is spelled `Stroke::dash = None`, so an empty array is
/// refused too: it has a valid PDF spelling, but it means the thing the
/// `Option` already says.
#[derive(Debug, Clone, PartialEq)]
pub struct Dash {
    /// Alternating on and off lengths, all finite and non-negative, summing
    /// to more than zero.
    lengths: Vec<f64>,
    /// How far into the pattern the line starts. Finite and non-negative.
    phase: f64,
}

impl Dash {
    /// A dash of alternating on/off `lengths`, starting `phase` units into
    /// the pattern.
    ///
    /// `None` if `lengths` is empty, holds anything negative or not finite,
    /// or sums to zero, or if `phase` is negative or not finite — each of
    /// which is an invalid `d` operand rather than an unusual one.
    ///
    /// ```
    /// use pdfrum::Dash;
    ///
    /// assert!(Dash::new(&[4.0, 2.0], 0.0).is_some());
    /// assert!(Dash::new(&[4.0, -2.0], 0.0).is_none());
    /// assert!(Dash::new(&[0.0, 0.0], 0.0).is_none());
    /// assert!(Dash::new(&[], 0.0).is_none());
    /// ```
    #[must_use]
    pub fn new(lengths: &[f64], phase: f64) -> Option<Self> {
        if lengths.is_empty() || !phase.is_finite() || phase < 0.0 {
            return None;
        }
        if lengths
            .iter()
            .any(|length| !length.is_finite() || *length < 0.0)
        {
            return None;
        }
        if lengths.iter().sum::<f64>() <= 0.0 {
            return None;
        }
        Some(Self {
            lengths: lengths.to_vec(),
            phase,
        })
    }

    /// The alternating on/off lengths.
    ///
    /// ```
    /// let dash = pdfrum::Dash::new(&[3.0, 1.0], 0.5).expect("a valid dash");
    /// assert_eq!(dash.lengths(), &[3.0, 1.0]);
    /// assert_eq!(dash.phase(), 0.5);
    /// ```
    #[must_use]
    pub fn lengths(&self) -> &[f64] {
        &self.lengths
    }

    /// How far into the pattern the line starts.
    #[must_use]
    pub fn phase(&self) -> f64 {
        self.phase
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
    /// What the operators are being written into.
    surface: Surface,
    /// The first error a drawing method hit. Reported once, from
    /// [`DocEdit::draw_page`], rather than at every call.
    failed: Option<Error>,
}

/// What a canvas's operators are being written into.
///
/// An enum rather than an `Option<PageIndex>`, because the two destinations
/// differ in more than whether a page index exists: a page's drawing is
/// *appended* to `/Contents` and merges into the page's own `/Resources`,
/// while a form's becomes a standalone `/Subtype /Form` stream with a
/// `/Resources` of its own and no page to collide with. [`Canvas::page`]
/// answers for the first and has nothing to answer for the second, which is
/// why it returns an `Option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// One page's appended content stream.
    Page(PageIndex),
    /// A Form `XObject`'s own stream, placed later by [`Canvas::place_form`].
    ///
    /// Behind the feature that is the only thing that compiles a form: with
    /// `svg-ingest` off nothing constructs it, and a variant nothing
    /// constructs is the dead code STYLE.md §4 forbids.
    #[cfg(feature = "svg-ingest")]
    Form,
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

    /// The page this canvas draws on, or `None` when it is compiling a Form
    /// `XObject` that no page owns yet.
    ///
    /// A canvas handed to [`DocEdit::draw_page`] or [`DocEdit::draw_pages`]
    /// always answers `Some`; the `None` case is the form compiled by
    /// [`DocEdit::compile_svg`], whose content belongs to no page until a
    /// [`Canvas::place_svg`](crate::Canvas::place_svg) puts it on one.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world_2_pages.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_pages(|c| assert!(c.page().is_some_and(|p| u32::from(p) < 2)))?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn page(&self) -> Option<PageIndex> {
        match self.surface {
            Surface::Page(index) => Some(index),
            #[cfg(feature = "svg-ingest")]
            Surface::Form => None,
        }
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
        let operator = paint_operator(&paint, rule);
        self.out.push_str("q\n");
        self.set_paint(paint);
        self.write_path(&path);
        self.out.push_str(operator);
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

    /// Paint `shape` with a shading dictionary, clipped to the shape.
    ///
    /// `sh` fills the *whole current clip*, so the shape becomes a clip and
    /// the shading is painted through it — which is how PDF spells a
    /// gradient-filled path. `transform` is the gradient's own coordinate
    /// mapping, applied inside the clip so it moves the gradient rather than
    /// the shape.
    ///
    /// Crate-internal because a caller-facing shading API is a design of its
    /// own — colour spaces, function types, the extend flags — and the one
    /// caller here is [`Canvas::draw_svg`](crate::Canvas::draw_svg), which
    /// builds the dictionary from a `usvg` gradient.
    #[cfg(feature = "svg-ingest")]
    pub(crate) fn shade(
        &mut self,
        shape: &BezPath,
        rule: Fill,
        shading: &Dict,
        transform: Affine,
        opacity: f64,
    ) {
        let name = self.realize(pdf_names::SHADING, Object::Dict(shading.clone()));
        self.out.push_str("q\n");
        self.write_path(shape);
        self.out.push_str(match rule {
            Fill::NonZero => " W n\n",
            Fill::EvenOdd => " W* n\n",
        });
        if opacity < 1.0 {
            self.opacity(opacity);
        }
        write_matrix(&mut self.out, transform);
        self.out.push_str(" cm /");
        self.push_name(&name);
        self.out.push_str(" sh\nQ\n");
    }

    /// Embed a PNG or JPEG an SVG `<image>` carried, as a new image
    /// `/XObject`.
    ///
    /// `None` when the bytes are neither, or decode to nothing this session
    /// can embed; the caller reports that as
    /// [`Unsupported::ImageFormat`](crate::Unsupported::ImageFormat) rather
    /// than failing the whole drawing, because one bad `<image>` should not
    /// cost the rest of the document.
    #[cfg(feature = "svg-ingest")]
    pub(crate) fn embed_svg_image(&mut self, bytes: &[u8]) -> Option<EmbeddedImage> {
        // JPEG passes through whole: `/DCTDecode` is the PDF filter for
        // exactly these bytes, so nothing is decoded and nothing is lost.
        if bytes.starts_with(&[0xFF, 0xD8]) {
            return self.edit.embed_jpeg(bytes).ok();
        }
        let decoded = crate::svg_ingest::decode_png(bytes)?;
        self.edit
            .embed_image(
                &decoded.pixels,
                decoded.width,
                decoded.height,
                decoded.format,
            )
            .ok()
    }

    /// The session this canvas draws into.
    ///
    /// Ingestion reads the SVG font set off it before parsing; there is no
    /// mutable access, because a drawing method that reached into the session
    /// past the resource machinery could add an object nothing names.
    #[cfg(feature = "svg-text")]
    pub(crate) fn session(&self) -> &DocEdit<'_> {
        self.edit
    }

    /// Record the first failure; later ones are dropped, because the first is
    /// the one that explains the rest.
    fn fail(&mut self, error: Error) {
        if self.failed.is_none() {
            self.failed = Some(error);
        }
    }

    /// Write the colour, width and pen operators `paint` asks for.
    fn set_paint(&mut self, paint: Paint) {
        let (fill, stroke) = match paint {
            Paint::Fill(color) => (Some(color), None),
            Paint::Stroke(stroke) => (None, Some(stroke)),
            Paint::FillStroke(color, stroke) => (Some(color), Some(stroke)),
        };

        // Alpha rides on an `/ExtGState`, since `rg`/`RG` carry none.
        let alpha = fill
            .map(alpha_of)
            .into_iter()
            .chain(stroke.as_ref().map(|stroke| alpha_of(stroke.color)))
            .fold(1.0_f64, f64::min);
        if alpha < 1.0 {
            self.opacity(alpha);
        }
        if let Some(color) = fill {
            self.write_rgb(color);
            self.out.push_str(" rg\n");
        }
        if let Some(stroke) = stroke {
            self.write_rgb(stroke.color);
            self.out.push_str(" RG\n");
            write_f64(&mut self.out, stroke.width.max(0.0));
            self.out.push_str(" w\n");
            self.write_pen(&stroke);
        }
    }

    /// Write the cap, join, miter-limit and dash operators, each only when it
    /// differs from the graphics state's own initial value (ISO 32000-1
    /// table 52): the drawing runs inside a fresh `q`, so an unwritten one is
    /// already what the caller asked for and the stream stays short.
    fn write_pen(&mut self, stroke: &Stroke) {
        if stroke.cap != LineCap::Butt {
            let _ = writeln!(self.out, "{} J", stroke.cap.operand());
        }
        if stroke.join != LineJoin::Miter {
            let _ = writeln!(self.out, "{} j", stroke.join.operand());
        }
        if stroke.miter_limit != MiterLimit::default() {
            write_f64(&mut self.out, stroke.miter_limit.get());
            self.out.push_str(" M\n");
        }
        if let Some(dash) = &stroke.dash {
            self.out.push('[');
            for (i, length) in dash.lengths().iter().enumerate() {
                if i > 0 {
                    self.out.push(' ');
                }
                write_f64(&mut self.out, *length);
            }
            self.out.push_str("] ");
            write_f64(&mut self.out, dash.phase());
            self.out.push_str(" d\n");
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

/// Drawing compiled once into a Form `XObject`, placeable on any number of
/// pages.
///
/// A `/Subtype /Form` stream with its own `/BBox` and `/Resources`, held as a
/// single object in the document. Placing it writes one `Do` — so the same
/// logo on twenty pages is one copy of the content and twenty references,
/// rather than twenty copies of the content.
///
/// Produced by [`DocEdit::compile_svg`] and placed by
/// [`Canvas::place_svg`](crate::Canvas::place_svg).
/// It carries no borrow of the session that made it, so a caller compiles
/// once and then places inside as many `draw_page` closures as they like.
#[cfg(feature = "svg-ingest")]
#[derive(Debug, Clone, PartialEq)]
pub struct SvgForm {
    /// The form's object in the session that compiled it.
    object: ObjRef,
    /// The form's own coordinate box, in its own space. A placement maps this
    /// onto the destination rectangle.
    bbox: Rect,
}

#[cfg(feature = "svg-ingest")]
impl SvgForm {
    /// The form's `/BBox`, in the form's own coordinate space.
    ///
    /// Its aspect ratio is what [`SvgFit`](crate::SvgFit) preserves when the
    /// destination rectangle has a different one.
    #[must_use]
    pub fn bbox(&self) -> Rect {
        self.bbox
    }
}

#[cfg(feature = "svg-ingest")]
impl Canvas<'_, '_> {
    /// Place `form` so its [`SvgForm::bbox`] covers `into`.
    ///
    /// One `Do` operator against the form's single object, so placing the
    /// same form on every page of a document costs one copy of the content
    /// and one reference per page. The placement is scoped in its own `q`/`Q`
    /// and clipped to `into`, so nothing the form draws escapes the rectangle
    /// and the canvas's own state survives it.
    ///
    /// The form's box is stretched onto `into`, with no fit of its own — the
    /// caller-facing spelling is
    /// [`Canvas::place_svg`](crate::Canvas::place_svg), which chooses the
    /// rectangle through an [`SvgFit`](crate::SvgFit) and then calls this.
    /// Crate-internal because a second public placement that differs only in
    /// taking a pre-fitted rectangle would be a way of saying the same thing
    /// twice (STYLE.md §4).
    pub(crate) fn place_form(&mut self, form: &SvgForm, into: Rect) {
        if into.width() == 0.0 || into.height() == 0.0 || form.bbox.is_zero_area() {
            return;
        }
        let name = self.realize(pdf_names::XOBJECT, Object::Ref(form.object));
        // The form's `/BBox` is mapped onto `into`: scale by the ratio of the
        // two, then carry the form's own origin to the destination's. `/BBox`
        // is *not* assumed to start at the origin, because a compiled SVG's
        // need not.
        let scale_x = into.width() / form.bbox.width();
        let scale_y = into.height() / form.bbox.height();
        let placement = Affine::new([
            scale_x,
            0.0,
            0.0,
            scale_y,
            into.x0 - form.bbox.x0 * scale_x,
            into.y0 - form.bbox.y0 * scale_y,
        ]);
        self.out.push_str("q\n");
        self.write_path(&into.into_path(0.1));
        self.out.push_str(" W n\n");
        write_matrix(&mut self.out, placement);
        self.out.push_str(" cm /");
        self.push_name(&name);
        self.out.push_str(" Do\nQ\n");
    }
}

#[cfg(feature = "svg-ingest")]
impl DocEdit<'_> {
    /// Compile `body`'s drawing into a Form `XObject` over `bbox`.
    ///
    /// The canvas `body` receives writes into the form's own stream and its
    /// own `/Resources`, so nothing it names can collide with a page's — a
    /// form is a fresh resource scope, which is why the placement is one
    /// object rather than a merge per page.
    ///
    /// The shared half of [`DocEdit::compile_svg`]; it is crate-internal
    /// because the caller-facing surface for "drawing a caller wrote once" is
    /// [`DocEdit::draw_page`] with the caller's own closure, and a second
    /// spelling of it would be an option with no reader (STYLE.md §4).
    ///
    /// # Errors
    ///
    /// Whatever `body` refused to draw, as [`DocEdit::draw_page`] reports it.
    pub(crate) fn compile_form(
        &mut self,
        bbox: Rect,
        body: impl FnOnce(&mut Canvas<'_, '_>),
    ) -> Result<SvgForm> {
        let mut canvas = Canvas {
            out: String::new(),
            edit: self,
            added: Vec::new(),
            // A form's resource scope is its own and starts empty: there is
            // no page dictionary whose names it has to avoid.
            taken: Vec::new(),
            size: bbox.size(),
            surface: Surface::Form,
            failed: None,
        };
        body(&mut canvas);
        if let Some(error) = canvas.failed {
            return Err(error);
        }
        let Canvas { out, added, .. } = canvas;

        let resources = merge_resources(&Dict::new(), &added);
        let bytes = out.into_bytes();
        let dict = Dict::from_pairs([
            (
                pdf_names::TYPE.clone(),
                Object::Name(pdf_names::XOBJECT.clone()),
            ),
            (pdf_names::SUBTYPE.clone(), Object::Name(Name::from("Form"))),
            (Name::from("FormType"), Object::Int(1)),
            (Name::from("BBox"), Object::Array(rect_array(bbox))),
            (pdf_names::RESOURCES.clone(), Object::Dict(resources)),
            (
                pdf_names::LENGTH.clone(),
                Object::Int(i64::try_from(bytes.len()).unwrap_or(0)),
            ),
        ]);
        let object = self
            .inner
            .add(Object::Stream(Box::new(Stream::new(dict, bytes.into()))));
        Ok(SvgForm { object, bbox })
    }
}

/// A rectangle as the four numbers a `/BBox` holds.
#[cfg(feature = "svg-ingest")]
fn rect_array(rect: Rect) -> Array {
    Array::of([rect.x0, rect.y0, rect.x1, rect.y1].map(|value| Object::Real(as_f32(value))))
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
fn paint_operator(paint: &Paint, rule: Fill) -> &'static str {
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
            surface: Surface::Page(index),
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
    // The categories the drawing used that the page had none of. Taken from
    // `added` rather than from a fixed list of the categories a canvas
    // happens to mint today: a drawing that reaches for a new one — `sh`
    // brought `/Shading` — must not silently lose its resources, which is a
    // resource named in the stream and absent from `/Resources`, and so a
    // draw that does nothing at all.
    for (category, _, _) in added {
        if out.contains_key(category) {
            continue;
        }
        let mut sub = Dict::new();
        for (_, name, held) in added.iter().filter(|(cat, _, _)| cat == category) {
            sub.push(name.clone(), held.clone());
        }
        if !sub.is_empty() {
            out.push((*category).clone(), Object::Dict(sub));
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
