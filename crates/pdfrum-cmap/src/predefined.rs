//! Resolution of a predefined CMap name (`/Encoding /GB-EUC-H`) into a
//! decoder and a static CID table.
//!
//! Two tables are involved and they are keyed differently, which is the whole
//! subtlety of this module:
//!
//! - the **decoder table** below (32 rows) is keyed by a name with its last
//!   two bytes removed, so `GB-EUC-H` and `GB-EUC-V` share the row `GB-EUC`;
//! - the **static CID tables** in the blob are keyed by the full name, so
//!   `GB-EUC-H` and `GB-EUC-V` are separate entries with different CIDs.
//!
//! The truncation is not "strip a `-H` or `-V` suffix" — it removes the last
//! two bytes of any name longer than two, whatever they are. That is
//! deliberate damage tolerance in the original and it changes real output, so
//! it is reproduced exactly; see [`decoder_row`].

use crate::decode::LeadingBytes;
use crate::ids::{CidCoding, CidSet, CodingScheme};

/// Inclusive byte range of leading bytes for a mixed-two-byte scheme.
type ByteRange = (u8, u8);

/// One row of the predefined-name table: the decoder half of a CMap.
struct Row {
    /// The name with its direction suffix removed, as looked up.
    stem: &'static [u8],
    charset: CidSet,
    coding: CidCoding,
    scheme: CodingScheme,
    /// Up to two inclusive leading-byte ranges, consulted only when `scheme`
    /// is [`CodingScheme::MixedTwoBytes`].
    leading: &'static [ByteRange],
}

/// The complete predefined-name table, in the order it is scanned.
///
/// The `H` and `V` rows carry leading-byte ranges that are never read, because
/// their scheme is `TwoBytes` and the ranges are loaded only for
/// `MixedTwoBytes`. They are kept so this table and the C++'s stay
/// comparable row for row.
const ROWS: &[Row] = &[
    row(b"GB-EUC", CidSet::Gb1, CidCoding::Gb, &[(0xa1, 0xfe)]),
    row(b"GBpc-EUC", CidSet::Gb1, CidCoding::Gb, &[(0xa1, 0xfc)]),
    row(b"GBK-EUC", CidSet::Gb1, CidCoding::Gb, &[(0x81, 0xfe)]),
    row(b"GBKp-EUC", CidSet::Gb1, CidCoding::Gb, &[(0x81, 0xfe)]),
    row(b"GBK2K-EUC", CidSet::Gb1, CidCoding::Gb, &[(0x81, 0xfe)]),
    row(b"GBK2K", CidSet::Gb1, CidCoding::Gb, &[(0x81, 0xfe)]),
    two(b"UniGB-UCS2", CidSet::Gb1, CidCoding::Ucs2),
    two(b"UniGB-UTF16", CidSet::Gb1, CidCoding::Utf16),
    row(b"B5pc", CidSet::Cns1, CidCoding::Big5, &[(0xa1, 0xfc)]),
    row(b"HKscs-B5", CidSet::Cns1, CidCoding::Big5, &[(0x88, 0xfe)]),
    row(b"ETen-B5", CidSet::Cns1, CidCoding::Big5, &[(0xa1, 0xfe)]),
    row(b"ETenms-B5", CidSet::Cns1, CidCoding::Big5, &[(0xa1, 0xfe)]),
    two(b"UniCNS-UCS2", CidSet::Cns1, CidCoding::Ucs2),
    two(b"UniCNS-UTF16", CidSet::Cns1, CidCoding::Utf16),
    rksj(b"83pv-RKSJ"),
    rksj(b"90ms-RKSJ"),
    rksj(b"90msp-RKSJ"),
    rksj(b"90pv-RKSJ"),
    rksj(b"Add-RKSJ"),
    row(
        b"EUC",
        CidSet::Japan1,
        CidCoding::Jis,
        &[(0x8e, 0x8e), (0xa1, 0xfe)],
    ),
    // `H` and `V` are TwoBytes, so their 0x21..=0x7e range is never loaded.
    Row {
        stem: b"H",
        charset: CidSet::Japan1,
        coding: CidCoding::Jis,
        scheme: CodingScheme::TwoBytes,
        leading: &[(0x21, 0x7e)],
    },
    Row {
        stem: b"V",
        charset: CidSet::Japan1,
        coding: CidCoding::Jis,
        scheme: CodingScheme::TwoBytes,
        leading: &[(0x21, 0x7e)],
    },
    rksj(b"Ext-RKSJ"),
    two(b"UniJIS-UCS2", CidSet::Japan1, CidCoding::Ucs2),
    two(b"UniJIS-UCS2-HW", CidSet::Japan1, CidCoding::Ucs2),
    two(b"UniJIS-UTF16", CidSet::Japan1, CidCoding::Utf16),
    row(
        b"KSC-EUC",
        CidSet::Korea1,
        CidCoding::Korea,
        &[(0xa1, 0xfe)],
    ),
    row(
        b"KSCms-UHC",
        CidSet::Korea1,
        CidCoding::Korea,
        &[(0x81, 0xfe)],
    ),
    row(
        b"KSCms-UHC-HW",
        CidSet::Korea1,
        CidCoding::Korea,
        &[(0x81, 0xfe)],
    ),
    row(
        b"KSCpc-EUC",
        CidSet::Korea1,
        CidCoding::Korea,
        &[(0xa1, 0xfd)],
    ),
    two(b"UniKS-UCS2", CidSet::Korea1, CidCoding::Ucs2),
    two(b"UniKS-UTF16", CidSet::Korea1, CidCoding::Utf16),
];

const fn row(
    stem: &'static [u8],
    charset: CidSet,
    coding: CidCoding,
    leading: &'static [ByteRange],
) -> Row {
    Row {
        stem,
        charset,
        coding,
        scheme: CodingScheme::MixedTwoBytes,
        leading,
    }
}

const fn two(stem: &'static [u8], charset: CidSet, coding: CidCoding) -> Row {
    Row {
        stem,
        charset,
        coding,
        scheme: CodingScheme::TwoBytes,
        leading: &[],
    }
}

/// The five Shift-JIS rows share one leading-byte pair.
const fn rksj(stem: &'static [u8]) -> Row {
    row(
        stem,
        CidSet::Japan1,
        CidCoding::Jis,
        &[(0x81, 0x9f), (0xe0, 0xfc)],
    )
}

/// What a name resolved to in the decoder table.
pub(crate) struct Resolved {
    pub(crate) charset: CidSet,
    pub(crate) coding: CidCoding,
    pub(crate) scheme: CodingScheme,
    /// The leading-byte set, present only for `MixedTwoBytes`.
    pub(crate) leading: Option<Box<LeadingBytes>>,
}

/// Look `name` up in the decoder table.
///
/// The last two bytes of any name longer than two are removed before the
/// comparison, unconditionally. Every consequence of that is intended:
///
/// - `GB-EUC-H` and `GB-EUC-V` both find `GB-EUC`;
/// - `H` and `V` are two bytes or fewer, so they are compared whole, and a
///   three-byte name such as `Hxy` truncates onto `H`;
/// - `GBK2K-EUC` and `GBK2K` exist as separate rows precisely so that
///   `GBK2K-EUC-H` and `GBK2K-H` both resolve;
/// - a name whose last two bytes are *not* a direction suffix still loses
///   them, so `GB-EUCXY` finds `GB-EUC` and gets a working decoder — real
///   damage tolerance, since the row's decoder is right even when the exact
///   spelling is not;
/// - and the truncation cuts blindly, so `UniJIS-UCS2-HW` — the half-width
///   name *without* a direction suffix — becomes `UniJIS-UCS2-`, with the
///   trailing hyphen, and matches no row at all. Likewise `GB-EUC-XY` becomes
///   `GB-EUC-` and misses. Whether a damaged name lands on a row or on nothing
///   turns on its length parity, which is exactly as arbitrary as it sounds
///   and is what the original does.
fn decoder_row(name: &[u8]) -> Option<&'static Row> {
    let stem = if name.len() > 2 {
        name.get(..name.len() - 2)?
    } else {
        name
    };
    ROWS.iter().find(|r| r.stem == stem)
}

/// Expand a row's inclusive leading-byte ranges into the set the decoder
/// consults. The original terminates its two-slot list with a `(0, 0)` entry,
/// which makes a range that genuinely starts and ends at byte 0 unrepresentable
/// there; here the list is simply as long as it is, and no real range needs
/// byte 0 anyway.
fn leading_set(row: &Row) -> Box<LeadingBytes> {
    let mut set = Box::new(LeadingBytes::none());
    for &(first, last) in row.leading {
        set.add(first, last);
    }
    set
}

/// Resolve a predefined name to its decoder, or `None` when no row matches.
pub(crate) fn resolve(name: &[u8]) -> Option<Resolved> {
    let row = decoder_row(name)?;
    Some(Resolved {
        charset: row.charset,
        coding: row.coding,
        scheme: row.scheme,
        leading: (row.scheme == CodingScheme::MixedTwoBytes).then(|| leading_set(row)),
    })
}

/// Whether a name is one of the two Identity CMaps, which short-circuit the
/// whole table: the character code *is* the CID and there is no charset.
pub(crate) fn is_identity(name: &[u8]) -> bool {
    name == b"Identity-H" || name == b"Identity-V"
}

/// A CMap name is vertical when its last byte is `V`. Computed on the name as
/// written — before the two-byte truncation and before the Identity check —
/// so `Identity-V` is vertical and so is any name that happens to end in `V`.
/// An empty name is not vertical.
pub(crate) fn is_vertical(name: &[u8]) -> bool {
    name.last() == Some(&b'V')
}

/// Remove one leading `/` from a name, as the `/Encoding` lookup path does
/// before it consults any table. Only one, and only if present.
pub(crate) fn strip_slash(name: &[u8]) -> &[u8] {
    name.strip_prefix(b"/").unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::{ROWS, decoder_row, is_identity, is_vertical, resolve, strip_slash};
    use crate::ids::{CidCoding, CidSet, CodingScheme};

    #[test]
    fn the_table_is_the_c_plus_plus_table() {
        assert_eq!(ROWS.len(), 32);
        let mut stems: Vec<&[u8]> = ROWS.iter().map(|r| r.stem).collect();
        stems.sort_unstable();
        let before = stems.len();
        stems.dedup();
        assert_eq!(before, stems.len(), "stems must be unique");
    }

    /// The unconditional two-byte truncation, in every shape that matters.
    #[test]
    fn name_resolution_strips_the_last_two_bytes() {
        for (name, stem) in [
            (&b"GB-EUC-H"[..], &b"GB-EUC"[..]),
            (b"GB-EUC-V", b"GB-EUC"),
            (b"UniJIS-UCS2-HW-H", b"UniJIS-UCS2-HW"),
            (b"GBK2K-EUC-H", b"GBK2K-EUC"),
            (b"GBK2K-H", b"GBK2K"),
            // Garbage: the last two bytes are eaten whatever they are, so a
            // name with a two-byte suffix still finds its family's decoder.
            (b"GB-EUCXY", b"GB-EUC"),
            (b"GB-EUC\0\0", b"GB-EUC"),
            (b"UniJIS-UCS2XX", b"UniJIS-UCS2"),
        ] {
            let row = decoder_row(name).unwrap_or_else(|| panic!("{name:?} must resolve"));
            assert_eq!(row.stem, stem, "{name:?}");
        }
    }

    /// The cut is blind, so a name whose damage is an odd number of bytes
    /// leaves a fragment that matches nothing. `UniJIS-UCS2-HW` is the case
    /// that matters: the half-width name with no direction suffix truncates to
    /// `UniJIS-UCS2-`, hyphen included, and resolves to no row whatsoever.
    #[test]
    fn a_truncation_that_lands_between_rows_matches_nothing() {
        for name in [
            &b"UniJIS-UCS2-HW"[..],
            b"GB-EUC-XY",
            b"GB-EUC-",
            b"UniJIS-UCS2-",
        ] {
            assert!(decoder_row(name).is_none(), "{name:?} must not resolve");
        }
    }

    /// One- and two-byte names are compared whole, which is the only reason
    /// `H` and `V` are reachable at all.
    #[test]
    fn short_names_are_not_truncated() {
        assert_eq!(decoder_row(b"H").map(|r| r.stem), Some(&b"H"[..]));
        assert_eq!(decoder_row(b"V").map(|r| r.stem), Some(&b"V"[..]));
        assert!(decoder_row(b"").is_none());
        assert!(decoder_row(b"X").is_none());
        assert!(decoder_row(b"XY").is_none());
        // Three bytes truncate to one, and "H"/"V" are one byte.
        assert_eq!(decoder_row(b"Hxy").map(|r| r.stem), Some(&b"H"[..]));
    }

    #[test]
    fn leading_bytes_load_only_for_mixed_two_byte_rows() {
        let gb = resolve(b"GB-EUC-H").unwrap();
        assert_eq!(gb.scheme, CodingScheme::MixedTwoBytes);
        let set = gb.leading.unwrap();
        assert!(!set.contains(0x40));
        assert!(set.contains(0xa1));
        assert!(set.contains(0xfe));
        assert!(!set.contains(0xff));
        assert!(!set.contains(0xa0));

        // `H` declares 0x21..=0x7e but is TwoBytes, so nothing is loaded.
        let h = resolve(b"H").unwrap();
        assert_eq!(h.scheme, CodingScheme::TwoBytes);
        assert!(h.leading.is_none());
    }

    #[test]
    fn two_disjoint_leading_ranges_both_load() {
        let euc = resolve(b"EUC-H").unwrap();
        let set = euc.leading.unwrap();
        assert!(set.contains(0x8e));
        assert!(!set.contains(0x8f));
        assert!(set.contains(0xa1));
        assert!(set.contains(0xfe));

        let rksj = resolve(b"90ms-RKSJ-H").unwrap();
        let set = rksj.leading.unwrap();
        assert!(set.contains(0x81) && set.contains(0x9f) && !set.contains(0xa0));
        assert!(set.contains(0xe0) && set.contains(0xfc) && !set.contains(0xfd));
    }

    #[test]
    fn charsets_and_codings_come_from_the_row() {
        let r = resolve(b"UniKS-UTF16-H").unwrap();
        assert_eq!(r.charset, CidSet::Korea1);
        assert_eq!(r.coding, CidCoding::Utf16);
        assert_eq!(r.scheme, CodingScheme::TwoBytes);
    }

    #[test]
    fn identity_and_verticality_read_the_unstripped_name() {
        assert!(is_identity(b"Identity-H"));
        assert!(is_identity(b"Identity-V"));
        assert!(!is_identity(b"Identity"));
        assert!(!is_identity(b"/Identity-H"));

        assert!(is_vertical(b"Identity-V"));
        assert!(is_vertical(b"GB-EUC-V"));
        assert!(is_vertical(b"V"));
        assert!(is_vertical(b"NonsenseV"));
        assert!(!is_vertical(b"Identity-H"));
        assert!(!is_vertical(b""));
    }

    #[test]
    fn one_leading_slash_is_removed() {
        assert_eq!(strip_slash(b"/Identity-H"), b"Identity-H");
        assert_eq!(strip_slash(b"Identity-H"), b"Identity-H");
        assert_eq!(strip_slash(b"//X"), b"/X");
        assert_eq!(strip_slash(b""), b"");
    }
}
