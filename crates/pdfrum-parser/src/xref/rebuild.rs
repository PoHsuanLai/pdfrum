//! Reconstructing the cross-reference table by reading the whole file
//! (no ISO section — the specification assumes files are not broken).
//!
//! # Why this exists
//!
//! Every structured path into a PDF depends on a byte offset being right,
//! and byte offsets are exactly what a truncated download, a careless text
//! editor, or a tool that rewrote objects without rewriting the table gets
//! wrong. This scan ignores all of it: it reads the file front to back
//! looking for `N G obj` headers and believes what it finds. It is the reason
//! a reader can open files nothing else will.
//!
//! # The two-number memory
//!
//! The scan keeps the last two number tokens it saw, with where each started.
//! When the word `obj` arrives, those two numbers are the object's number and
//! generation, and the *first* of them is where the object begins. Any
//! non-number word clears the memory, so `1 0 junk obj` records nothing.
//!
//! # What the scan refuses to be fooled by
//!
//! String bodies are skipped wholesale, because a document containing the
//! text `1 0 obj` inside a string would otherwise acquire a phantom object at
//! that offset. Object bodies are parsed *strictly* and the scan continues
//! from wherever the parse ended, so a nested `N G obj` inside a damaged body
//! is stepped over rather than mistaken for a real header.

use std::sync::Arc;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Object, Resolve, names};

use crate::lexer::{Delim, Lexer, Token, atoui};
use crate::syntax::{Context, Strictness, indirect};
use crate::xref::{Trailer, Xref, merge_trailers};

/// One number token the scan is holding onto.
#[derive(Debug, Clone, Copy)]
struct PendingNumber {
    /// The value.
    value: u32,
    /// Where its first byte is.
    at: usize,
}

/// Scan `file` for object headers and trailers, producing a table.
///
/// The result overlays whatever `xref` already held: entries found here win,
/// so a partial table assembled before the scan keeps the objects the scan
/// did not find. Succeeds only when both a trailer and at least one object
/// turned up — a file with objects but nothing naming a catalog is not a
/// document anyone can open.
pub(crate) fn rebuild<R: Resolve + ?Sized>(
    file: &Arc<[u8]>,
    xref: &mut Xref,
    trailer: &mut Trailer,
    limits: &Limits,
    diags: &mut Diagnostics,
    store: &R,
) -> bool {
    let mut found = Xref::new();
    let mut found_trailer = Trailer::default();
    let mut has_trailer = false;

    let mut lx = Lexer::new(file);
    // The last two numbers seen, oldest first.
    let mut numbers: Vec<PendingNumber> = Vec::with_capacity(2);

    loop {
        let word_start = lx.pos();
        let token = lx.next_word(limits);
        match token {
            Token::Eof => break,
            Token::Number(word) => {
                if numbers.len() == 2 {
                    numbers.remove(0);
                }
                numbers.push(PendingNumber {
                    value: atoui(word),
                    at: word_start_of(&lx, word_start),
                });
                continue;
            }
            // String bodies are content, not syntax: skip them so their text
            // cannot be read as object headers.
            Token::Delim(Delim::StringOpen) => {
                let _ = lx.read_literal_string();
            }
            Token::Delim(Delim::HexOpen) => {
                let _ = lx.read_hex_string();
            }
            Token::Keyword(b"trailer") => {
                if let Some(dict) = read_trailer_body(&mut lx, file, limits, diags, store) {
                    merge_trailers(
                        &mut found_trailer,
                        &Trailer {
                            dict,
                            object_number: 0,
                        },
                    );
                    has_trailer = true;
                }
            }
            Token::Keyword(b"obj") => {
                if let [first, second] = numbers.as_slice() {
                    let (obj_num, generation, at) = (
                        first.value,
                        u16::try_from(second.value).unwrap_or(u16::MAX),
                        first.at,
                    );
                    if record_object(
                        &mut lx,
                        file,
                        at,
                        obj_num,
                        generation,
                        &mut found,
                        &mut found_trailer,
                        &mut has_trailer,
                        limits,
                        diags,
                        store,
                    ) {
                        // Parsing continued past the object body.
                    }
                }
            }
            _ => {}
        }
        numbers.clear();
    }

    if !has_trailer || found.is_empty() {
        return false;
    }

    diags.record(Severity::Recovered, DiagKind::XrefRebuilt, None);
    xref.merge_up(&found);
    merge_trailers(trailer, &found_trailer);
    true
}

/// Where a word began, given the lexer's position before it was read.
///
/// Whitespace and comments sit between that position and the word itself, so
/// the recorded offset is the first non-skipped byte.
fn word_start_of(lx: &Lexer<'_>, before: usize) -> usize {
    let mut probe = Lexer::at(lx.bytes(), before);
    probe.skip_to_word();
    probe.pos()
}

/// Parse the object at `at` and record what it says.
///
/// Returns whether anything was recorded. The entry is added even when the
/// body will not parse: the header is evidence enough that an object lives
/// there, and a later fetch can try again.
#[expect(
    clippy::too_many_arguments,
    reason = "the scan's whole state has to reach the recorder; bundling it \
              into a struct would only rename the same parameters"
)]
fn record_object<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    file: &Arc<[u8]>,
    at: usize,
    obj_num: u32,
    generation: u16,
    found: &mut Xref,
    trailer: &mut Trailer,
    has_trailer: &mut bool,
    limits: &Limits,
    diags: &mut Diagnostics,
    store: &R,
) -> bool {
    let mut ctx = Context {
        limits,
        diags,
        file: Some(file),
        store: Some(store),
    };
    let mut object_lexer = Lexer::at(file, at);
    let parsed = indirect(&mut object_lexer, &mut ctx, Strictness::Strict, 0).ok();
    // Continue from wherever the body ended, so a nested header inside a
    // damaged body is not mistaken for a real one.
    lx.seek(object_lexer.pos().max(lx.pos()));

    let object = parsed.map(|p| p.object);

    // A cross-reference stream found this way carries the trailer a broken
    // `startxref` lost.
    if let Some(Object::Stream(stream)) = &object
        && stream.dict.name(names::TYPE) == Some(names::XREF)
    {
        merge_trailers(
            trailer,
            &Trailer {
                dict: stream.dict.clone(),
                object_number: obj_num,
            },
        );
        *has_trailer = true;
    }

    if !found.add_normal(obj_num, generation, false, at as u64, limits) {
        return false;
    }

    // An object stream found here contributes everything inside it, which is
    // how objects that exist only in compressed form are recovered.
    if let Some(Object::Stream(stream)) = &object
        && let Some(objstm) = crate::objstm::ObjStm::build(stream, limits, diags, store)
    {
        for (index, member) in objstm.entries().iter().enumerate() {
            let Ok(index) = u32::try_from(index) else {
                break;
            };
            found.add_compressed(member.num, obj_num, index, limits);
        }
    }
    true
}

/// Read the dictionary after a `trailer` keyword.
///
/// A stream is accepted too: a file whose `trailer` is followed by a
/// cross-reference stream's dictionary still names a catalog, and the
/// dictionary is what the reader wants either way.
fn read_trailer_body<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    file: &Arc<[u8]>,
    limits: &Limits,
    diags: &mut Diagnostics,
    store: &R,
) -> Option<pdfrum_object::Dict> {
    let mut ctx = Context {
        limits,
        diags,
        file: Some(file),
        store: Some(store),
    };
    match crate::syntax::body(lx, &mut ctx, Strictness::Loose, 0).ok()? {
        Object::Dict(d) => Some(d),
        Object::Stream(s) => Some(s.dict),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::rebuild;
    use crate::xref::{Entry, Trailer, Xref};
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::{NoResolve, names};
    use std::sync::Arc;

    fn scan(bytes: &[u8]) -> Option<(Xref, Trailer, Diagnostics)> {
        let file: Arc<[u8]> = Arc::from(bytes);
        let mut xref = Xref::new();
        let mut trailer = Trailer::default();
        let mut diags = Diagnostics::default();
        rebuild(
            &file,
            &mut xref,
            &mut trailer,
            &Limits::default(),
            &mut diags,
            &NoResolve,
        )
        .then_some((xref, trailer, diags))
    }

    #[test]
    fn finds_objects_and_a_trailer() {
        let file = b"%PDF-1.7\n\
                     1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
                     2 0 obj << /Type /Pages /Count 0 >> endobj\n\
                     trailer << /Root 1 0 R /Size 3 >>\n";
        let (xref, trailer, diags) = scan(file).expect("rebuild");
        assert_eq!(xref.entry(1), Some(Entry::Offset(9)));
        assert!(matches!(xref.entry(2), Some(Entry::Offset(_))));
        assert!(trailer.dict.raw(names::ROOT).is_some());
        assert_eq!(trailer.object_number, 0);
        assert!(diags.contains(&DiagKind::XrefRebuilt));
    }

    #[test]
    fn records_the_generation_from_the_header() {
        let file = b"1 4 obj << >> endobj\ntrailer << /Root 1 4 R >>";
        let (xref, _, _) = scan(file).expect("rebuild");
        assert_eq!(xref.generation(1), 4);
    }

    #[test]
    fn a_file_without_a_trailer_fails() {
        assert!(scan(b"1 0 obj << >> endobj\n").is_none());
    }

    #[test]
    fn a_file_without_objects_fails() {
        assert!(scan(b"trailer << /Root 1 0 R >>\n").is_none());
    }

    #[test]
    fn object_headers_inside_strings_are_invisible() {
        let file = b"1 0 obj (text 7 0 obj more) endobj\n\
                     trailer << /Root 1 0 R >>";
        let (xref, _, _) = scan(file).expect("rebuild");
        assert_eq!(xref.entry(7), None);
        assert!(xref.entry(1).is_some());
    }

    #[test]
    fn object_headers_inside_hex_strings_are_invisible() {
        let file = b"1 0 obj <312030206f626a> endobj\ntrailer << /Root 1 0 R >>";
        let (xref, _, _) = scan(file).expect("rebuild");
        assert_eq!(xref.len(), 1);
    }

    #[test]
    fn a_word_between_the_numbers_and_obj_clears_the_memory() {
        let file = b"1 0 junk obj << >> endobj\n2 0 obj << >> endobj\n\
                     trailer << /Root 2 0 R >>";
        let (xref, _, _) = scan(file).expect("rebuild");
        assert_eq!(xref.entry(1), None);
        assert!(xref.entry(2).is_some());
    }

    #[test]
    fn extra_numbers_keep_only_the_last_two() {
        let file = b"9 8 7 3 0 obj << >> endobj\ntrailer << /Root 3 0 R >>";
        let (xref, _, _) = scan(file).expect("rebuild");
        assert!(xref.entry(3).is_some());
        assert_eq!(xref.entry(9), None);
        assert_eq!(xref.entry(7), None);
    }

    #[test]
    fn a_later_trailer_overrides_an_earlier_one() {
        let file = b"1 0 obj << >> endobj\n\
                     trailer << /Root 1 0 R /Size 2 >>\n\
                     trailer << /Size 9 >>\n";
        let (_, trailer, _) = scan(file).expect("rebuild");
        assert_eq!(trailer.dict.direct_int(names::SIZE), Some(9));
        // The earlier /Root survives, since the later trailer did not say.
        assert!(trailer.dict.raw(names::ROOT).is_some());
    }

    #[test]
    fn a_cross_reference_stream_supplies_the_trailer() {
        // No `trailer` keyword anywhere: the /Type /XRef stream is it.
        let file = b"7 0 obj << /Type /XRef /Root 1 0 R /Size 8 /Length 3 >>\n\
                     stream\nabc\nendstream\nendobj\n";
        let (xref, trailer, _) = scan(file).expect("rebuild");
        assert!(xref.entry(7).is_some());
        assert!(trailer.dict.raw(names::ROOT).is_some());
        assert_eq!(trailer.object_number, 7);
    }

    #[test]
    fn an_unparsable_body_still_records_its_header() {
        // Object 1's body will not parse strictly, but the header is
        // evidence enough that an object lives there.
        let file = b"1 0 obj << /A\nendobj\n2 0 obj 5 endobj\n\
                     trailer << /Root 2 0 R >>";
        let (xref, _, _) = scan(file).expect("rebuild");
        assert!(xref.entry(1).is_some());
        assert!(xref.entry(2).is_some());
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        let seeds: &[&[u8]] = &[
            b"",
            b"obj obj obj",
            b"1 0 obj",
            b"trailer",
            b"((((",
            b"<<<<",
            b"999999999999 999999999999 obj",
            b"\x00\xff\x80\x0b1 0 obj",
        ];
        for seed in seeds {
            let _ = scan(seed);
        }
    }
}
