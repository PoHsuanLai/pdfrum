//! Annotations (ISO 32000-1 §12.5): the subtype table, the flag word, and
//! the record every reader in this crate works from.

pub mod appearance;
pub mod list;
pub mod quad;

pub use appearance::{ApMode, annot_ap, annot_matrix};
pub use list::{AnnotList, popup_appears_for};
pub use quad::{
    bounding_rect_from_quad_points, quad_point_count, rect_from_quad_points,
    rect_from_quad_points_array,
};

use kurbo::Rect;
use pdfrum_object::{Array, Dict, Name, Resolve};

use crate::names;

/// An annotation's `/Subtype`.
///
/// The variant order is API: it is the numbering the public annotation
/// interface exposes, and the `--annot` dump's subtype names come from the
/// same table. Three spellings differ from their variant names — `ThreeD` is
/// `3D`, `XfaWidget` is `XFAWidget`, and `PolyLine` keeps its capital L.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Subtype {
    /// A `/Subtype` matching none of the known spellings, or none at all.
    #[default]
    Unknown,
    /// A sticky note.
    Text,
    /// A hyperlink or destination jump.
    Link,
    /// Free text drawn directly on the page.
    FreeText,
    /// A straight line.
    Line,
    /// A rectangle.
    Square,
    /// An ellipse.
    Circle,
    /// A closed polygon.
    Polygon,
    /// An open polyline.
    PolyLine,
    /// Highlighted text.
    Highlight,
    /// Underlined text.
    Underline,
    /// Text with a wavy underline.
    Squiggly,
    /// Struck-out text.
    StrikeOut,
    /// A rubber stamp.
    Stamp,
    /// An editorial caret.
    Caret,
    /// Freehand ink strokes.
    Ink,
    /// A pop-up note attached to another annotation.
    Popup,
    /// An attached file.
    FileAttachment,
    /// A sound clip.
    Sound,
    /// A movie clip.
    Movie,
    /// A form field's on-page control.
    Widget,
    /// An embedded media screen.
    Screen,
    /// A printer's mark.
    PrinterMark,
    /// A trapping network.
    TrapNet,
    /// A watermark.
    Watermark,
    /// Three-dimensional artwork.
    ThreeD,
    /// Rich media.
    RichMedia,
    /// An XFA form control.
    XfaWidget,
    /// A redaction region.
    Redact,
}

/// The spelling table, used by both directions and by the `--annot` dump.
const SUBTYPES: [(Subtype, &[u8]); 28] = [
    (Subtype::Text, b"Text"),
    (Subtype::Link, b"Link"),
    (Subtype::FreeText, b"FreeText"),
    (Subtype::Line, b"Line"),
    (Subtype::Square, b"Square"),
    (Subtype::Circle, b"Circle"),
    (Subtype::Polygon, b"Polygon"),
    (Subtype::PolyLine, b"PolyLine"),
    (Subtype::Highlight, b"Highlight"),
    (Subtype::Underline, b"Underline"),
    (Subtype::Squiggly, b"Squiggly"),
    (Subtype::StrikeOut, b"StrikeOut"),
    (Subtype::Stamp, b"Stamp"),
    (Subtype::Caret, b"Caret"),
    (Subtype::Ink, b"Ink"),
    (Subtype::Popup, b"Popup"),
    (Subtype::FileAttachment, b"FileAttachment"),
    (Subtype::Sound, b"Sound"),
    (Subtype::Movie, b"Movie"),
    (Subtype::Widget, b"Widget"),
    (Subtype::Screen, b"Screen"),
    (Subtype::PrinterMark, b"PrinterMark"),
    (Subtype::TrapNet, b"TrapNet"),
    (Subtype::Watermark, b"Watermark"),
    (Subtype::ThreeD, b"3D"),
    (Subtype::RichMedia, b"RichMedia"),
    (Subtype::XfaWidget, b"XFAWidget"),
    (Subtype::Redact, b"Redact"),
];

impl Subtype {
    /// Reads a `/Subtype` spelling. Anything unrecognized is
    /// [`Subtype::Unknown`].
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Subtype {
        SUBTYPES
            .iter()
            .find(|(_, spelling)| *spelling == bytes)
            .map_or(Subtype::Unknown, |(subtype, _)| *subtype)
    }

    /// The spelling. [`Subtype::Unknown`] has none and answers empty.
    #[must_use]
    pub fn as_bytes(self) -> &'static [u8] {
        SUBTYPES
            .iter()
            .find(|(subtype, _)| *subtype == self)
            .map_or(&b""[..], |(_, spelling)| spelling)
    }

    /// The four text markup subtypes, whose drawing rectangle comes from
    /// their quadrilaterals once an appearance has been generated.
    ///
    /// This is **not** the same set as [`Subtype::has_attachment_points`],
    /// which the `--annot` dump uses and which also admits links.
    #[must_use]
    pub fn is_text_markup(self) -> bool {
        matches!(
            self,
            Subtype::Highlight | Subtype::Squiggly | Subtype::StrikeOut | Subtype::Underline
        )
    }

    /// Whether the dump prints a quadpoint count for this subtype.
    ///
    /// Links are in the set, which is why every link in the corpus prints
    /// `Number of quadpoints sets: 0`.
    #[must_use]
    pub fn has_attachment_points(self) -> bool {
        matches!(self, Subtype::Link) || self.is_text_markup()
    }
}

/// An annotation's `/F` flag word.
///
/// A newtype rather than a bitflags dependency: nine named predicates and one
/// bit-order iterator is the whole surface, and the dump needs the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnnotFlags(pub i64);

/// The nine flags the dump prints, in bit order. The tenth defined bit
/// (`LockedContents`) has no printed name.
const FLAG_NAMES: [(i64, &str); 9] = [
    (1, "Invisible"),
    (2, "Hidden"),
    (4, "Print"),
    (8, "NoZoom"),
    (16, "NoRotate"),
    (32, "NoView"),
    (64, "ReadOnly"),
    (128, "Locked"),
    (256, "ToggleNoView"),
];

impl AnnotFlags {
    /// The annotation is not displayed and not printed at all.
    #[must_use]
    pub fn is_hidden(self) -> bool {
        self.0 & 2 != 0
    }

    /// The annotation appears in printed output.
    #[must_use]
    pub fn prints(self) -> bool {
        self.0 & 4 != 0
    }

    /// The annotation is suppressed on screen.
    #[must_use]
    pub fn no_view(self) -> bool {
        self.0 & 32 != 0
    }

    /// The annotation ignores the page's rotation.
    #[must_use]
    pub fn no_rotate(self) -> bool {
        self.0 & 16 != 0
    }

    /// The set flags' names, in bit order.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        FLAG_NAMES
            .iter()
            .filter(|(bit, _)| self.0 & bit != 0)
            .map(|(_, name)| *name)
            .collect()
    }
}

/// One annotation, as every reader in this crate sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// The `/Subtype`.
    pub subtype: Subtype,
    /// `/Rect` **exactly as written**: not normalized, and not corrected for
    /// an inverted ordering. The dump prints these numbers verbatim.
    pub rect: Rect,
    /// `/F`.
    pub flags: AnnotFlags,
    /// The source dictionary, for the long tail of per-subtype keys.
    pub dict: Dict,
    /// `/QuadPoints`, when present.
    pub quad_points: Option<Array>,
}

impl Annotation {
    /// Reads one annotation dictionary.
    #[must_use]
    pub fn read<R: Resolve>(dict: &Dict, r: &R) -> Annotation {
        Annotation {
            // `/Subtype` is read coercively here: a *string* subtype names an
            // annotation just as a name does. The form-field reader uses a
            // name-typed accessor for the same key, so a string `(Widget)` is
            // an annotation widget but not a form widget.
            subtype: Subtype::from_bytes(&dict.byte_string(names::SUBTYPE, r).unwrap_or_default()),
            rect: dict.rect(names::RECT, r),
            flags: AnnotFlags(dict.int(names::F, r).unwrap_or(0)),
            dict: dict.clone(),
            quad_points: dict.array(names::QUAD_POINTS, r),
        }
    }

    /// The rectangle an appearance is drawn into.
    ///
    /// A text markup annotation whose appearance *we* generated draws at its
    /// quadrilaterals' bounding box rather than at its `/Rect`; everything
    /// else draws at `/Rect`.
    #[must_use]
    pub fn rect_for_drawing(&self, has_generated_ap: bool) -> Rect {
        if self.subtype.is_text_markup() && has_generated_ap {
            quad::bounding_rect_from_quad_points(self.quad_points.as_ref())
        } else {
            self.rect
        }
    }
}

/// Whether a `/Subtype` value names a pop-up, read coercively.
#[must_use]
pub fn is_popup<R: Resolve>(dict: &Dict, r: &R) -> bool {
    dict.byte_string(names::SUBTYPE, r).as_deref() == Some(b"Popup")
}

/// The `/Subtype` name value, for constructing dictionaries.
#[must_use]
pub fn subtype_name(subtype: Subtype) -> Name {
    Name::new(subtype.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::{AnnotFlags, Subtype};

    #[test]
    fn three_spellings_differ_from_their_variant_names() {
        assert_eq!(Subtype::ThreeD.as_bytes(), b"3D");
        assert_eq!(Subtype::XfaWidget.as_bytes(), b"XFAWidget");
        assert_eq!(Subtype::PolyLine.as_bytes(), b"PolyLine");
        assert_eq!(Subtype::from_bytes(b"3D"), Subtype::ThreeD);
        assert_eq!(Subtype::from_bytes(b"XFAWidget"), Subtype::XfaWidget);
    }

    #[test]
    fn matching_is_exact_and_case_sensitive() {
        assert_eq!(Subtype::from_bytes(b"Widget"), Subtype::Widget);
        assert_eq!(Subtype::from_bytes(b"widget"), Subtype::Unknown);
        assert_eq!(Subtype::from_bytes(b""), Subtype::Unknown);
        assert_eq!(Subtype::Unknown.as_bytes(), b"");
    }

    #[test]
    fn every_spelling_round_trips() {
        for subtype in [
            Subtype::Text,
            Subtype::Link,
            Subtype::FreeText,
            Subtype::Line,
            Subtype::Square,
            Subtype::Circle,
            Subtype::Polygon,
            Subtype::PolyLine,
            Subtype::Highlight,
            Subtype::Underline,
            Subtype::Squiggly,
            Subtype::StrikeOut,
            Subtype::Stamp,
            Subtype::Caret,
            Subtype::Ink,
            Subtype::Popup,
            Subtype::FileAttachment,
            Subtype::Sound,
            Subtype::Movie,
            Subtype::Widget,
            Subtype::Screen,
            Subtype::PrinterMark,
            Subtype::TrapNet,
            Subtype::Watermark,
            Subtype::ThreeD,
            Subtype::RichMedia,
            Subtype::XfaWidget,
            Subtype::Redact,
        ] {
            assert_eq!(Subtype::from_bytes(subtype.as_bytes()), subtype);
        }
    }

    #[test]
    fn text_markup_and_attachment_points_are_different_sets() {
        assert!(!Subtype::Link.is_text_markup());
        assert!(Subtype::Link.has_attachment_points());
        assert!(Subtype::Highlight.is_text_markup());
        assert!(Subtype::Highlight.has_attachment_points());
        assert!(!Subtype::Square.has_attachment_points());
    }

    #[test]
    fn flag_names_come_out_in_bit_order() {
        assert_eq!(
            AnnotFlags(4 | 8 | 16).names(),
            ["Print", "NoZoom", "NoRotate"]
        );
        assert!(AnnotFlags(0).names().is_empty());
        // The tenth bit has no printed name.
        assert!(AnnotFlags(512).names().is_empty());
        assert!(AnnotFlags(2).is_hidden());
    }
}
