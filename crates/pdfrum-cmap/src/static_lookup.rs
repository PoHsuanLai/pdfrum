//! Charcode↔CID lookup over the static tables of a predefined CMap.
//!
//! A predefined CMap is one entry of one registry's index table, plus a chain
//! of further entries reached through signed index deltas. The chain is the
//! *static* form of `usecmap`: a vertical CMap holds only the codes whose CID
//! differs from the horizontal one and defers everything else to it, and a
//! variant CMap (`GBKp-EUC-H`, `ETenms-B5-H`) defers almost everything.
//!
//! Two properties of the chain are worth stating because they constrain the
//! code below:
//!
//! - **Each link carries its own record type.** `UniCNS-UTF16-V` holds range
//!   records and defers to `UniCNS-UTF16-H`, which holds single records, so
//!   the search kind is decided per link and never per chain.
//! - **The dword path never falls back to the word path.** A code at or above
//!   `0x1_0000` is looked up only in the chain's dword tables, and only three
//!   CMaps have one; everywhere else such a code is CID 0.

use crate::blob::{self, Entry};

/// Longest `use_offset` chain in the blob is far below this; the cap exists so
/// a hypothetical malformed blob cannot spin.
const MAX_CHAIN: usize = 8;

/// Walk a chain from `(reg, index)`, yielding each link's entry.
fn chain(reg: usize, index: usize) -> impl Iterator<Item = Entry> {
    let mut at = Some(index);
    let mut links = 0usize;
    std::iter::from_fn(move || {
        let i = at?;
        let e = blob::entry(reg, i)?;
        links += 1;
        at = if e.use_offset == 0 || links >= MAX_CHAIN {
            None
        } else {
            isize::try_from(i)
                .ok()
                .and_then(|i| i.checked_add(isize::from(e.use_offset)))
                .and_then(|n| usize::try_from(n).ok())
        };
        Some(e)
    })
}

/// Index of the first record whose search key is not less than `needle` —
/// `std::lower_bound` over the same key the C++ compares on. Both record kinds
/// are read as `(low, high, cid)`, and `high` is the key in both (a single
/// record's `low` and `high` are the same code).
fn lower_bound(entry: &Entry, needle: u16) -> usize {
    let mut lo = 0usize;
    let mut len = usize::from(entry.word_count);
    while len > 0 {
        let half = len / 2;
        let mid = lo + half;
        let key = blob::word_record(entry, mid).map_or(u16::MAX, |(_, high, _)| high);
        if key < needle {
            lo = mid + 1;
            len -= half + 1;
        } else {
            len = half;
        }
    }
    lo
}

/// `lower_bound` over a dword table on the two-key `(hi_word, lo_word_high)`
/// comparator.
fn lower_bound_dword(entry: &Entry, hi: u16, lo: u16) -> usize {
    let mut base = 0usize;
    let mut len = usize::from(entry.dword_count);
    while len > 0 {
        let half = len / 2;
        let mid = base + half;
        let less = blob::dword_record(entry, mid).is_some_and(|(rec_hi, _, rec_lo_high, _)| {
            if rec_hi == hi {
                rec_lo_high < lo
            } else {
                rec_hi < hi
            }
        });
        if less {
            base = mid + 1;
            len -= half + 1;
        } else {
            len = half;
        }
    }
    base
}

/// The CID a predefined CMap's static tables give `charcode`, or 0 when no
/// link in the chain covers it.
pub(crate) fn cid_from_charcode(reg: usize, index: usize, charcode: u32) -> u16 {
    let hi = (charcode >> 16) as u16;
    let lo = charcode as u16;
    if hi != 0 {
        return cid_from_dword(reg, index, hi, lo);
    }
    for entry in chain(reg, index) {
        let at = lower_bound(&entry, lo);
        if let Some((low, high, cid)) = blob::word_record(&entry, at)
            && lo >= low
            && lo <= high
        {
            // Widened so the C++'s int promotion is reproduced rather than
            // its `uint16_t` wrap-around, which the sorted tables never reach.
            return (u32::from(cid) + u32::from(lo) - u32::from(low)) as u16;
        }
    }
    0
}

/// The four-byte path. Links without a dword table are skipped, and a miss
/// never falls back to the word tables.
fn cid_from_dword(reg: usize, index: usize, hi: u16, lo: u16) -> u16 {
    for entry in chain(reg, index) {
        if entry.dword_count == 0 {
            continue;
        }
        let at = lower_bound_dword(&entry, hi, lo);
        // The hit test deliberately does not re-check `hi`: when `hi` is
        // absent from the table, `lower_bound` lands on the first record of a
        // *greater* high word, and a record whose low range happens to
        // contain `lo` matches anyway. PDFium behaves this way and files
        // depend on the CIDs it produces.
        if let Some((_, lo_low, lo_high, cid)) = blob::dword_record(&entry, at)
            && lo >= lo_low
            && lo <= lo_high
        {
            return (u32::from(cid) + u32::from(lo) - u32::from(lo_low)) as u16;
        }
    }
    0
}

/// The charcode a predefined CMap maps to `cid`, or 0 when none does.
///
/// A linear scan, because the tables are sorted by code and not by CID. The
/// dword tables are not consulted at all, so a CID that exists only above
/// `0x1_0000` is unreachable in reverse — PDFium carries a standing TODO about
/// this and text extraction is measured against what it actually does.
pub(crate) fn charcode_from_cid(reg: usize, index: usize, cid: u16) -> u32 {
    for entry in chain(reg, index) {
        for r in 0..usize::from(entry.word_count) {
            let Some((low, high, rec_cid)) = blob::word_record(&entry, r) else {
                break;
            };
            let span = u32::from(rec_cid) + u32::from(high) - u32::from(low);
            if u32::from(cid) >= u32::from(rec_cid) && u32::from(cid) <= span {
                return u32::from(low) + u32::from(cid) - u32::from(rec_cid);
            }
        }
    }
    0
}

/// The index of `name` within registry `reg`, matched byte-for-byte against
/// the C++ index table's `name_` field.
pub(crate) fn find(reg: usize, name: &[u8]) -> Option<usize> {
    (0..blob::entry_count(reg))
        .find(|&i| blob::entry(reg, i).and_then(|e| blob::name(&e)) == Some(name))
}

#[cfg(test)]
mod tests {
    use super::{charcode_from_cid, cid_from_charcode, find};

    const GB1: usize = 0;
    const CNS1: usize = 1;
    const JAPAN1: usize = 2;
    const KOREA1: usize = 3;

    fn at(reg: usize, name: &[u8]) -> usize {
        find(reg, name).expect("name is in the blob")
    }

    /// Spot CIDs read straight out of `kGB_EUC_H_0`'s first three records:
    /// `{0x0020,0x0020,0x1E24}, {0x0021,0x007E,0x032E}, {0xA1A1,0xA1FE,0x0060}`.
    #[test]
    fn gb_euc_h_spot_lookups() {
        let i = at(GB1, b"GB-EUC-H");
        assert_eq!(cid_from_charcode(GB1, i, 0x0020), 0x1E24);
        assert_eq!(cid_from_charcode(GB1, i, 0x0021), 0x032E);
        assert_eq!(cid_from_charcode(GB1, i, 0x007E), 0x032E + 0x7E - 0x21);
        assert_eq!(cid_from_charcode(GB1, i, 0x0060), 0x032E + 0x60 - 0x21);
        assert_eq!(cid_from_charcode(GB1, i, 0xA1A1), 0x0060);
        assert_eq!(cid_from_charcode(GB1, i, 0xA1FE), 0x0060 + 0x5D);
        // Below the first range: lower_bound lands on record 0 and the
        // `>= low` test fails, so no chain link claims it.
        assert_eq!(cid_from_charcode(GB1, i, 0x0000), 0);
        assert_eq!(cid_from_charcode(GB1, i, 0x001F), 0);
        // Between two ranges.
        assert_eq!(cid_from_charcode(GB1, i, 0x00FF), 0);
    }

    /// `GB-EUC-V` holds 20 records and defers the rest to `GB-EUC-H` one row
    /// back, so a code only the horizontal table covers still resolves.
    #[test]
    fn vertical_chains_back_to_horizontal() {
        let (v, h) = (at(GB1, b"GB-EUC-V"), at(GB1, b"GB-EUC-H"));
        assert_eq!(v, h + 1);
        // A code the V table does not cover comes from H.
        assert_eq!(
            cid_from_charcode(GB1, v, 0xA1A1),
            cid_from_charcode(GB1, h, 0xA1A1)
        );
        // At least one code must disagree, or the chain proves nothing.
        let differs =
            (0u32..=0xFFFF).any(|c| cid_from_charcode(GB1, v, c) != cid_from_charcode(GB1, h, c));
        assert!(differs, "GB-EUC-V must override something");
    }

    /// `KSCpc-EUC-H` hops six rows back to `KSC-EUC-H`, the longest delta in
    /// the blob, and Korea1 has no `KSCpc-EUC-V` to pair it with.
    #[test]
    fn korea1_six_row_hop() {
        let (pc, ksc) = (at(KOREA1, b"KSCpc-EUC-H"), at(KOREA1, b"KSC-EUC-H"));
        assert_eq!(pc, ksc + 6);
        assert!(find(KOREA1, b"KSCpc-EUC-V").is_none());
        let borrowed = (0u32..=0xFFFF).find(|&c| {
            cid_from_charcode(KOREA1, pc, c) != 0
                && cid_from_charcode(KOREA1, pc, c) == cid_from_charcode(KOREA1, ksc, c)
        });
        assert!(borrowed.is_some(), "the -6 hop must reach KSC-EUC-H");
    }

    /// A range-record link deferring to a single-record link: the search kind
    /// is per entry, not per chain.
    #[test]
    fn chain_crosses_from_range_to_single() {
        let (v, h) = (at(CNS1, b"UniCNS-UTF16-V"), at(CNS1, b"UniCNS-UTF16-H"));
        assert_eq!(v, h + 1);
        assert!(crate::blob::entry(CNS1, v).unwrap().is_range);
        assert!(!crate::blob::entry(CNS1, h).unwrap().is_range);
        let reached = (0u32..=0xFFFF).find(|&c| {
            cid_from_charcode(CNS1, v, c) == cid_from_charcode(CNS1, h, c)
                && cid_from_charcode(CNS1, h, c) != 0
        });
        assert!(reached.is_some(), "V must reach H's single table");
    }

    /// The Japan1 equivalent: `UniJIS-UCS2-HW-H` has four range records and
    /// defers two rows back to a single table.
    #[test]
    fn japan1_hw_chain_crosses_type() {
        let (hw, base) = (
            at(JAPAN1, b"UniJIS-UCS2-HW-H"),
            at(JAPAN1, b"UniJIS-UCS2-H"),
        );
        assert_eq!(hw, base + 2);
        assert_eq!(crate::blob::entry(JAPAN1, hw).unwrap().word_count, 4);
        assert!(!crate::blob::entry(JAPAN1, base).unwrap().is_range);
        let reached = (0u32..=0xFFFF).find(|&c| {
            cid_from_charcode(JAPAN1, hw, c) == cid_from_charcode(JAPAN1, base, c)
                && cid_from_charcode(JAPAN1, base, c) != 0
        });
        assert!(reached.is_some());
    }

    /// `kGBK2K_H_5_DWord`'s first record is `{0x8130, 0x8436, 0x8436, 0x5752}`.
    #[test]
    fn dword_path_resolves_four_byte_codes() {
        let i = at(GB1, b"GBK2K-H");
        assert_eq!(cid_from_charcode(GB1, i, 0x8130_8436), 0x5752);
        // Second record `{0x8138, 0xFD38, 0xFD39, 0x579C}` covers two codes.
        assert_eq!(cid_from_charcode(GB1, i, 0x8138_FD38), 0x579C);
        assert_eq!(cid_from_charcode(GB1, i, 0x8138_FD39), 0x579D);
    }

    /// A CMap with no dword table returns 0 for a four-byte code rather than
    /// truncating it into the word tables.
    #[test]
    fn dword_path_never_falls_back_to_the_word_tables() {
        let i = at(GB1, b"GB-EUC-H");
        assert_eq!(cid_from_charcode(GB1, i, 0x0001_A1A1), 0);
        assert_ne!(cid_from_charcode(GB1, i, 0xA1A1), 0);
    }

    #[test]
    fn reverse_lookup_round_trips_the_spot_codes() {
        let i = at(GB1, b"GB-EUC-H");
        for code in [0x0020u32, 0x0021, 0x007E, 0xA1A1, 0xA1FE] {
            let cid = cid_from_charcode(GB1, i, code);
            assert_ne!(cid, 0, "code {code:#x} must map");
            assert_eq!(charcode_from_cid(GB1, i, cid), code, "round trip {code:#x}");
        }
        assert_eq!(charcode_from_cid(GB1, i, 0), 0);
    }

    /// A CID that exists only in a dword table is unreachable in reverse: the
    /// scan never looks at `dword_map_`.
    #[test]
    fn reverse_lookup_ignores_dword_tables() {
        let i = at(GB1, b"GBK2K-H");
        let cid = cid_from_charcode(GB1, i, 0x8130_8436);
        assert_eq!(cid, 0x5752);
        // Whatever the reverse scan returns, it is not the four-byte code.
        assert_ne!(charcode_from_cid(GB1, i, cid), 0x8130_8436);
    }

    #[test]
    fn unknown_names_and_indices_are_absent() {
        assert!(find(GB1, b"GB-EUC").is_none());
        assert!(find(GB1, b"").is_none());
        assert!(find(GB1, b"Identity-H").is_none());
        assert_eq!(cid_from_charcode(GB1, 999, 0x20), 0);
        assert_eq!(charcode_from_cid(GB1, 999, 1), 0);
    }
}
