//! Reader over the committed table blob (`tables/cmaps.bin`).
//!
//! The blob holds the four CJK registries' charcode→CID tables and their
//! CID→Unicode tables. It is generated from the C++ source arrays by
//! `build.rs` and committed; see `tables/PROVENANCE.md`.
//!
//! Everything here is checked indexing over `include_bytes!` — no transmute,
//! no alignment assumption, no panic path. A malformed blob is impossible in
//! a built crate (the generator asserts the layout it writes), but every
//! accessor still returns `Option` so that impossibility never becomes a
//! crash, and so `blob_integrity` can test the reader against the same
//! invariants the generator checked.

/// The generated table data.
static BLOB: &[u8] = include_bytes!("../tables/cmaps.bin");

/// `"PMC1"` little-endian: pdfrum CMaps, format 1.
#[cfg(test)]
const MAGIC: u32 = 0x504D_4331;
/// Bytes per index entry.
const ENTRY_LEN: usize = 20;
/// Bytes per registry directory record, in both directories.
const DIR_LEN: usize = 8;
/// Registries with static tables.
pub(crate) const REGISTRY_COUNT: usize = 4;

/// A `use_offset` of 0 terminates a chain, so the type is a signed delta and
/// not an index. One entry, borrowed out of the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) name_off: u32,
    pub(crate) word_off: u32,
    /// `u32::MAX` when the entry has no dword table.
    pub(crate) dword_off: u32,
    pub(crate) word_count: u16,
    pub(crate) dword_count: u16,
    /// `true` = range records (3 u16), `false` = single records (2 u16).
    pub(crate) is_range: bool,
    /// Signed index delta to the next CMap in the chain; 0 terminates.
    pub(crate) use_offset: i8,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    Some(u16::from_le_bytes([*bytes.get(at)?, *bytes.get(end - 1)?]))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let s = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([
        *s.first()?,
        *s.get(1)?,
        *s.get(2)?,
        *s.get(3)?,
    ]))
}

fn header_u32(slot: usize) -> Option<u32> {
    u32_at(BLOB, 8usize.checked_add(slot.checked_mul(4)?)?)
}

/// Whether the blob's header is the one this reader understands. Not consulted
/// on the hot path — the accessors are individually checked — but pinned by a
/// test so a regenerated blob with a bumped version fails loudly.
#[cfg(test)]
pub(crate) fn header_is_valid() -> bool {
    u32_at(BLOB, 0) == Some(MAGIC)
        && u16_at(BLOB, 4) == Some(1)
        && u16_at(BLOB, 6) == Some(REGISTRY_COUNT as u16)
        && header_u32(4) == Some(BLOB.len() as u32)
}

/// Total blob length in bytes, as the header records it.
#[cfg(test)]
pub(crate) fn total_len() -> usize {
    BLOB.len()
}

fn index_off() -> Option<usize> {
    header_u32(0).map(|v| v as usize)
}
fn words_off() -> Option<usize> {
    header_u32(1).map(|v| v as usize)
}
fn dwords_off() -> Option<usize> {
    header_u32(2).map(|v| v as usize)
}
fn cid2uni_off() -> Option<usize> {
    header_u32(3).map(|v| v as usize)
}
fn names_off() -> Option<usize> {
    header_u32(5).map(|v| v as usize)
}

/// How many CMaps registry `reg` (0-based, GB1 first) declares.
pub(crate) fn entry_count(reg: usize) -> usize {
    let at = match index_off() {
        Some(v) => v + reg * DIR_LEN,
        None => return 0,
    };
    if reg >= REGISTRY_COUNT {
        return 0;
    }
    u16_at(BLOB, at + 4).unwrap_or(0) as usize
}

/// Entry `i` of registry `reg`, or `None` when either index is out of range.
pub(crate) fn entry(reg: usize, i: usize) -> Option<Entry> {
    if reg >= REGISTRY_COUNT || i >= entry_count(reg) {
        return None;
    }
    let dir = index_off()?.checked_add(reg.checked_mul(DIR_LEN)?)?;
    let base = u32_at(BLOB, dir)? as usize;
    let at = base.checked_add(i.checked_mul(ENTRY_LEN)?)?;
    Some(Entry {
        name_off: u32_at(BLOB, at)?,
        word_off: u32_at(BLOB, at.checked_add(4)?)?,
        dword_off: u32_at(BLOB, at.checked_add(8)?)?,
        word_count: u16_at(BLOB, at.checked_add(12)?)?,
        dword_count: u16_at(BLOB, at.checked_add(14)?)?,
        is_range: *BLOB.get(at.checked_add(16)?)? != 0,
        use_offset: (*BLOB.get(at.checked_add(17)?)?).cast_signed(),
    })
}

/// The CMap's name as it appears in the C++ index table, e.g. `b"GB-EUC-H"`.
pub(crate) fn name(entry: &Entry) -> Option<&'static [u8]> {
    let at = names_off()?.checked_add(entry.name_off as usize)?;
    let len = *BLOB.get(at)? as usize;
    BLOB.get(at.checked_add(1)?..at.checked_add(1)?.checked_add(len)?)
}

/// Raw bytes of an entry's word records: `word_count * (3 or 2)` little-endian
/// `u16`s. Callers read records through [`word_record`].
fn word_bytes(entry: &Entry) -> Option<&'static [u8]> {
    let stride = if entry.is_range { 6usize } else { 4 };
    let len = usize::from(entry.word_count).checked_mul(stride)?;
    let at = words_off()?.checked_add(entry.word_off as usize)?;
    BLOB.get(at..at.checked_add(len)?)
}

/// Record `i` of an entry's word array: `(low, high, cid)` for a range table,
/// `(code, code, cid)` for a single table — a single record is exactly a
/// one-wide range, which lets the lookup share its hit test.
pub(crate) fn word_record(entry: &Entry, i: usize) -> Option<(u16, u16, u16)> {
    let bytes = word_bytes(entry)?;
    if entry.is_range {
        let at = i.checked_mul(6)?;
        Some((
            u16_at(bytes, at)?,
            u16_at(bytes, at.checked_add(2)?)?,
            u16_at(bytes, at.checked_add(4)?)?,
        ))
    } else {
        let at = i.checked_mul(4)?;
        let code = u16_at(bytes, at)?;
        Some((code, code, u16_at(bytes, at.checked_add(2)?)?))
    }
}

/// Record `i` of an entry's dword array: `(hi_word, lo_low, lo_high, cid)`.
pub(crate) fn dword_record(entry: &Entry, i: usize) -> Option<(u16, u16, u16, u16)> {
    if entry.dword_off == u32::MAX {
        return None;
    }
    let len = usize::from(entry.dword_count).checked_mul(8)?;
    let base = dwords_off()?.checked_add(entry.dword_off as usize)?;
    let bytes = BLOB.get(base..base.checked_add(len)?)?;
    let at = i.checked_mul(8)?;
    Some((
        u16_at(bytes, at)?,
        u16_at(bytes, at.checked_add(2)?)?,
        u16_at(bytes, at.checked_add(4)?)?,
        u16_at(bytes, at.checked_add(6)?)?,
    ))
}

/// Length in entries of registry `reg`'s CID→Unicode table.
pub(crate) fn cid2unicode_len(reg: usize) -> usize {
    if reg >= REGISTRY_COUNT {
        return 0;
    }
    let Some(at) = cid2uni_off() else { return 0 };
    u32_at(BLOB, at + reg * DIR_LEN + 4).unwrap_or(0) as usize
}

/// The Unicode scalar mapped to `cid` in registry `reg`, or `None` when the
/// registry has no table or the CID is past its end.
pub(crate) fn cid2unicode(reg: usize, cid: u16) -> Option<u16> {
    if reg >= REGISTRY_COUNT {
        return None;
    }
    let dir = cid2uni_off()?.checked_add(reg.checked_mul(DIR_LEN)?)?;
    let base = u32_at(BLOB, dir)? as usize;
    let len = u32_at(BLOB, dir.checked_add(4)?)? as usize;
    let i = usize::from(cid);
    if i >= len {
        return None;
    }
    u16_at(BLOB, base.checked_add(i.checked_mul(2)?)?)
}

/// Byte-identity of two entries' word arrays. Used by the alias test: the
/// UTF16 and UCS2 variants of several CMaps share one array in the C++ and
/// must share one copy here.
#[cfg(test)]
pub(crate) fn word_arrays_identical(a: &Entry, b: &Entry) -> bool {
    a.is_range == b.is_range && a.word_count == b.word_count && word_bytes(a) == word_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::{
        Entry, REGISTRY_COUNT, cid2unicode, cid2unicode_len, entry, entry_count, header_is_valid,
        name, total_len, word_arrays_identical, word_record,
    };

    /// The registry entry counts and CID→Unicode lengths tabulated from the
    /// C++ source.
    const EXPECT: [(usize, usize); REGISTRY_COUNT] =
        [(14, 30284), (14, 19088), (20, 15444), (11, 18352)];

    fn find(reg: usize, want: &[u8]) -> Entry {
        (0..entry_count(reg))
            .filter_map(|i| entry(reg, i))
            .find(|e| name(e) == Some(want))
            .unwrap()
    }

    #[test]
    fn header_matches_the_generator() {
        assert!(header_is_valid());
        assert_eq!(total_len(), 630_716);
    }

    #[test]
    fn registry_shapes_match_the_tabulated_counts() {
        for (reg, (entries, uni)) in EXPECT.iter().copied().enumerate() {
            assert_eq!(entry_count(reg), entries, "registry {reg} entry count");
            assert_eq!(cid2unicode_len(reg), uni, "registry {reg} CID2Unicode len");
            assert_eq!(cid2unicode(reg, 0), Some(0xFFFD), "registry {reg} CID 0");
        }
        assert_eq!(entry_count(REGISTRY_COUNT), 0);
        assert_eq!(entry(REGISTRY_COUNT, 0), None);
        assert_eq!(cid2unicode(REGISTRY_COUNT, 0), None);
    }

    /// Every `use_offset` chain stays in the table, terminates, and is short.
    #[test]
    fn every_chain_terminates() {
        for reg in 0..REGISTRY_COUNT {
            let n = entry_count(reg);
            for start in 0..n {
                let mut at = start;
                let mut links = 0usize;
                loop {
                    let e = entry(reg, at).expect("chain link in bounds");
                    assert!(e.word_count > 0 || e.is_range, "link has a word array");
                    links += 1;
                    assert!(
                        links <= 8,
                        "registry {reg} chain from {start} exceeds 8 links"
                    );
                    if e.use_offset == 0 {
                        break;
                    }
                    let next = isize::try_from(at).unwrap() + isize::from(e.use_offset);
                    let next = usize::try_from(next).expect("chain stays in bounds");
                    assert!(next < n, "chain leaves the table");
                    at = next;
                }
            }
        }
    }

    #[test]
    fn names_are_unique_within_a_registry() {
        for reg in 0..REGISTRY_COUNT {
            let mut seen: Vec<&[u8]> = (0..entry_count(reg))
                .filter_map(|i| entry(reg, i))
                .filter_map(|e| name(&e))
                .collect();
            assert_eq!(seen.len(), entry_count(reg));
            seen.sort_unstable();
            let before = seen.len();
            seen.dedup();
            assert_eq!(before, seen.len(), "registry {reg} has a duplicate name");
        }
    }

    /// The UTF16 index rows alias the UCS2 rows' arrays in the C++; the blob
    /// dedupes by symbol, so the two entries must point at the same bytes.
    #[test]
    fn utf16_rows_alias_their_ucs2_arrays() {
        for (reg, a, b) in [
            (0usize, &b"UniGB-UTF16-H"[..], &b"UniGB-UCS2-H"[..]),
            (0, b"UniGB-UTF16-V", b"UniGB-UCS2-V"),
            (1, b"UniCNS-UTF16-V", b"UniCNS-UCS2-V"),
            (2, b"UniJIS-UTF16-H", b"UniJIS-UCS2-H"),
            (2, b"UniJIS-UTF16-V", b"UniJIS-UCS2-V"),
            (3, b"UniKS-UTF16-V", b"UniKS-UCS2-V"),
        ] {
            let (x, y) = (find(reg, a), find(reg, b));
            assert_eq!(x.word_off, y.word_off, "{a:?} and {b:?} word offsets");
            assert!(word_arrays_identical(&x, &y), "{a:?} and {b:?} bytes");
        }
    }

    /// Sortedness under exactly the key the lookup's binary search compares
    /// on — re-checked here so the committed blob, not just the generator's
    /// inputs, is proven sorted.
    #[test]
    fn word_records_are_sorted_on_the_search_key() {
        for reg in 0..REGISTRY_COUNT {
            for i in 0..entry_count(reg) {
                let e = entry(reg, i).unwrap();
                let mut prev = None;
                for r in 0..usize::from(e.word_count) {
                    let (low, high, _) = word_record(&e, r).expect("record in bounds");
                    assert!(
                        low <= high,
                        "registry {reg} entry {i} record {r} low > high"
                    );
                    // The comparator is `high < needle` for ranges and
                    // `code < needle` for singles; a single's record is
                    // (code, code, cid), so `high` is the key either way.
                    if let Some(p) = prev {
                        assert!(p <= high, "registry {reg} entry {i} record {r} unsorted");
                    }
                    prev = Some(high);
                }
                assert_eq!(word_record(&e, usize::from(e.word_count)), None);
            }
        }
    }

    #[test]
    fn dword_records_are_sorted_and_only_three_entries_have_them() {
        let mut with_dwords = Vec::new();
        for reg in 0..REGISTRY_COUNT {
            for i in 0..entry_count(reg) {
                let e = entry(reg, i).unwrap();
                if e.dword_count == 0 {
                    assert_eq!(super::dword_record(&e, 0), None, "no dwords means no table");
                    continue;
                }
                with_dwords.push((name(&e).unwrap(), e.dword_count));
                let mut prev = None;
                for r in 0..usize::from(e.dword_count) {
                    let (hi, lo_low, lo_high, _) =
                        super::dword_record(&e, r).expect("dword record in bounds");
                    assert!(lo_low <= lo_high);
                    if let Some(p) = prev {
                        assert!(p <= (hi, lo_high), "dword record {r} unsorted");
                    }
                    prev = Some((hi, lo_high));
                }
            }
        }
        assert_eq!(
            with_dwords,
            vec![
                (&b"GBK2K-H"[..], 1017),
                (&b"CNS-EUC-H"[..], 238),
                (&b"CNS-EUC-V"[..], 261),
            ]
        );
    }

    /// The first three range records of `kGB_EUC_H_0`, read straight out of
    /// the C++ source, prove the record packing is right way round.
    #[test]
    fn gb_euc_h_head_matches_the_source_array() {
        let e = find(0, b"GB-EUC-H");
        assert!(e.is_range);
        assert_eq!(e.word_count, 90);
        assert_eq!(word_record(&e, 0), Some((0x0020, 0x0020, 0x1E24)));
        assert_eq!(word_record(&e, 1), Some((0x0021, 0x007E, 0x032E)));
        assert_eq!(word_record(&e, 2), Some((0xA1A1, 0xA1FE, 0x0060)));
    }

    /// A single-type table's records come back as one-wide ranges.
    #[test]
    fn single_tables_read_as_one_wide_ranges() {
        let e = find(1, b"UniCNS-UTF16-H");
        assert!(!e.is_range);
        assert_eq!(e.word_count, 14557);
        let (low, high, _) = word_record(&e, 0).unwrap();
        assert_eq!(low, high);
    }

    #[test]
    fn cid2unicode_is_bounded_by_the_table_length() {
        let len = cid2unicode_len(2);
        assert!(cid2unicode(2, u16::try_from(len - 1).unwrap()).is_some());
        assert_eq!(cid2unicode(2, u16::try_from(len).unwrap()), None);
    }
}
