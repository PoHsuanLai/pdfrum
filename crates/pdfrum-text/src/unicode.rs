//! Character properties, read from the committed table blob.
//!
//! Six lookups, each one a place `core/fpdftext/` reaches into ICU or into a
//! static table, and each one Tier-A load-bearing:
//!
//! | Lookup | Used by |
//! |---|---|
//! | [`bidi_class`] | the four-way segmenter ([`crate::bidi`]) |
//! | [`mirror_char`] | RTL normalization in `AddCharInfo` |
//! | [`normalize`] | the same, plus the `U+FB00..=U+FB06` ligature band |
//! | [`is_alpha`] / [`is_alnum`] | hyphen look-back and mail-link scanning |
//! | [`to_lower`] | case-insensitive search and web-link scanning |
//!
//! The blob's provenance — and why this is not a Unicode crate — is
//! `tables/PROVENANCE.md`. Above `U+FFFF` the two BMP-indexed tables (bidi
//! class and normalization) return their defaults, exactly as the C++'s
//! bounds check and `wch & 0xFFFF` mask do; the three ICU predicates cover
//! the whole code space, because the C++ hands ICU a 32-bit `wchar_t`.

/// The committed blob. See `tables/PROVENANCE.md`.
static BLOB: &[u8] = include_bytes!("../tables/unicode.bin");

/// The nineteen bidi classes PDFium distinguishes
/// (`core/fxcrt/fx_unicode.h`), in its own numbering.
///
/// Not the Unicode Bidirectional Algorithm's categories used for embedding
/// levels — PDFium only ever buckets these four ways (see
/// [`Direction`](crate::bidi::Direction)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BidiClass {
    /// Other neutral, and the value every unlisted code point takes.
    On,
    /// Left-to-right letter.
    L,
    /// Right-to-left letter.
    R,
    /// Arabic number.
    An,
    /// European number.
    En,
    /// Arabic letter.
    Al,
    /// Non-spacing mark.
    Nsm,
    /// Common number separator.
    Cs,
    /// European separator.
    Es,
    /// European number terminator.
    Et,
    /// Boundary neutral.
    Bn,
    /// Segment separator.
    S,
    /// Whitespace.
    Ws,
    /// Paragraph separator.
    B,
    /// Right-to-left override.
    Rlo,
    /// Right-to-left embedding.
    Rle,
    /// Left-to-right override.
    Lro,
    /// Left-to-right embedding.
    Lre,
    /// Pop directional format.
    Pdf,
}

impl BidiClass {
    /// The class stored as `value` in the packed property word, or
    /// [`BidiClass::On`] for a value the table cannot hold.
    fn from_packed(value: u16) -> Self {
        match value {
            1 => Self::L,
            2 => Self::R,
            3 => Self::An,
            4 => Self::En,
            5 => Self::Al,
            6 => Self::Nsm,
            7 => Self::Cs,
            8 => Self::Es,
            9 => Self::Et,
            10 => Self::Bn,
            11 => Self::S,
            12 => Self::Ws,
            13 => Self::B,
            14 => Self::Rlo,
            15 => Self::Rle,
            16 => Self::Lro,
            17 => Self::Lre,
            18 => Self::Pdf,
            _ => Self::On,
        }
    }
}

/// Locates one tagged section's payload in the blob.
///
/// Returns an empty slice for a blob that does not parse, which turns every
/// lookup into its default rather than into a panic — the tables are
/// committed and tested, so this is belt-and-braces, not a recovery path.
fn section(tag: [u8; 4]) -> &'static [u8] {
    let Some(count) = BLOB.get(0..8).filter(|head| head.starts_with(b"PDRT")) else {
        return &[];
    };
    let Some(count) = count.get(4..8).and_then(|b| b.try_into().ok()) else {
        return &[];
    };
    let count = u32::from_le_bytes(count) as usize;
    let mut at = 8usize;
    for _ in 0..count {
        let Some(header) = BLOB.get(at..at + 8) else {
            return &[];
        };
        let Some(len) = header.get(4..8).and_then(|b| b.try_into().ok()) else {
            return &[];
        };
        let len = u32::from_le_bytes(len) as usize;
        let body = at + 8;
        if header.starts_with(&tag) {
            return BLOB.get(body..body + len).unwrap_or(&[]);
        }
        at = body + len;
    }
    &[]
}

/// A run-length-encoded 65 536-entry `u16` table, expanded once on first use.
///
/// Expansion costs 128 KiB of heap per table and turns every lookup into one
/// indexed read. Walking the runs instead would put a binary search — and its
/// cumulative-length arithmetic — on the path of every character of every
/// page, which on a CJK document is millions of probes. The two tables are
/// immutable, code-point-indexed and read constantly: this is exactly the
/// lazy cache STYLE.md §2 sanctions.
struct RleTable {
    payload: &'static [u8],
    expanded: std::sync::OnceLock<Box<[u16]>>,
}

/// The `index`-th little-endian `u16` of a slice.
fn word_at(bytes: &[u8], index: usize) -> Option<u16> {
    let at = index.checked_mul(2)?;
    bytes
        .get(at..at + 2)
        .and_then(|pair| pair.try_into().ok())
        .map(u16::from_le_bytes)
}

impl RleTable {
    const fn new(payload: &'static [u8]) -> Self {
        Self {
            payload,
            expanded: std::sync::OnceLock::new(),
        }
    }

    fn get(&self, index: usize) -> u16 {
        let table = self.expanded.get_or_init(|| {
            let mut out = Vec::with_capacity(0x1_0000);
            for run in self.payload.chunks_exact(4) {
                let (Some(value), Some(len)) = (word_at(run, 0), word_at(run, 1)) else {
                    continue;
                };
                out.extend(std::iter::repeat_n(value, usize::from(len)));
            }
            out.into_boxed_slice()
        });
        table.get(index).copied().unwrap_or(0)
    }
}

/// The packed `(mirror << 5) | bidi_class` property word for a BMP code
/// point; `0` above the BMP, which is `kON` — and, because `0 >> 5` is a
/// *valid* mirror index rather than the sentinel, a mirror of `)`. That is
/// the C++'s behaviour too (`GetUnicodeProperties` bounds-checks to `0`, and
/// `GetMirrorChar` then reads pair zero), and [`mirror_char`]'s tests pin it.
fn properties(code: u32) -> u16 {
    if code > 0xFFFF {
        return 0;
    }
    ucd_table().get(code as usize)
}

fn ucd_table() -> &'static RleTable {
    static TABLE: std::sync::OnceLock<RleTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| RleTable::new(section(*b"UCDR")))
}

fn normalization_table() -> &'static RleTable {
    static TABLE: std::sync::OnceLock<RleTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| RleTable::new(section(*b"NRMR")))
}

/// The bidi class PDFium assigns a code point (`GetBidiClass`).
///
/// ```
/// use pdfrum_text::unicode::{BidiClass, bidi_class};
///
/// assert_eq!(bidi_class('A' as u32), BidiClass::L);
/// assert_eq!(bidi_class(0x05D0), BidiClass::R);  // HEBREW ALEF
/// assert_eq!(bidi_class(0x0627), BidiClass::Al); // ARABIC ALEF
/// assert_eq!(bidi_class('(' as u32), BidiClass::On);
/// // Above the BMP the table is not consulted at all.
/// assert_eq!(bidi_class(0x1_0000), BidiClass::On);
/// ```
#[must_use]
pub fn bidi_class(code: u32) -> BidiClass {
    BidiClass::from_packed(properties(code) & 0x1F)
}

/// The character a code point mirrors to in a right-to-left run, or the code
/// point itself (`GetMirrorChar`).
///
/// ```
/// use pdfrum_text::unicode::mirror_char;
///
/// assert_eq!(mirror_char('(' as u32), ')' as u32);
/// assert_eq!(mirror_char('[' as u32), ']' as u32);
/// assert_eq!(mirror_char('a' as u32), 'a' as u32);
/// // `[oracle-bug]` Above the BMP nothing mirrors.
/// assert_eq!(mirror_char(0x10800), 0x10800);
/// ```
#[must_use]
pub fn mirror_char(code: u32) -> u32 {
    // `[oracle-bug]` A supplementary code point is its own mirror.
    // `fx_unicode.cpp:48` returns **`0`** for `wch >= 0x10000`, but the
    // in-table "no mirror" value is `0x1FF` (`kMirrorMax`, `:27`), so
    // `GetMirrorChar`'s sentinel test at `:147` misses and index `0` is read
    // instead — and `kFXTextLayoutBidiMirror[0] == 0x0029` (`:87-88`), the
    // pair belonging to `'('` (`fx_ucddata.inc:41`). Every character above
    // `U+FFFF` therefore mirrors to `')'`. UAX #9 rule L4 mirrors only
    // characters *possessing* `Bidi_Mirrored`, and `BidiMirroring.txt` has no
    // mappings above the BMP at all, so the identity is the whole answer;
    // pdf.js declines to mirror even in the BMP, with the reason at
    // `bidi.js:441` ("characters are already mirrored in the pdf").
    if code > 0xFFFF {
        return code;
    }
    let index = usize::from(properties(code) >> 5);
    // 0x1FF is the "no mirror" sentinel.
    if index == 0x1FF {
        return code;
    }
    let pairs = section(*b"MIRR");
    let at = index * 2;
    pairs
        .get(at..at + 2)
        .and_then(|b| b.try_into().ok())
        .map_or(code, |b| u32::from(u16::from_le_bytes(b)))
}

/// The normalization of a code point (`GetUnicodeNormalization`), as one or
/// more code points.
///
/// Applied only inside right-to-left runs and to the `U+FB00..=U+FB06` Latin
/// ligature band (`AddCharInfo`), never to text at large. The index table is
/// `wch & 0xFFFF`-indexed in the C++, so a supplementary-plane code point
/// reads a *BMP* entry — reproduced here, mask included.
///
/// ```
/// use pdfrum_text::unicode::normalize;
///
/// // LATIN SMALL LIGATURE FI decomposes.
/// assert_eq!(normalize(0xFB01), vec![u32::from(b'f'), u32::from(b'i')]);
/// // An unlisted code point is itself.
/// assert_eq!(normalize(u32::from(b'a')), vec![u32::from(b'a')]);
/// ```
#[must_use]
pub fn normalize(code: u32) -> Vec<u32> {
    let code = code & 0xFFFF;
    let found = normalization_table().get(code as usize);
    if found == 0 {
        return vec![code];
    }
    if found >= 0x8000 {
        let index = usize::from(found - 0x8000);
        return match word_at(section(*b"NRM1"), index) {
            Some(value) => vec![u32::from(value)],
            None => vec![code],
        };
    }
    let index = usize::from(found & 0x0FFF);
    let table = found >> 12;
    let payload = match table {
        2 => section(*b"NRM2"),
        3 => section(*b"NRM3"),
        4 => section(*b"NRM4"),
        _ => return vec![code],
    };
    // Tables 2 and 3 hold fixed-length runs; table 4 is length-prefixed.
    let (start, len) = if table == 4 {
        let Some(len) = word_at(payload, index) else {
            return vec![code];
        };
        (index + 1, usize::from(len))
    } else {
        (index, usize::from(table))
    };
    let mut out = Vec::with_capacity(len);
    for offset in 0..len {
        match word_at(payload, start + offset) {
            Some(value) => out.push(u32::from(value)),
            None => return vec![code],
        }
    }
    if out.is_empty() { vec![code] } else { out }
}

/// Whether a sorted, disjoint `(start, end)` range table contains a code
/// point.
fn in_ranges(payload: &[u8], code: u32) -> bool {
    let rows = payload.len() / 8;
    let bound = |i: usize, half: usize| -> u32 {
        let at = i * 8 + half * 4;
        payload
            .get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .map_or(u32::MAX, u32::from_le_bytes)
    };
    let (mut lo, mut hi) = (0usize, rows);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if code < bound(mid, 0) {
            hi = mid;
        } else if code > bound(mid, 1) {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// General category `L*` — ICU's `u_isalpha`, which `IsHyphen` consults.
///
/// ```
/// use pdfrum_text::unicode::is_alpha;
///
/// assert!(is_alpha(u32::from(b'a')));
/// assert!(is_alpha(0x4E00));      // CJK ideograph
/// assert!(!is_alpha(u32::from(b'0')));
/// assert!(!is_alpha(u32::from(b'-')));
/// ```
#[must_use]
pub fn is_alpha(code: u32) -> bool {
    in_ranges(section(*b"ALPH"), code)
}

/// General category `L*` or `Nd` — ICU's `u_isalnum`, which `IsHyphen` and
/// `CheckMailLink` consult.
///
/// ```
/// use pdfrum_text::unicode::is_alnum;
///
/// assert!(is_alnum(u32::from(b'z')));
/// assert!(is_alnum(u32::from(b'7')));
/// assert!(is_alnum(0x0660));      // ARABIC-INDIC DIGIT ZERO, not alphabetic
/// assert!(!is_alnum(u32::from(b'@')));
/// ```
#[must_use]
pub fn is_alnum(code: u32) -> bool {
    in_ranges(section(*b"ALNM"), code)
}

/// Simple Unicode lowercase — ICU's `u_tolower`, which is what the search
/// engine's `MakeLower` and `CheckWebLink` apply.
///
/// Simple, not full: one code point in, one out, so a needle and a haystack
/// lowercased this way keep their lengths and their offsets stay comparable.
///
/// ```
/// use pdfrum_text::unicode::to_lower;
///
/// assert_eq!(to_lower(u32::from(b'A')), u32::from(b'a'));
/// assert_eq!(to_lower(0x0102), 0x0103);      // LATIN CAPITAL A WITH BREVE
/// assert_eq!(to_lower(0x1_0400), 0x1_0428);  // DESERET, above the BMP
/// assert_eq!(to_lower(u32::from(b'!')), u32::from(b'!'));
/// ```
#[must_use]
pub fn to_lower(code: u32) -> u32 {
    let payload = section(*b"LOWR");
    let rows = payload.len() / 12;
    let field = |i: usize, which: usize| -> u32 {
        let at = i * 12 + which * 4;
        payload
            .get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u32::from_le_bytes)
    };
    let (mut lo, mut hi) = (0usize, rows);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if code < field(mid, 0) {
            hi = mid;
        } else if code > field(mid, 1) {
            lo = mid + 1;
        } else {
            #[expect(
                clippy::cast_possible_wrap,
                reason = "the delta was written from an i32"
            )]
            let delta = field(mid, 2) as i32;
            return u32::try_from(i64::from(code) + i64::from(delta)).unwrap_or(code);
        }
    }
    code
}

/// ASCII `0`–`9` only — `FXSYS_IsDecimalDigit`, which is *not* `u_isdigit`:
/// the C++ masks off everything but the low seven bits first, so an
/// Arabic-Indic digit is not a decimal digit here.
#[must_use]
pub fn is_decimal_digit(code: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&code)
}

/// The C-locale `isprint` on a narrowed code point: `0x20..=0x7E`.
///
/// Called only under a `code <= 0x80` guard in the C++, where `0x80` itself
/// reaches `isprint` out of `unsigned char` range and comes back false on
/// glibc — so the range stops at `0x7E` and `0x80` is not printable.
#[must_use]
pub fn is_print(code: u32) -> bool {
    (0x20..=0x7E).contains(&code)
}

/// Case-folds a string the way the search engine does: simple lowercase,
/// code point by code point.
#[must_use]
pub fn lower_string(text: &str) -> String {
    text.chars()
        .map(|ch| char::from_u32(to_lower(u32::from(ch))).unwrap_or(ch))
        .collect()
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;

    #[test]
    fn the_blob_parses_and_every_section_is_present() {
        for tag in [
            *b"UCDR", *b"MIRR", *b"NRMR", *b"NRM1", *b"NRM2", *b"NRM3", *b"NRM4", *b"ALPH",
            *b"ALNM", *b"LOWR",
        ] {
            assert!(
                !section(tag).is_empty(),
                "section {} is missing",
                String::from_utf8_lossy(&tag)
            );
        }
    }

    #[test]
    fn the_property_table_covers_the_whole_bmp() {
        // The run lengths must sum to exactly 65536, or every lookup past the
        // gap silently returns kON.
        let payload = section(*b"UCDR");
        let total: usize = payload
            .chunks_exact(4)
            .map(|run| usize::from(u16::from_le_bytes([run[2], run[3]])))
            .sum();
        assert_eq!(total, 65536);
        let norm: usize = section(*b"NRMR")
            .chunks_exact(4)
            .map(|run| usize::from(u16::from_le_bytes([run[2], run[3]])))
            .sum();
        assert_eq!(norm, 65536);
    }

    #[test]
    fn bidi_classes_match_the_oracle_table() {
        // Spot values a reviewer can check against fx_ucddata.inc.
        assert_eq!(bidi_class(0x0000), BidiClass::Bn);
        assert_eq!(bidi_class(0x0009), BidiClass::S);
        assert_eq!(bidi_class(0x000A), BidiClass::B);
        assert_eq!(bidi_class(0x0020), BidiClass::Ws);
        assert_eq!(bidi_class(0x0030), BidiClass::En);
        assert_eq!(bidi_class(0x002C), BidiClass::Cs);
        assert_eq!(bidi_class(0x0041), BidiClass::L);
        assert_eq!(bidi_class(0x05D0), BidiClass::R);
        assert_eq!(bidi_class(0x0627), BidiClass::Al);
        assert_eq!(bidi_class(0x0660), BidiClass::An);
        assert_eq!(bidi_class(0x0300), BidiClass::Nsm);
        assert_eq!(bidi_class(0x202B), BidiClass::Rle);
        assert_eq!(bidi_class(0x202C), BidiClass::Pdf);
    }

    #[test]
    fn mirroring_is_symmetric_where_the_table_says_so() {
        for (a, b) in [
            (0x0028, 0x0029),
            (0x003C, 0x003E),
            (0x005B, 0x005D),
            (0x007B, 0x007D),
            (0x00AB, 0x00BB),
            (0x2018, 0x2019),
            (0x3008, 0x3009),
        ] {
            assert_eq!(mirror_char(a), b, "{a:#X}");
            assert_eq!(mirror_char(b), a, "{b:#X}");
        }
        // A code point with the sentinel index is its own mirror.
        assert_eq!(mirror_char(0x0041), 0x0041);
        // Audit item **A48**. This used to assert `0x0029`, reproducing the
        // oracle: `fx_unicode.cpp:48` returns 0 above the BMP where the "no
        // mirror" sentinel is `0x1FF`, so index 0 is read and
        // `kFXTextLayoutBidiMirror[0] == 0x0029`. UAX #9 rule L4 mirrors only
        // characters possessing `Bidi_Mirrored` and `BidiMirroring.txt` has no
        // mappings above the BMP, so a supplementary code point is its own
        // mirror.
        assert_eq!(mirror_char(0x1_0000), 0x1_0000);
        // Including one that really is an RTL letter: GOTHIC LETTER AHSA.
        assert_eq!(mirror_char(0x1_0330), 0x1_0330);
    }

    #[test]
    fn normalization_covers_all_four_map_tables() {
        // Map1 (single replacement): NO-BREAK SPACE becomes a plain space.
        assert_eq!(normalize(0x00A0), vec![0x0020]);
        // Map2 (two code points): the fi ligature.
        assert_eq!(normalize(0xFB01), vec![0x0066, 0x0069]);
        // Three code points: the ffi ligature.
        assert_eq!(normalize(0xFB03), vec![0x0066, 0x0066, 0x0069]);
        // Identity for anything unlisted.
        assert_eq!(normalize(0x0041), vec![0x0041]);
        // The mask means a supplementary code point reads its low half.
        assert_eq!(normalize(0x1_FB01), normalize(0xFB01));
    }

    #[test]
    fn normalization_never_returns_an_empty_vector() {
        for code in 0u32..=0xFFFF {
            assert!(!normalize(code).is_empty(), "{code:#X}");
        }
    }

    #[test]
    fn character_classes_match_icu() {
        assert!(is_alpha(u32::from(b'A')) && is_alpha(u32::from(b'z')));
        assert!(!is_alpha(u32::from(b'0')) && !is_alpha(u32::from(b'-')));
        assert!(is_alnum(u32::from(b'0')) && is_alnum(u32::from(b'A')));
        assert!(!is_alnum(u32::from(b'.')) && !is_alnum(u32::from(b'@')));
        // Arabic-Indic digits are numeric but not alphabetic.
        assert!(is_alnum(0x0660) && !is_alpha(0x0660));
        // CJK, Hebrew and Devanagari are alphabetic.
        for code in [0x4E00, 0x05D0, 0x0905, 0x3042, 0xAC00] {
            assert!(is_alpha(code), "{code:#X}");
        }
        // Supplementary-plane letters are covered.
        assert!(is_alpha(0x1_0400) && is_alpha(0x2_0000));
        // Punctuation and control characters are neither.
        for code in [0x0000, 0x0002, 0x0020, 0x2010, 0xFFFD] {
            assert!(!is_alpha(code) && !is_alnum(code), "{code:#X}");
        }
    }

    #[test]
    fn lowercase_is_the_simple_unicode_mapping() {
        assert_eq!(to_lower(u32::from(b'A')), u32::from(b'a'));
        assert_eq!(to_lower(u32::from(b'a')), u32::from(b'a'));
        assert_eq!(to_lower(0x0102), 0x0103);
        assert_eq!(to_lower(0x0103), 0x0103);
        // Greek, Cyrillic, and a supplementary-plane script.
        assert_eq!(to_lower(0x0391), 0x03B1);
        assert_eq!(to_lower(0x0410), 0x0430);
        assert_eq!(to_lower(0x1_0400), 0x1_0428);
        // Caseless characters are unchanged.
        for code in [u32::from(b'!'), 0x4E00, 0x05D0, 0x0000] {
            assert_eq!(to_lower(code), code, "{code:#X}");
        }
        assert_eq!(lower_string("Hello, WORLD!"), "hello, world!");
    }

    #[test]
    fn decimal_digits_are_ascii_only() {
        assert!(is_decimal_digit(u32::from(b'0')));
        assert!(is_decimal_digit(u32::from(b'9')));
        assert!(!is_decimal_digit(u32::from(b'/')));
        assert!(!is_decimal_digit(u32::from(b':')));
        // The C++ masks off the high bits before calling iswdigit.
        assert!(!is_decimal_digit(0x0660));
        assert!(!is_decimal_digit(0xFF10));
    }

    #[test]
    fn printability_is_the_c_locale_band() {
        assert!(!is_print(0x1F));
        assert!(is_print(0x20));
        assert!(is_print(0x7E));
        assert!(!is_print(0x7F));
        // The C++ tests isprint(0x80) with an out-of-range value; glibc says
        // no, so the guard's own boundary is not printable.
        assert!(!is_print(0x80));
    }

    #[test]
    fn the_expanded_tables_agree_with_a_linear_decode() {
        let linear = |tag: [u8; 4]| {
            let mut out = Vec::with_capacity(65536);
            for run in section(tag).chunks_exact(4) {
                let value = u16::from_le_bytes([run[0], run[1]]);
                let len = usize::from(u16::from_le_bytes([run[2], run[3]]));
                out.extend(std::iter::repeat_n(value, len));
            }
            out
        };
        for (table, expected) in [
            (ucd_table(), linear(*b"UCDR")),
            (normalization_table(), linear(*b"NRMR")),
        ] {
            assert_eq!(expected.len(), 65536);
            for (index, want) in expected.iter().enumerate() {
                assert_eq!(table.get(index), *want, "{index:#X}");
            }
            // Past the end the lookup is the default, never a panic.
            assert_eq!(table.get(65536), 0);
            assert_eq!(table.get(usize::MAX), 0);
        }
    }
}
