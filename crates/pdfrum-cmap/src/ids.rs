//! The identifier vocabulary of CID-keyed text: character codes, CIDs, the
//! character collection a CID belongs to, and the two ways a CMap describes
//! how bytes become codes.
//!
//! The ordinals of [`CidSet`] and [`CidCoding`] are load-bearing — the static
//! tables are addressed by `CidSet` and a file's `/Ordering` string resolves
//! through the same numbering — so both are `#[repr(u8)]` with explicit
//! discriminants.

/// A character code: one unit of a PDF string as split by a CMap's decoder
/// (ISO 32000-1 §9.7.5). Between 1 and 4 bytes wide depending on the coding
/// scheme, so the value alone does not say how many bytes it came from; ask
/// [`CMap::char_size`](crate::CMap::char_size).
///
/// ```
/// use pdfrum_cmap::CharCode;
///
/// let code = CharCode(0x8140);
/// assert_eq!(u32::from(code), 0x8140);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CharCode(pub u32);

impl From<CharCode> for u32 {
    fn from(c: CharCode) -> Self {
        c.0
    }
}

impl From<u32> for CharCode {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

/// A character identifier: an index into a character collection, which a
/// `CIDFont` turns into a glyph (ISO 32000-1 §9.7.4). CID 0 is `.notdef` and is
/// also what an unmapped character code yields.
///
/// ```
/// use pdfrum_cmap::Cid;
///
/// assert_eq!(Cid::default(), Cid(0)); // .notdef
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cid(pub u16);

impl From<Cid> for u16 {
    fn from(c: Cid) -> Self {
        c.0
    }
}

impl From<u16> for Cid {
    fn from(v: u16) -> Self {
        Self(v)
    }
}

/// The character collection a CID belongs to — the `/Registry`–`/Ordering`
/// pair of a `/CIDSystemInfo` (ISO 32000-1 §9.7.3), reduced to the five
/// collections that have built-in tables plus "none of them".
///
/// The discriminants index the static blob's registry directory, so they are
/// part of the format, not an implementation detail.
///
/// ```
/// use pdfrum_cmap::{CidSet, charset_from_ordering};
///
/// assert_eq!(charset_from_ordering(b"Japan1"), CidSet::Japan1);
/// assert_eq!(charset_from_ordering(b"Latin1"), CidSet::Unknown);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum CidSet {
    /// No recognised collection: the CMap has no CID table and no CID→Unicode
    /// table.
    #[default]
    Unknown = 0,
    /// Adobe-GB1 — Simplified Chinese.
    Gb1 = 1,
    /// Adobe-CNS1 — Traditional Chinese.
    Cns1 = 2,
    /// Adobe-Japan1 — Japanese.
    Japan1 = 3,
    /// Adobe-Korea1 — Korean.
    Korea1 = 4,
    /// Adobe-Identity / `UCS`: the CID *is* the Unicode scalar value.
    Unicode = 5,
}

impl CidSet {
    /// The registry's ordinal, as the blob and the `/Ordering` table use it.
    #[must_use]
    pub fn ordinal(self) -> u8 {
        self as u8
    }

    /// Index into the blob's four-registry directory, or `None` for the two
    /// collections that have no static tables.
    pub(crate) fn registry_index(self) -> Option<usize> {
        match self {
            Self::Gb1 => Some(0),
            Self::Cns1 => Some(1),
            Self::Japan1 => Some(2),
            Self::Korea1 => Some(3),
            Self::Unknown | Self::Unicode => None,
        }
    }
}

/// How a predefined CMap's character codes relate to a legacy encoding. Purely
/// descriptive at this layer — nothing in this crate branches on it — but
/// `pdfrum-font` uses it to pick a code page when a CID font falls back to a
/// system face, so it is part of the CMap's observable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum CidCoding {
    /// An unrecognised `/Encoding` name, or any embedded CMap.
    #[default]
    Unknown = 0,
    /// GB 2312 / GBK family.
    Gb = 1,
    /// Big5 family.
    Big5 = 2,
    /// Shift-JIS / EUC-JP family.
    Jis = 3,
    /// KS X 1001 / UHC family.
    Korea = 4,
    /// UCS-2 code points.
    Ucs2 = 5,
    /// `Identity-H` / `Identity-V`: the code *is* the CID.
    Cid = 6,
    /// UTF-16 code units.
    Utf16 = 7,
}

/// How a byte string splits into character codes (ISO 32000-1 §9.7.6.2).
///
/// The default is [`TwoBytes`](CodingScheme::TwoBytes), and that default is
/// load-bearing: an unrecognised predefined name and a CMap stream with no
/// usable `codespacerange` both decode as fixed 2-byte codes.
///
/// ```
/// use pdfrum_cmap::CodingScheme;
///
/// assert_eq!(CodingScheme::default(), CodingScheme::TwoBytes);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CodingScheme {
    /// Every byte is its own code.
    OneByte,
    /// Every code is exactly two bytes, big-endian.
    #[default]
    TwoBytes,
    /// A set of leading bytes starts a two-byte code; every other byte is a
    /// one-byte code.
    MixedTwoBytes,
    /// Codespace ranges decide the width of each code, one to four bytes.
    MixedFourBytes,
}

#[cfg(test)]
mod tests {
    use super::{CharCode, Cid, CidCoding, CidSet, CodingScheme};

    #[test]
    fn ordinals_match_the_blob_numbering() {
        assert_eq!(CidSet::Unknown.ordinal(), 0);
        assert_eq!(CidSet::Gb1.ordinal(), 1);
        assert_eq!(CidSet::Cns1.ordinal(), 2);
        assert_eq!(CidSet::Japan1.ordinal(), 3);
        assert_eq!(CidSet::Korea1.ordinal(), 4);
        assert_eq!(CidSet::Unicode.ordinal(), 5);
        assert_eq!(CidCoding::Unknown as u8, 0);
        assert_eq!(CidCoding::Utf16 as u8, 7);
    }

    #[test]
    fn only_the_four_cjk_registries_have_a_blob_index() {
        assert_eq!(CidSet::Gb1.registry_index(), Some(0));
        assert_eq!(CidSet::Korea1.registry_index(), Some(3));
        assert_eq!(CidSet::Unicode.registry_index(), None);
        assert_eq!(CidSet::Unknown.registry_index(), None);
    }

    #[test]
    fn defaults_are_the_fallback_state() {
        assert_eq!(CodingScheme::default(), CodingScheme::TwoBytes);
        assert_eq!(CidSet::default(), CidSet::Unknown);
        assert_eq!(CidCoding::default(), CidCoding::Unknown);
        assert_eq!(CharCode::default().0, 0);
        assert_eq!(Cid::default().0, 0);
    }
}
