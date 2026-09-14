//! Creating page annotations and attaching them to a page's `/Annots`.
//!
//! When a generator exists for the subtype (`Highlight`, `Underline`, `StrikeOut`,
//! `Squiggly`, `Ink`, `FreeText`, `Text`, `Square`, `Circle`, `Line`, `Link`, `Caret`, …), an `/AP /N` appearance stream is written so
//! [`crate::flatten`] and viewers that require appearances can draw them.

use kurbo::{Point, Rect};
use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_doc::{AnnotFlags, Subtype, vt::Alignment};
use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, encode_text,
};
use peniko::Color;

use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

/// An annotation write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// The Print bit of `/F` (ISO 32000-1 §12.5.3): annotations appear when the
/// page is printed.
const FLAG_PRINT: i64 = 4;

/// Default `/DA` for [`AnnotSpec::FreeText`]: black Helvetica 12 pt.
///
/// Matches the string Rotero writes today.
pub const DEFAULT_DA: &str = "0 0 0 rg /Helvetica 12 Tf";

/// One quadrilateral for a text-markup annotation (`/QuadPoints`).
///
/// Eight numbers are written in **top-left, top-right, bottom-left,
/// bottom-right** order — the order Rotero writes and the order
/// [`pdfrum_doc::annot`] reads as left/bottom = bl, right/top = tr.
///
/// The read path returns axis-aligned [`Rect`]s via
/// [`pdfrum_doc::Annotation::quad_points`]; `Quad` is the write-side type for
/// the same geometry. Convert with [`Quad::from_rect`] or [`From<Rect>`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// Top-left corner in page space.
    pub top_left: Point,
    /// Top-right corner in page space.
    pub top_right: Point,
    /// Bottom-left corner in page space.
    pub bottom_left: Point,
    /// Bottom-right corner in page space.
    pub bottom_right: Point,
}

impl Quad {
    /// An axis-aligned quadrilateral from a rectangle: corners in tl, tr, bl,
    /// br order.
    ///
    /// ```
    /// use pdfrum_edit::Quad;
    /// use kurbo::Rect;
    ///
    /// let q = Quad::from_rect(Rect::new(10.0, 20.0, 110.0, 40.0));
    /// assert_eq!(q.top_left, kurbo::Point::new(10.0, 40.0));
    /// assert_eq!(q.bottom_right, kurbo::Point::new(110.0, 20.0));
    /// ```
    #[must_use]
    pub fn from_rect(rect: Rect) -> Self {
        let rect = rect.abs();
        Self {
            top_left: Point::new(rect.x0, rect.y1),
            top_right: Point::new(rect.x1, rect.y1),
            bottom_left: Point::new(rect.x0, rect.y0),
            bottom_right: Point::new(rect.x1, rect.y0),
        }
    }
}

impl From<Rect> for Quad {
    /// Delegates to [`Quad::from_rect`].
    ///
    /// ```
    /// use pdfrum_edit::Quad;
    /// use kurbo::Rect;
    ///
    /// let q: Quad = Rect::new(10.0, 20.0, 110.0, 40.0).into();
    /// assert_eq!(q, Quad::from_rect(Rect::new(10.0, 20.0, 110.0, 40.0)));
    /// ```
    fn from(rect: Rect) -> Self {
        Self::from_rect(rect)
    }
}

/// Border style written as `/BS /S` (ISO 32000-1 table 166).
///
/// This is [`pdfrum_doc::ap::BorderStyle`], the same type the read side and
/// the form layer already use: one set of five values, named once. Reading a
/// `/BS` yields it, and writing one takes it.
///
/// The two sides differ in how they *arrive* at a value, not in the value.
/// Reading is deliberately lenient — the style comes from the first byte of
/// `/S` alone, so `/Dotted` reads as `Dash` — while writing goes through
/// [`BorderStyleName::as_bytes`], which emits exactly the five legal names.
///
/// ```
/// use pdfrum_edit::{AnnotBorderStyle, BorderStyleName, MarkupKind, MarkupSpec, SquareSpec, TextSpec};
///
/// assert_eq!(AnnotBorderStyle::Solid.as_bytes(), b"S");
/// assert_eq!(AnnotBorderStyle::Dash.as_bytes(), b"D");
/// ```
pub type AnnotBorderStyle = pdfrum_doc::ap::BorderStyle;

/// The `/BS /S` name a [`AnnotBorderStyle`] is written as.
///
/// An extension trait rather than an inherent `impl`, because the type it
/// extends belongs to `pdfrum_doc`. Only the write side needs the spelling;
/// the read side only ever matches on the value.
pub trait BorderStyleName {
    /// The PDF name bytes for `/BS /S`.
    fn as_bytes(&self) -> &'static [u8];
}

impl BorderStyleName for AnnotBorderStyle {
    fn as_bytes(&self) -> &'static [u8] {
        match self {
            Self::Solid => b"S",
            Self::Dash => b"D",
            Self::Beveled => b"B",
            Self::Inset => b"I",
            Self::Underline => b"U",
        }
    }
}

/// Line ending style written in `/LE` (ISO 32000-1 table 166 / §12.5.6.7).
///
/// ```
/// use pdfrum_edit::LineEndingStyle;
///
/// assert_eq!(LineEndingStyle::OpenArrow.as_bytes(), b"OpenArrow");
/// assert_eq!(LineEndingStyle::None.as_bytes(), b"None");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]
pub enum LineEndingStyle {
    /// No special ending (`/None`).
    #[default]
    None,
    /// Square.
    Square,
    /// Circle.
    Circle,
    /// Diamond.
    Diamond,
    /// Open arrow.
    OpenArrow,
    /// Closed arrow.
    ClosedArrow,
    /// Butt.
    Butt,
    /// Reversed open arrow.
    ROpenArrow,
    /// Reversed closed arrow.
    RClosedArrow,
    /// Slash.
    Slash,
}

impl LineEndingStyle {
    /// The PDF name bytes for one `/LE` entry.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::None => b"None",
            Self::Square => b"Square",
            Self::Circle => b"Circle",
            Self::Diamond => b"Diamond",
            Self::OpenArrow => b"OpenArrow",
            Self::ClosedArrow => b"ClosedArrow",
            Self::Butt => b"Butt",
            Self::ROpenArrow => b"ROpenArrow",
            Self::RClosedArrow => b"RClosedArrow",
            Self::Slash => b"Slash",
        }
    }
}

/// Width and style for an annotation `/BS` dictionary.
///
/// Defaults match what Square and Ink wrote previously: width `2`, solid.
///
/// ```
/// use pdfrum_edit::{AnnotBorder, AnnotBorderStyle};
///
/// let border = AnnotBorder::default();
/// assert_eq!(border.width, 2.0);
/// assert_eq!(border.style, AnnotBorderStyle::Solid);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AnnotBorder {
    /// Border width (`/W`) in points.
    pub width: f32,
    /// Border style (`/S`).
    pub style: AnnotBorderStyle,
    /// Dash pattern `/D` as `[on gap phase]`, for a [`AnnotBorderStyle::Dash`]
    /// border.
    ///
    /// `None` writes no `/D`, and a reader then falls back to its own default
    /// of `[3 0 0]` — so a dashed border without this is dashed, just not to a
    /// pattern the file states. The key is written only for a dashed style,
    /// since it means nothing to the others.
    pub dash: Option<[i64; 3]>,
}

impl Default for AnnotBorder {
    fn default() -> Self {
        Self {
            width: 2.0,
            style: AnnotBorderStyle::Solid,
            dash: None,
        }
    }
}

impl AnnotBorder {
    /// A solid border of the given width.
    ///
    /// ```
    /// use pdfrum_edit::{AnnotBorder, AnnotBorderStyle};
    ///
    /// let border = AnnotBorder::solid(1.5);
    /// assert_eq!(border.width, 1.5);
    /// assert_eq!(border.style, AnnotBorderStyle::Solid);
    /// ```
    #[must_use]
    pub fn solid(width: f32) -> Self {
        Self {
            width,
            style: AnnotBorderStyle::Solid,
            dash: None,
        }
    }

    /// Sets the style, keeping the current width.
    #[must_use]
    pub fn with_style(mut self, style: AnnotBorderStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets `/D`, the dash pattern, as `[on gap phase]`.
    ///
    /// Only a [`AnnotBorderStyle::Dash`] border writes it. The reader takes
    /// the three entries as on-length, gap-length and phase, and an absent
    /// `/D` as its own `[3 0 0]`.
    ///
    /// ```
    /// use pdfrum_edit::{AnnotBorder, AnnotBorderStyle};
    ///
    /// let border = AnnotBorder::solid(1.0)
    ///     .with_style(AnnotBorderStyle::Dash)
    ///     .with_dash([4, 2, 0]);
    /// assert_eq!(border.dash, Some([4, 2, 0]));
    /// ```
    #[must_use]
    pub fn with_dash(mut self, dash: [i64; 3]) -> Self {
        self.dash = Some(dash);
        self
    }
}

/// Destination view for a [`AnnotLinkAction::GoTo`] action.
///
/// Mirrors the modes [`pdfrum_doc::Dest`] / [`pdfrum_doc::ZoomMode`] reads.
/// Optional coordinates of `None` write PDF `null` (leave unchanged).
///
/// ```
/// use pdfrum_edit::AnnotGoToView;
///
/// assert!(matches!(AnnotGoToView::Fit, AnnotGoToView::Fit));
/// assert!(matches!(AnnotGoToView::FitH { top: None }, AnnotGoToView::FitH { top: None }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum AnnotGoToView {
    /// Fit the whole page (`/Fit`).
    Fit,
    /// Position at `(left, top)` with optional zoom (`/XYZ`).
    Xyz {
        /// Left edge in page space, or unchanged when `None`.
        left: Option<f32>,
        /// Top edge in page space, or unchanged when `None`.
        top: Option<f32>,
        /// Zoom factor, or unchanged when `None` / `Some(0.0)`.
        zoom: Option<f32>,
    },
    /// Fit the page width; `top` is the top edge (`/FitH`).
    FitH {
        /// Top edge in page space, or unchanged when `None`.
        top: Option<f32>,
    },
    /// Fit the page height; `left` is the left edge (`/FitV`).
    FitV {
        /// Left edge in page space, or unchanged when `None`.
        left: Option<f32>,
    },
    /// Fit the rectangle (`/FitR`).
    FitR {
        /// Left edge.
        left: f32,
        /// Bottom edge.
        bottom: f32,
        /// Right edge.
        right: f32,
        /// Top edge.
        top: f32,
    },
    /// Fit the bounding box of the page's contents (`/FitB`).
    FitB,
    /// Fit the bounding box width; `top` is the top edge (`/FitBH`).
    FitBH {
        /// Top edge in page space, or unchanged when `None`.
        top: Option<f32>,
    },
    /// Fit the bounding box height; `left` is the left edge (`/FitBV`).
    FitBV {
        /// Left edge in page space, or unchanged when `None`.
        left: Option<f32>,
    },
}

/// Link annotation highlight mode (`/H`, ISO 32000-1 table 173).
///
/// Written on [`AnnotSpec::Link`] for **viewer click feedback**. The static
/// `/AP` stream draws border chrome only — modes like [`Self::Invert`] and
/// [`Self::Push`] cannot be simulated in a static appearance and are left to
/// the viewer.
///
/// ```
/// use pdfrum_edit::AnnotLinkHighlight;
///
/// assert_eq!(AnnotLinkHighlight::Invert.as_bytes(), b"I");
/// assert_eq!(AnnotLinkHighlight::Outline.as_bytes(), b"O");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]
pub enum AnnotLinkHighlight {
    /// No highlighting (`/N`).
    None,
    /// Invert content (`/I`). Acrobat default.
    #[default]
    Invert,
    /// Invert the border (`/O`).
    Outline,
    /// Depress into the page (`/P`).
    Push,
}

impl AnnotLinkHighlight {
    /// The PDF name bytes for `/H`.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::None => b"N",
            Self::Invert => b"I",
            Self::Outline => b"O",
            Self::Push => b"P",
        }
    }
}

/// Remote destination for [`AnnotLinkAction::GoToR`].
///
/// Remote `/D` values use a **page number** (not a page object ref) or a
/// named-destination string in the remote file.
///
/// ```
/// use pdfrum_edit::{AnnotGoToView, AnnotRemoteDest};
///
/// let _ = AnnotRemoteDest::Page {
///     page: 0,
///     view: AnnotGoToView::Fit,
/// };
/// let _ = AnnotRemoteDest::Named(String::from("Chapter1"));
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AnnotRemoteDest {
    /// Explicit destination: page number + view.
    Page {
        /// Zero-based page number in the remote file.
        page: i64,
        /// How to display that page.
        view: AnnotGoToView,
    },
    /// Named destination string in the remote file.
    Named(String),
}

/// Write-side link action, aligned with [`pdfrum_doc::ActionKind`] values we
/// support on annotations.
///
/// Reuses the document Action model conceptually (`URI`, `GoTo`); this enum is
/// the typed payload [`AnnotSpec::Link`] writes into `/A`. A [`Self::Named`]
/// action also upserts `/Names /Dests` so reopen/navigation can resolve the
/// name (see [`set_named_destination`]). [`Self::NamedExisting`] writes the
/// same `/D` string but does **not** touch the name tree — use it when the
/// destination is already registered.
///
/// ```
/// use pdfrum_edit::AnnotLinkAction;
///
/// let a = AnnotLinkAction::Uri("https://example.test/".into());
/// assert!(matches!(a, AnnotLinkAction::Uri(_)));
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AnnotLinkAction {
    /// Resolve a URI (`/S /URI`).
    Uri(String),
    /// Go to a page in this document (`/S /GoTo` with an explicit destination
    /// array naming `page`).
    GoTo {
        /// Page object reference (the same refs [`crate::EditDoc::page_state`] returns).
        page: pdfrum_object::ObjRef,
        /// How to display that page.
        view: AnnotGoToView,
    },
    /// `GoTo` whose `/D` is a named destination string.
    ///
    /// Also registers (or updates) `name` under the catalog `/Names /Dests`
    /// tree so [`pdfrum_doc::nav::lookup_named_dest`] can resolve it after save.
    Named {
        /// Destination name written as `/D` and as the name-tree key.
        name: String,
        /// Page the name resolves to.
        page: pdfrum_object::ObjRef,
        /// How to display that page.
        view: AnnotGoToView,
    },
    /// `GoTo` whose `/D` is a named destination that must already exist.
    ///
    /// Unlike [`Self::Named`], this does not upsert `/Names /Dests` — the
    /// name is assumed to resolve already (or will be registered separately
    /// via [`set_named_destination`]).
    NamedExisting {
        /// Destination name written as `/D`.
        name: String,
    },
    /// Remote go-to (`/S /GoToR`): open `file` at [`AnnotRemoteDest`].
    ///
    /// `/F` is written as a filespec dictionary with `/F` and `/UF`.
    GoToR {
        /// Remote file path (filespec `/F` + `/UF`).
        file: String,
        /// Page number + view, or a remote named destination.
        dest: AnnotRemoteDest,
        /// Optional `/NewWindow`.
        new_window: Option<bool>,
    },
    /// Launch a file / application (`/S /Launch`).
    ///
    /// `/F` is written as a filespec dictionary with `/F` and `/UF`.
    Launch {
        /// File / application path (filespec `/F` + `/UF`).
        file: String,
    },
}

/// What kind of annotation to create and attach to a page.
///
/// This is the **write** payload for [`add_annotation`]. The ISO subtype
/// spelling itself is [`pdfrum_doc::Subtype`] (also re-exported from the
/// facade): reading an annotation yields `Subtype`, while building one takes
/// an `AnnotSpec` variant that carries the keys that subtype needs.
///
/// Each variant carries the keys Rotero's `write_annotations` needs today.
/// Appearance streams are generated when the subtype has a generator.
///
/// Prefer the associated constructors (`highlight`, `text`, …) over spelling
/// every field at the call site.
///
/// ```
/// use pdfrum::{Color, Document, MarkupKind, MarkupSpec, Rect, SaveOptions};
///
/// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
/// let mut edit = doc.edit();
/// let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
/// edit.add_annotation(
///     0,
///     MarkupSpec::new(MarkupKind::Highlight, rect, Color::from_rgb8(255, 230, 0)).contents("note"),
/// )?;
/// let mut bytes = Vec::new();
/// edit.write_to(&mut bytes, &SaveOptions::default())?;
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AnnotSpec {
    /// A highlight over one or more text runs (`/Subtype /Highlight`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
    #[non_exhaustive]
    Highlight {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Text runs covered; each becomes eight numbers in tl, tr, bl, br
        /// order.
        quads: Vec<Quad>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// A sticky-note text annotation (`/Subtype /Text`).
    ///
    /// Defaults to `/Name /Comment` and `/Open false` (see [`AnnotSpec::text`]).
    #[non_exhaustive]
    Text {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Sticky-note icon name (`/Name`). Defaults to `/Comment`.
        icon: Name,
        /// Whether the pop-up starts open (`/Open`). Defaults to `false`.
        open: bool,
    },
    /// A square / area annotation (`/Subtype /Square`).
    ///
    /// Writes `/BS` with [`AnnotBorder`] (default width 2, solid) and
    /// `/Type /Border`.
    #[non_exhaustive]
    Square {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Border style dictionary (`/BS`).
        border: AnnotBorder,
        /// Interior fill `/IC`. `None` leaves the shape unfilled, which is
        /// what an absent or empty `/IC` means to a reader.
        interior: Option<Color>,
    },
    /// An underline over one or more text runs (`/Subtype /Underline`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
    #[non_exhaustive]
    Underline {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Text runs covered; each becomes eight numbers in tl, tr, bl, br
        /// order.
        quads: Vec<Quad>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// A strike-out over one or more text runs (`/Subtype /StrikeOut`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
    #[non_exhaustive]
    StrikeOut {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Text runs covered; each becomes eight numbers in tl, tr, bl, br
        /// order.
        quads: Vec<Quad>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// A squiggly underline over one or more text runs (`/Subtype /Squiggly`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
    #[non_exhaustive]
    Squiggly {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Text runs covered; each becomes eight numbers in tl, tr, bl, br
        /// order.
        quads: Vec<Quad>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// Freehand ink strokes (`/Subtype /Ink`).
    ///
    /// Writes `/InkList` as an array of strokes (each a flat array of x,y
    /// pairs) and `/BS` from [`AnnotBorder`] (no `/Type /Border`, matching
    /// prior Ink writes).
    #[non_exhaustive]
    Ink {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Strokes in page space; each stroke is a sequence of points.
        strokes: Vec<Vec<Point>>,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Border style dictionary (`/BS`).
        border: AnnotBorder,
    },
    /// A free-text annotation (`/Subtype /FreeText`).
    ///
    /// `/Contents` and `/DA` are both required. See [`DEFAULT_DA`] for a
    /// common appearance string.
    #[non_exhaustive]
    FreeText {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// The visible text (`/Contents`).
        contents: String,
        /// Default appearance string (`/DA`), e.g. [`DEFAULT_DA`].
        da: String,
        /// Text alignment, written as `/Q`. `None` writes no key, which a
        /// reader takes as flush left.
        align: Option<Alignment>,
    },
    /// A circle / ellipse annotation (`/Subtype /Circle`).
    ///
    /// Writes `/BS` like [`AnnotSpec::Square`] (includes `/Type /Border`).
    /// Appearance is generated when the circle AP pipeline is available.
    #[non_exhaustive]
    Circle {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Border style dictionary (`/BS`).
        border: AnnotBorder,
        /// Interior fill `/IC`. `None` leaves the shape unfilled, which is
        /// what an absent or empty `/IC` means to a reader.
        interior: Option<Color>,
    },
    /// A straight line (`/Subtype /Line`) with endpoints `/L`.
    ///
    /// Appearance strokes between the endpoints using `/BS` width and `/C`,
    /// with optional `/LE` endings and `/IC` interior fill for closed endings.
    #[non_exhaustive]
    Line {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Line start in page space (`/L` x1,y1).
        start: Point,
        /// Line end in page space (`/L` x2,y2).
        end: Point,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Border style dictionary (`/BS`).
        border: AnnotBorder,
        /// Optional line endings (`/LE` start, end). `None` omits `/LE`
        /// (prior behaviour).
        line_endings: Option<(LineEndingStyle, LineEndingStyle)>,
        /// Optional interior colour (`/IC`) for filled line endings.
        interior: Option<Color>,
    },
    /// A link annotation (`/Subtype /Link`) with a typed `/A` action.
    ///
    /// Appearance honours `/BS` / `/C`. `/H` is written for viewer click
    /// feedback only — see [`AnnotLinkHighlight`].
    /// See [`AnnotLinkAction`] for URI, `GoTo`, and named-destination forms.
    #[non_exhaustive]
    Link {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Action dictionary payload (`/A`).
        action: AnnotLinkAction,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Optional annotation colour `/C`. `None` omits `/C` (AP uses muted blue).
        color: Option<Color>,
        /// Border style dictionary (`/BS`). Default width 1, solid.
        border: AnnotBorder,
        /// Highlight mode (`/H`). Default [`AnnotLinkHighlight::Invert`].
        highlight: AnnotLinkHighlight,
    },
    /// A caret / insertion-point annotation (`/Subtype /Caret`).
    ///
    /// Appearance draws a simple caret mark inside `/Rect`.
    #[non_exhaustive]
    Caret {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
}

impl AnnotSpec {
    /// Sets `/Contents` on variants that take optional contents.
    ///
    /// [`AnnotSpec::FreeText`] already requires contents at construction; this
    /// leaves it unchanged.
    ///
    /// ```
    /// use pdfrum_edit::{AnnotSpec, TextSpec};
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec: AnnotSpec = TextSpec::new(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .contents("sticky")
    ///     .into();
    /// assert!(matches!(
    ///     spec,
    ///     AnnotSpec::Text {
    ///         contents: Some(ref c),
    ///         ..
    ///     } if c == "sticky"
    /// ));
    /// ```
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per AnnotSpec variant; stays exhaustive as subtypes grow"
    )]
    pub fn with_contents(self, contents: impl Into<String>) -> Self {
        let contents = Some(contents.into());
        match self {
            Self::Highlight {
                rect, color, quads, ..
            } => Self::Highlight {
                rect,
                color,
                quads,
                contents,
            },
            Self::Text {
                rect,
                color,
                icon,
                open,
                ..
            } => Self::Text {
                rect,
                color,
                contents,
                icon,
                open,
            },
            Self::Square {
                rect,
                color,
                border,
                interior,
                ..
            } => Self::Square {
                rect,
                color,
                contents,
                border,
                interior,
            },
            Self::Underline {
                rect, color, quads, ..
            } => Self::Underline {
                rect,
                color,
                quads,
                contents,
            },
            Self::StrikeOut {
                rect, color, quads, ..
            } => Self::StrikeOut {
                rect,
                color,
                quads,
                contents,
            },
            Self::Squiggly {
                rect, color, quads, ..
            } => Self::Squiggly {
                rect,
                color,
                quads,
                contents,
            },
            Self::Ink {
                rect,
                color,
                strokes,
                border,
                ..
            } => Self::Ink {
                rect,
                color,
                strokes,
                contents,
                border,
            },
            Self::Circle {
                rect,
                color,
                border,
                interior,
                ..
            } => Self::Circle {
                rect,
                color,
                contents,
                border,
                interior,
            },
            Self::Line {
                rect,
                color,
                start,
                end,
                border,
                line_endings,
                interior,
                ..
            } => Self::Line {
                rect,
                color,
                start,
                end,
                contents,
                border,
                line_endings,
                interior,
            },
            Self::Link {
                rect,
                action,
                color,
                border,
                highlight,
                ..
            } => Self::Link {
                rect,
                action,
                contents,
                color,
                border,
                highlight,
            },
            Self::Caret { rect, color, .. } => Self::Caret {
                rect,
                color,
                contents,
            },
            other @ Self::FreeText { .. } => other,
        }
    }

    /// Attach author (`/T`), unique name (`/NM`), and/or modification date (`/M`).
    #[must_use]
    pub fn with_meta(self, meta: AnnotMeta) -> AnnotWrite {
        AnnotWrite { spec: self, meta }
    }

    /// Sets the annotation author (`/T`).
    #[must_use]
    pub fn with_author(self, author: impl Into<String>) -> AnnotWrite {
        self.with_meta(AnnotMeta::default().with_author(author))
    }

    /// Sets the annotation unique name (`/NM`), typically a stable id.
    #[must_use]
    pub fn with_name(self, name: impl Into<String>) -> AnnotWrite {
        self.with_meta(AnnotMeta::default().with_name(name))
    }

    /// Sets `/M` from a PDF date string (e.g. from [`crate::pdf_date`]).
    #[must_use]
    pub fn with_modified(self, modified: impl Into<String>) -> AnnotWrite {
        self.with_meta(AnnotMeta::default().with_modified(modified))
    }

    /// Sets `/F` annotation flags (default when omitted is Print).
    ///
    /// ```
    /// use pdfrum_doc::AnnotFlags;
    /// use pdfrum_edit::{AnnotSpec, TextSpec};
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let write = TextSpec::new(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .flags(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM);
    /// assert_eq!(write.meta.flags, Some(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM));
    /// ```
    #[must_use]
    pub fn with_flags(self, flags: AnnotFlags) -> AnnotWrite {
        self.with_meta(AnnotMeta::default().with_flags(flags))
    }

    /// Sets `/CA`, the constant opacity, from 0.0 to 1.0.
    ///
    /// Applies to every subtype: the generator folds it into the appearance's
    /// `/ExtGState`, so a translucent highlight is written the same way an
    /// opaque one is.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let write = AnnotSpec::highlight(Rect::new(0.0, 0.0, 10.0, 2.0), Color::from_rgb8(255, 255, 0))
    ///     .with_opacity(0.4);
    /// assert_eq!(write.meta.opacity, Some(0.4));
    /// ```
    #[must_use]
    pub fn with_opacity(self, opacity: f32) -> AnnotWrite {
        self.with_meta(AnnotMeta::default().with_opacity(opacity))
    }
}

/// Optional dictionary fields common to every annotation subtype.
///
/// Applied when writing via [`add_annotation`] / [`AnnotWrite`].
///
/// `/F` defaults to [`AnnotFlags::PRINT`] when [`Self::flags`] is `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotMeta {
    /// Author / title string written as `/T`.
    pub author: Option<String>,
    /// Unique name written as `/NM` (often an application id).
    pub name: Option<String>,
    /// Modification date written as `/M` (PDF date string).
    pub modified: Option<String>,
    /// Annotation flags written as `/F`. `None` means [`AnnotFlags::PRINT`].
    pub flags: Option<AnnotFlags>,
    /// Constant opacity written as `/CA`, from 0.0 (invisible) to 1.0.
    ///
    /// `None` writes no key, which a reader takes as fully opaque. The
    /// appearance generator reads `/CA` into the stream's `/ExtGState`, so a
    /// written one is honoured without the caller drawing anything.
    pub opacity: Option<f32>,
}

impl AnnotMeta {
    /// Sets `/T`.
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Sets `/NM`.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets `/M` to a PDF date string.
    #[must_use]
    pub fn with_modified(mut self, modified: impl Into<String>) -> Self {
        self.modified = Some(modified.into());
        self
    }

    /// Sets `/F` annotation flags.
    ///
    /// ```
    /// use pdfrum_doc::AnnotFlags;
    /// use pdfrum_edit::AnnotMeta;
    ///
    /// let meta = AnnotMeta::default().with_flags(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM);
    /// assert_eq!(meta.flags, Some(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM));
    /// ```
    #[must_use]
    pub fn with_flags(mut self, flags: AnnotFlags) -> Self {
        self.flags = Some(flags);
        self
    }

    /// Sets `/CA`, the constant opacity, clamped to 0.0..=1.0 when written.
    ///
    /// ```
    /// use pdfrum_edit::AnnotMeta;
    ///
    /// assert_eq!(AnnotMeta::default().with_opacity(0.4).opacity, Some(0.4));
    /// ```
    #[must_use]
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }
}

/// An [`AnnotSpec`] plus optional [`AnnotMeta`] for writing.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotWrite {
    /// Subtype-specific fields.
    pub spec: AnnotSpec,
    /// Author / name / date.
    pub meta: AnnotMeta,
}

impl From<AnnotSpec> for AnnotWrite {
    fn from(spec: AnnotSpec) -> Self {
        Self {
            spec,
            meta: AnnotMeta::default(),
        }
    }
}

impl AnnotWrite {
    /// Sets `/Contents` on the inner spec (same rules as [`AnnotSpec::with_contents`]).
    #[must_use]
    pub fn with_contents(mut self, contents: impl Into<String>) -> Self {
        self.spec = self.spec.with_contents(contents);
        self
    }

    /// Sets `/T`.
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.meta.author = Some(author.into());
        self
    }

    /// Sets `/NM`.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.meta.name = Some(name.into());
        self
    }

    /// Sets `/M`.
    #[must_use]
    pub fn with_modified(mut self, modified: impl Into<String>) -> Self {
        self.meta.modified = Some(modified.into());
        self
    }

    /// Replaces the full metadata block.
    #[must_use]
    pub fn with_meta(mut self, meta: AnnotMeta) -> Self {
        self.meta = meta;
        self
    }

    /// Sets `/F` annotation flags.
    #[must_use]
    pub fn with_flags(mut self, flags: AnnotFlags) -> Self {
        self.meta.flags = Some(flags);
        self
    }

    /// Sets `/CA`, the constant opacity, from 0.0 to 1.0.
    #[must_use]
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.meta.opacity = Some(opacity);
        self
    }

    /// Sets `/Contents`. The bare-verb spelling the typed builders use, so a
    /// chain that starts on a builder keeps reading the same way after the
    /// first metadata setter hands back an `AnnotWrite`.
    #[must_use]
    pub fn contents(self, contents: impl Into<String>) -> Self {
        self.with_contents(contents)
    }

    /// Sets `/T`, the author. See [`AnnotWrite::contents`] on the spelling.
    #[must_use]
    pub fn author(self, author: impl Into<String>) -> Self {
        self.with_author(author)
    }

    /// Sets `/NM`, the annotation's unique name.
    #[must_use]
    pub fn name(self, name: impl Into<String>) -> Self {
        self.with_name(name)
    }

    /// Sets `/M`, the modification date.
    #[must_use]
    pub fn modified(self, modified: impl Into<String>) -> Self {
        self.with_modified(modified)
    }

    /// Sets `/F`, the annotation flags.
    #[must_use]
    pub fn flags(self, flags: AnnotFlags) -> Self {
        self.with_flags(flags)
    }

    /// Sets every metadata field at once.
    #[must_use]
    pub fn meta(self, meta: AnnotMeta) -> Self {
        self.with_meta(meta)
    }
}

/// Adds an annotation described by `spec` to `page`, returning the new
/// annotation's object reference.
///
/// The annotation is written as a new indirect object (`/Type /Annot`,
/// `/Subtype`, `/Rect`, `/C`, `/F` — Print by default, or [`AnnotMeta::flags`]) and appended to the
/// page's `/Annots`: a missing array is created; an indirect array is
/// extended in place; an inline array is rewritten on the page. `/P` is set
/// to the page object.
///
/// # Errors
///
/// - [`Error::PageIndexOutOfRange`] when `page` is outside the document.
/// - [`Error::InlinePage`] when the page has no object of its own.
/// - [`Error::EmptyQuadPoints`] when a text-markup annot has no quads.
pub fn add_annotation(
    edit: &mut EditDoc<'_>,
    page: impl Into<PageIndex>,
    write: impl Into<AnnotWrite>,
) -> Result<ObjRef> {
    let AnnotWrite { spec, meta } = write.into();
    let page = page.into();
    let Some((page_ref, mut page_dict, _)) = edit.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    register_named_dest_from_spec(edit, &spec)?;
    let mut dict = build_dict(spec, page_ref)?;
    apply_meta(&mut dict, &meta);
    attach_appearance(edit, &mut dict);
    let annot_ref = edit.add(Object::Dict(dict));
    attach_to_page(edit, page_ref, &mut page_dict, annot_ref);
    Ok(annot_ref)
}

/// Replaces an existing annotation object in place, regenerating `/AP` when
/// the subtype has an appearance generator.
///
/// The object number is preserved. `annot` must already appear in `page`'s
/// `/Annots`.
///
/// ```
/// use pdfrum_edit::{TextSpec, add_annotation, update_annotation};
/// use pdfrum_edit::EditDoc;
/// use kurbo::Rect;
/// use peniko::Color;
/// # use pdfrum_parser::{load, LoadOptions};
/// # use std::sync::Arc;
/// #
/// # let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello_world.pdf")).unwrap();
/// # let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).unwrap();
/// # let mut edit = EditDoc::new(&base);
/// let rect = Rect::new(10.0, 10.0, 40.0, 40.0);
/// let r = add_annotation(&mut edit, 0, TextSpec::new(rect, Color::from_rgb8(255, 200, 0)))?;
/// update_annotation(
///     &mut edit,
///     0,
///     r,
///     TextSpec::new(rect, Color::from_rgb8(255, 200, 0)).contents("updated"),
/// )?;
/// # Ok::<(), pdfrum_edit::Error>(())
/// ```
///
/// # Errors
///
/// - [`Error::PageIndexOutOfRange`] / [`Error::InlinePage`] for a bad page
/// - [`Error::AnnotNotOnPage`] when `annot` is not listed on that page
/// - [`Error::EmptyQuadPoints`] for empty markup quads
pub fn update_annotation(
    edit: &mut EditDoc<'_>,
    page: impl Into<PageIndex>,
    annot: ObjRef,
    write: impl Into<AnnotWrite>,
) -> Result<()> {
    let AnnotWrite { spec, meta } = write.into();
    let page = page.into();
    let Some((page_ref, page_dict, _)) = edit.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    if !page_lists_annot(edit, &page_dict, annot) {
        return Err(Error::AnnotNotOnPage(annot, page));
    }
    register_named_dest_from_spec(edit, &spec)?;
    let mut dict = build_dict(spec, page_ref)?;
    apply_meta(&mut dict, &meta);
    attach_appearance(edit, &mut dict);
    edit.replace(annot, Object::Dict(dict));
    Ok(())
}

/// Removes `annot` from `page`'s `/Annots` and drops the annotation object.
///
/// Returns `Ok(true)` when it was listed and removed, `Ok(false)` when it was
/// not on that page.
///
/// ```
/// use pdfrum_edit::{AnnotSpec, SquareSpec, add_annotation, delete_annotation};
/// use pdfrum_edit::EditDoc;
/// use kurbo::Rect;
/// use peniko::Color;
/// # use pdfrum_parser::{load, LoadOptions};
/// # use std::sync::Arc;
/// #
/// # let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello_world.pdf")).unwrap();
/// # let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).unwrap();
/// # let mut edit = EditDoc::new(&base);
/// let r = add_annotation(
///     &mut edit,
///     0,
///     SquareSpec::new(Rect::new(0.0, 0.0, 10.0, 10.0), Color::from_rgb8(0, 0, 255)),
/// )?;
/// assert!(delete_annotation(&mut edit, 0, r)?);
/// assert!(!delete_annotation(&mut edit, 0, r)?);
/// # Ok::<(), pdfrum_edit::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::PageIndexOutOfRange`] or [`Error::InlinePage`] for a bad page.
pub fn delete_annotation(
    edit: &mut EditDoc<'_>,
    page: impl Into<PageIndex>,
    annot: ObjRef,
) -> Result<bool> {
    let page = page.into();
    let Some((page_ref, mut page_dict, _)) = edit.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    if !detach_from_page(edit, page_ref, &mut page_dict, annot) {
        return Ok(false);
    }
    edit.remove(annot);
    Ok(true)
}

/// Resolves `index` in `page`'s `/Annots` array (0-based) to an [`ObjRef`], then
/// calls [`update_annotation`].
///
/// An **inline** dictionary at that index is promoted to a new indirect object
/// (the array slot becomes a reference) before the update, so mixed
/// indirect/inline `/Annots` arrays work.
///
/// ```
/// use pdfrum_edit::{TextSpec, add_annotation, update_annotation_at};
/// use pdfrum_edit::EditDoc;
/// use kurbo::Rect;
/// use peniko::Color;
/// # use pdfrum_parser::{load, LoadOptions};
/// # use std::sync::Arc;
/// #
/// # let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello_world.pdf")).unwrap();
/// # let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).unwrap();
/// # let mut edit = EditDoc::new(&base);
/// let rect = Rect::new(10.0, 10.0, 40.0, 40.0);
/// add_annotation(&mut edit, 0, TextSpec::new(rect, Color::from_rgb8(255, 200, 0)))?;
/// update_annotation_at(
///     &mut edit,
///     0,
///     0,
///     TextSpec::new(rect, Color::from_rgb8(255, 200, 0)).contents("by index"),
/// )?;
/// # Ok::<(), pdfrum_edit::Error>(())
/// ```
///
/// # Errors
///
/// - [`Error::AnnotIndexOutOfRange`] when `index` is outside `/Annots` or the
///   entry is neither a reference nor a dictionary
/// - Otherwise the same errors as [`update_annotation`]
pub fn update_annotation_at(
    edit: &mut EditDoc<'_>,
    page: impl Into<PageIndex>,
    index: usize,
    write: impl Into<AnnotWrite>,
) -> Result<()> {
    let page = page.into();
    let annot = ensure_annot_ref_at(edit, page, index)?;
    update_annotation(edit, page, annot, write)
}

/// Removes the annotation at `index` in `page`'s `/Annots` (0-based).
///
/// Indirect entries are detached and the object is dropped (same as
/// [`delete_annotation`]). Inline dictionary entries are removed from the
/// array only.
///
/// ```
/// use pdfrum_edit::{AnnotSpec, SquareSpec, add_annotation, delete_annotation_at};
/// use pdfrum_edit::EditDoc;
/// use kurbo::Rect;
/// use peniko::Color;
/// # use pdfrum_parser::{load, LoadOptions};
/// # use std::sync::Arc;
/// #
/// # let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello_world.pdf")).unwrap();
/// # let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).unwrap();
/// # let mut edit = EditDoc::new(&base);
/// add_annotation(
///     &mut edit,
///     0,
///     SquareSpec::new(Rect::new(0.0, 0.0, 10.0, 10.0), Color::from_rgb8(0, 0, 255)),
/// )?;
/// assert!(delete_annotation_at(&mut edit, 0, 0)?);
/// # Ok::<(), pdfrum_edit::Error>(())
/// ```
///
/// # Errors
///
/// - [`Error::AnnotIndexOutOfRange`] when `index` is outside `/Annots`
/// - [`Error::PageIndexOutOfRange`] / [`Error::InlinePage`] for a bad page
pub fn delete_annotation_at(
    edit: &mut EditDoc<'_>,
    page: impl Into<PageIndex>,
    index: usize,
) -> Result<bool> {
    let page = page.into();
    let Some((page_ref, mut page_dict, _)) = edit.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    match annots_entry_at(edit, &page_dict, page, index)? {
        Object::Ref(annot) => delete_annotation(edit, page, annot),
        Object::Dict(_) => Ok(remove_annots_index(edit, page_ref, &mut page_dict, index)),
        _ => Err(Error::AnnotIndexOutOfRange(index, page)),
    }
}

/// Promotes an inline `/Annots` dict at `index` to an indirect object when
/// needed, returning the reference callers can update.
fn ensure_annot_ref_at(edit: &mut EditDoc<'_>, page: PageIndex, index: usize) -> Result<ObjRef> {
    let Some((page_ref, mut page_dict, _)) = edit.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    match annots_entry_at(edit, &page_dict, page, index)? {
        Object::Ref(annot) => Ok(annot),
        Object::Dict(dict) => {
            let annot = edit.add(Object::Dict(dict));
            replace_annots_index(
                edit,
                page_ref,
                &mut page_dict,
                page,
                index,
                Object::Ref(annot),
            )?;
            Ok(annot)
        }
        _ => Err(Error::AnnotIndexOutOfRange(index, page)),
    }
}

/// The raw `/Annots` element at `index`, without promoting.
fn annots_entry_at(
    edit: &EditDoc<'_>,
    page_dict: &Dict,
    page: PageIndex,
    index: usize,
) -> Result<Object> {
    let array = annots_array(edit, page_dict).ok_or(Error::AnnotIndexOutOfRange(index, page))?;
    array
        .raw_at(index)
        .cloned()
        .ok_or(Error::AnnotIndexOutOfRange(index, page))
}

fn annots_array(edit: &EditDoc<'_>, page_dict: &Dict) -> Option<Array> {
    match page_dict.raw(names::ANNOTS) {
        Some(Object::Ref(array_ref)) => edit
            .fetch(*array_ref)
            .ok()
            .as_deref()
            .and_then(Object::as_array)
            .cloned(),
        Some(Object::Array(array)) => Some(array.clone()),
        _ => None,
    }
}

/// Replaces the `/Annots` element at `index` with `value`.
fn replace_annots_index(
    edit: &mut EditDoc<'_>,
    page_ref: ObjRef,
    page_dict: &mut Dict,
    page: PageIndex,
    index: usize,
    value: Object,
) -> Result<()> {
    let annots_key = names::ANNOTS.clone();
    match page_dict.raw(&annots_key).cloned() {
        Some(Object::Ref(array_ref)) => {
            let mut array = edit
                .fetch(array_ref)
                .ok()
                .as_deref()
                .and_then(Object::as_array)
                .cloned()
                .ok_or(Error::AnnotIndexOutOfRange(index, page))?;
            if index >= array.len() {
                return Err(Error::AnnotIndexOutOfRange(index, page));
            }
            array.remove(index);
            array.insert(index, value);
            edit.replace(array_ref, Object::Array(array));
            Ok(())
        }
        Some(Object::Array(mut array)) => {
            if index >= array.len() {
                return Err(Error::AnnotIndexOutOfRange(index, page));
            }
            array.remove(index);
            array.insert(index, value);
            page_dict.insert(annots_key, Object::Array(array));
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
            Ok(())
        }
        _ => Err(Error::AnnotIndexOutOfRange(index, page)),
    }
}

/// Removes the `/Annots` element at `index`. Returns whether it was present.
fn remove_annots_index(
    edit: &mut EditDoc<'_>,
    page_ref: ObjRef,
    page_dict: &mut Dict,
    index: usize,
) -> bool {
    let annots_key = names::ANNOTS.clone();
    match page_dict.raw(&annots_key).cloned() {
        Some(Object::Ref(array_ref)) => {
            let Some(mut array) = edit
                .fetch(array_ref)
                .ok()
                .as_deref()
                .and_then(Object::as_array)
                .cloned()
            else {
                return false;
            };
            if array.remove(index).is_none() {
                return false;
            }
            edit.replace(array_ref, Object::Array(array));
            true
        }
        Some(Object::Array(mut array)) => {
            if array.remove(index).is_none() {
                return false;
            }
            page_dict.insert(annots_key, Object::Array(array));
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
            true
        }
        _ => false,
    }
}

fn apply_meta(dict: &mut Dict, meta: &AnnotMeta) {
    if let Some(author) = meta.author.as_deref().filter(|s| !s.is_empty()) {
        dict.insert(names::T.clone(), Object::Str(pdf_string(author)));
    }
    if let Some(name) = meta.name.as_deref().filter(|s| !s.is_empty()) {
        dict.insert(names::NM.clone(), Object::Str(pdf_string(name)));
    }
    if let Some(modified) = meta.modified.as_deref().filter(|s| !s.is_empty()) {
        dict.insert(names::M.clone(), Object::Str(pdf_string(modified)));
    }
    let flags = meta.flags.unwrap_or(AnnotFlags::PRINT);
    dict.insert(names::F.clone(), Object::Int(flags.bits()));
    // `/CA` goes in before the appearance is generated, so `ext_gstate_dict`
    // picks it up and the stream carries the alpha rather than the caller
    // having to draw it.
    if let Some(opacity) = meta.opacity {
        dict.insert(Name::from("CA"), Object::Real(opacity.clamp(0.0, 1.0)));
    }
}

/// Generate `/AP /N` for `dict` when a subtype generator exists.
///
/// Uses the same appearance generators flatten consults, so a newly written
/// annotation survives flatten and viewers that require appearance streams.
fn attach_appearance(edit: &mut EditDoc<'_>, dict: &mut Dict) {
    let catalog = edit.base().catalog().unwrap_or_default();
    let mut build = pdfrum_page::BuildContext::new();
    let fonts = pdfrum_doc::ap::FormFonts::load(&catalog, edit, &mut build);
    let mut diags = Diagnostics::default();
    // Synthetic one-annot page so we can reuse the page-walk generators.
    let page = Dict::from_pairs([(
        Name::from("Annots"),
        Object::Array(Array::of([Object::Dict(dict.clone())])),
    )]);
    let overlay = pdfrum_doc::ap::generate_appearances_with_text(
        &page,
        &catalog,
        Some(&fonts),
        edit,
        &mut diags,
    );
    let Some(generated) = overlay.get(0) else {
        return;
    };
    if generated.stream.is_empty() {
        return;
    }
    if let Some(rect) = generated.rect_override {
        dict.insert(names::RECT.clone(), rect_object(rect));
    }
    let stream = Stream::new(
        pdfrum_doc::ap::stream_dict(generated),
        ByteSpan::from(generated.stream.clone()),
    );
    let ap_ref = edit.add(Object::Stream(Box::new(stream)));
    let mut ap = Dict::new();
    ap.insert(Name::from("N"), Object::Ref(ap_ref));
    dict.insert(Name::from("AP"), Object::Dict(ap));
}

/// Upserts `name` into the catalog `/Names /Dests` name tree so named
/// destinations (and [`AnnotLinkAction::Named`] links) resolve after save.
///
/// `dest` is the explicit destination array for `page` + `view` (same shape
/// a `GoTo` `/D` array uses). An existing entry with the same name is replaced.
///
/// ```
/// use pdfrum_edit::{AnnotGoToView, set_named_destination};
/// use pdfrum_edit::EditDoc;
/// use pdfrum_object::ObjRef;
/// # use pdfrum_parser::{load, LoadOptions};
/// # use std::sync::Arc;
/// #
/// # let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello_world.pdf")).unwrap();
/// # let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).unwrap();
/// # let mut edit = EditDoc::new(&base);
/// # let page = ObjRef::new(3, 0);
/// set_named_destination(&mut edit, "Chapter1", page, AnnotGoToView::Fit)?;
/// # Ok::<(), pdfrum_edit::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub fn set_named_destination(
    edit: &mut EditDoc<'_>,
    name: impl Into<String>,
    page: ObjRef,
    view: AnnotGoToView,
) -> Result<()> {
    let name = name.into();
    let dest = Object::Array(goto_dest_array(page, view));
    crate::dests::upsert_named_dest(edit, &name, dest)
}

/// Like [`set_named_destination`], but leaves an existing name untouched.
///
/// Returns `true` when a new entry was written.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub fn ensure_named_destination(
    edit: &mut EditDoc<'_>,
    name: impl Into<String>,
    page: ObjRef,
    view: AnnotGoToView,
) -> Result<bool> {
    let name = name.into();
    let dest = Object::Array(goto_dest_array(page, view));
    crate::dests::ensure_named_dest(edit, &name, dest)
}

fn register_named_dest_from_spec(edit: &mut EditDoc<'_>, spec: &AnnotSpec) -> Result<()> {
    if let AnnotSpec::Link {
        action: AnnotLinkAction::Named { name, page, view },
        ..
    } = spec
    {
        set_named_destination(edit, name.clone(), *page, *view)?;
    }
    Ok(())
}

/// Builds the annotation dictionary for `spec`, with `/P` naming `page_ref`.
#[allow(
    clippy::too_many_lines,
    reason = "one arm per AnnotSpec variant; stays exhaustive as subtypes grow"
)]
fn build_dict(spec: AnnotSpec, page_ref: ObjRef) -> Result<Dict> {
    match spec {
        AnnotSpec::Highlight {
            rect,
            color,
            quads,
            contents,
        } => markup_dict(
            Subtype::Highlight,
            rect,
            color,
            &quads,
            contents.as_deref(),
            page_ref,
        ),
        AnnotSpec::Underline {
            rect,
            color,
            quads,
            contents,
        } => markup_dict(
            Subtype::Underline,
            rect,
            color,
            &quads,
            contents.as_deref(),
            page_ref,
        ),
        AnnotSpec::StrikeOut {
            rect,
            color,
            quads,
            contents,
        } => markup_dict(
            Subtype::StrikeOut,
            rect,
            color,
            &quads,
            contents.as_deref(),
            page_ref,
        ),
        AnnotSpec::Squiggly {
            rect,
            color,
            quads,
            contents,
        } => markup_dict(
            Subtype::Squiggly,
            rect,
            color,
            &quads,
            contents.as_deref(),
            page_ref,
        ),
        AnnotSpec::Text {
            rect,
            color,
            contents,
            icon,
            open,
        } => {
            let mut dict = common(Subtype::Text, rect, color, page_ref);
            dict.insert(names::NAME.clone(), Object::Name(icon));
            dict.insert(names::OPEN.clone(), Object::Bool(open));
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Square {
            rect,
            color,
            contents,
            border,
            interior,
        } => {
            let mut dict = common(Subtype::Square, rect, color, page_ref);
            dict.insert(
                names::BS.clone(),
                Object::Dict(border_style_dict(border, true)),
            );
            // An absent `/IC` and an empty one both mean "do not fill", which
            // is what `None` says; only a colour writes the key.
            if let Some(interior) = interior {
                dict.insert(names::IC.clone(), color_object(interior));
            }
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Ink {
            rect,
            color,
            strokes,
            contents,
            border,
        } => {
            let mut dict = common(Subtype::Ink, rect, color, page_ref);
            dict.insert(names::INK_LIST.clone(), Object::Array(ink_list(&strokes)));
            dict.insert(
                names::BS.clone(),
                Object::Dict(border_style_dict(border, false)),
            );
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::FreeText {
            rect,
            color,
            contents,
            da,
            align,
        } => {
            let mut dict = common(Subtype::FreeText, rect, color, page_ref);
            dict.insert(names::CONTENTS.clone(), Object::Str(pdf_string(&contents)));
            dict.insert(names::DA.clone(), Object::Str(pdf_string(&da)));
            // An absent `/Q` reads as flush left, so only a stated alignment
            // writes the key.
            if let Some(align) = align {
                dict.insert(names::Q.clone(), Object::Int(align.to_quadding()));
            }
            Ok(dict)
        }
        AnnotSpec::Circle {
            rect,
            color,
            contents,
            border,
            interior,
        } => {
            let mut dict = common(Subtype::Circle, rect, color, page_ref);
            dict.insert(
                names::BS.clone(),
                Object::Dict(border_style_dict(border, true)),
            );
            // An absent `/IC` and an empty one both mean "do not fill", which
            // is what `None` says; only a colour writes the key.
            if let Some(interior) = interior {
                dict.insert(names::IC.clone(), color_object(interior));
            }
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Line {
            rect,
            color,
            start,
            end,
            contents,
            border,
            line_endings,
            interior,
        } => {
            let mut dict = common(Subtype::Line, rect, color, page_ref);
            dict.insert(
                names::L.clone(),
                Object::Array(Array::of([
                    Object::Real(as_f32(start.x)),
                    Object::Real(as_f32(start.y)),
                    Object::Real(as_f32(end.x)),
                    Object::Real(as_f32(end.y)),
                ])),
            );
            dict.insert(
                names::BS.clone(),
                Object::Dict(border_style_dict(border, false)),
            );
            if let Some((start_style, end_style)) = line_endings {
                dict.insert(
                    names::LE.clone(),
                    Object::Array(Array::of([
                        Object::Name(Name::from(start_style.as_bytes())),
                        Object::Name(Name::from(end_style.as_bytes())),
                    ])),
                );
            }
            if let Some(interior) = interior {
                dict.insert(names::IC.clone(), color_object(interior));
            }
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Link {
            rect,
            action,
            contents,
            color,
            border,
            highlight,
        } => {
            let mut dict = Dict::new();
            dict.insert(names::TYPE.clone(), Object::Name(names::ANNOT.clone()));
            dict.insert(
                names::SUBTYPE.clone(),
                Object::Name(Name::from(Subtype::Link.as_bytes())),
            );
            dict.insert(names::RECT.clone(), rect_object(rect));
            dict.insert(names::F.clone(), Object::Int(FLAG_PRINT));
            dict.insert(names::P.clone(), Object::Ref(page_ref));
            dict.insert(names::A.clone(), Object::Dict(link_action_dict(&action)));
            if let Some(color) = color {
                dict.insert(names::C.clone(), color_object(color));
            }
            dict.insert(
                names::BS.clone(),
                Object::Dict(border_style_dict(border, false)),
            );
            dict.insert(
                Name::from("H"),
                Object::Name(Name::from(highlight.as_bytes())),
            );
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Caret {
            rect,
            color,
            contents,
        } => {
            let mut dict = common(Subtype::Caret, rect, color, page_ref);
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
    }
}

/// Shared `/QuadPoints` markup path for `Highlight` / `Underline` / `StrikeOut` / `Squiggly`.
fn markup_dict(
    subtype: Subtype,
    rect: Rect,
    color: Color,
    quads: &[Quad],
    contents: Option<&str>,
    page_ref: ObjRef,
) -> Result<Dict> {
    if quads.is_empty() {
        return Err(Error::EmptyQuadPoints);
    }
    let mut dict = common(subtype, rect, color, page_ref);
    dict.insert(
        names::QUAD_POINTS.clone(),
        Object::Array(quad_points(quads)),
    );
    insert_contents(&mut dict, contents);
    Ok(dict)
}

/// `/Type /Annot`, `/Subtype`, `/Rect`, `/C`, provisional `/F` Print, and `/P`.
///
/// [`apply_meta`] overwrites `/F` from [`AnnotMeta::flags`] (Print when `None`).
///
/// Subtype spellings come from [`Subtype::as_bytes`] so they stay aligned with
/// the ISO enum in `pdfrum-doc`.
fn common(subtype: Subtype, rect: Rect, color: Color, page_ref: ObjRef) -> Dict {
    let mut dict = Dict::new();
    dict.insert(names::TYPE.clone(), Object::Name(names::ANNOT.clone()));
    dict.insert(
        names::SUBTYPE.clone(),
        Object::Name(Name::from(subtype.as_bytes())),
    );
    dict.insert(names::RECT.clone(), rect_object(rect));
    dict.insert(names::C.clone(), color_object(color));
    dict.insert(names::F.clone(), Object::Int(FLAG_PRINT));
    dict.insert(names::P.clone(), Object::Ref(page_ref));
    dict
}

/// Whether `annot` appears (as a direct or indirect ref) in the page's `/Annots`.
fn page_lists_annot(edit: &EditDoc<'_>, page_dict: &Dict, annot: ObjRef) -> bool {
    match page_dict.raw(names::ANNOTS) {
        Some(Object::Ref(array_ref)) => edit
            .fetch(*array_ref)
            .ok()
            .as_deref()
            .and_then(Object::as_array)
            .is_some_and(|array| array_contains_ref(array, annot)),
        Some(Object::Array(array)) => array_contains_ref(array, annot),
        _ => false,
    }
}

fn array_contains_ref(array: &Array, annot: ObjRef) -> bool {
    array
        .iter()
        .any(|obj| matches!(obj, Object::Ref(r) if *r == annot))
}

/// Removes `annot_ref` from `/Annots`. Returns whether it was present.
fn detach_from_page(
    edit: &mut EditDoc<'_>,
    page_ref: ObjRef,
    page_dict: &mut Dict,
    annot_ref: ObjRef,
) -> bool {
    let annots_key = names::ANNOTS.clone();
    match page_dict.raw(&annots_key).cloned() {
        Some(Object::Ref(array_ref)) => {
            let Some(array) = edit
                .fetch(array_ref)
                .ok()
                .as_deref()
                .and_then(Object::as_array)
                .cloned()
            else {
                return false;
            };
            let filtered = filter_annot_ref(&array, annot_ref);
            if filtered.len() == array.len() {
                return false;
            }
            edit.replace(array_ref, Object::Array(filtered));
            true
        }
        Some(Object::Array(array)) => {
            let filtered = filter_annot_ref(&array, annot_ref);
            if filtered.len() == array.len() {
                return false;
            }
            page_dict.insert(annots_key, Object::Array(filtered));
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
            true
        }
        _ => false,
    }
}

fn filter_annot_ref(array: &Array, annot_ref: ObjRef) -> Array {
    Array::of(
        array
            .iter()
            .filter(|obj| !matches!(obj, Object::Ref(r) if *r == annot_ref))
            .cloned(),
    )
}

/// Appends `annot_ref` to the page's `/Annots`, creating or extending as
/// needed. When `/Annots` is already an indirect array, that array is
/// replaced and the page dictionary is left alone.
fn attach_to_page(
    edit: &mut EditDoc<'_>,
    page_ref: ObjRef,
    page_dict: &mut Dict,
    annot_ref: ObjRef,
) {
    let annots_key = names::ANNOTS.clone();
    match page_dict.raw(&annots_key).cloned() {
        Some(Object::Ref(array_ref)) => {
            let mut array = edit
                .fetch(array_ref)
                .ok()
                .as_deref()
                .and_then(Object::as_array)
                .cloned()
                .unwrap_or_default();
            array.push(Object::Ref(annot_ref));
            edit.replace(array_ref, Object::Array(array));
        }
        Some(Object::Array(mut array)) => {
            array.push(Object::Ref(annot_ref));
            page_dict.insert(annots_key, Object::Array(array));
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
        }
        _ => {
            page_dict.insert(
                annots_key,
                Object::Array(Array::of([Object::Ref(annot_ref)])),
            );
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
        }
    }
}

fn insert_contents(dict: &mut Dict, contents: Option<&str>) {
    if let Some(text) = contents.filter(|t| !t.is_empty()) {
        dict.insert(names::CONTENTS.clone(), Object::Str(pdf_string(text)));
    }
}

/// PDF text string: `PDFDocEncoding` when it fits, otherwise UTF-16BE with a
/// byte-order mark — the same encoding [`encode_text`] produces for Info
/// strings and attachments.
fn pdf_string(text: &str) -> PdfString {
    PdfString::literal(encode_text(text))
}

/// Simple filespec dictionary with `/Type /Filespec`, `/F`, and `/UF`.
fn filespec_object(path: &str) -> Object {
    let mut dict = Dict::new();
    dict.insert(names::TYPE.clone(), Object::Name(names::FILESPEC.clone()));
    let s = Object::Str(pdf_string(path));
    dict.insert(names::F.clone(), s.clone());
    dict.insert(names::UF.clone(), s);
    Object::Dict(dict)
}

fn rect_object(rect: Rect) -> Object {
    let rect = rect.abs();
    Object::Array(Array::of([
        Object::Real(as_f32(rect.x0)),
        Object::Real(as_f32(rect.y0)),
        Object::Real(as_f32(rect.x1)),
        Object::Real(as_f32(rect.y1)),
    ]))
}

fn color_object(color: Color) -> Object {
    let [r, g, b, _] = color.components;
    Object::Array(Array::of([
        Object::Real(r.clamp(0.0, 1.0)),
        Object::Real(g.clamp(0.0, 1.0)),
        Object::Real(b.clamp(0.0, 1.0)),
    ]))
}

/// Flat `/QuadPoints` array: eight numbers per quad in tl, tr, bl, br order.
fn quad_points(quads: &[Quad]) -> Array {
    let mut out = Array::new();
    for quad in quads {
        for point in [
            quad.top_left,
            quad.top_right,
            quad.bottom_left,
            quad.bottom_right,
        ] {
            out.push(Object::Real(as_f32(point.x)));
            out.push(Object::Real(as_f32(point.y)));
        }
    }
    out
}

/// `/InkList`: array of strokes, each a flat array of x,y pairs.
fn ink_list(strokes: &[Vec<Point>]) -> Array {
    let mut out = Array::new();
    for stroke in strokes {
        let mut points = Array::new();
        for point in stroke {
            points.push(Object::Real(as_f32(point.x)));
            points.push(Object::Real(as_f32(point.y)));
        }
        out.push(Object::Array(points));
    }
    out
}

/// `/A` dictionary for a link action.
fn link_action_dict(action: &AnnotLinkAction) -> Dict {
    let mut a = Dict::new();
    a.insert(names::TYPE.clone(), Object::Name(Name::from("Action")));
    match action {
        AnnotLinkAction::Uri(uri) => {
            a.insert(names::S.clone(), Object::Name(names::URI.clone()));
            a.insert(names::URI.clone(), Object::Str(pdf_string(uri)));
        }
        AnnotLinkAction::GoTo { page, view } => {
            a.insert(names::S.clone(), Object::Name(names::GO_TO.clone()));
            a.insert(
                names::D.clone(),
                Object::Array(goto_dest_array(*page, *view)),
            );
        }
        AnnotLinkAction::Named { name, .. } | AnnotLinkAction::NamedExisting { name } => {
            a.insert(names::S.clone(), Object::Name(names::GO_TO.clone()));
            a.insert(names::D.clone(), Object::Str(pdf_string(name)));
        }
        AnnotLinkAction::GoToR {
            file,
            dest,
            new_window,
        } => {
            a.insert(names::S.clone(), Object::Name(names::GO_TO_R.clone()));
            a.insert(names::F.clone(), filespec_object(file));
            match dest {
                AnnotRemoteDest::Page { page, view } => {
                    a.insert(
                        names::D.clone(),
                        Object::Array(remote_goto_dest_array(*page, *view)),
                    );
                }
                AnnotRemoteDest::Named(name) => {
                    a.insert(names::D.clone(), Object::Str(pdf_string(name)));
                }
            }
            if let Some(new_window) = *new_window {
                a.insert(names::NEW_WINDOW.clone(), Object::Bool(new_window));
            }
        }
        AnnotLinkAction::Launch { file } => {
            a.insert(names::S.clone(), Object::Name(names::LAUNCH.clone()));
            a.insert(names::F.clone(), filespec_object(file));
        }
    }
    a
}

/// Explicit destination array `[page /Fit|…]`.
fn goto_dest_array(page: pdfrum_object::ObjRef, view: AnnotGoToView) -> Array {
    dest_array_with_page(Object::Ref(page), view)
}

/// Remote destination array: page **number** + view (for `/GoToR`).
fn remote_goto_dest_array(page: i64, view: AnnotGoToView) -> Array {
    dest_array_with_page(Object::Int(page), view)
}

fn dest_array_with_page(page: Object, view: AnnotGoToView) -> Array {
    match view {
        AnnotGoToView::Fit => Array::of([page, Object::Name(names::FIT.clone())]),
        AnnotGoToView::Xyz { left, top, zoom } => Array::of([
            page,
            Object::Name(names::XYZ.clone()),
            optional_dest_number(left, false),
            optional_dest_number(top, false),
            optional_dest_number(zoom, true),
        ]),
        AnnotGoToView::FitH { top } => Array::of([
            page,
            Object::Name(names::FIT_H.clone()),
            optional_dest_number(top, false),
        ]),
        AnnotGoToView::FitV { left } => Array::of([
            page,
            Object::Name(names::FIT_V.clone()),
            optional_dest_number(left, false),
        ]),
        AnnotGoToView::FitR {
            left,
            bottom,
            right,
            top,
        } => Array::of([
            page,
            Object::Name(names::FIT_R.clone()),
            Object::Real(left),
            Object::Real(bottom),
            Object::Real(right),
            Object::Real(top),
        ]),
        AnnotGoToView::FitB => Array::of([page, Object::Name(names::FIT_B.clone())]),
        AnnotGoToView::FitBH { top } => Array::of([
            page,
            Object::Name(names::FIT_BH.clone()),
            optional_dest_number(top, false),
        ]),
        AnnotGoToView::FitBV { left } => Array::of([
            page,
            Object::Name(names::FIT_BV.clone()),
            optional_dest_number(left, false),
        ]),
    }
}

/// PDF destination number: `null` when absent (or zoom that means unchanged).
fn optional_dest_number(value: Option<f32>, zero_means_null: bool) -> Object {
    match value {
        None => Object::Null,
        Some(n) if zero_means_null && n == 0.0 => Object::Null,
        Some(n) => Object::Real(n),
    }
}

/// Border style dict from [`AnnotBorder`].
///
/// Square gets `/Type /Border`; Ink does not (matching Rotero).
fn border_style_dict(border: AnnotBorder, with_type: bool) -> Dict {
    let mut bs = Dict::new();
    if with_type {
        bs.insert(names::TYPE.clone(), Object::Name(names::BORDER.clone()));
    }
    bs.insert(names::W.clone(), Object::Real(border.width));
    bs.insert(
        names::S.clone(),
        Object::Name(Name::from(border.style.as_bytes())),
    );
    // `/D` means nothing to a style that is not dashed, so it is written only
    // where a reader would consult it.
    if let (AnnotBorderStyle::Dash, Some([on, gap, phase])) = (border.style, border.dash) {
        bs.insert(
            names::D.clone(),
            Object::Array(Array::of([
                Object::Int(on),
                Object::Int(gap),
                Object::Int(phase),
            ])),
        );
    }
    bs
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "PDF reals are f32; page-space points fit"
)]
fn as_f32(value: f64) -> f32 {
    value as f32
}
