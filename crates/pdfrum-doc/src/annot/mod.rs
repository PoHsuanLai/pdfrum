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

/// An annotation's `/F` flag word (ISO 32000-1 table 165).
///
/// A hand-rolled newtype rather than a `bitflags` dependency, for two reasons
/// the crate would get wrong: **unknown bits round-trip**
/// ([`AnnotFlags::from_bits`] keeps the whole word), and the dump needs a
/// **fixed name order** — [`AnnotFlags::names`] is that order, and the tenth
/// defined bit, `LOCKED_CONTENTS`, has no printed name at all.
///
/// ```
/// use pdfrum_doc::AnnotFlags;
///
/// let f = AnnotFlags::PRINT | AnnotFlags::NO_ZOOM;
/// assert!(f.contains(AnnotFlags::PRINT));
/// assert_eq!(f.names(), ["Print", "NoZoom"]);
/// assert_eq!(AnnotFlags::from_bits(1 << 20).bits(), 1 << 20);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct AnnotFlags(i64);

/// The nine flags the dump prints, in bit order. The tenth defined bit
/// (`LockedContents`) has no printed name.
const FLAG_NAMES: [(AnnotFlags, &str); 9] = [
    (AnnotFlags::INVISIBLE, "Invisible"),
    (AnnotFlags::HIDDEN, "Hidden"),
    (AnnotFlags::PRINT, "Print"),
    (AnnotFlags::NO_ZOOM, "NoZoom"),
    (AnnotFlags::NO_ROTATE, "NoRotate"),
    (AnnotFlags::NO_VIEW, "NoView"),
    (AnnotFlags::READ_ONLY, "ReadOnly"),
    (AnnotFlags::LOCKED, "Locked"),
    (AnnotFlags::TOGGLE_NO_VIEW, "ToggleNoView"),
];

impl AnnotFlags {
    /// Bit 1: an annotation whose subtype the viewer does not handle is not
    /// drawn at all, rather than falling back to its appearance stream.
    ///
    /// Only widgets consult it here — `CPDFSDK_BAAnnot::IsVisible` adds it,
    /// and Pass A never tests it.
    pub const INVISIBLE: Self = Self(1 << 0);
    /// Bit 2: not displayed and not printed at all.
    pub const HIDDEN: Self = Self(1 << 1);
    /// Bit 3: appears in printed output.
    pub const PRINT: Self = Self(1 << 2);
    /// Bit 4: the annotation keeps its size when the page is zoomed.
    pub const NO_ZOOM: Self = Self(1 << 3);
    /// Bit 5: the annotation ignores the page's rotation.
    pub const NO_ROTATE: Self = Self(1 << 4);
    /// Bit 6: suppressed on screen, though it may still print.
    pub const NO_VIEW: Self = Self(1 << 5);
    /// Bit 7: the annotation does not interact with the user.
    pub const READ_ONLY: Self = Self(1 << 6);
    /// Bit 8: the annotation may not be deleted, moved or resized.
    pub const LOCKED: Self = Self(1 << 7);
    /// Bit 9: [`AnnotFlags::NO_VIEW`]'s sense is inverted for the viewer's
    /// own idea of "selected".
    pub const TOGGLE_NO_VIEW: Self = Self(1 << 8);
    /// Bit 10: the annotation's contents may not be changed.
    ///
    /// The one defined bit the dump has no printed name for.
    pub const LOCKED_CONTENTS: Self = Self(1 << 9);

    /// No bit set.
    pub const NONE: Self = Self(0);

    /// The raw `/F` word, including any bit this type does not name.
    #[must_use]
    pub const fn bits(self) -> i64 {
        self.0
    }

    /// The word as written in the file. **Unknown bits are retained**: a
    /// reserved bit a damaged file sets is kept, not dropped.
    #[must_use]
    pub const fn from_bits(bits: i64) -> Self {
        Self(bits)
    }

    /// Whether every bit of `other` is set here.
    ///
    /// [`AnnotFlags::NONE`] is contained in everything, so `contains` is the
    /// wrong question to ask about "no flags at all" — use
    /// `== AnnotFlags::NONE`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of bits.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// A copy with `other`'s bits set. An alias for [`AnnotFlags::union`].
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        self.union(other)
    }

    /// The bits of `self` that are not in `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Whether no bit at all is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The annotation is drawn even when its subtype has no handler.
    ///
    /// The positive reading of the spec's `Invisible` bit, and **the bit
    /// alone**: whether an annotation is actually drawn also depends on
    /// [`AnnotFlags::is_hidden`], [`AnnotFlags::no_view`] and the subtype.
    #[must_use]
    #[doc(alias = "Invisible")]
    pub const fn is_visible(self) -> bool {
        !self.contains(Self::INVISIBLE)
    }

    /// The annotation is not displayed and not printed at all.
    #[must_use]
    pub const fn is_hidden(self) -> bool {
        self.contains(Self::HIDDEN)
    }

    /// The annotation appears in printed output.
    #[must_use]
    pub const fn prints(self) -> bool {
        self.contains(Self::PRINT)
    }

    /// The annotation is shown on screen.
    ///
    /// The positive reading of the spec's `NoView` bit.
    #[must_use]
    #[doc(alias = "NoView")]
    pub const fn views(self) -> bool {
        !self.contains(Self::NO_VIEW)
    }

    /// The annotation is suppressed on screen.
    #[must_use]
    #[doc(alias = "NoView")]
    pub const fn no_view(self) -> bool {
        self.contains(Self::NO_VIEW)
    }

    /// The annotation scales with the page.
    ///
    /// The positive reading of the spec's `NoZoom` bit.
    #[must_use]
    #[doc(alias = "NoZoom")]
    pub const fn zooms(self) -> bool {
        !self.contains(Self::NO_ZOOM)
    }

    /// The annotation turns with the page.
    ///
    /// The positive reading of the spec's `NoRotate` bit.
    #[must_use]
    #[doc(alias = "NoRotate")]
    pub const fn rotates(self) -> bool {
        !self.contains(Self::NO_ROTATE)
    }

    /// The annotation ignores the page's rotation.
    #[must_use]
    #[doc(alias = "NoRotate")]
    pub const fn no_rotate(self) -> bool {
        self.contains(Self::NO_ROTATE)
    }

    /// The set flags' names, in bit order.
    ///
    /// **The order is the dump's** — `--annot` prints exactly this sequence,
    /// and [`AnnotFlags::LOCKED_CONTENTS`] never appears because it has no
    /// printed name.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        FLAG_NAMES
            .iter()
            .filter(|(bit, _)| self.contains(*bit))
            .map(|(_, name)| *name)
            .collect()
    }
}

impl std::ops::BitOr for AnnotFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
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
            flags: AnnotFlags::from_bits(dict.int(names::F, r).unwrap_or(0)),
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
            AnnotFlags::from_bits(4 | 8 | 16).names(),
            ["Print", "NoZoom", "NoRotate"]
        );
        assert!(AnnotFlags::NONE.names().is_empty());
        // The tenth bit has no printed name.
        assert!(AnnotFlags::LOCKED_CONTENTS.names().is_empty());
        assert!(AnnotFlags::HIDDEN.is_hidden());
    }

    /// The dump's `Flags set:` line is byte-compared by the conformance
    /// harness, so the name order is behaviour, not presentation. Every
    /// printed name, in the one order they may appear in.
    #[test]
    fn the_dump_order_is_fixed() {
        let all = AnnotFlags::from_bits(0x3FF);
        assert_eq!(
            all.names().join(", "),
            "Invisible, Hidden, Print, NoZoom, NoRotate, NoView, ReadOnly, Locked, ToggleNoView"
        );
        // Setting the unnamed tenth bit, or a reserved one, changes nothing.
        assert_eq!(
            AnnotFlags::from_bits(0x3FF | (1 << 20)).names().join(", "),
            "Invisible, Hidden, Print, NoZoom, NoRotate, NoView, ReadOnly, Locked, ToggleNoView"
        );
    }

    #[test]
    fn unknown_bits_round_trip() {
        let f = AnnotFlags::from_bits((1 << 20) | AnnotFlags::PRINT.bits());
        assert_eq!(f.bits(), (1 << 20) | 4);
        assert!(f.contains(AnnotFlags::PRINT));
        assert!(!f.contains(AnnotFlags::HIDDEN));
    }

    #[test]
    fn set_algebra_and_the_positive_predicates() {
        let f = AnnotFlags::PRINT | AnnotFlags::NO_VIEW | AnnotFlags::NO_ROTATE;
        assert!(f.contains(AnnotFlags::PRINT | AnnotFlags::NO_VIEW));
        assert!(f.contains(AnnotFlags::NONE));
        assert!(!f.contains(AnnotFlags::PRINT | AnnotFlags::LOCKED));
        assert!(!f.views() && f.no_view());
        assert!(!f.rotates() && f.no_rotate());
        assert!(f.zooms());
        assert!(f.is_visible());
        assert!(!AnnotFlags::INVISIBLE.is_visible());

        let cleared = f.without(AnnotFlags::NO_VIEW);
        assert!(cleared.views());
        assert!(cleared.contains(AnnotFlags::PRINT));
        assert!(AnnotFlags::NONE.is_empty());
        assert_eq!(
            AnnotFlags::NONE.with(AnnotFlags::LOCKED),
            AnnotFlags::LOCKED
        );
    }
}
