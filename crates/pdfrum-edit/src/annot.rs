//! Creating page annotations and attaching them to a page's `/Annots`.
//!
//! When a generator exists for the subtype (`Highlight`, `Underline`, `StrikeOut`,
//! `Squiggly`, `Ink`, `FreeText`, `Text`, `Square`, …), an `/AP /N` appearance stream is written so
//! [`crate::flatten`] and viewers that require appearances can draw them.

use kurbo::{Point, Rect};
use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_doc::{AnnotFlags, Subtype};
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

/// Border style name written as `/BS /S` (ISO 32000-1 table 166).
///
/// ```
/// use pdfrum_edit::AnnotBorderStyle;
///
/// assert_eq!(AnnotBorderStyle::Solid.as_bytes(), b"S");
/// assert_eq!(AnnotBorderStyle::Dashed.as_bytes(), b"D");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]
pub enum AnnotBorderStyle {
    /// Solid (`/S`).
    #[default]
    Solid,
    /// Dashed (`/D`).
    Dashed,
    /// Beveled (`/B`).
    Beveled,
    /// Inset (`/I`).
    Inset,
    /// Underline (`/U`).
    Underline,
}

impl AnnotBorderStyle {
    /// The PDF name bytes for `/BS /S`.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Solid => b"S",
            Self::Dashed => b"D",
            Self::Beveled => b"B",
            Self::Inset => b"I",
            Self::Underline => b"U",
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
pub struct AnnotBorder {
    /// Border width (`/W`) in points.
    pub width: f32,
    /// Border style (`/S`).
    pub style: AnnotBorderStyle,
}

impl Default for AnnotBorder {
    fn default() -> Self {
        Self {
            width: 2.0,
            style: AnnotBorderStyle::Solid,
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
        }
    }

    /// Sets the style, keeping the current width.
    #[must_use]
    pub fn with_style(mut self, style: AnnotBorderStyle) -> Self {
        self.style = style;
        self
    }
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
/// use pdfrum::{AnnotSpec, Color, Document, Rect, SaveOptions};
///
/// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
/// let mut edit = doc.edit();
/// let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
/// edit.add_annotation(
///     0,
///     AnnotSpec::highlight(rect, Color::from_rgb8(255, 230, 0)).with_contents("note"),
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
    Square {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
        /// Border style dictionary (`/BS`).
        border: AnnotBorder,
    },
    /// An underline over one or more text runs (`/Subtype /Underline`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
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
    FreeText {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// The visible text (`/Contents`).
        contents: String,
        /// Default appearance string (`/DA`), e.g. [`DEFAULT_DA`].
        da: String,
    },
}

impl AnnotSpec {
    /// A highlight covering `rect` as a single quadrilateral, with no
    /// contents.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::highlight(Rect::new(0.0, 0.0, 10.0, 2.0), Color::from_rgb8(255, 255, 0));
    /// assert!(matches!(spec, AnnotSpec::Highlight { contents: None, .. }));
    /// ```
    #[must_use]
    pub fn highlight(rect: Rect, color: Color) -> Self {
        Self::Highlight {
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// An underline covering `rect` as a single quadrilateral, with no
    /// contents.
    #[must_use]
    pub fn underline(rect: Rect, color: Color) -> Self {
        Self::Underline {
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// A strike-out covering `rect` as a single quadrilateral, with no
    /// contents.
    #[must_use]
    pub fn strike_out(rect: Rect, color: Color) -> Self {
        Self::StrikeOut {
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// A squiggly underline covering `rect` as a single quadrilateral, with no
    /// contents.
    #[must_use]
    pub fn squiggly(rect: Rect, color: Color) -> Self {
        Self::Squiggly {
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// A sticky-note text annotation with no contents.
    ///
    /// Uses `/Name /Comment` and `/Open false`. Override with
    /// [`AnnotSpec::with_icon`] / [`AnnotSpec::with_open`].
    #[must_use]
    pub fn text(rect: Rect, color: Color) -> Self {
        Self::Text {
            rect,
            color,
            contents: None,
            icon: names::COMMENT.clone(),
            open: false,
        }
    }

    /// A square / area annotation with no contents.
    ///
    /// Uses a solid `/BS` of width 2. Override with [`AnnotSpec::with_border`].
    #[must_use]
    pub fn square(rect: Rect, color: Color) -> Self {
        Self::Square {
            rect,
            color,
            contents: None,
            border: AnnotBorder::default(),
        }
    }

    /// Freehand ink with the given strokes and no contents.
    ///
    /// Uses a solid `/BS` of width 2. Override with [`AnnotSpec::with_border`].
    #[must_use]
    pub fn ink(rect: Rect, color: Color, strokes: Vec<Vec<Point>>) -> Self {
        Self::Ink {
            rect,
            color,
            strokes,
            contents: None,
            border: AnnotBorder::default(),
        }
    }

    /// A free-text annotation with required contents and `/DA`.
    ///
    /// Pass [`DEFAULT_DA`] when the caller has no custom appearance string.
    #[must_use]
    pub fn free_text(
        rect: Rect,
        color: Color,
        contents: impl Into<String>,
        da: impl Into<String>,
    ) -> Self {
        Self::FreeText {
            rect,
            color,
            contents: contents.into(),
            da: da.into(),
        }
    }

    /// Sets `/Contents` on variants that take optional contents.
    ///
    /// [`AnnotSpec::FreeText`] already requires contents at construction; this
    /// leaves it unchanged.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::text(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .with_contents("sticky");
    /// assert!(matches!(
    ///     spec,
    ///     AnnotSpec::Text {
    ///         contents: Some(ref c),
    ///         ..
    ///     } if c == "sticky"
    /// ));
    /// ```
    #[must_use]
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
                ..
            } => Self::Square {
                rect,
                color,
                contents,
                border,
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
            other @ Self::FreeText { .. } => other,
        }
    }

    /// Sets `/BS` on [`AnnotSpec::Square`] or [`AnnotSpec::Ink`].
    ///
    /// Other variants are unchanged.
    ///
    /// ```
    /// use pdfrum_edit::{AnnotBorder, AnnotBorderStyle, AnnotSpec};
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::square(Rect::new(0.0, 0.0, 10.0, 10.0), Color::from_rgb8(0, 0, 255))
    ///     .with_border(AnnotBorder::solid(1.0).with_style(AnnotBorderStyle::Dashed));
    /// assert!(matches!(
    ///     spec,
    ///     AnnotSpec::Square {
    ///         border: AnnotBorder {
    ///             width,
    ///             style: AnnotBorderStyle::Dashed,
    ///         },
    ///         ..
    ///     } if (width - 1.0).abs() < f32::EPSILON
    /// ));
    /// ```
    #[must_use]
    pub fn with_border(self, border: AnnotBorder) -> Self {
        match self {
            Self::Square {
                rect,
                color,
                contents,
                ..
            } => Self::Square {
                rect,
                color,
                contents,
                border,
            },
            Self::Ink {
                rect,
                color,
                strokes,
                contents,
                ..
            } => Self::Ink {
                rect,
                color,
                strokes,
                contents,
                border,
            },
            other => other,
        }
    }

    /// Sets the sticky-note icon (`/Name`) on [`AnnotSpec::Text`].
    ///
    /// Other variants are unchanged. Common values: `Comment`, `Key`, `Note`,
    /// `Help`, `NewParagraph`, `Paragraph`, `Insert`.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use pdfrum_object::Name;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::text(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .with_icon(Name::from("Key"));
    /// assert!(matches!(
    ///     spec,
    ///     AnnotSpec::Text { ref icon, .. } if icon.as_bytes() == b"Key"
    /// ));
    /// ```
    #[must_use]
    pub fn with_icon(self, icon: impl Into<Name>) -> Self {
        let icon = icon.into();
        match self {
            Self::Text {
                rect,
                color,
                contents,
                open,
                ..
            } => Self::Text {
                rect,
                color,
                contents,
                icon,
                open,
            },
            other => other,
        }
    }

    /// Sets whether a sticky-note pop-up starts open (`/Open`) on
    /// [`AnnotSpec::Text`]. Other variants are unchanged.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::text(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .with_open(true);
    /// assert!(matches!(spec, AnnotSpec::Text { open: true, .. }));
    /// ```
    #[must_use]
    pub fn with_open(self, open: bool) -> Self {
        match self {
            Self::Text {
                rect,
                color,
                contents,
                icon,
                ..
            } => Self::Text {
                rect,
                color,
                contents,
                icon,
                open,
            },
            other => other,
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
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let write = AnnotSpec::text(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .with_flags(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM);
    /// assert_eq!(write.meta.flags, Some(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM));
    /// ```
    #[must_use]
    pub fn with_flags(self, flags: AnnotFlags) -> AnnotWrite {
        self.with_meta(AnnotMeta::default().with_flags(flags))
    }
}

/// Optional dictionary fields common to every annotation subtype.
///
/// Applied when writing via [`add_annotation`] / [`AnnotWrite`].
///
/// `/F` defaults to [`AnnotFlags::PRINT`] when [`Self::flags`] is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnotMeta {
    /// Author / title string written as `/T`.
    pub author: Option<String>,
    /// Unique name written as `/NM` (often an application id).
    pub name: Option<String>,
    /// Modification date written as `/M` (PDF date string).
    pub modified: Option<String>,
    /// Annotation flags written as `/F`. `None` means [`AnnotFlags::PRINT`].
    pub flags: Option<AnnotFlags>,
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
    let mut dict = build_dict(spec, page_ref)?;
    apply_meta(&mut dict, &meta);
    attach_appearance(edit, &mut dict);
    let annot_ref = edit.add(Object::Dict(dict));
    attach_to_page(edit, page_ref, &mut page_dict, annot_ref);
    Ok(annot_ref)
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

/// Builds the annotation dictionary for `spec`, with `/P` naming `page_ref`.
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
        } => {
            let mut dict = common(Subtype::Square, rect, color, page_ref);
            dict.insert(names::BS.clone(), Object::Dict(border_style_dict(border, true)));
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
            dict.insert(names::BS.clone(), Object::Dict(border_style_dict(border, false)));
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::FreeText {
            rect,
            color,
            contents,
            da,
        } => {
            let mut dict = common(Subtype::FreeText, rect, color, page_ref);
            dict.insert(names::CONTENTS.clone(), Object::Str(pdf_string(&contents)));
            dict.insert(names::DA.clone(), Object::Str(pdf_string(&da)));
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
    bs
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "PDF reals are f32; page-space points fit"
)]
fn as_f32(value: f64) -> f32 {
    value as f32
}
