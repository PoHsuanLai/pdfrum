//! Cross-reference emission (ISO 32000-1 §7.5.4 and §7.5.8.4).
//!
//! Three shapes, chosen by what the *input* was:
//!
//! - a **full classic table** — every object written, one subsection per run
//!   of consecutive numbers. This is what a full save emits, and what an
//!   incremental save of a rebuilt-table document emits too, because a
//!   rebuilt document has no previous section to chain from;
//! - a **delta table** — only the appended objects, for an incremental save
//!   over a document whose own cross-reference was a classic table;
//! - an **xref stream**, for an incremental save over a document whose main
//!   cross-reference was a stream. There is no table at all then: the whole
//!   cross-reference goes into the trailer object (see [`super::trailer`]).
//!
//! # The free head is not decoration
//!
//! Object 0 is always free with generation **65535**, and every subsection
//! that starts at object 1 folds that free head in by beginning at 0 and
//! counting one higher. When object 1 was *dropped* — garbage-collected, or
//! never present — a standalone `0 1` subsection is emitted up front instead,
//! because a table with no entry for object 0 is malformed.
//!
//! # Every entry says generation 00000
//!
//! The writer renumbers every object to generation 0, so the table must agree
//! (invariant R9). The only `65535` in the output is the free head's.

use std::collections::BTreeMap;

/// Where each object was written, by object number.
///
/// The insert-and-erase discipline is load-bearing: an old object whose fetch
/// fails has its offset recorded *before* the fetch and erased after, so it
/// vanishes from both the body and the table together. An entry here is a
/// promise that `N 0 obj` really sits at that byte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectOffsets(BTreeMap<u32, u64>);

impl ObjectOffsets {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Record that object `num` starts at `offset`.
    pub fn set(&mut self, num: u32, offset: u64) {
        self.0.insert(num, offset);
    }

    /// Forget object `num` — it turned out not to be writable.
    pub fn erase(&mut self, num: u32) {
        self.0.remove(&num);
    }

    /// Where object `num` was written.
    #[must_use]
    pub fn get(&self, num: u32) -> Option<u64> {
        self.0.get(&num).copied()
    }

    /// Whether object `num` was written.
    #[must_use]
    pub fn contains(&self, num: u32) -> bool {
        self.0.contains_key(&num)
    }

    /// The object numbers written, ascending.
    pub fn numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.keys().copied()
    }

    /// The highest object number written, or 0 when nothing was.
    #[must_use]
    pub fn last(&self) -> u32 {
        self.0.keys().next_back().copied().unwrap_or(0)
    }

    /// How many objects were written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One entry line: a ten-digit offset, a five-digit generation of zero, and
/// the in-use marker.
fn entry_line(out: &mut Vec<u8>, offset: u64) {
    out.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
}

/// The free head: object 0, generation 65535 — **not** 65536, which is what
/// an off-by-one in the generation arithmetic would produce.
const FREE_HEAD: &[u8] = b"0000000000 65535 f\r\n";

/// A full classic table covering everything in `offsets`, up to and including
/// `last`.
///
/// Subsections are runs of consecutive present numbers; a gap starts a new
/// one. The run that starts at object 1 is written as starting at object 0
/// with the free head inline, so its count is one higher than the run length.
pub fn classic_full(out: &mut Vec<u8>, offsets: &ObjectOffsets, last: u32) {
    out.extend_from_slice(b"xref\r\n");
    // Object 1 missing means no subsection will carry the free head, so it
    // needs one of its own.
    if !offsets.contains(1) {
        out.extend_from_slice(b"0 1\r\n");
        out.extend_from_slice(FREE_HEAD);
    }

    let mut i: u32 = 1;
    while i <= last {
        while i <= last && !offsets.contains(i) {
            i = i.saturating_add(1);
        }
        if i > last {
            break;
        }
        let start = i;
        let mut end = i;
        while end <= last && offsets.contains(end) {
            end = end.saturating_add(1);
        }

        if start == 1 {
            // Count includes the free head this subsection carries.
            out.extend_from_slice(format!("0 {end}\r\n").as_bytes());
            out.extend_from_slice(FREE_HEAD);
        } else {
            out.extend_from_slice(format!("{start} {}\r\n", end - start).as_bytes());
        }
        for n in start..end {
            entry_line(out, offsets.get(n).unwrap_or(0));
        }
        i = end;
    }
}

/// A delta table covering exactly `written` — the objects an incremental save
/// appended, in ascending order.
pub fn classic_delta(out: &mut Vec<u8>, offsets: &ObjectOffsets, written: &[u32]) {
    out.extend_from_slice(b"xref\r\n");

    let mut i = 0usize;
    while i < written.len() {
        let Some(&start) = written.get(i) else {
            break;
        };
        // Advance to the end of this run of consecutive numbers.
        let mut j = i.saturating_add(1);
        while written
            .get(j)
            .zip(written.get(j.saturating_sub(1)))
            .is_some_and(|(next, prev)| *next == prev.saturating_add(1))
        {
            j = j.saturating_add(1);
        }
        let run = j - i;

        if start == 1 {
            out.extend_from_slice(format!("0 {}\r\n", run + 1).as_bytes());
            out.extend_from_slice(FREE_HEAD);
        } else {
            out.extend_from_slice(format!("{start} {run}\r\n").as_bytes());
        }
        for n in written.get(i..j).unwrap_or_default() {
            entry_line(out, offsets.get(*n).unwrap_or(0));
        }
        i = j;
    }
}

/// One five-byte xref-stream record under `/W [0 4 1]`: a four-byte
/// big-endian offset and a one-byte generation.
///
/// The type field has width zero, which ISO 32000-1 §7.5.8.3 says defaults to
/// type 1 — an in-use object. The generation byte is always zero, matching
/// what the body writes.
pub fn stream_record(out: &mut Vec<u8>, offset: u64) {
    let truncated = u32::try_from(offset).unwrap_or(u32::MAX);
    out.extend_from_slice(&truncated.to_be_bytes());
    out.push(0);
}

#[cfg(test)]
mod tests {
    use super::{ObjectOffsets, classic_delta, classic_full, stream_record};

    fn offsets(entries: &[(u32, u64)]) -> ObjectOffsets {
        let mut o = ObjectOffsets::new();
        for (num, at) in entries {
            o.set(*num, *at);
        }
        o
    }

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn one_run_from_object_one_folds_the_free_head_in() {
        let o = offsets(&[(1, 9), (2, 100), (3, 250)]);
        assert_eq!(
            text(|out| classic_full(out, &o, 3)),
            "xref\r\n\
             0 4\r\n\
             0000000000 65535 f\r\n\
             0000000009 00000 n\r\n\
             0000000100 00000 n\r\n\
             0000000250 00000 n\r\n"
        );
    }

    // Bug342: the free entry's generation is 65535, not 65536.
    #[test]
    fn the_free_head_generation_is_sixty_five_five_three_five() {
        let o = offsets(&[(1, 9)]);
        let table = text(|out| classic_full(out, &o, 1));
        assert!(table.contains("0000000000 65535 f\r\n"));
        assert!(!table.contains("65536"));
    }

    // Object 1 dropped: a standalone free-head subsection goes up front.
    #[test]
    fn a_missing_object_one_gets_a_standalone_free_subsection() {
        let o = offsets(&[(2, 100), (3, 250)]);
        assert_eq!(
            text(|out| classic_full(out, &o, 3)),
            "xref\r\n\
             0 1\r\n\
             0000000000 65535 f\r\n\
             2 2\r\n\
             0000000100 00000 n\r\n\
             0000000250 00000 n\r\n"
        );
    }

    #[test]
    fn a_gap_starts_a_second_subsection() {
        let o = offsets(&[(1, 9), (2, 100), (7, 700), (8, 800)]);
        assert_eq!(
            text(|out| classic_full(out, &o, 8)),
            "xref\r\n\
             0 3\r\n\
             0000000000 65535 f\r\n\
             0000000009 00000 n\r\n\
             0000000100 00000 n\r\n\
             7 2\r\n\
             0000000700 00000 n\r\n\
             0000000800 00000 n\r\n"
        );
    }

    #[test]
    fn nothing_past_last_is_written() {
        let o = offsets(&[(1, 9), (2, 100), (50, 5000)]);
        let table = text(|out| classic_full(out, &o, 2));
        assert!(!table.contains("0000005000"));
    }

    // The delta table walks the appended list, not the whole numbering.
    #[test]
    fn a_delta_table_emits_one_subsection_per_run() {
        let o = offsets(&[(2, 200), (3, 300), (4, 400), (9, 900), (10, 1000)]);
        assert_eq!(
            text(|out| classic_delta(out, &o, &[2, 3, 4, 9, 10])),
            "xref\r\n\
             2 3\r\n\
             0000000200 00000 n\r\n\
             0000000300 00000 n\r\n\
             0000000400 00000 n\r\n\
             9 2\r\n\
             0000000900 00000 n\r\n\
             0000001000 00000 n\r\n"
        );
    }

    #[test]
    fn a_delta_run_starting_at_one_still_carries_the_free_head() {
        let o = offsets(&[(1, 10), (2, 20)]);
        assert_eq!(
            text(|out| classic_delta(out, &o, &[1, 2])),
            "xref\r\n\
             0 3\r\n\
             0000000000 65535 f\r\n\
             0000000010 00000 n\r\n\
             0000000020 00000 n\r\n"
        );
    }

    #[test]
    fn an_empty_delta_writes_only_the_keyword() {
        let o = ObjectOffsets::new();
        assert_eq!(text(|out| classic_delta(out, &o, &[])), "xref\r\n");
    }

    #[test]
    fn a_stream_record_is_a_big_endian_offset_and_a_zero() {
        let mut out = Vec::new();
        stream_record(&mut out, 0x0001_0203);
        assert_eq!(out, vec![0x00, 0x01, 0x02, 0x03, 0x00]);
    }

    #[test]
    fn offsets_erase_removes_the_promise_entirely() {
        let mut o = offsets(&[(1, 9), (2, 100)]);
        o.erase(2);
        assert!(!o.contains(2));
        assert_eq!(o.last(), 1);
        assert_eq!(
            text(|out| classic_full(out, &o, 2)),
            "xref\r\n0 2\r\n0000000000 65535 f\r\n0000000009 00000 n\r\n"
        );
    }

    // Offsets past 2^32 saturate rather than wrap: a wrapped offset would
    // name a wrong byte, which is unrecoverable; a saturated one is at least
    // visibly wrong. Unreachable on any real file.
    #[test]
    fn a_huge_offset_saturates_rather_than_wrapping() {
        let mut out = Vec::new();
        stream_record(&mut out, u64::from(u32::MAX) + 1);
        assert_eq!(out, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    }
}
