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
use pdfrum_object::Resolve as _;
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

/// A hybrid-reference file: a base classic table, then an update section
/// carrying *both* a classic table and a cross-reference stream.
///
/// The update's plain table names only what an old reader must see; its
/// trailer points a modern reader at the stream through `/XRefStm`. Every
/// object the tables name is really written, because the reader verifies that
/// a table's first entry finds its own object header.
///
/// Objects 5 and 6 exist twice — an original and a revision — and the three
/// sections disagree about which copy is current. Returns the file and the
/// four offsets, in the order (old 5, old 6, new 5, new 6).
fn hybrid(update_table_names_stale_six: bool) -> (Vec<u8>, [usize; 4]) {
    let mut out = String::from("%PDF-1.7\n%\u{a0}\u{f2}\u{a4}\u{f4}\n");
    let obj = |out: &mut String, num: u32, body: &str| {
        let at = out.len();
        let _ = write!(out, "{num} 0 obj << {body} >>\nendobj\n");
        at
    };
    let old_five = obj(&mut out, 5, "/Version 1");
    let old_six = obj(&mut out, 6, "/Version 1");

    let table = |entries: &[(u32, usize)]| {
        let mut t = String::from("xref\n");
        for (num, offset) in entries {
            let _ = write!(t, "{num} 1\n{offset:010} 00000 n \n");
        }
        t
    };

    let base_at = out.len();
    out.push_str(&table(&[(5, old_five), (6, old_six)]));
    out.push_str("trailer << /Root 1 0 R /Size 9 >>\n");

    // The update's two revised objects, then the stream that finds them.
    let new_five = obj(&mut out, 5, "/Version 2");
    let new_six = obj(&mut out, 6, "/Version 2");

    let stream_at = out.len();
    let _ = write!(
        out,
        "8 0 obj <<\n  /Type /XRef\n  /Filter /ASCIIHexDecode\n  /Root 1 0 R\n  \
         /Size 9\n  /Index [5 2]\n  /W [1 2 1]\n>>\nstream\n\
         01 {new_five:04X} 00\n01 {new_six:04X} 00\nendstream\nendobj\n"
    );

    // The update's plain table speaks only about object 6, so object 5's
    // revision is reachable through the `/XRefStm` alone.
    let update_at = out.len();
    let six = if update_table_names_stale_six {
        old_six
    } else {
        new_six
    };
    out.push_str(&table(&[(6, six)]));
    let _ = write!(
        out,
        "trailer << /Root 1 0 R /Size 9 /Prev {base_at} /XRefStm {stream_at} >>\n\
         startxref\n{update_at}\n%%EOF\n"
    );
    (out.into_bytes(), [old_five, old_six, new_five, new_six])
}

#[test]
fn the_newest_sections_xref_stm_is_honoured() {
    // `pixel/bug_1484283.pdf` in miniature. Its second revision's plain table
    // lists only the objects it rewrote in place; the object it *revised* —
    // there a `/Pages` node with a new `/MediaBox` — moved into an object
    // stream and is reachable only through the trailer's `/XRefStm`. Skipping
    // that pointer leaves the object at its first-revision version, with no
    // symptom other than a wrong answer.
    let (file, [_, _, new_five, new_six]) = hybrid(false);
    let xref = read(&file).expect("a hybrid chain");
    assert_eq!(
        xref.entry(5),
        Some(Entry::Offset(new_five as u64)),
        "the newest /XRefStm must revise object 5, which no plain table names"
    );
    assert_eq!(
        xref.entry(6),
        Some(Entry::Offset(new_six as u64)),
        "and object 6, which the update's plain table names identically"
    );
}

#[test]
fn a_plain_table_still_beats_its_own_sections_xref_stm() {
    // The other half of ISO 32000-1 §7.5.8.4: within one section the stream
    // is merged in *first* so the table can overwrite it. Pointing the
    // update's table at the stale copy of object 6 makes that order visible —
    // the table's answer must survive the stream's, and honouring the newest
    // `/XRefStm` must not quietly invert that.
    let (file, [_, old_six, new_five, _]) = hybrid(true);
    let xref = read(&file).expect("a hybrid chain");
    assert_eq!(
        xref.entry(6),
        Some(Entry::Offset(old_six as u64)),
        "the update's plain table beats its own /XRefStm"
    );
    assert_eq!(
        xref.entry(5),
        Some(Entry::Offset(new_five as u64)),
        "an object only the stream names is unaffected"
    );
}

/// Build `bug_717.pdf`'s shape: a hybrid-reference file whose classic table
/// marks the compressed objects **free** while its `/XRefStm` names them as
/// members of an object stream.
///
/// This is what ISO 32000-1 §7.5.8.4 asks a writer to produce. A reader that
/// predates object streams sees the compressed objects as absent — which is
/// better than seeing them as garbage — and a reader that follows the
/// `/XRefStm` finds them for real. The two descriptions are meant to
/// disagree, and the free half is the one that must lose.
///
/// Written as **two** sections, like the real file: a base table and an
/// update whose trailer carries the `/XRefStm`. That is not decoration. The
/// first section's hybrid pointer is deliberately skipped — §7.5.8.4 says an
/// `/XRefStm` belongs to an update section, and `cpdf_parser.cpp:452-457`
/// skips `xref_stream_list.front()` accordingly — so a single-section file
/// would never read the stream at all and would prove nothing.
///
/// Returns the file and the offset of the object stream's own header.
fn hybrid_freeing_its_compressed_objects() -> (Vec<u8>, usize) {
    let mut out = String::from("%PDF-1.5\n%\u{a0}\u{f2}\u{a4}\u{f4}\n");
    let plain = {
        let at = out.len();
        let _ = write!(out, "1 0 obj << /Type /Catalog /Pages 6 0 R >>\nendobj\n");
        at
    };
    // A page tree, so the first load attempt succeeds and the file is read
    // through its chain. Without one the loader falls back to the recovery
    // scan, which finds the compressed objects by other means and would make
    // the assertions below pass whatever the table said.
    let pages = out.len();
    let _ = write!(
        out,
        "6 0 obj << /Type /Pages /Count 1 /Kids [7 0 R] >>\nendobj\n"
    );
    let page = out.len();
    let _ = write!(
        out,
        "7 0 obj << /Type /Page /Parent 6 0 R /MediaBox [0 0 10 10] >>\nendobj\n"
    );

    // Object stream 4 holding objects 2 and 3, exactly as a Word-produced
    // file packs its structure tree away.
    let two = "<< /Kind /Two >> ";
    let three = "<< /Kind /Three >>";
    let header = format!("2 0 3 {} ", two.len());
    let first = header.len();
    let payload = format!("{header}{two}{three}");
    let archive = out.len();
    let _ = write!(
        out,
        "4 0 obj << /Type /ObjStm /N 2 /First {first} /Length {} >>\nstream\n{payload}\n\
         endstream\nendobj\n",
        payload.len()
    );

    // The `/XRefStm`: object 1 in place, objects 2 and 3 compressed into 4.
    let stream_at = out.len();
    let _ = write!(
        out,
        "5 0 obj <<\n  /Type /XRef\n  /Filter /ASCIIHexDecode\n  /Root 1 0 R\n  \
         /Size 8\n  /Index [1 4]\n  /W [1 2 1]\n>>\nstream\n\
         01 {plain:04X} 00\n02 0004 00\n02 0004 01\n01 {archive:04X} 00\n\
         endstream\nendobj\n"
    );

    // The base section: everything but the two compressed objects.
    let base_at = out.len();
    let _ = write!(
        out,
        "xref\n0 2\n\
         0000000000 65535 f \n\
         {plain:010} 00000 n \n\
         4 1\n\
         {archive:010} 00000 n \n\
         6 2\n\
         {pages:010} 00000 n \n\
         {page:010} 00000 n \n\
         trailer << /Root 1 0 R /Size 8 >>\n\
         startxref\n{base_at}\n%%EOF\n"
    );

    // The update section, whose free entries are the ones under test.
    // Objects 2 and 3 are written as a free-list chain at generation 65535 —
    // the spelling a real hybrid file uses — while the `/XRefStm` its own
    // trailer names says they are members of object stream 4.
    let table_at = out.len();
    let _ = write!(
        out,
        "xref\n0 8\n\
         0000000002 65535 f \n\
         {plain:010} 00000 n \n\
         0000000003 65535 f \n\
         0000000000 65535 f \n\
         {archive:010} 00000 n \n\
         0000000000 65535 f \n\
         {pages:010} 00000 n \n\
         {page:010} 00000 n \n\
         trailer << /Root 1 0 R /Size 8 /Prev {base_at} /XRefStm {stream_at} >>\n\
         startxref\n{table_at}\n%%EOF\n"
    );
    (out.into_bytes(), archive)
}

#[test]
fn a_classic_tables_free_entry_does_not_undo_the_xref_stms_compressed_one() {
    // `bug_717.pdf`. Its classic table lists objects 8 through 15 as free
    // while its `/XRefStm` puts all eight inside object stream 14; honouring
    // the free entries loses the document's whole structure tree, because
    // the table is applied *after* the stream so that it can revise it.
    //
    // The rule that makes this come out right is that a classic table's free
    // entry never reaches the map at all: PDFium's subsection reader does
    // not parse the generation field of a free entry, and its merge only
    // frees a slot when that unparsed generation is non-zero.
    let (file, archive) = hybrid_freeing_its_compressed_objects();
    let xref = read(&file).expect("a hybrid chain");
    for (num, index) in [(2u32, 0u32), (3, 1)] {
        assert_eq!(
            xref.entry(num),
            Some(Entry::InObjStream {
                stream: pdfrum_object::ObjRef::new(4, 0),
                index,
            }),
            "object {num} must stay compressed, not be freed by the table"
        );
    }
    assert_eq!(xref.entry(4), Some(Entry::Offset(archive as u64)));
}

#[test]
fn the_objects_such_a_file_hides_in_its_stream_actually_resolve() {
    // The same file read the whole way: the entries surviving is only worth
    // something if the objects behind them come back.
    let (file, _) = hybrid_freeing_its_compressed_objects();
    let doc = load(Arc::from(file), &LoadOptions::default()).expect("the document loads");
    // Through the cross-reference information, not by scanning: the recovery
    // scan finds these objects either way, so without this the assertions
    // below would pass even with the free entries honoured.
    assert!(
        !doc.xref_was_rebuilt(),
        "the chain must carry this file, or the test proves nothing"
    );
    for (num, kind) in [(2u32, "Two"), (3, "Three")] {
        let object = doc
            .fetch(pdfrum_object::ObjRef::new(num, 0))
            .expect("the object resolves");
        let name = object
            .as_dict()
            .and_then(|d| d.get(&pdfrum_object::Name::new(b"Kind"), &doc))
            .and_then(|v| v.as_name().map(|n| n.as_bytes().to_vec()));
        assert_eq!(
            name.as_deref(),
            Some(kind.as_bytes()),
            "object {num} must resolve out of the object stream"
        );
    }
}
