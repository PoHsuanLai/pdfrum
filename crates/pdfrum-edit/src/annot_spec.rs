//! Per-subtype builders for [`AnnotSpec`].
//!
//! [`AnnotSpec`] is one enum because the write path matches on it, but its
//! options are not shared: `/IC` belongs to Line, `/H` to Link, `/Open` to
//! Text. Setting them through the enum means every setter has to accept every
//! variant and do nothing on the ones it does not apply to, which turns a
//! misuse into silence rather than an error.
//!
//! The builders here carry one subtype each, so only the options that subtype
//! has exist as methods. Asking a [`TextSpec`] for an interior colour does not
//! compile, where [`AnnotSpec::with_interior`] on a `Text` would have been
//! accepted and dropped.
//!
//! An option that does not belong to a subtype is not a method on that
//! subtype's builder, so the mistake is a compile error rather than a
//! silently dropped call:
//!
//! ```compile_fail
//! use kurbo::Rect;
//! use peniko::Color;
//! use pdfrum_edit::TextSpec;
//!
//! // `/IC` belongs to Line, not Text.
//! let _ = TextSpec::new(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(0, 0, 0))
//!     .interior(Color::from_rgb8(0, 0, 255));
//! ```
//!
//! ```compile_fail
//! use kurbo::Rect;
//! use peniko::Color;
//! use pdfrum_edit::{AnnotLinkHighlight, CaretSpec};
//!
//! // `/H` belongs to Link, not Caret.
//! let _ = CaretSpec::new(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(0, 0, 0))
//!     .highlight(AnnotLinkHighlight::Outline);
//! ```
//!
//! Each builder converts with [`Into<AnnotSpec>`], so they are accepted
//! wherever a spec is:
//!
//! ```
//! use kurbo::{Point, Rect};
//! use peniko::Color;
//! use pdfrum_edit::{LineEndingStyle, LineSpec};
//!
//! let spec = LineSpec::new(
//!     Rect::new(0.0, 0.0, 100.0, 20.0),
//!     Color::from_rgb8(255, 0, 0),
//!     Point::new(0.0, 0.0),
//!     Point::new(100.0, 20.0),
//! )
//! .endings(LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow)
//! .interior(Color::from_rgb8(0, 0, 255));
//! ```

use kurbo::{Point, Rect};
use pdfrum_object::{Name, ObjRef};
use peniko::Color;

use crate::annot::{
    AnnotBorder, AnnotGoToView, AnnotLinkAction, AnnotLinkHighlight, AnnotSpec, LineEndingStyle,
    Quad,
};

/// Declares the shared `contents` setter and the `From` conversion.
macro_rules! spec_builder {
    ($builder:ident, $doc:literal) => {
        impl $builder {
            #[doc = $doc]
            ///
            /// Sets `/Contents`.
            #[must_use]
            pub fn contents(mut self, contents: impl Into<String>) -> Self {
                self.contents = Some(contents.into());
                self
            }
        }
    };
}

/// A text-markup annotation: Highlight, Underline, `StrikeOut`, or Squiggly.
///
/// The four share a shape — a colour and a set of `/QuadPoints` — so one
/// builder covers them and [`MarkupKind`] picks the subtype.
///
/// ```
/// use kurbo::Rect;
/// use peniko::Color;
/// use pdfrum_edit::{AnnotSpec, MarkupKind, MarkupSpec};
///
/// let rect = Rect::new(0.0, 0.0, 80.0, 12.0);
/// let spec: AnnotSpec = MarkupSpec::new(MarkupKind::Highlight, rect, Color::from_rgb8(255, 255, 0))
///     .quads([rect.into()])
///     .contents("note")
///     .into();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct MarkupSpec {
    kind: MarkupKind,
    rect: Rect,
    color: Color,
    quads: Vec<Quad>,
    contents: Option<String>,
}

/// Which text-markup subtype a [`MarkupSpec`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MarkupKind {
    /// `/Subtype /Highlight`.
    Highlight,
    /// `/Subtype /Underline`.
    Underline,
    /// `/Subtype /StrikeOut`.
    StrikeOut,
    /// `/Subtype /Squiggly`.
    Squiggly,
}

impl MarkupSpec {
    /// A markup annotation covering `rect` as a single quadrilateral.
    ///
    /// Replace the geometry with [`MarkupSpec::quads`] to mark up a
    /// selection of several runs.
    #[must_use]
    pub fn new(kind: MarkupKind, rect: Rect, color: Color) -> Self {
        Self {
            kind,
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// Replaces the `/QuadPoints`, one quadrilateral per marked run.
    ///
    /// Writing a spec whose quads are empty fails with
    /// [`crate::Error::EmptyQuadPoints`]. `pdfrum_text::rects_loose` gives
    /// the em-box rectangles Acrobat marks up.
    #[must_use]
    pub fn quads(mut self, quads: impl IntoIterator<Item = Quad>) -> Self {
        self.quads = quads.into_iter().collect();
        self
    }
}

spec_builder!(MarkupSpec, "A text-markup annotation.");

impl From<MarkupSpec> for AnnotSpec {
    fn from(b: MarkupSpec) -> Self {
        let MarkupSpec {
            kind,
            rect,
            color,
            quads,
            contents,
        } = b;
        let spec = match kind {
            MarkupKind::Highlight => Self::highlight(rect, color),
            MarkupKind::Underline => Self::underline(rect, color),
            MarkupKind::StrikeOut => Self::strike_out(rect, color),
            MarkupKind::Squiggly => Self::squiggly(rect, color),
        }
        .with_quads(quads);
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A sticky-note text annotation (`/Subtype /Text`).
///
/// ```
/// use kurbo::Rect;
/// use peniko::Color;
/// use pdfrum_edit::{AnnotSpec, TextSpec};
///
/// let spec: AnnotSpec = TextSpec::new(Rect::new(0.0, 0.0, 20.0, 20.0), Color::from_rgb8(255, 255, 0))
///     .icon("Note")
///     .open(true)
///     .into();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    rect: Rect,
    color: Color,
    contents: Option<String>,
    icon: Option<Name>,
    open: bool,
}

impl TextSpec {
    /// A sticky note at `rect`, `/Comment` icon, pop-up closed.
    #[must_use]
    pub fn new(rect: Rect, color: Color) -> Self {
        Self {
            rect,
            color,
            contents: None,
            icon: None,
            open: false,
        }
    }

    /// Sets the icon name (`/Name`), such as `Note` or `Help`.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<Name>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Sets whether the pop-up starts open (`/Open`).
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
}

spec_builder!(TextSpec, "A sticky-note annotation.");

impl From<TextSpec> for AnnotSpec {
    fn from(b: TextSpec) -> Self {
        let TextSpec {
            rect,
            color,
            contents,
            icon,
            open,
        } = b;
        let mut spec = Self::text(rect, color).with_open(open);
        if let Some(icon) = icon {
            spec = spec.with_icon(icon);
        }
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A square / area annotation (`/Subtype /Square`).
#[derive(Debug, Clone, PartialEq)]
pub struct SquareSpec {
    rect: Rect,
    color: Color,
    contents: Option<String>,
    border: Option<AnnotBorder>,
}

impl SquareSpec {
    /// A square over `rect` with the default border (width 2, solid).
    #[must_use]
    pub fn new(rect: Rect, color: Color) -> Self {
        Self {
            rect,
            color,
            contents: None,
            border: None,
        }
    }

    /// Sets `/BS` width and style.
    #[must_use]
    pub fn border(mut self, border: AnnotBorder) -> Self {
        self.border = Some(border);
        self
    }
}

spec_builder!(SquareSpec, "A square annotation.");

impl From<SquareSpec> for AnnotSpec {
    fn from(b: SquareSpec) -> Self {
        let SquareSpec {
            rect,
            color,
            contents,
            border,
        } = b;
        let mut spec = Self::square(rect, color);
        if let Some(border) = border {
            spec = spec.with_border(border);
        }
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A circle / ellipse annotation (`/Subtype /Circle`).
#[derive(Debug, Clone, PartialEq)]
pub struct CircleSpec {
    rect: Rect,
    color: Color,
    contents: Option<String>,
    border: Option<AnnotBorder>,
}

impl CircleSpec {
    /// An ellipse inscribed in `rect` with the default border.
    #[must_use]
    pub fn new(rect: Rect, color: Color) -> Self {
        Self {
            rect,
            color,
            contents: None,
            border: None,
        }
    }

    /// Sets `/BS` width and style.
    #[must_use]
    pub fn border(mut self, border: AnnotBorder) -> Self {
        self.border = Some(border);
        self
    }
}

spec_builder!(CircleSpec, "A circle annotation.");

impl From<CircleSpec> for AnnotSpec {
    fn from(b: CircleSpec) -> Self {
        let CircleSpec {
            rect,
            color,
            contents,
            border,
        } = b;
        let mut spec = Self::circle(rect, color);
        if let Some(border) = border {
            spec = spec.with_border(border);
        }
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A freehand ink annotation (`/Subtype /Ink`).
#[derive(Debug, Clone, PartialEq)]
pub struct InkSpec {
    rect: Rect,
    color: Color,
    strokes: Vec<Vec<Point>>,
    contents: Option<String>,
    border: Option<AnnotBorder>,
}

impl InkSpec {
    /// Ink over `rect`, one polyline per stroke.
    #[must_use]
    pub fn new(rect: Rect, color: Color, strokes: Vec<Vec<Point>>) -> Self {
        Self {
            rect,
            color,
            strokes,
            contents: None,
            border: None,
        }
    }

    /// Sets `/BS` width and style.
    #[must_use]
    pub fn border(mut self, border: AnnotBorder) -> Self {
        self.border = Some(border);
        self
    }
}

spec_builder!(InkSpec, "An ink annotation.");

impl From<InkSpec> for AnnotSpec {
    fn from(b: InkSpec) -> Self {
        let InkSpec {
            rect,
            color,
            strokes,
            contents,
            border,
        } = b;
        let mut spec = Self::ink(rect, color, strokes);
        if let Some(border) = border {
            spec = spec.with_border(border);
        }
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A line annotation (`/Subtype /Line`).
///
/// ```
/// use kurbo::{Point, Rect};
/// use peniko::Color;
/// use pdfrum_edit::{AnnotSpec, LineEndingStyle, LineSpec};
///
/// let spec: AnnotSpec = LineSpec::new(
///     Rect::new(0.0, 0.0, 100.0, 20.0),
///     Color::from_rgb8(255, 0, 0),
///     Point::new(0.0, 0.0),
///     Point::new(100.0, 20.0),
/// )
/// .endings(LineEndingStyle::OpenArrow, LineEndingStyle::ClosedArrow)
/// .into();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LineSpec {
    rect: Rect,
    color: Color,
    start: Point,
    end: Point,
    contents: Option<String>,
    border: Option<AnnotBorder>,
    endings: Option<(LineEndingStyle, LineEndingStyle)>,
    interior: Option<Color>,
}

impl LineSpec {
    /// A line from `start` to `end`, with `/Rect` as its bounding box.
    #[must_use]
    pub fn new(rect: Rect, color: Color, start: Point, end: Point) -> Self {
        Self {
            rect,
            color,
            start,
            end,
            contents: None,
            border: None,
            endings: None,
            interior: None,
        }
    }

    /// Sets `/BS` width and style. Ending decorations scale with the width.
    #[must_use]
    pub fn border(mut self, border: AnnotBorder) -> Self {
        self.border = Some(border);
        self
    }

    /// Sets the `/LE` ending styles. Omitted by default.
    #[must_use]
    pub fn endings(mut self, start: LineEndingStyle, end: LineEndingStyle) -> Self {
        self.endings = Some((start, end));
        self
    }

    /// Sets `/IC`, the fill for closed endings. Unfilled without it.
    #[must_use]
    pub fn interior(mut self, color: Color) -> Self {
        self.interior = Some(color);
        self
    }
}

spec_builder!(LineSpec, "A line annotation.");

impl From<LineSpec> for AnnotSpec {
    fn from(b: LineSpec) -> Self {
        let LineSpec {
            rect,
            color,
            start,
            end,
            contents,
            border,
            endings,
            interior,
        } = b;
        let mut spec = Self::line(rect, color, start, end);
        if let Some(border) = border {
            spec = spec.with_border(border);
        }
        if let Some((s, e)) = endings {
            spec = spec.with_line_endings(s, e);
        }
        if let Some(interior) = interior {
            spec = spec.with_interior(interior);
        }
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A link annotation (`/Subtype /Link`).
///
/// The action is required, so it is taken at construction; the constructors
/// mirror [`AnnotSpec`]'s link family.
///
/// ```
/// use kurbo::Rect;
/// use pdfrum_edit::{AnnotLinkHighlight, AnnotSpec, LinkSpec};
///
/// let spec: AnnotSpec = LinkSpec::uri(Rect::new(0.0, 0.0, 50.0, 10.0), "https://example.com")
///     .highlight(AnnotLinkHighlight::Outline)
///     .into();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LinkSpec {
    rect: Rect,
    action: AnnotLinkAction,
    contents: Option<String>,
    color: Option<Color>,
    border: Option<AnnotBorder>,
    highlight: Option<AnnotLinkHighlight>,
}

impl LinkSpec {
    /// A link at `rect` running `action`.
    #[must_use]
    pub fn new(rect: Rect, action: AnnotLinkAction) -> Self {
        Self {
            rect,
            action,
            contents: None,
            color: None,
            border: None,
            highlight: None,
        }
    }

    /// A link opening `uri`.
    #[must_use]
    pub fn uri(rect: Rect, uri: impl Into<String>) -> Self {
        Self::new(rect, AnnotLinkAction::Uri(uri.into()))
    }

    /// A link jumping to `page` in this document.
    #[must_use]
    pub fn goto(rect: Rect, page: ObjRef, view: AnnotGoToView) -> Self {
        Self::new(rect, AnnotLinkAction::GoTo { page, view })
    }

    /// A link to a named destination, registering `name` on write.
    #[must_use]
    pub fn named(rect: Rect, name: impl Into<String>, page: ObjRef, view: AnnotGoToView) -> Self {
        Self::new(
            rect,
            AnnotLinkAction::Named {
                name: name.into(),
                page,
                view,
            },
        )
    }

    /// A link to a destination the document already names.
    #[must_use]
    pub fn named_existing(rect: Rect, name: impl Into<String>) -> Self {
        Self::new(rect, AnnotLinkAction::NamedExisting { name: name.into() })
    }

    /// Sets `/C`, the colour of the link's border chrome.
    #[must_use]
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Sets `/BS` width and style.
    #[must_use]
    pub fn border(mut self, border: AnnotBorder) -> Self {
        self.border = Some(border);
        self
    }

    /// Sets `/H`, the viewer's click feedback. Defaults to Invert.
    #[must_use]
    pub fn highlight(mut self, highlight: AnnotLinkHighlight) -> Self {
        self.highlight = Some(highlight);
        self
    }
}

spec_builder!(LinkSpec, "A link annotation.");

impl From<LinkSpec> for AnnotSpec {
    fn from(b: LinkSpec) -> Self {
        let LinkSpec {
            rect,
            action,
            contents,
            color,
            border,
            highlight,
        } = b;
        let mut spec = Self::link_action(rect, action);
        if let Some(color) = color {
            spec = spec.with_color(color);
        }
        if let Some(border) = border {
            spec = spec.with_border(border);
        }
        if let Some(highlight) = highlight {
            spec = spec.with_highlight(highlight);
        }
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}

/// A caret annotation (`/Subtype /Caret`).
#[derive(Debug, Clone, PartialEq)]
pub struct CaretSpec {
    rect: Rect,
    color: Color,
    contents: Option<String>,
}

impl CaretSpec {
    /// A caret over `rect`.
    #[must_use]
    pub fn new(rect: Rect, color: Color) -> Self {
        Self {
            rect,
            color,
            contents: None,
        }
    }
}

spec_builder!(CaretSpec, "A caret annotation.");

impl From<CaretSpec> for AnnotSpec {
    fn from(b: CaretSpec) -> Self {
        let CaretSpec {
            rect,
            color,
            contents,
        } = b;
        let spec = AnnotSpec::caret(rect, color);
        match contents {
            Some(c) => spec.with_contents(c),
            None => spec,
        }
    }
}
