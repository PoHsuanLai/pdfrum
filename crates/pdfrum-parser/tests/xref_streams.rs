//! Cross-reference streams read the way a document reads them: through
//! `load`, not through the module that decodes their fields.
//!
//! The documents here are the ones a reader has to survive rather than the
//! ones a writer produces — an `/Index` that overlaps itself, a `/Size` that
//! disagrees with the entries, a `/W` too short to describe anything. Each
//! pins a decision that changes which objects a real file resolves to, and
//! together they are the reason the cross-reference stream path can be
//! changed later without silently changing behavior.

use std::fmt::Write as _;
use std::sync::Arc;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_parser::{Entry, LoadError, LoadOptions, Xref, load, read_xref};

/// Build a document whose only cross-reference information is one stream.
///
/// The stream's entries are written as hexadecimal text so the payload stays
/// readable, and the header carries the four binary bytes real files put
/// there to mark themselves as containing binary data.
fn document(dict_body: &str, entries: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF1-7\n%\xa0\xf2\xa4\xf4\n");
    out.extend_from_slice(b"7 0 obj <<\n");
    out.extend_from_slice(b"  /Filter /ASCIIHexDecode\n");
    out.extend_from_slice(dict_body.as_bytes());
    out.extend_from_slice(b">>\nstream\n");
    for entry in entries {
        out.extend_from_slice(entry.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"endstream\nendobj\nstartxref\n14\n%%EOF\n");
    out
}

/// Read one such document's cross-reference information.
fn read(file: &[u8]) -> Option<Xref> {
    let mut diags = Diagnostics::default();
    read_xref(file, &Limits::default(), &mut diags)
        .ok()
        .map(|(xref, _)| xref)
}

/// The entries a table describes, as object number and offset.
fn offsets(xref: &Xref) -> Vec<(u32, u64)> {
    xref.object_numbers()
        .filter_map(|num| match xref.entry(num) {
            Some(Entry::Offset(pos)) => Some((num, pos)),
            _ => None,
        })
        .collect()
}

/// Whether such a document opens at all. These carry no catalog, so a
/// successful read is a cross-reference read, not a load.
fn opens(file: &[u8]) -> bool {
    read(file).is_some()
}

#[test]
fn the_highest_legal_object_number_is_accepted() {
    let limits = Limits::default();
    let file = document(
        &format!(
            "  /Index [{} 1]\n  /Root 1 0 R\n  /Size {}\n  /W [1 1 1]\n",
            limits.max_object_number, limits.max_xref_size
        ),
        &["01 00 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(xref.entry(limits.max_object_number), Some(Entry::Offset(0)));
}

#[test]
fn object_numbers_past_the_cap_are_refused() {
    let limits = Limits::default();
    let file = document(
        &format!(
            "  /Index [{} 2]\n  /Root 1 0 R\n  /Size {}\n  /W [1 1 1]\n",
            limits.max_object_number,
            u64::from(limits.max_xref_size) + 1
        ),
        &["01 00 00", "01 0F 00", "01 12 00"],
    );
    // Nothing usable, and the recovery scan finds no trailer either.
    assert!(!opens(&file));
}

#[test]
fn an_archive_number_past_the_table_skips_only_that_entry() {
    let file = document(
        "  /Root 1 0 R\n  /Size 3\n  /W [1 1 1]\n",
        &["02 FF 00", "01 0F 00", "01 12 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(offsets(&xref), vec![(1, 15), (2, 18)]);
}

#[test]
fn a_dictionary_that_is_not_a_stream_is_not_a_section() {
    let mut file = Vec::new();
    file.extend_from_slice(b"%PDF1-7\n%\xa0\xf2\xa4\xf4\n");
    file.extend_from_slice(
        b"7 0 obj <<\n  /Filter /ASCIIHexDecode\n  /Root 1 0 R\n  /Size 3\n  /W [1 1 1]\n>>\n",
    );
    file.extend_from_slice(b"endobj\nstartxref\n14\n%%EOF\n");
    assert!(!opens(&file));
}

#[test]
fn a_negative_previous_offset_is_refused() {
    let file = document(
        "  /Root 1 0 R\n  /Size 3\n  /W [1 1 1]\n  /Prev -1\n",
        &["02 FF 00", "01 0F 00", "01 12 00"],
    );
    assert!(!opens(&file));
}

#[test]
fn a_negative_size_is_refused() {
    // The later `/Size` wins, and it is negative.
    let file = document(
        "  /Root 1 0 R\n  /Size 3\n  /W [1 1 1]\n  /Size -1\n",
        &["02 FF 00", "01 0F 00", "01 12 00"],
    );
    assert!(!opens(&file));
}

#[test]
fn a_zero_size_reads_an_empty_table() {
    let file = document(
        "  /Root 1 0 R\n  /Size 0\n  /W [1 1 1]\n  /Size 0\n",
        &["02 FF 00", "01 0F 00", "01 12 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert!(xref.is_empty());
}

#[test]
fn a_width_array_of_two_is_not_a_cross_reference_stream() {
    // Fewer than three field widths describes nothing, so the structured
    // path gives up. The recovery scan then finds the object by its header
    // and takes the trailer out of its `/Type /XRef` dictionary — which is
    // how such a file still opens.
    let file = document(
        "  /Type /XRef\n  /Root 1 0 R\n  /Size 3\n  /W [1 1]\n",
        &["02 FF 00", "01 0F 00", "01 12 00"],
    );
    let xref = read(&file).expect("recovered by scanning");
    assert!(xref.entry(7).is_some());

    // Without a trailer to recover, nothing is left to open.
    let hopeless = document(
        "  /Root 1 0 R\n  /Size 3\n  /W [1 1]\n",
        &["02 FF 00", "01 0F 00", "01 12 00"],
    );
    assert!(!opens(&hopeless));
}

#[test]
fn a_zero_width_type_field_means_every_entry_is_in_use() {
    let file = document(
        "  /Root 1 0 R\n  /Size 2\n  /W [0 1 1]\n",
        &["0F 00", "12 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(offsets(&xref), vec![(0, 15), (1, 18)]);
}

#[test]
fn an_index_names_which_objects_the_entries_describe() {
    let file = document(
        "  /Root 1 0 R\n  /Size 83\n  /Index [2 1 4 2 80 3]\n  /W [1 1 1]\n",
        &[
            "01 00 00", "01 0F 00", "01 12 00", "01 20 00", "01 22 00", "01 25 00",
        ],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(
        offsets(&xref),
        vec![(2, 0), (4, 15), (5, 18), (80, 32), (81, 34), (82, 37)]
    );
}

#[test]
fn overlapping_index_ranges_let_the_later_entry_win() {
    // The ranges (2,2) and (3,1) both describe object 3; the second range
    // reads the third entry, so that is what object 3 ends up as.
    let file = document(
        "  /Root 1 0 R\n  /Size 4\n  /Index [2 2 3 1]\n  /W [1 1 1]\n",
        &["01 00 00", "01 0F 00", "01 12 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(offsets(&xref), vec![(2, 0), (3, 18)]);
}

#[test]
fn index_ranges_need_not_be_in_order() {
    let file = document(
        "  /Root 1 0 R\n  /Size 5\n  /Index [3 2 2 1]\n  /W [1 1 1]\n",
        &["01 00 00", "01 0F 00", "01 12 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(offsets(&xref), vec![(2, 18), (3, 0), (4, 15)]);
}

#[test]
fn an_index_reaching_past_the_declared_size_still_loads() {
    // The ranges describe objects 2, 80 and 81, so `/Size` should be 82.
    let file = document(
        "  /Root 1 0 R\n  /Size 81\n  /Index [2 1 80 2]\n  /W [1 1 1]\n",
        &["01 00 00", "01 0F 00", "01 12 00"],
    );
    let xref = read(&file).expect("cross-reference stream");
    assert_eq!(offsets(&xref), vec![(2, 0), (80, 15), (81, 18)]);
}

#[test]
fn a_start_xref_pointing_at_a_stream_body_builds_no_table() {
    // `startxref` names offset 6, which is inside the header — the classic
    // failure mode of a file whose offsets were never updated.
    let file: &[u8] = b"%PDF1-7 0 obj <</Size 2 /W [0 0 0]\n>>\n\
                        stream\n\
                        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
                        endstream\n\
                        endobj\n\
                        startxref\n\
                        6\n\
                        %%EOF\n\0";
    let bytes: Arc<[u8]> = Arc::from(file);
    assert!(matches!(
        load(bytes, &LoadOptions::default()),
        Err(LoadError::Broken(_))
    ));
}

/// A two-section chain of cross-reference streams, base first then update.
///
/// Both sections are complete stream objects; the update's `/Prev` points at
/// the base, and `startxref` names the update. Offsets are computed rather
/// than written by hand so the entries actually name their own objects.
fn chained(base_dict: &str, base: &[&str], update_dict: &str, update: &[&str]) -> Vec<u8> {
    let section = |num: u32, dict: &str, entries: &[&str]| {
        let mut out = format!("{num} 0 obj <<\n  /Filter /ASCIIHexDecode\n{dict}>>\nstream\n");
        for entry in entries {
            out.push_str(entry);
            out.push('\n');
        }
        out.push_str("endstream\nendobj\n");
        out
    };
    let mut out = String::from("%PDF-1.7\n%\u{a0}\u{f2}\u{a4}\u{f4}\n");
    let base_at = out.len();
    out.push_str(&section(7, base_dict, base));
    let update_at = out.len();
    out.push_str(&section(
        8,
        &format!("{update_dict}  /Prev {base_at}\n"),
        update,
    ));
    let _ = write!(out, "startxref\n{update_at}\n%%EOF\n");
    out.into_bytes()
}

#[test]
fn an_update_section_wins_over_the_base_for_an_object_both_describe() {
    // The ordinary incremental-update rule, and the one that must survive any
    // change to how the chain is walked: `bug_781804.pdf` is the corpus file
    // whose `/Info` object is described by both its newest and its oldest
    // cross-reference stream, and reading the older one gives a document the
    // wrong modification date.
    let file = chained(
        "  /Root 1 0 R\n  /Size 6\n  /Index [5 1]\n  /W [1 1 1]\n",
        &["01 AA 00"],
        "  /Root 1 0 R\n  /Size 6\n  /Index [5 1]\n  /W [1 1 1]\n",
        &["01 BB 00"],
    );
    let xref = read(&file).expect("a chain of cross-reference streams");
    assert_eq!(xref.entry(5), Some(Entry::Offset(0xBB)));
}

#[test]
fn a_base_entry_survives_a_newer_sections_size_phantom() {
    // `/Size` materializes a free entry at its last slot even when no section
    // wrote one. When an update names only a few objects, that phantom must
    // not bury an object the base section really describes:
    // `pixel/bug_345274934.pdf` is exactly this shape, and losing object 4
    // costs it its only page.
    let file = chained(
        "  /Root 1 0 R\n  /Size 6\n  /W [1 1 1]\n",
        &[
            "00 00 00", "01 11 00", "01 22 00", "01 33 00", "01 44 00", "01 55 00",
        ],
        // The update speaks only about object 2, but claims six objects, so
        // its phantom lands on object 5.
        "  /Root 1 0 R\n  /Size 6\n  /Index [2 1]\n  /W [1 1 1]\n",
        &["01 99 00"],
    );
    let xref = read(&file).expect("a chain of cross-reference streams");
    assert_eq!(xref.entry(2), Some(Entry::Offset(0x99)), "the update wins");
    assert_eq!(
        xref.entry(5),
        Some(Entry::Offset(0x55)),
        "the base's last object survives the update's phantom"
    );
}
