//! The small newtypes and flag set every other module is written in terms of.

use std::fmt;

pub use pdfrum_cmap::{CharCode, Cid};

/// A glyph index into a font program.
///
/// Zero is a legitimate value — it is the `.notdef` glyph, which PDFium
/// deliberately distinguishes from "no glyph at all" (that is `None`, the C++'s
/// `-1`). Every ladder in this crate returns `Option<Gid>` for exactly that
/// reason.
///
/// Whose numbering this is depends on the loaded program: `skrifa`'s
/// `GlyphId` for an sfnt or bare-CFF face, and `/CharStrings` declaration
/// order for a Type 1 one. [`pdfrum_type1::Gid`] names that second space in
/// its own crate and stays a separate type; the `From` impls below are the
/// conversion, and they live here because this is the one crate that holds
/// both (`docs/design/idiomatic-api.md` §B.3, and §A.11's step-12 ruling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Gid(pub u16);

impl From<pdfrum_type1::Gid> for Gid {
    fn from(g: pdfrum_type1::Gid) -> Self {
        Self(g.0)
    }
}

impl From<Gid> for pdfrum_type1::Gid {
    fn from(g: Gid) -> Self {
        Self(g.0)
    }
}

/// Identifies one loaded font within a [`FontCache`](crate::FontCache), so a
/// glyph cache entry cannot be mistaken for another font's.
///
/// Opaque and monotonically assigned; the numeric value means nothing beyond
/// "not the same font as a different value".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontId(pub u64);

/// A glyph name from an `/Encoding` `/Differences` array or a predefined
/// character set.
///
/// Glyph names are compared byte-exactly against `.notdef` and `space` in the
/// Type 1 ladder and are looked up in the Adobe Glyph List, so they stay bytes
/// rather than becoming `str`: a `/Differences` entry may name anything.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlyphName(Box<[u8]>);

impl GlyphName {
    /// Wrap a name's bytes.
    #[must_use]
    pub fn new(bytes: impl Into<Box<[u8]>>) -> Self {
        Self(bytes.into())
    }

    /// The name's bytes, as they appeared in the file.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name as UTF-8, when it is valid UTF-8. Every real glyph name is
    /// ASCII; a name that is not is simply not in any table we consult.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }
}

impl fmt::Debug for GlyphName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(s) => write!(f, "GlyphName({s:?})"),
            None => write!(f, "GlyphName({:?})", self.0),
        }
    }
}

impl From<&str> for GlyphName {
    fn from(s: &str) -> Self {
        Self::new(s.as_bytes().to_vec())
    }
}

/// The `/FontDescriptor` `/Flags` bit set (ISO 32000-1 table 123), plus
/// PDFium's own `USE_EXTERN_ATTR` bit.
///
/// A hand-rolled newtype rather than a `bitflags` dependency, for the reason
/// `bitflags` would get wrong: **unknown bits round-trip**. Files set reserved
/// bits, and `SYMBOLIC` and `NON_SYMBOLIC` co-occur in the wild, so
/// [`FontFlags::from_bits`] keeps the whole word and [`FontFlags::bits`]
/// hands it back unchanged.
///
/// ```
/// use pdfrum_font::FontFlags;
///
/// let f = FontFlags::SERIF | FontFlags::ITALIC;
/// assert!(f.contains(FontFlags::SERIF));
/// assert!(!f.without(FontFlags::SERIF).contains(FontFlags::SERIF));
///
/// // A reserved bit survives the trip.
/// assert_eq!(FontFlags::from_bits(1 << 30).bits(), 1 << 30);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct FontFlags(u32);

impl FontFlags {
    /// All glyphs have the same width.
    pub const FIXED_PITCH: Self = Self(1 << 0);
    /// Glyphs have serifs.
    pub const SERIF: Self = Self(1 << 1);
    /// The font uses its own built-in encoding rather than a standard one.
    pub const SYMBOLIC: Self = Self(1 << 2);
    /// Glyphs resemble cursive handwriting.
    pub const SCRIPT: Self = Self(1 << 3);
    /// The font uses the Adobe standard Latin character set.
    pub const NON_SYMBOLIC: Self = Self(1 << 5);
    /// Glyphs have dominant vertical strokes that are slanted.
    pub const ITALIC: Self = Self(1 << 6);
    /// No lowercase letters.
    pub const ALL_CAP: Self = Self(1 << 16);
    /// Lowercase letters have the shapes of uppercase ones at reduced size.
    pub const SMALL_CAP: Self = Self(1 << 17);
    /// Bold glyphs are painted with extra pixels at small sizes.
    pub const FORCE_BOLD: Self = Self(1 << 18);
    /// **Not** a PDF flag. PDFium sets this bit when the descriptor carried a
    /// complete enough metric set to be trusted, and the substitution ladder
    /// discards the caller's weight and slant entirely when it is absent
    /// (`docs/design/pdfrum-font.md` §1.2, §1.12 step 0).
    pub const USE_EXTERN_ATTR: Self = Self(1 << 19);

    /// No bit set.
    pub const NONE: Self = Self(0);

    /// The default when a font has no `/FontDescriptor` at all.
    pub const DEFAULT: Self = Self::NON_SYMBOLIC;

    /// The raw `/Flags` word, including any bit this type does not name.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The word as written in the file. **Unknown bits are retained**: a
    /// reserved bit a damaged file sets is kept, not dropped.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether every bit of `other` is set here.
    ///
    /// [`FontFlags::NONE`] is contained in everything, so `contains` is the
    /// wrong question to ask about "no flags at all" — use `== FontFlags::NONE`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of bits.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// A copy with `other`'s bits set. An alias for [`FontFlags::union`].
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

    /// Symbolic fonts use their own encoding vector.
    #[must_use]
    pub const fn is_symbolic(self) -> bool {
        self.contains(Self::SYMBOLIC)
    }

    /// Non-symbolic fonts use the Adobe standard Latin set.
    #[must_use]
    pub const fn is_non_symbolic(self) -> bool {
        self.contains(Self::NON_SYMBOLIC)
    }

    /// Italic, per the descriptor's own flag rather than its `/ItalicAngle`.
    #[must_use]
    pub const fn is_italic(self) -> bool {
        self.contains(Self::ITALIC)
    }

    /// Every glyph the same width.
    #[must_use]
    pub const fn is_fixed_pitch(self) -> bool {
        self.contains(Self::FIXED_PITCH)
    }

    /// The descriptor's metrics are complete enough to trust (§1.2).
    #[must_use]
    pub const fn uses_extern_attr(self) -> bool {
        self.contains(Self::USE_EXTERN_ATTR)
    }

    /// No lowercase letters — triggers the all-caps glyph aliasing of §1.4.
    #[must_use]
    pub const fn is_all_cap(self) -> bool {
        self.contains(Self::ALL_CAP)
    }
}

impl std::ops::BitOr for FontFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_predicates_read_the_right_bits() {
        let f = FontFlags::SYMBOLIC | FontFlags::ITALIC;
        assert!(f.is_symbolic());
        assert!(f.is_italic());
        assert!(!f.is_non_symbolic());
        assert!(!f.uses_extern_attr());
        assert_eq!(FontFlags::DEFAULT.bits(), 32);
    }

    #[test]
    fn with_and_without_are_inverses() {
        let f = FontFlags::NONE.with(FontFlags::ALL_CAP);
        assert!(f.is_all_cap());
        assert!(!f.without(FontFlags::ALL_CAP).is_all_cap());
    }

    #[test]
    fn unknown_bits_round_trip() {
        // Bit 30 is reserved; a file that sets it keeps it.
        let reserved = 1 << 30;
        let f = FontFlags::from_bits(reserved | FontFlags::SERIF.bits());
        assert_eq!(f.bits(), reserved | FontFlags::SERIF.bits());
        assert!(f.contains(FontFlags::SERIF));
        assert!(!f.contains(FontFlags::ITALIC));
    }

    #[test]
    fn symbolic_and_non_symbolic_co_occur() {
        // The spec says they are exclusive; files disagree, and both
        // predicates must answer for what is written.
        let f = FontFlags::SYMBOLIC | FontFlags::NON_SYMBOLIC;
        assert!(f.is_symbolic());
        assert!(f.is_non_symbolic());
        assert_eq!(f.bits(), (1 << 2) | (1 << 5));
    }

    #[test]
    fn contains_holds_for_a_subset_and_the_empty_set() {
        let f = FontFlags::SERIF | FontFlags::ITALIC | FontFlags::ALL_CAP;
        assert!(f.contains(FontFlags::SERIF | FontFlags::ALL_CAP));
        assert!(f.contains(FontFlags::NONE));
        assert!(!f.contains(FontFlags::SERIF | FontFlags::SMALL_CAP));
        assert!(FontFlags::NONE.is_empty());
        assert!(!f.is_empty());
    }

    #[test]
    fn glyph_names_keep_their_bytes() {
        let n = GlyphName::from("quotesingle");
        assert_eq!(n.as_bytes(), b"quotesingle");
        assert_eq!(n.as_str(), Some("quotesingle"));
        // A name that is not UTF-8 is still a name; it just matches no table.
        let raw = GlyphName::new(vec![0xff, 0xfe]);
        assert_eq!(raw.as_str(), None);
    }

    #[test]
    fn gid_round_trips_through_the_type1_newtype() {
        let g = Gid(42);
        assert_eq!(Gid::from(pdfrum_type1::Gid::from(g)), g);
    }
}
