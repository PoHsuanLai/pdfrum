//! The content-operator table (ISO 32000-1 §8-9), and the [`Op`] enum it
//! generates.
//!
//! One `ops!` invocation is the single source of truth for every operator:
//! its spelling, its variant, the operands it reads and in which order, and
//! whether a wrong operand count makes it a no-op. Everything else in the
//! crate matches on `Op` and never spells an operator name, so adding one is
//! a single-line edit here and every consumer fails to compile until it
//! handles the new variant.
//!
//! # Operand order is the contract
//!
//! PDFium keeps operands in a 16-slot ring indexed from the *newest*, and
//! each handler reaches back by a fixed index. `1 2 m` reads x from index 1
//! and y from index 0. The `from` positions in the table below are those
//! indices, so a row reads exactly like the handler it replaces — see
//! [`crate::content::OperandRing`] for the ring itself and what a missing
//! operand yields (`0`, `""`, null; never an error).

use crate::content::OperandRing;
use kurbo::{Affine, Point};
use pdfrum_object::{Name, Object, PdfString};
use smallvec::SmallVec;

/// Line-cap style (`J`, `/LC`; ISO 32000-1 table 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum LineCap {
    /// Square butt at the exact endpoint.
    #[default]
    Butt,
    /// Semicircular cap centred on the endpoint.
    Round,
    /// Square cap projecting half the line width past the endpoint.
    Square,
}

impl LineCap {
    /// The cap an operand names, clamped into range.
    ///
    /// PDFium casts the operand into a three-valued enum unchecked and lets
    /// the rasterizer's `default:` arm absorb anything out of range; clamping
    /// here is observably identical and keeps the enum honest
    /// (design brief D5).
    #[must_use]
    pub fn from_int(v: i64) -> Self {
        match v {
            1 => Self::Round,
            2 => Self::Square,
            _ => Self::Butt,
        }
    }

    /// Whether `v` named this cap exactly, i.e. no clamping happened.
    #[must_use]
    pub fn is_exact(v: i64) -> bool {
        (0..=2).contains(&v)
    }
}

/// Line-join style (`j`, `/LJ`; ISO 32000-1 table 53).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum LineJoin {
    /// Extend the outer edges until they meet, subject to the miter limit.
    #[default]
    Miter,
    /// Round off the corner with an arc.
    Round,
    /// Cut the corner off with a straight edge.
    Bevel,
}

impl LineJoin {
    /// The join an operand names, clamped into range (see
    /// [`LineCap::from_int`]).
    #[must_use]
    pub fn from_int(v: i64) -> Self {
        match v {
            1 => Self::Round,
            2 => Self::Bevel,
            _ => Self::Miter,
        }
    }

    /// Whether `v` named this join exactly.
    #[must_use]
    pub fn is_exact(v: i64) -> bool {
        (0..=2).contains(&v)
    }
}

/// How a path-painting operator fills its interior (ISO 32000-1 §8.5.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum FillRule {
    /// Do not fill at all — the operator only strokes, or only clips.
    #[default]
    None,
    /// Nonzero winding number rule (`f`, `B`, `W`).
    Winding,
    /// Even-odd rule (`f*`, `B*`, `W*`).
    EvenOdd,
}

/// How glyphs are painted (`Tr`; ISO 32000-1 table 106).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum TextRenderMode {
    /// Fill the glyphs.
    #[default]
    Fill,
    /// Stroke the glyph outlines.
    Stroke,
    /// Fill, then stroke.
    FillStroke,
    /// Paint nothing.
    Invisible,
    /// Fill and add to the clipping path.
    FillClip,
    /// Stroke and add to the clipping path.
    StrokeClip,
    /// Fill, stroke and add to the clipping path.
    FillStrokeClip,
    /// Add to the clipping path only.
    Clip,
}

impl TextRenderMode {
    /// The mode `v` names, or `None` when it is outside 0..=7.
    ///
    /// PDFium leaves the mode *unchanged* for an out-of-range operand rather
    /// than defaulting it, so the caller must keep the old value on `None`.
    #[must_use]
    pub fn from_int(v: i64) -> Option<Self> {
        Some(match v {
            0 => Self::Fill,
            1 => Self::Stroke,
            2 => Self::FillStroke,
            3 => Self::Invisible,
            4 => Self::FillClip,
            5 => Self::StrokeClip,
            6 => Self::FillStrokeClip,
            7 => Self::Clip,
            _ => return None,
        })
    }

    /// Whether the mode paints glyph interiors.
    #[must_use]
    pub fn fills(self) -> bool {
        matches!(
            self,
            Self::Fill | Self::FillStroke | Self::FillClip | Self::FillStrokeClip
        )
    }

    /// Whether the mode paints glyph outlines.
    #[must_use]
    pub fn strokes(self) -> bool {
        matches!(
            self,
            Self::Stroke | Self::FillStroke | Self::StrokeClip | Self::FillStrokeClip
        )
    }

    /// Whether the mode contributes the glyphs to the clipping path.
    #[must_use]
    pub fn clips(self) -> bool {
        matches!(
            self,
            Self::FillClip | Self::StrokeClip | Self::FillStrokeClip | Self::Clip
        )
    }
}

/// One element of a `TJ` array: a string to show, or an adjustment in
/// thousandths of a text-space unit.
#[derive(Debug, Clone, PartialEq)]
pub enum TextItem {
    /// A string of character codes.
    Show(Box<[u8]>),
    /// A displacement subtracted from the current position.
    Adjust(f32),
}

/// The property list a `BDC` operator carries: a name into `/Properties`, or
/// an inline dictionary.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkProperties {
    /// `BDC /Tag /Name` — resolved through the `/Properties` resource.
    Named(Name),
    /// `BDC /Tag << … >>` — the dictionary is written out inline.
    Inline(Box<pdfrum_object::Dict>),
}

/// An inline image (`BI … ID … EI`), already separated into its dictionary
/// and its raw sample bytes by the tokenizer (ISO 32000-1 §8.9.7).
#[derive(Debug, Clone, PartialEq)]
pub struct InlineImage {
    /// The image dictionary with abbreviations expanded and `/Subtype /Image`
    /// established.
    pub dict: pdfrum_object::Dict,
    /// The bytes between `ID` and the terminating `EI`, still filtered.
    pub data: Box<[u8]>,
}

/// How an operand is pulled out of the ring, and what a missing one yields.
///
/// This mirrors PDFium's accessors exactly: a number that is not there reads
/// `0`, a string reads empty, an object reads null. Only the rows carrying an
/// `exactly N` guard ever refuse to run.
trait FromOperands: Sized {
    fn extract(ring: &OperandRing, index: usize) -> Self;
}

impl FromOperands for f32 {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        ring.number(index)
    }
}

impl FromOperands for i64 {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        ring.integer(index).unwrap_or(0)
    }
}

impl FromOperands for LineCap {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        Self::from_int(ring.integer(index).unwrap_or(0))
    }
}

impl FromOperands for LineJoin {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        Self::from_int(ring.integer(index).unwrap_or(0))
    }
}

impl FromOperands for Name {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        Name::new(ring.string(index))
    }
}

impl FromOperands for PdfString {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        PdfString::literal(ring.string(index))
    }
}

impl FromOperands for Object {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        ring.object(index)
    }
}

/// A point read as `(number at index+1, number at index)` — x is the *older*
/// operand, matching `GetPoint`.
impl FromOperands for Point {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        Self::new(
            f64::from(ring.number(index + 1)),
            f64::from(ring.number(index)),
        )
    }
}

/// Six numbers read oldest-to-newest as `(a, b, c, d, e, f)`, matching
/// `GetMatrix`. Note kurbo's `Affine` takes the same `[a b c d e f]` layout.
impl FromOperands for Affine {
    fn extract(ring: &OperandRing, index: usize) -> Self {
        let n = |k: usize| f64::from(ring.number(index + k));
        Self::new([n(5), n(4), n(3), n(2), n(1), n(0)])
    }
}

macro_rules! ops {
    ($(
        $(#[$meta:meta])*
        $spelling:literal => $variant:ident ( $($ty:ty : $from:tt),* ) $($guard:literal)?;
    )*) => {
        /// One content-stream operator with its operands already extracted.
        ///
        /// Generated from the table in this module, which is the only place
        /// operator spellings appear.
        #[derive(Debug, Clone, PartialEq)]
        #[non_exhaustive]
        pub enum Op {
            $(
                $(#[$meta])*
                $variant ( $($ty),* ),
            )*
            /// An inline image, consumed whole by the tokenizer.
            InlineImage(Box<InlineImage>),
            /// A keyword that names no operator. PDFium drops it silently
            /// after clearing the operands; we keep the spelling so a
            /// diagnostic and a dump can name it.
            Unknown(Box<[u8]>),
        }

        impl Op {
            /// The operator's spelling as it appears in a content stream.
            #[must_use]
            pub fn keyword(&self) -> &[u8] {
                match self {
                    $( Self::$variant { .. } => $spelling, )*
                    Self::InlineImage(_) => b"BI",
                    Self::Unknown(w) => w,
                }
            }

            /// Whether `word` names a registered operator.
            ///
            /// `BI`/`ID`/`EI` are registered but never reach dispatch — the
            /// tokenizer consumes an inline image whole.
            #[must_use]
            pub fn is_operator(word: &[u8]) -> bool {
                matches!(word, $( $spelling )|*)
            }

            /// Build the operator `word` names from the operands currently in
            /// `ring`.
            ///
            /// A registered operator whose `param_count != N` guard fails
            /// yields [`Dispatch::GuardFailed`] and produces no `Op` at all,
            /// exactly as PDFium's early `return` does.
            pub(crate) fn from_ring(word: &[u8], ring: &OperandRing) -> Dispatch {
                let _ = ring;
                match word {
                    $(
                        $spelling => {
                            $(
                                if ring.len() != $guard {
                                    return Dispatch::GuardFailed;
                                }
                            )?
                            Dispatch::Op(Op::$variant(
                                $( <$ty as FromOperands>::extract(ring, $from) ),*
                            ))
                        }
                    )*
                    _ => Dispatch::NotAnOperator,
                }
            }
        }
    };
}

/// What looking a keyword up in the table produced.
pub(crate) enum Dispatch {
    /// A complete operator.
    Op(Op),
    /// A registered operator whose `param_count != N` guard refused it. The
    /// operator is dropped and the operands are cleared, exactly as PDFium's
    /// early `return` does.
    GuardFailed,
    /// The keyword names no operator at all.
    NotAnOperator,
}

include!("ops_table.rs");

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{LineCap, LineJoin, Op, TextRenderMode};

    #[test]
    fn every_operator_round_trips_its_spelling() {
        // The `keyword` arm and the dispatch arm come from the same row, so
        // this catches a row whose spelling and variant drifted apart.
        for word in [
            &b"q"[..],
            b"Q",
            b"cm",
            b"w",
            b"J",
            b"j",
            b"M",
            b"d",
            b"ri",
            b"i",
            b"gs",
            b"m",
            b"l",
            b"c",
            b"v",
            b"y",
            b"h",
            b"re",
            b"S",
            b"s",
            b"f",
            b"F",
            b"f*",
            b"B",
            b"B*",
            b"b",
            b"b*",
            b"n",
            b"W",
            b"W*",
            b"BT",
            b"ET",
            b"Td",
            b"TD",
            b"Tm",
            b"T*",
            b"TL",
            b"Ts",
            b"Tz",
            b"Tc",
            b"Tw",
            b"Tf",
            b"Tr",
            b"Tj",
            b"'",
            b"\"",
            b"TJ",
            b"d0",
            b"d1",
            b"CS",
            b"cs",
            b"SC",
            b"sc",
            b"SCN",
            b"scn",
            b"G",
            b"g",
            b"RG",
            b"rg",
            b"K",
            b"k",
            b"Do",
            b"sh",
            b"BI",
            b"ID",
            b"EI",
            b"BMC",
            b"BDC",
            b"EMC",
            b"MP",
            b"DP",
            b"BX",
            b"EX",
        ] {
            assert!(Op::is_operator(word), "{word:?} is not registered");
        }
        assert!(!Op::is_operator(b"Tjxx"));
        assert!(!Op::is_operator(b""));
        assert!(!Op::is_operator(b"true"));
    }

    #[test]
    fn caps_and_joins_clamp_out_of_range_operands() {
        assert_eq!(LineCap::from_int(1), LineCap::Round);
        assert_eq!(LineCap::from_int(5), LineCap::Butt);
        assert_eq!(LineCap::from_int(-1), LineCap::Butt);
        assert!(!LineCap::is_exact(3));
        assert_eq!(LineJoin::from_int(2), LineJoin::Bevel);
        assert_eq!(LineJoin::from_int(99), LineJoin::Miter);
        assert!(LineJoin::is_exact(0));
    }

    #[test]
    fn text_render_modes_outside_zero_to_seven_are_refused() {
        assert_eq!(TextRenderMode::from_int(0), Some(TextRenderMode::Fill));
        assert_eq!(TextRenderMode::from_int(7), Some(TextRenderMode::Clip));
        assert_eq!(TextRenderMode::from_int(8), None);
        assert_eq!(TextRenderMode::from_int(-1), None);
        assert!(TextRenderMode::FillStrokeClip.fills());
        assert!(TextRenderMode::FillStrokeClip.strokes());
        assert!(TextRenderMode::FillStrokeClip.clips());
        assert!(!TextRenderMode::Invisible.fills());
        assert!(!TextRenderMode::Fill.clips());
    }
}
