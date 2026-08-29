//! The small newtypes and flag set every other module is written in terms of.

use std::fmt;

pub use pdfrum_cmap::{CharCode, Cid};

/// A glyph index into a font program.
///
/// Zero is a legitimate value — it is the `.notdef` glyph, which PDFium
/// deliberately distinguishes from "no glyph at all" (that is `None`, the C++'s
/// `-1`). Every ladder in this crate returns `Option<Gid>` for exactly that
/// reason.
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
/// A plain bitfield rather than a `bitflags` dependency: the set is closed and
/// eight predicates are cheaper than a crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct FontFlags(pub u32);

impl FontFlags {
    /// All glyphs have the same width.
    pub const FIXED_PITCH: u32 = 1 << 0;
    /// Glyphs have serifs.
    pub const SERIF: u32 = 1 << 1;
    /// The font uses its own built-in encoding rather than a standard one.
    pub const SYMBOLIC: u32 = 1 << 2;
    /// Glyphs resemble cursive handwriting.
    pub const SCRIPT: u32 = 1 << 3;
    /// The font uses the Adobe standard Latin character set.
    pub const NON_SYMBOLIC: u32 = 1 << 5;
    /// Glyphs have dominant vertical strokes that are slanted.
    pub const ITALIC: u32 = 1 << 6;
    /// No lowercase letters.
    pub const ALL_CAP: u32 = 1 << 16;
    /// Lowercase letters have the shapes of uppercase ones at reduced size.
    pub const SMALL_CAP: u32 = 1 << 17;
    /// Bold glyphs are painted with extra pixels at small sizes.
    pub const FORCE_BOLD: u32 = 1 << 18;
    /// **Not** a PDF flag. PDFium sets this bit when the descriptor carried a
    /// complete enough metric set to be trusted, and the substitution ladder
    /// discards the caller's weight and slant entirely when it is absent
    /// (`docs/design/pdfrum-font.md` §1.2, §1.12 step 0).
    pub const USE_EXTERN_ATTR: u32 = 1 << 19;

    /// The default when a font has no `/FontDescriptor` at all.
    pub const DEFAULT: Self = Self(Self::NON_SYMBOLIC);

    /// Is `bit` (one of the constants above) set?
    #[must_use]
    pub const fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    /// A copy with `bit` set.
    #[must_use]
    pub const fn with(self, bit: u32) -> Self {
        Self(self.0 | bit)
    }

    /// A copy with `bit` cleared.
    #[must_use]
    pub const fn without(self, bit: u32) -> Self {
        Self(self.0 & !bit)
    }

    /// Symbolic fonts use their own encoding vector.
    #[must_use]
    pub const fn is_symbolic(self) -> bool {
        self.has(Self::SYMBOLIC)
    }

    /// Non-symbolic fonts use the Adobe standard Latin set.
    #[must_use]
    pub const fn is_non_symbolic(self) -> bool {
        self.has(Self::NON_SYMBOLIC)
    }

    /// Italic, per the descriptor's own flag rather than its `/ItalicAngle`.
    #[must_use]
    pub const fn is_italic(self) -> bool {
        self.has(Self::ITALIC)
    }

    /// Every glyph the same width.
    #[must_use]
    pub const fn is_fixed_pitch(self) -> bool {
        self.has(Self::FIXED_PITCH)
    }

    /// The descriptor's metrics are complete enough to trust (§1.2).
    #[must_use]
    pub const fn uses_extern_attr(self) -> bool {
        self.has(Self::USE_EXTERN_ATTR)
    }

    /// No lowercase letters — triggers the all-caps glyph aliasing of §1.4.
    #[must_use]
    pub const fn is_all_cap(self) -> bool {
        self.has(Self::ALL_CAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_predicates_read_the_right_bits() {
        let f = FontFlags(FontFlags::SYMBOLIC | FontFlags::ITALIC);
        assert!(f.is_symbolic());
        assert!(f.is_italic());
        assert!(!f.is_non_symbolic());
        assert!(!f.uses_extern_attr());
        assert_eq!(FontFlags::DEFAULT.0, 32);
    }

    #[test]
    fn with_and_without_are_inverses() {
        let f = FontFlags(0).with(FontFlags::ALL_CAP);
        assert!(f.is_all_cap());
        assert!(!f.without(FontFlags::ALL_CAP).is_all_cap());
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
