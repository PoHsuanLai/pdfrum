//! Cross-reference streams: the same information as a classic table, packed
//! into a stream of fixed-width big-endian fields (ISO 32000-1 §7.5.8).
//!
//! # Three fields, and the one that may be missing
//!
//! `/W` gives the byte width of each field. A width of zero means the field
//! is not stored, and for the *type* field that is not an absence but a
//! default: every entry is a normal in-use object, which is what a file
//! containing no free or compressed objects writes. For the other two fields
//! a zero width reads as the value zero.
//!
//! # The cursor that does not advance
//!
//! `/Index` lists the object-number ranges the stream describes, and the data
//! is read as one run of entries laid end to end. When a range would read
//! past the end of the data it is skipped — but the read cursor does **not**
//! advance past it, so the next range starts at the bytes the skipped one
//! would have used. That is not a design; it is what the code does, and files
//! with a wrong `/Index` land on it, so it is reproduced exactly.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, NoResolve, Object, Stream, names};

use crate::xref::{Trailer, Xref};

/// The smallest number of fields a `/W` array may declare.
const MIN_FIELDS: usize = 3;

/// What reading a cross-reference stream produced.
pub(crate) struct XrefStream {
    /// The stream's own dictionary, which is also the section's trailer.
    pub trailer: Trailer,
    /// Where the previous section is, or zero to stop walking.
    pub prev: i64,
}

/// Read a cross-reference stream's entries into `xref`.
///
/// The stream must already have been parsed out of the file and its data
/// decoded. `is_main` marks the newest section, whose `/Size` sizes the whole
/// table. Returns `None` when the dictionary is not a usable cross-reference
/// stream at all, which sends the caller to the next repair.
pub(crate) fn read_xref_stream(
    stream: &Stream,
    obj_num: u32,
    data: &[u8],
    is_main: bool,
    xref: &mut Xref,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<XrefStream> {
    let dict = &stream.dict;

    // `/Prev`, `/Size` and `/W` are read through the resolving accessors,
    // which is what the C++ uses here — but the only store available while
    // the table is still being built resolves nothing, so an indirect value
    // reads as absent. Written this way because the distinction is real:
    // the *classic* trailer reads `/Prev` and `/Size` through accessors that
    // would not resolve even given a working store.
    let prev = dict.int(names::PREV, &NoResolve).unwrap_or(0);
    if prev < 0 {
        return None;
    }
    let size = dict.int(names::SIZE, &NoResolve).unwrap_or(0);
    if size < 0 || size > i64::from(limits.max_xref_size) {
        return None;
    }

    let widths = field_widths(dict)?;
    let total_width = widths.total()?;

    // The declared size is applied *before* the entries are read, which is
    // what lets an entry whose object number equals `/Size` survive: the
    // truncation happens first and the entry lands after it.
    if is_main
        && size >= 1
        && let Ok(size) = u32::try_from(size)
    {
        xref.set_size(size);
    }

    let ranges = index_ranges(dict, size);

    // Where in the data the next range reads from, measured in entries.
    let mut cursor: u64 = 0;
    for (start, count) in ranges {
        let Some(range) = entry_range(cursor, count, total_width, data.len()) else {
            // The range runs past the data. It is skipped — and the cursor
            // stays put, so the next range reads these same bytes.
            diags.record(Severity::Suspicious, DiagKind::XrefStreamEntryDropped, None);
            continue;
        };

        grow_for(xref, start, count, limits);
        read_entries(
            data.get(range).unwrap_or_default(),
            start,
            &widths,
            xref,
            limits,
            diags,
        );
        cursor = cursor.saturating_add(u64::from(count));
    }

    Some(XrefStream {
        trailer: Trailer {
            dict: dict.clone(),
            object_number: obj_num,
        },
        prev,
    })
}

/// The byte widths of the three fields.
#[derive(Debug, Clone, Copy)]
struct Widths {
    /// Width of the entry-type field. Zero means "every entry is normal".
    kind: u32,
    /// Width of the offset or archive-number field.
    second: u32,
    /// Width of the generation or member-index field.
    third: u32,
}

impl Widths {
    /// How many bytes one entry occupies, or `None` on overflow.
    fn total(self) -> Option<usize> {
        let sum = u64::from(self.kind) + u64::from(self.second) + u64::from(self.third);
        usize::try_from(sum).ok().filter(|&n| n > 0)
    }
}

/// Read `/W`, which needs at least three entries.
///
/// Extra entries past the third are ignored rather than rejected: their
/// widths do not shift anything, because the three fields are read from the
/// front of each entry.
fn field_widths(dict: &Dict) -> Option<Widths> {
    let w = dict.array(names::W, &NoResolve)?;
    if w.len() < MIN_FIELDS {
        return None;
    }
    // A width that is not a number reads as zero rather than failing the
    // stream, and no single field is capped — only their sum has to fit,
    // which [`Widths::total`] checks.
    let width = |i: usize| -> u32 {
        w.get(i, &NoResolve)
            .and_then(|v| v.as_direct().and_then(Object::as_int))
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
    };
    Some(Widths {
        kind: width(0),
        second: width(1),
        third: width(2),
    })
}

/// Read `/Index` as object-number ranges, defaulting to the whole `/Size`.
///
/// A pair whose members are missing or not numbers is skipped, as is one
/// naming a negative start or a non-positive count.
fn index_ranges(dict: &Dict, size: i64) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if let Some(index) = dict.array(names::INDEX, &NoResolve) {
        let mut i = 0;
        while i + 1 < index.len() {
            let start = index.number_obj_at(i).and_then(Object::as_int);
            let count = index.number_obj_at(i + 1).and_then(Object::as_int);
            i += 2;
            let (Some(start), Some(count)) = (start, count) else {
                continue;
            };
            if start < 0 || count <= 0 {
                continue;
            }
            let (Ok(start), Ok(count)) = (u32::try_from(start), u32::try_from(count)) else {
                continue;
            };
            out.push((start, count));
        }
    }
    if out.is_empty()
        && let Ok(count) = u32::try_from(size)
        && count > 0
    {
        out.push((0, count));
    }
    out
}

/// The byte range `count` entries occupy starting at entry `cursor`.
fn entry_range(
    cursor: u64,
    count: u32,
    total_width: usize,
    data_len: usize,
) -> Option<std::ops::Range<usize>> {
    let width = total_width as u64;
    let start = cursor.checked_mul(width)?;
    let end = cursor.checked_add(u64::from(count))?.checked_mul(width)?;
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    (end <= data_len).then_some(start..end)
}

/// Make room for a range's object numbers, honoring the table's ceiling.
fn grow_for(xref: &mut Xref, start: u32, count: u32, limits: &Limits) {
    let wanted = u64::from(start)
        .saturating_add(u64::from(count))
        .min(u64::from(limits.max_xref_size));
    let wanted = u32::try_from(wanted).unwrap_or(limits.max_xref_size);
    if wanted > xref.last_object_number().saturating_add(1) {
        xref.set_size(wanted);
    }
}

/// Read one range's worth of entries.
fn read_entries(
    data: &[u8],
    start: u32,
    widths: &Widths,
    xref: &mut Xref,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    let Some(width) = widths.total() else { return };
    for (i, entry) in data.chunks_exact(width).enumerate() {
        let Ok(i) = u32::try_from(i) else { return };
        let Some(num) = start.checked_add(i) else {
            return;
        };
        // Past the largest legal object number the rest of this range is
        // meaningless, so it stops rather than skipping entry by entry.
        if num > limits.max_object_number {
            return;
        }

        let kind = if widths.kind == 0 {
            // No type field: ISO 32000-1 table 17 says every entry is in
            // use, which is exactly what such a file means.
            1
        } else {
            var_int(entry, 0, widths.kind as usize)
        };
        let second = var_int(entry, widths.kind as usize, widths.second as usize);
        let third = var_int(
            entry,
            (widths.kind as usize).saturating_add(widths.second as usize),
            widths.third as usize,
        );

        match kind {
            0 => match u16::try_from(third) {
                Ok(generation) => xref.set_free(num, generation),
                Err(_) => {
                    diags.record(Severity::Suspicious, DiagKind::XrefStreamEntryDropped, None);
                }
            },
            1 => match u16::try_from(third) {
                Ok(generation) => {
                    xref.add_normal(num, generation, false, second, limits);
                }
                Err(_) => {
                    diags.record(Severity::Suspicious, DiagKind::XrefStreamEntryDropped, None);
                }
            },
            2 => {
                let archive = u32::try_from(second).unwrap_or(u32::MAX);
                let index = u32::try_from(third).unwrap_or(u32::MAX);
                // The archive must be an object this table already knows
                // about; a number past its end names nothing.
                if xref.is_valid_object_number(archive) {
                    xref.add_compressed(num, archive, index, limits);
                } else {
                    diags.record(Severity::Suspicious, DiagKind::XrefStreamEntryDropped, None);
                }
            }
            _ => {
                diags.record(Severity::Suspicious, DiagKind::XrefStreamEntryDropped, None);
            }
        }
    }
}

/// Read `len` big-endian bytes at `offset` as an unsigned integer. An empty
/// field is zero.
fn var_int(entry: &[u8], offset: usize, len: usize) -> u64 {
    entry
        .get(offset..offset.saturating_add(len))
        .unwrap_or_default()
        .iter()
        .fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

#[cfg(test)]
mod tests {
    use super::{index_ranges, read_xref_stream, var_int};
    use crate::xref::{Entry, Xref};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{ByteSpan, Dict, Object, Stream};

    fn dict(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Dict {
        Dict::from_pairs(
            pairs
                .into_iter()
                .map(|(k, v)| (pdfrum_object::Name::from(k), v)),
        )
    }

    fn ints(values: &[i64]) -> Object {
        Object::Array(pdfrum_object::Array::of(
            values.iter().map(|&v| Object::Int(v)),
        ))
    }

    fn read(d: Dict, data: &[u8]) -> (Xref, Diagnostics, bool) {
        let stream = Stream::new(d, ByteSpan::from(data.to_vec()));
        let mut xref = Xref::new();
        let mut diags = Diagnostics::default();
        let ok = read_xref_stream(
            &stream,
            7,
            data,
            false,
            &mut xref,
            &Limits::default(),
            &mut diags,
        )
        .is_some();
        (xref, diags, ok)
    }

    #[test]
    fn var_int_is_big_endian_and_zero_when_empty() {
        assert_eq!(var_int(&[0x01, 0x02, 0x03], 0, 3), 0x0001_0203);
        assert_eq!(var_int(&[0xFF], 0, 1), 255);
        assert_eq!(var_int(&[0x01], 0, 0), 0);
        assert_eq!(var_int(&[], 5, 2), 0);
    }

    #[test]
    fn index_defaults_to_the_whole_size() {
        assert_eq!(index_ranges(&Dict::new(), 5), vec![(0, 5)]);
        // A zero size yields nothing to read.
        assert!(index_ranges(&Dict::new(), 0).is_empty());
    }

    #[test]
    fn index_pairs_that_make_no_sense_are_skipped() {
        let d = dict([("Index", ints(&[-1, 2, 3, 0, 5, 2]))]);
        assert_eq!(index_ranges(&d, 9), vec![(5, 2)]);
    }

    #[test]
    fn a_zero_type_width_means_every_entry_is_in_use() {
        let d = dict([("Size", Object::Int(2)), ("W", ints(&[0, 1, 1]))]);
        let (xref, _, ok) = read(d, &[0x0F, 0x00, 0x12, 0x00]);
        assert!(ok);
        assert_eq!(xref.entry(0), Some(Entry::Offset(15)));
        assert_eq!(xref.entry(1), Some(Entry::Offset(18)));
    }

    #[test]
    fn fewer_than_three_widths_is_not_a_cross_reference_stream() {
        let d = dict([("Size", Object::Int(3)), ("W", ints(&[1, 1]))]);
        assert!(!read(d, &[0x01, 0x00, 0x00]).2);
    }

    #[test]
    fn a_negative_prev_or_size_is_refused() {
        let d = dict([
            ("Size", Object::Int(3)),
            ("W", ints(&[1, 1, 1])),
            ("Prev", Object::Int(-1)),
        ]);
        assert!(!read(d, &[0x01, 0x00, 0x00]).2);

        let d = dict([("Size", Object::Int(-1)), ("W", ints(&[1, 1, 1]))]);
        assert!(!read(d, &[0x01, 0x00, 0x00]).2);
    }

    #[test]
    fn a_zero_size_reads_nothing_but_succeeds() {
        let d = dict([("Size", Object::Int(0)), ("W", ints(&[1, 1, 1]))]);
        let (xref, _, ok) = read(d, &[0x01, 0x00, 0x00]);
        assert!(ok);
        assert!(xref.is_empty());
    }

    #[test]
    fn an_archive_number_past_the_table_is_skipped() {
        // Entry 0 names archive 0xFF, but /Size is 3.
        let d = dict([("Size", Object::Int(3)), ("W", ints(&[1, 1, 1]))]);
        let data = [0x02, 0xFF, 0x00, 0x01, 0x0F, 0x00, 0x01, 0x12, 0x00];
        let (xref, diags, ok) = read(d, &data);
        assert!(ok);
        // Object 0's entry is dropped outright, not turned into a free slot.
        assert_eq!(xref.entry(0), None);
        assert_eq!(xref.entry(1), Some(Entry::Offset(15)));
        assert_eq!(xref.entry(2), Some(Entry::Offset(18)));
        assert!(diags.contains(&pdfrum_common::DiagKind::XrefStreamEntryDropped));
    }

    #[test]
    fn an_index_names_the_object_numbers() {
        let d = dict([
            ("Size", Object::Int(83)),
            ("Index", ints(&[2, 1, 4, 2, 80, 3])),
            ("W", ints(&[1, 1, 1])),
        ]);
        let data = [
            0x01, 0x00, 0x00, // 2
            0x01, 0x0F, 0x00, // 4
            0x01, 0x12, 0x00, // 5
            0x01, 0x20, 0x00, // 80
            0x01, 0x22, 0x00, // 81
            0x01, 0x25, 0x00, // 82
        ];
        let (xref, _, ok) = read(d, &data);
        assert!(ok);
        for (num, pos) in [(2, 0), (4, 15), (5, 18), (80, 32), (81, 34), (82, 37)] {
            assert_eq!(xref.entry(num), Some(Entry::Offset(pos)), "object {num}");
        }
    }

    #[test]
    fn overlapping_index_ranges_let_the_later_one_win() {
        // Ranges (2,2) and (3,1): object 3 is written twice, and the second
        // range reads the third entry.
        let d = dict([
            ("Size", Object::Int(4)),
            ("Index", ints(&[2, 2, 3, 1])),
            ("W", ints(&[1, 1, 1])),
        ]);
        let data = [0x01, 0x00, 0x00, 0x01, 0x0F, 0x00, 0x01, 0x12, 0x00];
        let (xref, _, ok) = read(d, &data);
        assert!(ok);
        assert_eq!(xref.entry(2), Some(Entry::Offset(0)));
        assert_eq!(xref.entry(3), Some(Entry::Offset(18)));
    }

    #[test]
    fn index_ranges_need_not_ascend() {
        let d = dict([
            ("Size", Object::Int(5)),
            ("Index", ints(&[3, 2, 2, 1])),
            ("W", ints(&[1, 1, 1])),
        ]);
        let data = [0x01, 0x00, 0x00, 0x01, 0x0F, 0x00, 0x01, 0x12, 0x00];
        let (xref, _, ok) = read(d, &data);
        assert!(ok);
        assert_eq!(xref.entry(3), Some(Entry::Offset(0)));
        assert_eq!(xref.entry(4), Some(Entry::Offset(15)));
        assert_eq!(xref.entry(2), Some(Entry::Offset(18)));
    }

    #[test]
    fn an_index_may_exceed_the_declared_size() {
        let d = dict([
            ("Size", Object::Int(81)),
            ("Index", ints(&[2, 1, 80, 2])),
            ("W", ints(&[1, 1, 1])),
        ]);
        let data = [0x01, 0x00, 0x00, 0x01, 0x0F, 0x00, 0x01, 0x12, 0x00];
        let (xref, _, ok) = read(d, &data);
        assert!(ok);
        assert_eq!(xref.entry(2), Some(Entry::Offset(0)));
        assert_eq!(xref.entry(80), Some(Entry::Offset(15)));
        assert_eq!(xref.entry(81), Some(Entry::Offset(18)));
    }

    #[test]
    fn the_highest_legal_object_number_is_accepted() {
        let limits = Limits::default();
        let d = dict([
            ("Size", Object::Int(i64::from(limits.max_xref_size))),
            ("Index", ints(&[i64::from(limits.max_object_number), 1])),
            ("W", ints(&[1, 1, 1])),
        ]);
        let (xref, _, ok) = read(d, &[0x01, 0x00, 0x00]);
        assert!(ok);
        assert_eq!(xref.entry(limits.max_object_number), Some(Entry::Offset(0)));
    }

    #[test]
    fn a_skipped_range_does_not_advance_the_cursor() {
        // The first range wants six entries but the data holds three, so it
        // is skipped and the second range reads from the very start.
        let d = dict([
            ("Size", Object::Int(20)),
            ("Index", ints(&[10, 6, 1, 1])),
            ("W", ints(&[1, 1, 1])),
        ]);
        let data = [0x01, 0x07, 0x00, 0x01, 0x08, 0x00, 0x01, 0x09, 0x00];
        let (xref, diags, ok) = read(d, &data);
        assert!(ok);
        assert_eq!(xref.entry(10), None);
        assert_eq!(xref.entry(1), Some(Entry::Offset(7)));
        assert!(diags.contains(&pdfrum_common::DiagKind::XrefStreamEntryDropped));
    }

    #[test]
    fn an_unknown_entry_type_is_dropped() {
        let d = dict([("Size", Object::Int(2)), ("W", ints(&[1, 1, 1]))]);
        let (xref, diags, ok) = read(d, &[0x05, 0x00, 0x00, 0x01, 0x0F, 0x00]);
        assert!(ok);
        assert_eq!(xref.entry(0), None);
        assert_eq!(xref.entry(1), Some(Entry::Offset(15)));
        assert!(diags.contains(&pdfrum_common::DiagKind::XrefStreamEntryDropped));
    }

    #[test]
    fn a_generation_beyond_sixteen_bits_drops_the_entry() {
        let d = dict([("Size", Object::Int(1)), ("W", ints(&[1, 1, 4]))]);
        let (xref, diags, ok) = read(d, &[0x01, 0x0F, 0x00, 0x01, 0x00, 0x00]);
        assert!(ok);
        assert_eq!(xref.entry(0), None);
        assert!(diags.contains(&pdfrum_common::DiagKind::XrefStreamEntryDropped));
    }
}
