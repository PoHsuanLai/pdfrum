//! Classic cross-reference tables: `xref`, subsection headers, and twenty
//! bytes per entry (ISO 32000-1 §7.5.4).
//!
//! # Twenty bytes, counted not parsed
//!
//! An entry is `nnnnnnnnnn ggggg t \r\n` and the reader indexes into it by
//! position rather than tokenizing, because that is the only way to notice
//! the failure mode this format has: a table whose entries are nineteen or
//! twenty-one bytes long still *looks* like numbers to a tokenizer, and
//! reading it that way silently yields offsets shifted by one field. So the
//! type flag is read from byte 17 and nowhere else, and an offset that parses
//! as zero without ten leading digits fails the whole table.
//!
//! # Trusting a table, once
//!
//! Even a well-formed table can be wrong: a file edited by a tool that
//! rewrote its objects without rewriting the table has entries pointing at
//! the wrong bytes. [`verify_table`] catches that by seeking to the first
//! entry with a real offset and checking the object number written there. One
//! entry, not all of them — a full check would cost a pass over the file, and
//! one mismatch is already conclusive.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};

use crate::lexer::{Lexer, Token, atoi64, atoui};
use crate::xref::Xref;

/// How many bytes one entry occupies.
const ENTRY_SIZE: usize = 20;

/// Read a classic table at `pos`, applying its entries to `xref`.
///
/// Entries are written **straight into** the table given, rather than into a
/// fresh one to be merged afterwards. That is not an optimization: applied
/// directly, an entry of equal generation overwrites what is already there
/// and a free entry clears it, which is how a section revises the one before
/// it. Merging instead would let whatever is already recorded win.
///
/// With `skip` the entries are stepped over rather than read, which is how
/// the chain walk probes an offset to learn whether a table lives there.
/// The size caps below apply only when reading for real, so an absurd
/// declared count still identifies the offset as holding a classic table.
///
/// Returns where the table ended, so the caller can look for `trailer` there.
pub(crate) fn parse_table(
    file: &[u8],
    pos: usize,
    skip: bool,
    xref: &mut Xref,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<usize> {
    let mut lx = Lexer::at(file, pos);
    if !lx.next_word(limits).is_keyword(b"xref") {
        return None;
    }

    // How many entries the whole table may declare: never more than the file
    // could physically hold, since each one costs twenty bytes.
    let entry_ceiling = (file.len() / ENTRY_SIZE) as u64;
    let mut total: u64 = 0;

    loop {
        let before = lx.pos();
        let token = lx.next_word(limits);
        match token {
            Token::Eof => return None,
            // A word that is not a number ends the subsection list — which
            // is how `trailer` gets us out of here.
            Token::Number(word) => {
                let start = atoui(word);
                if start > limits.max_object_number {
                    return None;
                }
                // The count is read leniently: a word that is not a number
                // counts as zero and is still consumed, so a subsection
                // header of junk yields an empty subsection rather than
                // failing the table.
                let count = match lx.next_word(limits) {
                    Token::Number(w) => atoui(w),
                    _ => 0,
                };
                total = total.saturating_add(u64::from(count));
                if !skip && (total > u64::from(limits.max_xref_size) || total > entry_ceiling) {
                    return None;
                }
                lx.skip_to_word();
                let body_start = lx.pos();
                let body_len = (count as usize).saturating_mul(ENTRY_SIZE);
                if !skip && !read_subsection(file, body_start, start, count, xref, limits, diags) {
                    return None;
                }
                lx.seek(body_start.saturating_add(body_len));
            }
            _ => {
                lx.seek(before);
                return Some(before);
            }
        }
    }
}

/// Read one subsection's entries into the table.
///
/// Returns false when the table cannot be trusted at all: an offset field
/// that is not ten digits yet parses as zero, an object number past the
/// largest legal one, or a subsection running off the end of the file.
/// Everything else is tolerated.
fn read_subsection(
    file: &[u8],
    start: usize,
    first_obj: u32,
    count: u32,
    xref: &mut Xref,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> bool {
    for i in 0..count {
        let at = start.saturating_add((i as usize).saturating_mul(ENTRY_SIZE));
        let Some(entry) = file.get(at..at.saturating_add(ENTRY_SIZE)) else {
            // The table claims more entries than the file holds, so its
            // arithmetic is wrong and none of it can be relied on.
            diags.record(
                Severity::Suspicious,
                DiagKind::XrefEntriesShifted,
                Some(at as u64),
            );
            return false;
        };
        let Some(num) = first_obj.checked_add(i) else {
            return false;
        };

        let offset_field = entry.get(..10).unwrap_or_default();
        let offset = atoi64(offset_field);
        // Byte 17 is the type flag, and only `f` means free — anything else,
        // including garbage, is an object in use.
        let free = entry.get(17) == Some(&b'f');

        if !free && offset == 0 && !offset_field.iter().all(u8::is_ascii_digit) {
            // Not a zero offset: a table whose fields have drifted out of
            // alignment. Reading it would give every object a wrong address.
            diags.record(
                Severity::Suspicious,
                DiagKind::XrefEntriesShifted,
                Some(at as u64),
            );
            return false;
        }

        // The generation field is parsed as a signed integer and then kept
        // in sixteen bits, so `70000` lands as 4464 rather than being
        // rejected.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the narrowing is the recorded generation, wrap included"
        )]
        let generation = atoi64(entry.get(11..17).unwrap_or_default()) as u16;

        // An object number past the largest legal one fails the whole table:
        // the file is describing objects that cannot exist.
        if num > limits.max_object_number {
            return false;
        }

        if free {
            // A free entry of generation zero says nothing: it is the shape
            // a never-written slot has, and honoring it would erase whatever
            // a newer section put there.
            if generation > 0 {
                xref.set_free(num, generation);
            }
        } else {
            let pos = u64::try_from(offset).unwrap_or(0);
            if !xref.add_normal(num, generation, false, pos, limits) {
                return false;
            }
        }
    }
    true
}

/// Check that a table's entries point at the objects they name.
///
/// Only the first entry with a real offset is checked. The object header
/// there must open with the entry's own object number; anything else means
/// the table describes a version of the file that no longer exists, and the
/// caller abandons it for a full rebuild.
pub(crate) fn verify_table(file: &[u8], xref: &Xref, limits: &Limits) -> bool {
    let Some((num, pos)) = xref.iter().find_map(|(num, entry)| match entry.kind {
        crate::xref::Entry::Offset(pos) if pos > 0 => Some((num, pos)),
        _ => None,
    }) else {
        // Nothing to check against is not a failure.
        return true;
    };

    let Ok(pos) = usize::try_from(pos) else {
        return false;
    };
    let mut lx = Lexer::at(file, pos);
    match lx.next_word(limits) {
        Token::Number(word) => atoui(word) == num,
        // A keyword, punctuation, or the end of the file: whatever is there,
        // it is not the object header the table promised.
        _ => false,
    }
}

/// Read a `trailer` keyword and the dictionary after it.
pub(crate) fn read_trailer<R: pdfrum_object::Resolve + ?Sized>(
    file: &[u8],
    pos: usize,
    limits: &Limits,
    diags: &mut Diagnostics,
    store: &R,
) -> Option<pdfrum_object::Dict> {
    let mut lx = Lexer::at(file, pos);
    if !lx.next_word(limits).is_keyword(b"trailer") {
        return None;
    }
    let mut ctx = crate::syntax::Context {
        limits,
        diags,
        file: None,
        store: Some(store),
    };
    let object =
        crate::syntax::body(&mut lx, &mut ctx, crate::syntax::Strictness::Loose, 0).ok()?;
    object.as_dict().cloned()
}

#[cfg(test)]
mod tests {
    use super::{parse_table, verify_table};
    use crate::xref::{Entry, Xref};
    use pdfrum_common::{Diagnostics, Limits};

    /// The C++ unit tests' table shape: twenty bytes an entry.
    fn read(buffer: &[u8]) -> Option<Xref> {
        let mut xref = Xref::new();
        let mut diags = Diagnostics::default();
        parse_table(buffer, 0, false, &mut xref, &Limits::default(), &mut diags).map(|_| xref)
    }

    /// What the C++ tests assert on a missing entry: position zero, free.
    fn info(xref: &Xref, num: u32) -> (u64, Entry) {
        match xref.entry(num) {
            Some(Entry::Offset(pos)) => (pos, Entry::Offset(pos)),
            Some(other) => (0, other),
            None => (0, Entry::Free),
        }
    }

    #[test]
    fn simple_table() {
        let buffer = b"xref \n\
                       0 6 \n\
                       0000000003 65535 f \n\
                       0000000017 00000 n \n\
                       0000000081 00000 n \n\
                       0000000000 00007 f \n\
                       0000000331 00000 n \n\
                       0000000409 00000 n \n\
                       trail\0";
        let xref = read(buffer).expect("table");
        let expected: [(u64, bool); 6] = [
            (0, false),
            (17, true),
            (81, true),
            (0, false),
            (331, true),
            (409, true),
        ];
        for (num, (pos, normal)) in (0u32..).zip(expected.iter()) {
            let (got_pos, got) = info(&xref, num);
            assert_eq!(got_pos, *pos, "object {num} offset");
            assert_eq!(
                matches!(got, Entry::Offset(_)),
                *normal,
                "object {num} type"
            );
        }
    }

    #[test]
    fn multiple_subsections_leave_gaps_free() {
        let buffer = b"xref \n\
                       0 1 \n\
                       0000000000 65535 f \n\
                       3 1 \n\
                       0000025325 00000 n \n\
                       8 2 \n\
                       0000025518 00002 n \n\
                       0000025635 00000 n \n\
                       12 1 \n\
                       0000025777 00000 n \n\
                       trail\0";
        let xref = read(buffer).expect("table");
        let expected: [u64; 13] = [0, 0, 0, 25325, 0, 0, 0, 0, 25518, 25635, 0, 0, 25777];
        for (num, pos) in (0u32..).zip(expected.iter()) {
            assert_eq!(info(&xref, num).0, *pos, "object {num}");
        }
    }

    #[test]
    fn a_free_entry_mid_table() {
        let buffer = b"xref \n\
                       0 1 \n\
                       0000000000 65535 f \n\
                       3 1 \n\
                       0000025325 00000 n \n\
                       8 2 \n\
                       0000000000 65535 f \n\
                       0000025635 00000 n \n\
                       12 1 \n\
                       0000025777 00000 n \n\
                       trail\0";
        let xref = read(buffer).expect("table");
        assert_eq!(info(&xref, 8), (0, Entry::Free));
        assert_eq!(info(&xref, 9).0, 25635);
        assert_eq!(info(&xref, 3).0, 25325);
        assert_eq!(info(&xref, 12).0, 25777);
    }

    #[test]
    fn a_free_list_chain() {
        let buffer = b"xref \n\
                       0 7 \n\
                       0000000002 65535 f \n\
                       0000000023 00000 n \n\
                       0000000003 65535 f \n\
                       0000000004 65535 f \n\
                       0000000000 65535 f \n\
                       0000000045 00000 n \n\
                       0000000179 00000 n \n\
                       trail\0";
        let xref = read(buffer).expect("table");
        let expected: [(u64, bool); 7] = [
            (0, false),
            (23, true),
            (0, false),
            (0, false),
            (0, false),
            (45, true),
            (179, true),
        ];
        for (num, (pos, normal)) in (0u32..).zip(expected.iter()) {
            let (got_pos, got) = info(&xref, num);
            assert_eq!(got_pos, *pos, "object {num}");
            assert_eq!(matches!(got, Entry::Offset(_)), *normal, "object {num}");
        }
    }

    #[test]
    fn a_table_whose_entry_count_is_a_multiple_of_the_block_size() {
        // The regression this guards: 2048 entries read in blocks.
        let mut buffer = b"xref \n 0 2048 \n".to_vec();
        for i in 0..2048u32 {
            buffer.extend_from_slice(format!("{:010} 00000 n \n", i + 1).as_bytes());
        }
        buffer.extend_from_slice(b"trail");
        let xref = read(&buffer).expect("table");
        for i in 0..2048u32 {
            assert_eq!(info(&xref, i).0, u64::from(i) + 1, "object {i}");
            assert!(matches!(xref.entry(i), Some(Entry::Offset(_))));
        }
    }

    #[test]
    fn a_missing_xref_keyword_is_not_a_table() {
        assert!(read(b"0 1 \n0000000000 65535 f \ntrail").is_none());
    }

    #[test]
    fn an_offset_field_that_is_not_ten_digits_fails_the_whole_table() {
        // The second entry's offset field holds letters, so it parses as
        // zero without being one — the signature of a table whose fields
        // have drifted out of alignment.
        let buffer = b"xref \n\
                       0 2 \n\
                       0000000017 00000 n \n\
                       00000000ab 00000 n \n\
                       trail";
        assert!(read(buffer).is_none());
        // A genuine zero offset written as ten digits is fine.
        let buffer = b"xref \n\
                       0 2 \n\
                       0000000017 00000 n \n\
                       0000000000 00000 n \n\
                       trail";
        assert!(read(buffer).is_some());
    }

    #[test]
    fn verification_checks_the_first_real_offset() {
        let file = b"junkjunk\n7 0 obj << >> endobj";
        let mut xref = Xref::new();
        xref.add_normal(7, 0, false, 9, &Limits::default());
        assert!(verify_table(file, &xref, &Limits::default()));

        // Point the entry one object number off.
        let mut wrong = Xref::new();
        wrong.add_normal(8, 0, false, 9, &Limits::default());
        assert!(!verify_table(file, &wrong, &Limits::default()));
    }

    #[test]
    fn verification_passes_a_table_with_nothing_to_check() {
        let mut xref = Xref::new();
        xref.set_free(0, 65535);
        assert!(verify_table(b"", &xref, &Limits::default()));
    }
}
