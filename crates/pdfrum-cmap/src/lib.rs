//! CMaps: how a PDF string becomes character codes, and how those codes
//! become CIDs (ISO 32000-1 §9.7.5).
//!
//! A [`CMap`] fuses two jobs that PDF keeps in one object. It is a
//! **byte decoder** — the rule that splits a show operator's string into
//! character codes, which may be one, two or four bytes wide and may mix
//! widths within a single string — and it is a **charcode→CID map**, turning
//! each code into an index into a character collection that a `CIDFont` can draw.
//!
//! There are two ways a font names one, and they share nothing but their
//! observable surface:
//!
//! - **predefined**: `/Encoding /GB-EUC-H` selects one of 32 built-in CJK
//!   CMaps (plus `Identity-H` and `Identity-V`), whose tables ship inside this
//!   crate. [`from_encoding_name`].
//! - **embedded**: `/Encoding` is a stream holding a CMap program, which
//!   [`parse_embedded`] reads.
//!
//! ```
//! use pdfrum_cmap::{CharCode, Cid, CodingScheme, from_encoding_name};
//! use pdfrum_common::Diagnostics;
//! use pdfrum_object::Name;
//!
//! let mut diags = Diagnostics::default();
//! let cmap = from_encoding_name(&Name::from("Identity-H"), &mut diags);
//!
//! // Identity-H reads fixed two-byte codes and maps each to itself.
//! assert_eq!(cmap.coding_scheme(), CodingScheme::TwoBytes);
//! let decoded: Vec<(CharCode, Cid)> = cmap.decode(&[0x00, 0x41, 0x30, 0x42]).collect();
//! assert_eq!(decoded, vec![
//!     (CharCode(0x0041), Cid(0x0041)),
//!     (CharCode(0x3042), Cid(0x3042)),
//! ]);
//! ```
//!
//! # Damage tolerance
//!
//! Nothing in this crate refuses to work. An unrecognised `/Encoding` name
//! yields a CMap that decodes two-byte codes and maps each code to itself; a
//! truncated multi-byte code yields character code 0; a CMap program full of
//! garbage yields whatever it managed to say. Every such recovery is recorded
//! on a [`Diagnostics`](pdfrum_common::Diagnostics) sink rather than raised, so
//! a caller can see what was bent without having to handle it.
//!
//! # What is *not* here
//!
//! `usecmap` inside an embedded CMap program is recognised and ignored — a
//! program that inherits a predefined base and overrides a few codes gets only
//! its overrides (SPEC.md §6). The inheritance that *is* implemented is the
//! built-in tables' own chaining, which is what makes `GB-EUC-V` a thin
//! override of `GB-EUC-H`.

#![forbid(unsafe_code)]
// Every byte reaching this crate came from an untrusted file or a generated
// blob: index with `get()`.
#![warn(clippy::indexing_slicing)]
// Narrowing casts are *behavior* in this crate, not accidents. A CID is a
// 16-bit value and a CMap program declaring `<10000>` means CID 0; a character
// code is written out one byte at a time, each one the low bits of a wider
// value. Every such cast below is deliberate and pinned by a test, so the
// lint is off crate-wide rather than annotated a few dozen times.
#![allow(clippy::cast_possible_truncation)]

mod blob;
mod cid2unicode;
mod decode;
mod error;
mod ids;
pub mod lexer;
mod parser;
mod predefined;
mod static_lookup;

pub use error::Error;
pub use ids::{CharCode, Cid, CidCoding, CidSet, CodingScheme};

use decode::Decoder;
use parser::{CidRange, DirectTable};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::Name;

/// Where a CMap's CIDs come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CidMap {
    /// The character code *is* the CID. Both `Identity-H`/`-V` and the
    /// fallback for a name that resolved to nothing behave this way, and they
    /// are indistinguishable once built.
    Identity,
    /// One entry of a built-in registry's table, plus whatever its chain
    /// defers to.
    Static { registry: usize, index: usize },
    /// A CMap program's own tables: a dense array for codes below `0x1_0000`
    /// and a sorted list for the rest.
    Embedded {
        direct: DirectTable,
        additional: Vec<CidRange>,
    },
}

/// A CMap: a byte decoder plus a charcode→CID map (ISO 32000-1 §9.7.5).
///
/// Build one with [`from_encoding_name`], [`predefined`] or
/// [`parse_embedded`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CMap {
    decoder: Decoder,
    map: CidMap,
    vertical: bool,
    loaded: bool,
    charset: CidSet,
    coding: CidCoding,
}

impl CMap {
    /// The CMap a file gets when its `/Encoding` names nothing recognisable:
    /// fixed two-byte codes, every code its own CID, no character collection.
    fn unrecognized(vertical: bool) -> Self {
        Self {
            decoder: Decoder::TwoBytes,
            map: CidMap::Identity,
            vertical,
            loaded: false,
            charset: CidSet::Unknown,
            coding: CidCoding::Unknown,
        }
    }

    /// Decode a string into `(character code, CID)` pairs — the entry point
    /// every text path shares.
    ///
    /// Iteration ends when the string is exhausted. Damage never stops it
    /// early: a code no codespace range accepts comes back as `CharCode(0)`,
    /// and a code cut off by the end of the string does too.
    ///
    /// ```
    /// use pdfrum_cmap::{CharCode, Cid, from_encoding_name};
    /// use pdfrum_common::Diagnostics;
    /// use pdfrum_object::Name;
    ///
    /// let mut diags = Diagnostics::default();
    /// let cmap = from_encoding_name(&Name::from("GB-EUC-H"), &mut diags);
    ///
    /// // 0x41 is not a lead byte, so it is a one-byte code; 0xA1 0xA1 is a pair.
    /// let codes: Vec<CharCode> = cmap.decode(&[0x41, 0xA1, 0xA1]).map(|(c, _)| c).collect();
    /// assert_eq!(codes, vec![CharCode(0x41), CharCode(0xA1A1)]);
    /// assert_eq!(cmap.cid(CharCode(0xA1A1)), Cid(0x0060));
    /// ```
    pub fn decode<'a>(&'a self, bytes: &'a [u8]) -> impl Iterator<Item = (CharCode, Cid)> + 'a {
        let mut offset = 0usize;
        std::iter::from_fn(move || {
            if offset >= bytes.len() {
                return None;
            }
            let before = offset;
            let code = self.next_char(bytes, &mut offset);
            // A decoder that consumed nothing would loop forever; no scheme
            // does, but the guard makes that a property of this loop rather
            // than of every decoder arm.
            if offset == before {
                return None;
            }
            Some((code, self.cid(code)))
        })
    }

    /// Decode one character code, advancing `offset`.
    ///
    /// Returns `CharCode(0)` rather than failing when the bytes are damaged.
    /// `offset` may land past a truncated code's start without reaching the
    /// end of the string, which is how iteration terminates.
    pub fn next_char(&self, bytes: &[u8], offset: &mut usize) -> CharCode {
        decode::next_char(&self.decoder, bytes, offset)
    }

    /// The CID a character code maps to. Unmapped codes give [`Cid(0)`](Cid),
    /// which is `.notdef`.
    #[must_use]
    pub fn cid(&self, code: CharCode) -> Cid {
        let charcode = code.0;
        Cid(match &self.map {
            CidMap::Identity => charcode as u16,
            CidMap::Static { registry, index } => {
                static_lookup::cid_from_charcode(*registry, *index, charcode)
            }
            CidMap::Embedded { direct, additional } => direct
                .get(charcode)
                .unwrap_or_else(|| lookup_additional(additional, charcode)),
        })
    }

    /// The character code that maps to `cid`, or [`CharCode(0)`](CharCode)
    /// when none does.
    ///
    /// Defined only for a predefined CMap backed by the built-in tables; every
    /// other kind returns 0. Even for those it is partial: the scan does not
    /// consult the four-byte tables, so a CID that exists only above
    /// `0x1_0000` is unreachable in reverse. This is used by CID fonts to find
    /// a code for a character they were asked to draw by Unicode.
    #[must_use]
    pub fn charcode_from_cid(&self, cid: Cid) -> CharCode {
        CharCode(match &self.map {
            CidMap::Static { registry, index } => {
                static_lookup::charcode_from_cid(*registry, *index, cid.0)
            }
            CidMap::Identity | CidMap::Embedded { .. } => 0,
        })
    }

    /// How many bytes a code of this value occupies.
    ///
    /// Derived from the value alone, not from the bytes it was decoded from
    /// and not from the codespace ranges. That makes it disagree with
    /// [`append_char`](CMap::append_char) in exactly one case: under a
    /// mixed-two-byte scheme a code below `0x100` whose value is itself a
    /// lead byte reports width 1 but is *written* as two bytes. Code that
    /// needs the encoded width should measure what `append_char` produced.
    #[must_use]
    pub fn char_size(&self, code: CharCode) -> u8 {
        decode::char_size(&self.decoder, code)
    }

    /// How many character codes a byte string holds. Always equal to the
    /// number of pairs [`decode`](CMap::decode) yields for the same string.
    #[must_use]
    pub fn count_chars(&self, bytes: &[u8]) -> usize {
        decode::count_chars(&self.decoder, bytes)
    }

    /// Append a character code to a byte string in this CMap's encoding — the
    /// inverse of [`next_char`](CMap::next_char).
    pub fn append_char(&self, out: &mut Vec<u8>, code: CharCode) {
        decode::append_char(&self.decoder, out, code);
    }

    /// Whether this CMap selects vertical writing mode (ISO 32000-1 §9.7.4.3).
    ///
    /// Decided by the last byte of the name as written, before any suffix
    /// handling, so `Identity-V` is vertical and so is any name ending in `V`.
    #[must_use]
    pub fn is_vertical(&self) -> bool {
        self.vertical
    }

    /// Whether the name resolved to real tables.
    ///
    /// `false` covers two different failures that behave the same way: a name
    /// matching no row at all, and a name whose row was found but whose
    /// built-in table was not. Either way the CMap still decodes and still
    /// maps codes; it just maps them to themselves.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// The legacy encoding family this CMap's codes belong to.
    #[must_use]
    pub fn coding(&self) -> CidCoding {
        self.coding
    }

    /// The character collection its CIDs index.
    #[must_use]
    pub fn charset(&self) -> CidSet {
        self.charset
    }

    /// How its bytes split into character codes.
    #[must_use]
    pub fn coding_scheme(&self) -> CodingScheme {
        self.decoder.scheme()
    }

    /// Whether this CMap has no dense charcode→CID table — true for every
    /// predefined CMap and false for every embedded one, whatever the program
    /// contained. CID fonts branch on it when choosing how to find a glyph.
    #[must_use]
    pub fn has_no_direct_table(&self) -> bool {
        !matches!(self.map, CidMap::Embedded { .. })
    }

    /// Whether this CMap resolved to one of the built-in static tables.
    #[must_use]
    pub fn has_static_map(&self) -> bool {
        matches!(self.map, CidMap::Static { .. })
    }
}

/// Binary search of the wide-code ranges an embedded CMap declared.
fn lookup_additional(ranges: &[CidRange], charcode: u32) -> u16 {
    let at = ranges.partition_point(|r| r.end_code < charcode);
    match ranges.get(at) {
        Some(r) if r.start_code <= charcode => {
            (u32::from(r.start_cid) + charcode - r.start_code) as u16
        }
        _ => 0,
    }
}

/// Look up one of the built-in CMap names.
///
/// Returns `None` for a name that matches no row and is not `Identity-H` or
/// `Identity-V`. A caller resolving a font's `/Encoding` almost always wants
/// [`from_encoding_name`] instead, which builds the same fallback the oracle
/// does rather than reporting the miss.
///
/// ```
/// use pdfrum_cmap::{CidSet, CodingScheme, predefined};
/// use pdfrum_object::Name;
///
/// let gb = predefined(&Name::from("GB-EUC-H")).unwrap();
/// assert_eq!(gb.charset(), CidSet::Gb1);
/// assert_eq!(gb.coding_scheme(), CodingScheme::MixedTwoBytes);
/// assert!(gb.is_loaded());
///
/// assert!(predefined(&Name::from("Nonsense")).is_none());
/// ```
#[must_use]
pub fn predefined(name: &Name) -> Option<CMap> {
    let raw = predefined::strip_slash(name.as_bytes());
    let vertical = predefined::is_vertical(raw);
    if predefined::is_identity(raw) {
        return Some(CMap {
            decoder: Decoder::TwoBytes,
            map: CidMap::Identity,
            vertical,
            loaded: true,
            charset: CidSet::Unknown,
            coding: CidCoding::Cid,
        });
    }
    let row = predefined::resolve(raw)?;
    let decoder = match (row.scheme, row.leading) {
        (CodingScheme::MixedTwoBytes, Some(leading)) => Decoder::MixedTwoBytes { leading },
        (CodingScheme::OneByte, _) => Decoder::OneByte,
        (CodingScheme::MixedFourBytes, _) => Decoder::MixedFourBytes { ranges: Vec::new() },
        _ => Decoder::TwoBytes,
    };
    // The static table is keyed by the *full* name, not the truncated stem, so
    // a name that found a decoder row can still find no table — a real,
    // reachable state with a correct decoder and no CID map.
    let table = row
        .charset
        .registry_index()
        .and_then(|reg| static_lookup::find(reg, raw).map(|index| (reg, index)));
    let (map, loaded) = match table {
        Some((registry, index)) => (CidMap::Static { registry, index }, true),
        None => (CidMap::Identity, false),
    };
    Some(CMap {
        decoder,
        map,
        vertical,
        loaded,
        charset: row.charset,
        coding: row.coding,
    })
}

/// Resolve a font's `/Encoding` name to a CMap, failures included.
///
/// Never fails. A name that resolves to nothing produces a CMap that decodes
/// fixed two-byte codes and maps every code to itself, and records a
/// diagnostic — that is what the oracle does, and a file with
/// `/Encoding /Nonsense` renders because of it.
///
/// ```
/// use pdfrum_cmap::{CharCode, Cid, CidSet, CodingScheme, from_encoding_name};
/// use pdfrum_common::Diagnostics;
/// use pdfrum_object::Name;
///
/// let mut diags = Diagnostics::default();
/// let cmap = from_encoding_name(&Name::from("Nonsense"), &mut diags);
///
/// assert!(!cmap.is_loaded());
/// assert_eq!(cmap.charset(), CidSet::Unknown);
/// assert_eq!(cmap.coding_scheme(), CodingScheme::TwoBytes);
/// assert_eq!(cmap.cid(CharCode(0x1234)), Cid(0x1234));  // identity fallback
/// assert_eq!(diags.len(), 1);
/// ```
#[must_use]
pub fn from_encoding_name(name: &Name, diags: &mut Diagnostics) -> CMap {
    let raw = predefined::strip_slash(name.as_bytes());
    let Some(cmap) = predefined(name) else {
        diags.record(Severity::Suspicious, DiagKind::CMapNameUnknown, None);
        return CMap::unrecognized(predefined::is_vertical(raw));
    };
    if !cmap.is_loaded() {
        diags.record(Severity::Suspicious, DiagKind::CMapTableMissing, None);
    }
    cmap
}

/// Read an embedded CMap program — the decoded bytes of an `/Encoding` stream.
///
/// Never fails; damage is recorded on `diags`. `limits` caps how many
/// codespace and wide-code ranges one program may declare, which the oracle
/// leaves unbounded.
///
/// ```
/// use pdfrum_cmap::{CharCode, Cid, CodingScheme, parse_embedded};
/// use pdfrum_common::{Diagnostics, Limits};
///
/// let program = b"
///     begincodespacerange <0000> <ffff> endcodespacerange
///     1 begincidrange <0020> <007e> <0001> endcidrange
/// ";
/// let mut diags = Diagnostics::default();
/// let cmap = parse_embedded(program, &Limits::default(), &mut diags);
///
/// assert_eq!(cmap.coding_scheme(), CodingScheme::TwoBytes);
/// assert_eq!(cmap.cid(CharCode(0x20)), Cid(1));
/// assert_eq!(cmap.cid(CharCode(0x7e)), Cid(0x5F));
/// assert_eq!(cmap.cid(CharCode(0x7f)), Cid(0));   // outside the range
/// ```
#[must_use]
pub fn parse_embedded(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> CMap {
    let parsed = parser::parse(bytes, limits, diags);
    CMap {
        decoder: parsed.decoder,
        map: CidMap::Embedded {
            direct: parsed.direct,
            additional: parsed.additional,
        },
        vertical: parsed.vertical,
        loaded: true,
        charset: parsed.charset,
        coding: CidCoding::Unknown,
    }
}

/// The Unicode scalar a character collection assigns to a CID
/// (ISO 32000-1 §9.10.2), or `None` when it assigns none.
///
/// [`CidSet::Unicode`] is the identity map. CID 0 is U+FFFD in every built-in
/// table, so an unmapped character code extracts as the replacement character.
///
/// ```
/// use pdfrum_cmap::{Cid, CidSet, unicode_from_cid};
///
/// assert_eq!(unicode_from_cid(CidSet::Gb1, Cid(0)), Some('\u{FFFD}'));
/// assert_eq!(unicode_from_cid(CidSet::Gb1, Cid(34)), Some('A'));
/// assert_eq!(unicode_from_cid(CidSet::Unicode, Cid(0x41)), Some('A'));
/// assert_eq!(unicode_from_cid(CidSet::Unknown, Cid(1)), None);
/// ```
#[must_use]
pub fn unicode_from_cid(set: CidSet, cid: Cid) -> Option<char> {
    cid2unicode::unicode_from_cid(set, cid)
}

/// Whether a character collection has a built-in CID→Unicode table. Only the
/// four CJK collections do.
#[must_use]
pub fn has_cid2unicode(set: CidSet) -> bool {
    cid2unicode::has_table(set)
}

/// The character code that would draw `unicode` through this CMap, or
/// [`CharCode(0)`](CharCode) when none would.
///
/// A linear scan of the collection's whole CID→Unicode table looking for a
/// match, then a reverse table lookup — expensive, and only used when a CID
/// font is asked to draw a character it can only identify by Unicode.
///
/// ```
/// use pdfrum_cmap::{CharCode, charcode_from_unicode, predefined};
/// use pdfrum_object::Name;
///
/// let gb = predefined(&Name::from("GB-EUC-H")).unwrap();
/// // U+0020 is CID 0x1E24 in Adobe-GB1, which GB-EUC-H reaches from code 0x20.
/// assert_eq!(charcode_from_unicode(&gb, ' '), CharCode(0x20));
/// ```
#[must_use]
pub fn charcode_from_unicode(cmap: &CMap, unicode: char) -> CharCode {
    let Some(reg) = cmap.charset.registry_index() else {
        return CharCode(0);
    };
    let want = u32::from(unicode);
    let len = blob::cid2unicode_len(reg);
    for cid in 0..len {
        let Ok(cid) = u16::try_from(cid) else { break };
        if blob::cid2unicode(reg, cid).map(u32::from) == Some(want) {
            let code = cmap.charcode_from_cid(Cid(cid));
            if code.0 != 0 {
                return code;
            }
        }
    }
    CharCode(0)
}

/// The character collection a `/CIDSystemInfo`'s `/Ordering` names
/// (ISO 32000-1 §9.7.3).
///
/// The five recognised spellings are `GB1`, `CNS1`, `Japan1`, `Korea1` and
/// `UCS` — note `UCS`, not `UCS2` and not `Identity`. Anything else is
/// [`CidSet::Unknown`].
///
/// ```
/// use pdfrum_cmap::{CidSet, charset_from_ordering};
///
/// assert_eq!(charset_from_ordering(b"Korea1"), CidSet::Korea1);
/// assert_eq!(charset_from_ordering(b"UCS"), CidSet::Unicode);
/// assert_eq!(charset_from_ordering(b"UCS2"), CidSet::Unknown);
/// assert_eq!(charset_from_ordering(b"Identity"), CidSet::Unknown);
/// ```
#[must_use]
pub fn charset_from_ordering(ordering: &[u8]) -> CidSet {
    match ordering {
        b"GB1" => CidSet::Gb1,
        b"CNS1" => CidSet::Cns1,
        b"Japan1" => CidSet::Japan1,
        b"Korea1" => CidSet::Korea1,
        b"UCS" => CidSet::Unicode,
        _ => CidSet::Unknown,
    }
}

#[cfg(test)]
mod tests;
