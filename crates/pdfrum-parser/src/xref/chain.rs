//! Finding the cross-reference sections and reading them in the right order
//! (ISO 32000-1 §7.5.5 and §7.5.6).
//!
//! # Newest first, applied last
//!
//! A file records its edits as a chain: the newest section sits at the end,
//! and each links back to the one before through `/Prev`. The walk therefore
//! visits sections newest-first, but the *entries* have to end up with the
//! newest version of each object winning. Both are satisfied by loading the
//! oldest section first and merging every newer one on top of it, which is
//! why the walk collects offsets before reading anything.
//!
//! # Hybrids
//!
//! A file can carry both a classic table and a cross-reference stream for the
//! same section, so that old readers see the table and new ones see the
//! stream. When both exist the table's entries win, and the stream's are
//! merged in first.
//!
//! The **oldest** section's `/XRefStm` is the one that is ignored, not the
//! newest's. ISO 32000-1 §7.5.8.4 puts hybrid information in update sections,
//! and the base of the chain is not an update. Honoring the newest section's
//! pointer is what makes a hybrid file work at all: its plain table lists
//! only the objects an old reader must see, and every object the update
//! *revised* — a `/Pages` node with a new `/MediaBox`, say — reaches a modern
//! reader through the `/XRefStm` alone.

use std::sync::Arc;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{NoResolve, Object, names};

use crate::error::Error;
use crate::lexer::{Lexer, Token, atoi64};
use crate::syntax::{Context, Strictness, indirect};
use crate::xref::{Trailer, Xref, classic, merge_into_walk, merge_trailers, rebuild, stream};

/// The smallest offset that could name a cross-reference section: nothing
/// useful fits in the bytes a header occupies.
const MIN_XREF_OFFSET: i64 = 9;

/// One section of the chain: where its table and its stream are.
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    /// Offset of a classic table, or zero.
    table: usize,
    /// Offset of a cross-reference stream, or zero.
    stream: usize,
}

/// Read a document's cross-reference information.
///
/// Tries the structured chain first and falls back to a full-file scan; the
/// returned flag says which happened, because a rebuilt table means the file
/// cannot be incrementally saved.
pub(crate) fn load(
    file: &[u8],
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<(Xref, Trailer, XrefShape), Error> {
    let shared: Arc<[u8]> = Arc::from(file);
    let start = start_xref(file, limits, diags);

    let mut xref = Xref::new();
    let mut trailer = Trailer::default();

    if start >= MIN_XREF_OFFSET
        && let Ok(pos) = usize::try_from(start)
    {
        // The probe that decides table-versus-stream is the same one
        // `read_chain` runs; doing it here keeps the answer even when the
        // chain load later fails and we fall through to the rebuild.
        let main_is_stream = !probe_is_table(&shared, pos, limits, diags);
        if read_chain(&shared, pos, &mut xref, &mut trailer, limits, diags) {
            return Ok((
                xref,
                trailer,
                XrefShape {
                    rebuilt: false,
                    last_offset: u64::try_from(start).unwrap_or(0),
                    main_is_stream,
                },
            ));
        }
    }

    diags.record(Severity::Recovered, DiagKind::XrefRebuilt, None);
    if rebuild::rebuild(&shared, &mut xref, &mut trailer, limits, diags, &NoResolve) {
        // A rebuilt table has no previous section to chain from: offset zero
        // is what makes an incremental save rewrite the whole table instead
        // of emitting a `/Prev` that names nothing.
        Ok((xref, trailer, XrefShape::rebuilt()))
    } else {
        Err(Error::XrefBroken)
    }
}

/// What shape the cross-reference the load actually used had.
///
/// Three facts a *writer* needs and a reader does not: whether the table was
/// reconstructed by scanning, where the newest section starts (an incremental
/// update's `/Prev`), and whether that section was a stream (which decides
/// between emitting a classic table and folding the whole cross-reference
/// into a stream object).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct XrefShape {
    pub rebuilt: bool,
    pub last_offset: u64,
    pub main_is_stream: bool,
}

impl XrefShape {
    /// The shape of a table that was scanned out of the file body.
    pub(crate) const fn rebuilt() -> Self {
        Self {
            rebuilt: true,
            last_offset: 0,
            main_is_stream: false,
        }
    }
}

/// Does a classic `xref` table live at `pos`?
fn probe_is_table(file: &Arc<[u8]>, pos: usize, limits: &Limits, diags: &mut Diagnostics) -> bool {
    let mut probe = Xref::new();
    classic::parse_table(file, pos, true, &mut probe, limits, diags).is_some()
}

/// Find the offset `startxref` names, or zero.
///
/// The keyword is searched for backwards from the end of the file within a
/// fixed window, because everything after it is optional and files append
/// junk there. An offset at or past the end of the file names nothing.
pub(crate) fn start_xref(file: &[u8], limits: &Limits, diags: &mut Diagnostics) -> i64 {
    let from = file.len().saturating_sub(9);
    let window = usize::try_from(limits.startxref_scan).unwrap_or(usize::MAX);
    let mut lx = Lexer::at(file, from);
    let bad = |diags: &mut Diagnostics| {
        diags.record(Severity::Recovered, DiagKind::BadStartXref, None);
        0
    };

    if !lx.search_back(b"startxref", window) {
        return bad(diags);
    }
    // Step over the keyword itself before reading the number.
    let _ = lx.next_word(limits);
    match lx.next_word(limits) {
        Token::Number(word) => {
            let offset = atoi64(word);
            if u64::try_from(offset).is_ok_and(|o| o < file.len() as u64) {
                offset
            } else {
                bad(diags)
            }
        }
        _ => bad(diags),
    }
}

/// Read the whole chain starting at `pos`.
///
/// Returns false when anything about it is unusable, which sends the caller
/// to the rebuild. Everything read so far is left in `xref`, because the
/// rebuild overlays rather than replaces.
pub(crate) fn read_chain(
    file: &Arc<[u8]>,
    pos: usize,
    xref: &mut Xref,
    trailer: &mut Trailer,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> bool {
    // Probe: does a classic table live here?
    let main_is_table = probe_is_table(file, pos, limits, diags);

    let Some(sections) = walk(file, pos, main_is_table, xref, trailer, limits, diags) else {
        return false;
    };

    // The oldest section is loaded first and verified; everything newer is
    // merged on top of it.
    let Some((oldest, newer)) = sections.split_first() else {
        return !xref.is_empty();
    };

    if oldest.table > 0 {
        if classic::parse_table(file, oldest.table, false, xref, limits, diags).is_none() {
            return false;
        }
        // The one sanity check on a table, and it runs against the whole
        // accumulation rather than the section alone: the first entry with a
        // real offset must find its own object number at that offset.
        if !classic::verify_table(file, xref, limits) {
            diags.record(
                Severity::Suspicious,
                DiagKind::XrefEntriesShifted,
                Some(oldest.table as u64),
            );
            return false;
        }
    }

    // An update section that will not load abandons the whole chain rather
    // than leaving a partial table: the file has already proved unreliable,
    // and the recovery scan reads more of it than half a chain does.
    for section in newer {
        // Within a section the table wins, so the stream is applied first.
        // Both are applied *directly* onto the accumulation rather than
        // merged under it: this pass runs oldest-first, so a later iteration
        // is a newer section and is meant to overwrite. The newest section is
        // re-applied here even though the discovery walk already read it,
        // which is what makes its entries win over everything the walk laid
        // down in the opposite order.
        if section.stream > 0
            && read_stream_section(file, section.stream, false, xref, limits, diags).is_none()
        {
            return false;
        }
        if section.table > 0
            && classic::parse_table(file, section.table, false, xref, limits, diags).is_none()
        {
            return false;
        }
    }

    !xref.is_empty() || !trailer.dict.is_empty()
}

/// Walk the `/Prev` chain, collecting section offsets oldest-first.
///
/// Stream sections have their entries read during the walk — a stream's
/// `/Prev` is inside the stream, so there is no way to learn where to go next
/// without reading it. Classic sections only have their offsets recorded.
fn walk(
    file: &Arc<[u8]>,
    main: usize,
    main_is_table: bool,
    xref: &mut Xref,
    trailer: &mut Trailer,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Vec<Section>> {
    let mut sections: Vec<Section> = Vec::new();
    let mut seen: Vec<usize> = vec![main];

    // The newest section, read for its trailer and (if a stream) entries.
    let mut next = if main_is_table {
        let end = classic::parse_table(file, main, true, &mut Xref::new(), limits, diags)?;
        let dict = classic::read_trailer(file, end, limits, diags, &NoResolve)?;
        let prev = dict.direct_int(names::PREV).unwrap_or(0);
        let hybrid = dict.int(names::XREF_STM, &NoResolve).unwrap_or(0);
        // This is the newest section, so its trailer simply becomes the
        // accumulation rather than being merged against one.
        merge_trailers(
            trailer,
            &Trailer {
                dict,
                object_number: 0,
            },
        );
        // The trailer's `/Size` sizes the table now, before any section is
        // read — running it afterwards would truncate away entries that
        // update sections legitimately added above it.
        apply_size(xref, trailer, limits);
        sections.push(Section {
            table: main,
            stream: usize::try_from(hybrid).unwrap_or(0),
        });
        prev
    } else {
        // A main cross-reference stream sizes the table *before* its entries
        // are read, not after, so an entry whose object number equals the
        // declared `/Size` survives rather than being truncated away.
        let mut entries = Xref::new();
        let read = read_stream_section(file, main, true, &mut entries, limits, diags)?;
        xref.merge_up(&entries);
        merge_trailers(trailer, &read.trailer);
        sections.push(Section {
            table: 0,
            stream: main,
        });
        read.prev
    };

    while next > 0 {
        let Ok(pos) = usize::try_from(next) else {
            return None;
        };
        if seen.contains(&pos) {
            // A chain that points back at a section it already visited would
            // never terminate; the file is lying about its history.
            diags.record(
                Severity::Suspicious,
                DiagKind::XrefPrevLoop,
                Some(pos as u64),
            );
            return None;
        }
        seen.push(pos);

        // The discovery walk loads each stream section it finds, applying its
        // entries directly onto the accumulation: an entry an older section
        // declares overwrites whatever a newer one left at that object
        // number, phantoms from a newer `/Size` included. That inversion is
        // deliberate in the C++ and is undone afterwards, because every
        // section — the newest included — is applied a second time in
        // newest-last order once the chain is known.
        if let Some(read) = read_stream_section(file, pos, false, xref, limits, diags) {
            merge_into_walk(trailer, &read.trailer);
            sections.insert(
                0,
                Section {
                    table: 0,
                    stream: pos,
                },
            );
            next = read.prev;
            continue;
        }

        // A classic section: record where its table and its hybrid stream
        // are, and read its trailer for the next pointer. A skip-scan that
        // does not find a clean table is not fatal — what matters is that a
        // trailer follows, so the scan's end position is used either way and
        // only a missing trailer ends the walk.
        let end =
            classic::parse_table(file, pos, true, &mut Xref::new(), limits, diags).unwrap_or(pos);
        let dict = classic::read_trailer(file, end, limits, diags, &NoResolve)?;
        let prev = dict.direct_int(names::PREV).unwrap_or(0);
        // `/XRefStm` is the one key here read through an accessor that
        // *would* follow a reference — but the store it would ask has no
        // usable table yet, so an indirect one reads as absent either way.
        // Written with a resolver rather than without to keep the
        // distinction from `/Prev` above, which never resolves.
        let hybrid = dict.int(names::XREF_STM, &NoResolve).unwrap_or(0);
        merge_into_walk(
            trailer,
            &Trailer {
                dict,
                object_number: 0,
            },
        );
        sections.insert(
            0,
            Section {
                table: pos,
                stream: usize::try_from(hybrid).unwrap_or(0),
            },
        );
        next = prev;
    }

    Some(sections)
}

/// Parse the cross-reference stream at `pos` and read its entries.
fn read_stream_section(
    file: &Arc<[u8]>,
    pos: usize,
    is_main: bool,
    xref: &mut Xref,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<stream::XrefStream> {
    let mut lx = Lexer::at(file, pos);
    let mut ctx = Context {
        limits,
        diags,
        file: Some(file),
        store: Some(&NoResolve),
    };
    let parsed = indirect(&mut lx, &mut ctx, Strictness::Loose, 0).ok()?;
    if parsed.num == 0 {
        return None;
    }
    let Object::Stream(s) = parsed.object else {
        return None;
    };
    // The entries are compressed like any other stream's payload — but a
    // chain ending at an image codec yields no fields to read, and such a
    // stream is not a cross-reference section at all.
    let decoded = crate::decode::structural_bytes(&s, &NoResolve, limits, diags)?;
    stream::read_xref_stream(&s, parsed.num, &decoded, is_main, xref, limits, diags)
}

/// Honor the trailer's `/Size`, which is a claim about how many objects the
/// document has.
///
/// Read without resolving: an indirect `/Size` is ignored, because the table
/// that would resolve it is the one being sized.
fn apply_size(xref: &mut Xref, trailer: &Trailer, limits: &Limits) {
    let Some(size) = trailer.dict.direct_int(names::SIZE) else {
        return;
    };
    if size >= 1
        && size <= i64::from(limits.max_xref_size)
        && let Ok(size) = u32::try_from(size)
    {
        xref.set_size(size);
    }
}

#[cfg(test)]
mod tests {
    use super::{load, start_xref};
    use crate::xref::Entry;
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::names;

    fn read(file: &[u8]) -> Option<(crate::xref::Xref, crate::xref::Trailer, bool, Diagnostics)> {
        shape(file).map(|(x, t, s, d)| (x, t, s.rebuilt, d))
    }

    fn shape(
        file: &[u8],
    ) -> Option<(
        crate::xref::Xref,
        crate::xref::Trailer,
        super::XrefShape,
        Diagnostics,
    )> {
        let mut diags = Diagnostics::default();
        load(file, &Limits::default(), &mut diags)
            .ok()
            .map(|(x, t, s)| (x, t, s, diags))
    }

    #[test]
    fn start_xref_reads_the_last_one() {
        let mut diags = Diagnostics::default();
        let file = b"junk\nstartxref\n17\n%%EOF\n";
        assert_eq!(start_xref(file, &Limits::default(), &mut diags), 17);
    }

    #[test]
    fn a_missing_start_xref_reads_as_zero() {
        let mut diags = Diagnostics::default();
        assert_eq!(
            start_xref(b"no marker here", &Limits::default(), &mut diags),
            0
        );
        assert!(diags.contains(&DiagKind::BadStartXref));
    }

    #[test]
    fn an_offset_past_the_file_reads_as_zero() {
        let mut diags = Diagnostics::default();
        let file = b"startxref\n99999\n%%EOF\n";
        assert_eq!(start_xref(file, &Limits::default(), &mut diags), 0);
    }

    #[test]
    fn a_classic_chain_loads() {
        // A minimal file with a real table.
        let file = build_classic();
        let (xref, trailer, rebuilt, _) = read(&file).expect("loaded");
        assert!(!rebuilt);
        assert!(matches!(xref.entry(1), Some(Entry::Offset(_))));
        assert!(trailer.dict.raw(names::ROOT).is_some());
    }

    #[test]
    fn a_start_xref_ending_at_the_search_origin_is_still_found() {
        // The backward search starts nine bytes from the end, so a file whose
        // `startxref` keyword ends exactly there sits on the boundary: its
        // last byte is the very byte the search begins at. Such a file is
        // truncated — no `%%EOF`, no trailing end-of-line, just the offset —
        // and reading its table rather than rebuilding depends on the search
        // including that byte.
        let base = build_classic();
        let text = String::from_utf8_lossy(&base).into_owned();
        let Some((head, tail)) = text.rsplit_once("startxref\n") else {
            panic!("the builder writes a startxref");
        };
        let Some((offset, _)) = tail.split_once('\n') else {
            panic!("the offset is on its own line");
        };
        // "startxref" + one separator + the offset, and nothing after it.
        // The search begins nine bytes from the end, so eight bytes have to
        // follow the keyword's last byte for that byte to *be* the origin:
        // one separator and a seven-digit offset.
        let padded = format!("{offset:0>7}");
        let truncated = format!("{head}startxref {padded}");

        let origin = truncated.len() - 9;
        assert_eq!(
            truncated.as_bytes().get(origin - 8..=origin),
            Some(&b"startxref"[..]),
            "the keyword must end at the search origin"
        );

        let (xref, trailer, rebuilt, _) = read(truncated.as_bytes()).expect("loaded");
        assert!(!rebuilt, "the table should be read, not rebuilt");
        assert!(matches!(xref.entry(1), Some(Entry::Offset(_))));
        assert!(trailer.dict.raw(names::ROOT).is_some());
    }

    #[test]
    fn a_broken_start_xref_falls_back_to_the_scan() {
        // Point `startxref` at the middle of the file, where no table is.
        let text = String::from_utf8_lossy(&build_classic()).into_owned();
        let Some((head, tail)) = text.rsplit_once("startxref\n") else {
            panic!("the builder writes a startxref");
        };
        let Some((_, after)) = tail.split_once('\n') else {
            panic!("the offset is on its own line");
        };
        let broken = format!("{head}startxref\n30\n{after}");

        let (xref, trailer, rebuilt, diags) = read(broken.as_bytes()).expect("loaded");
        assert!(rebuilt);
        assert!(xref.entry(1).is_some());
        assert!(trailer.dict.raw(names::ROOT).is_some());
        assert!(diags.contains(&DiagKind::XrefRebuilt));
    }

    #[test]
    fn a_file_with_nothing_readable_fails() {
        assert!(read(b"not a pdf at all").is_none());
    }

    // The three facts the writer reads out of the load (SPEC §5, 2026-08-29).
    #[test]
    fn a_classic_chain_reports_its_offset_and_that_it_is_not_a_stream() {
        let file = build_classic();
        let (_, _, shape, _) = shape(&file).expect("loaded");
        assert!(!shape.rebuilt);
        assert!(!shape.main_is_stream);
        // The offset names the `xref` keyword, which the file really holds.
        let at = usize::try_from(shape.last_offset).expect("fits");
        assert_eq!(file.get(at..at + 4), Some(&b"xref"[..]));
    }

    #[test]
    fn a_rebuilt_table_reports_offset_zero() {
        // Point `startxref` where no table is, forcing the scan.
        let text = String::from_utf8_lossy(&build_classic()).into_owned();
        let Some((head, tail)) = text.rsplit_once("startxref\n") else {
            panic!("the builder writes a startxref");
        };
        let Some((_, after)) = tail.split_once('\n') else {
            panic!("the offset is on its own line");
        };
        let broken = format!("{head}startxref\n30\n{after}");

        let (_, _, shape, _) = shape(broken.as_bytes()).expect("loaded");
        assert!(shape.rebuilt);
        // Zero here is a value, not an absence: it is what tells an
        // incremental save it has no previous section to chain from.
        assert_eq!(shape.last_offset, 0);
        assert!(!shape.main_is_stream);
    }

    /// A tiny well-formed document with a classic table.
    fn build_classic() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n");
        let obj1 = out.len();
        out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let obj2 = out.len();
        out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        let xref_at = out.len();
        out.extend_from_slice(b"xref\n0 3\n");
        out.extend_from_slice(b"0000000000 65535 f \n");
        out.extend_from_slice(format!("{obj1:010} 00000 n \n").as_bytes());
        out.extend_from_slice(format!("{obj2:010} 00000 n \n").as_bytes());
        out.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n");
        out.extend_from_slice(format!("{xref_at}\n").as_bytes());
        out.extend_from_slice(b"%%EOF\n");
        out
    }
}
