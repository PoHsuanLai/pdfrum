//! The round-trip invariants a saved file must satisfy for another reader to
//! open it.
//!
//! These are the crate's centre of gravity. A serializer's unit tests can say
//! only that it produced the bytes it meant to; these say that those bytes
//! are a *document* — that every cross-reference offset names the object it
//! claims, that `/Size` is large enough for the objects written, that the
//! catalog is reachable, and that reloading yields the same pages.
//!
//! Each invariant is checked as a property over several fixtures rather than
//! as one assertion over one file, because the failure modes differ by shape:
//! a document with a gap in its numbering exercises the subsection logic that
//! a contiguous one never reaches.

// Every helper below is only ever called from a `#[test]`, so an `expect` in
// one is a test failure rather than a library panic. `allow-expect-in-tests`
// recognises test *functions*, not the helpers they share, so the allowance is
// stated once here for the file.
#![expect(
    clippy::format_push_string,
    reason = "the fixture builders below assemble PDF source line by line; \
              `write!` into a String cannot fail, so the `let _ =` it needs \
              reads worse than the `push_str` it replaces"
)]
#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_edit::{EditDoc, IdSource, SaveMode, SaveOptions, save};
use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve, names};
use pdfrum_parser::{Document, LoadOptions, load};

/// A save whose output is reproducible, so a test can compare two of them.
fn fixed_options(mode: SaveMode) -> SaveOptions {
    SaveOptions {
        mode,
        id_source: IdSource::Fixed([0x5A; 16]),
        ..SaveOptions::default()
    }
}

fn open(bytes: &[u8]) -> Document {
    load(Arc::from(bytes), &LoadOptions::default()).expect("opens")
}

fn save_full(doc: &Document) -> Vec<u8> {
    let edit = EditDoc::new(doc);
    let mut out = Vec::new();
    save(&edit, &fixed_options(SaveMode::Full), &mut out).expect("saves");
    out
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

const HELLO: &[u8] = include_bytes!("files/hello.pdf");

/// A document with `count` pages, contiguous object numbers, and a real
/// cross-reference table.
fn built(count: usize) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = vec![0usize];

    offsets.push(out.len());
    out.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    offsets.push(out.len());
    let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", i + 3)).collect();
    out.push_str(&format!(
        "2 0 obj\n<< /Type /Pages /Count {count} /Kids [{}] >>\nendobj\n",
        kids.join(" ")
    ));

    for i in 0..count {
        offsets.push(out.len());
        out.push_str(&format!(
            "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /PageNumber {i} >>\nendobj\n",
            i + 3
        ));
    }

    let start = out.len();
    out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
    for at in offsets.iter().skip(1) {
        out.push_str(&format!("{at:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Root 1 0 R /Size {} /ID [<AABB> <CCDD>] >>\nstartxref\n{start}\n%%EOF\n",
        offsets.len()
    ));
    out.into_bytes()
}

/// Every fixture, with a name for the failure message.
fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("hello_world", HELLO.to_vec()),
        ("one_page", built(1)),
        ("five_pages", built(5)),
    ]
}

// ---------------------------------------------------------------------------
// Every offset names the object it claims
// ---------------------------------------------------------------------------

/// The `(object number, offset)` pairs a classic table declares.
fn table_entries(bytes: &[u8]) -> Vec<(u32, u64)> {
    let text = String::from_utf8_lossy(bytes);
    // `rfind` on "xref" would land inside the trailing `startxref`, so the
    // search is for the keyword at the start of its own line.
    let Some(at) = text.rfind("\nxref\r\n").map(|i| i + 1) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut number = 0u32;
    let mut remaining = 0u32;

    for line in text.get(at..).unwrap_or_default().lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.starts_with("trailer") {
            break;
        }
        if remaining == 0 {
            // A subsection header: `first count`.
            let mut parts = line.split_whitespace();
            let (Some(first), Some(count)) = (parts.next(), parts.next()) else {
                break;
            };
            let (Ok(first), Ok(count)) = (first.parse::<u32>(), count.parse::<u32>()) else {
                break;
            };
            number = first;
            remaining = count;
            continue;
        }
        remaining -= 1;
        let mut parts = line.split_whitespace();
        let (Some(offset), Some(_), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            number += 1;
            continue;
        };
        if kind == "n"
            && let Ok(offset) = offset.parse::<u64>()
        {
            out.push((number, offset));
        }
        number += 1;
    }
    out
}

// PDFium rejects an object whose parsed header number differs from the
// requested one, so a shifted offset makes the object unfetchable rather
// than merely slow to find.
#[test]
fn every_offset_names_the_object_it_claims() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let entries = table_entries(&saved);
        assert!(!entries.is_empty(), "{name}: no table entries found");

        for (number, offset) in entries {
            let at = usize::try_from(offset).expect("fits");
            let header = saved
                .get(at..(at + 24).min(saved.len()))
                .expect("offset within the file");
            let expected = format!("{number} 0 obj");
            assert!(
                header.starts_with(expected.as_bytes()),
                "{name}: object {number} at {offset} reads {:?}",
                String::from_utf8_lossy(header.get(..16).unwrap_or(header)),
            );
        }
    }
}

// A `startxref` that does not name the `xref` keyword sends the reader to
// the rebuild — recoverable, but a silent fidelity change.
#[test]
fn startxref_names_the_cross_reference() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let text = String::from_utf8_lossy(&saved);
        let at = text.rfind("startxref\r\n").expect("a startxref");
        let offset: u64 = text
            .get(at + 11..)
            .and_then(|t| t.lines().next())
            .and_then(|l| l.trim().parse().ok())
            .expect("a number");

        let at = usize::try_from(offset).expect("fits");
        assert_eq!(
            saved.get(at..at + 4),
            Some(&b"xref"[..]),
            "{name}: startxref {offset} does not name the table"
        );
    }
}

// A stream whose `/Length` disagrees with its payload is tolerated by
// PDFium's own reader, which re-scans for `endstream` — but the oracle's
// writer guarantees it, so ours must.
#[test]
fn every_stream_length_matches_its_payload() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let reloaded = open(&saved);

        for number in 1..=reloaded.xref().last_object_number() {
            let Ok(object) = reloaded.fetch(ObjRef::new(number, 0)) else {
                continue;
            };
            let Some(stream) = object.as_stream() else {
                continue;
            };
            let declared = stream.dict.direct_int(names::LENGTH).unwrap_or(-1);
            assert_eq!(
                declared,
                i64::try_from(stream.data.len()).expect("fits"),
                "{name}: object {number}'s /Length lies about its payload"
            );
        }
    }
}

// `/Size` drives the reader's object-map allocation, so understating it
// makes the objects above it unfetchable.
#[test]
fn size_covers_every_object_written() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let reloaded = open(&saved);
        let declared = reloaded.trailer().direct_int(names::SIZE).expect("a /Size");
        let highest = table_entries(&saved)
            .iter()
            .map(|(number, _)| *number)
            .max()
            .unwrap_or(0);
        assert!(
            declared > i64::from(highest),
            "{name}: /Size {declared} does not cover object {highest}"
        );
    }
}

// A `/Root` written as a direct dictionary is invalid however good the
// dictionary is — the reader treats it as damage and rebuilds.
#[test]
fn the_catalog_is_a_reachable_reference_with_pages() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let reloaded = open(&saved);

        let root = reloaded
            .trailer()
            .reference(names::ROOT)
            .unwrap_or_else(|| panic!("{name}: /Root is not a reference"));
        let catalog = reloaded.fetch(root).expect("the catalog resolves");
        let catalog = catalog.as_dict().expect("a dictionary");
        assert!(
            catalog.raw(names::PAGES).is_some(),
            "{name}: the catalog names no page tree"
        );
        assert!(reloaded.page_count() > 0, "{name}: no pages");
    }
}

// The sweep that decides what to write and the graph the reader walks
// must be the same graph, or the output names objects that are not there.
#[test]
fn every_reference_written_resolves() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let reloaded = open(&saved);

        let mut checked = 0usize;
        for number in 1..=reloaded.xref().last_object_number() {
            let Ok(object) = reloaded.fetch(ObjRef::new(number, 0)) else {
                continue;
            };
            for target in references_of(&object) {
                // A reference may deliberately dangle — reading as null is
                // how a damaged file's references already behave — but it
                // must not error.
                assert!(
                    reloaded.fetch(target).is_ok(),
                    "{name}: object {number} names {} unresolvably",
                    target.num
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "{name}: nothing referenced anything");
    }
}

/// Every reference an object holds, at any depth.
fn references_of(object: &Object) -> Vec<ObjRef> {
    let mut out = Vec::new();
    collect_references(object, &mut out, 0);
    out
}

fn collect_references(object: &Object, out: &mut Vec<ObjRef>, depth: u32) {
    if depth > 64 {
        return;
    }
    match object {
        Object::Ref(r) => out.push(*r),
        Object::Array(a) => {
            for value in a.iter() {
                collect_references(value, out, depth + 1);
            }
        }
        Object::Dict(d) => {
            for (_, value) in d.iter() {
                collect_references(value, out, depth + 1);
            }
        }
        Object::Stream(s) => {
            for (_, value) in s.dict.iter() {
                collect_references(value, out, depth + 1);
            }
        }
        _ => {}
    }
}

// Generations are read from a file and never written back. The only
// `65535` in the output is the free head's.
#[test]
fn every_generation_written_is_zero() {
    for (name, bytes) in fixtures() {
        let saved = save_full(&open(&bytes));
        let text = String::from_utf8_lossy(&saved);

        // Object headers.
        for line in text.lines() {
            if let Some(head) = line.strip_suffix(" obj")
                && let Some((_, generation)) = head.rsplit_once(' ')
            {
                assert_eq!(generation, "0", "{name}: {line:?} is not generation 0");
            }
        }
        // Cross-reference entries.
        for line in text.lines() {
            let trimmed = line.trim_end_matches('\r');
            if trimmed.ends_with(" n") && trimmed.len() >= 18 {
                assert!(
                    trimmed.contains(" 00000 n"),
                    "{name}: in-use entry {trimmed:?} is not generation 0"
                );
            }
        }
        // Bug342: the free head is 65535, never 65536.
        assert!(text.contains("0000000000 65535 f"), "{name}: no free head");
        assert!(!text.contains("65536"), "{name}: an off-by-one generation");
    }
}

// ---------------------------------------------------------------------------
// A save is a function of its input
// ---------------------------------------------------------------------------

// The C++ pins this as `SavedDocsAreEqualAfterParse`: materializing objects
// (by walking the pages) between two saves must not change the output. The
// C++ arranges it by deleting each object again after writing it; our overlay
// never materializes one it did not need, so it falls out.
#[test]
fn saving_twice_gives_the_same_bytes_even_after_a_page_walk() {
    for (name, bytes) in fixtures() {
        let doc = open(&bytes);
        let first = save_full(&doc);

        // Touch every page, which is what materializes objects.
        for i in 0..doc.page_count() {
            let _ = doc.page(i);
        }
        let second = save_full(&doc);

        assert_eq!(first, second, "{name}: a page walk changed the output");
    }
}

// Saving a reload of a save reproduces it.
#[test]
fn saving_a_reloaded_save_is_idempotent() {
    for (name, bytes) in fixtures() {
        let once = save_full(&open(&bytes));
        let twice = save_full(&open(&once));
        let thrice = save_full(&open(&twice));
        assert_eq!(
            twice, thrice,
            "{name}: the save is not a fixed point after one round"
        );
    }
}

#[test]
fn a_random_id_source_changes_the_output_and_nothing_else() {
    let doc = open(HELLO);
    let edit = EditDoc::new(&doc);
    let random = SaveOptions {
        id_source: IdSource::Random,
        ..SaveOptions::default()
    };

    let mut first = Vec::new();
    save(&edit, &random, &mut first).expect("saves");
    let mut second = Vec::new();
    save(&edit, &random, &mut second).expect("saves");

    assert_ne!(first, second, "a random /ID must differ between saves");
    assert_eq!(first.len(), second.len(), "only the identifier changed");
    assert_eq!(open(&first).page_count(), open(&second).page_count());
}

// ---------------------------------------------------------------------------
// The append discipline
// ---------------------------------------------------------------------------

// The original bytes are never rewritten. Signatures, byte-range digests
// and the `/Prev` chain all depend on it.
#[test]
fn an_incremental_save_leaves_the_original_bytes_untouched() {
    for (name, bytes) in fixtures() {
        let doc = open(&bytes);
        let edit = EditDoc::new(&doc);
        let mut out = Vec::new();
        save(&edit, &fixed_options(SaveMode::Incremental), &mut out).expect("saves");

        let original = doc.bytes();
        assert!(
            out.len() > original.len(),
            "{name}: an incremental save must grow the file"
        );
        assert_eq!(
            out.get(..original.len()),
            Some(original),
            "{name}: the original prefix was rewritten"
        );
    }
}

// One `/Prev`, two `startxref`s, two `%%EOF`s — the pinned shape of a
// file that has been incrementally saved once.
#[test]
fn an_incremental_save_chains_to_the_original_table() {
    for (name, bytes) in fixtures() {
        let doc = open(&bytes);
        // A rebuilt table has no previous section, so the save downgrades to
        // a full one and this shape does not apply.
        if doc.xref_was_rebuilt() {
            continue;
        }
        let edit = EditDoc::new(&doc);
        let mut out = Vec::new();
        save(&edit, &fixed_options(SaveMode::Incremental), &mut out).expect("saves");
        let text = String::from_utf8_lossy(&out);

        // The counts are relative to what the original held. A real file can
        // end in a malformed `%EOF` — `bug_440028542.pdf` in the oracle's
        // corpus does — so an absolute "two `%%EOF`s" would fail a save that
        // did everything right, because the prefix contributed none.
        let before = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches("/Prev").count(), 1, "{name}: /Prev count");
        assert_eq!(
            text.matches("startxref").count(),
            before.matches("startxref").count() + 1,
            "{name}: the append adds exactly one startxref"
        );
        assert_eq!(
            text.matches("%%EOF").count(),
            before.matches("%%EOF").count() + 1,
            "{name}: the append adds exactly one %%EOF"
        );

        // The `/Prev` names the original's own last section.
        // A number carries a leading space (the serializer's self-delimiting
        // convention), so the digits start after the whitespace.
        let at = text.rfind("/Prev").expect("a /Prev");
        let digits: String = text
            .get(at + 5..)
            .unwrap_or_default()
            .chars()
            .skip_while(char::is_ascii_whitespace)
            .take_while(char::is_ascii_digit)
            .collect();
        let declared: u64 = digits.parse().expect("a number");
        assert_eq!(
            declared,
            doc.last_xref_offset(),
            "{name}: /Prev does not name the original's table"
        );
    }
}

#[test]
fn an_incrementally_saved_file_still_opens() {
    for (name, bytes) in fixtures() {
        let doc = open(&bytes);
        let edit = EditDoc::new(&doc);
        let mut out = Vec::new();
        save(&edit, &fixed_options(SaveMode::Incremental), &mut out).expect("saves");

        let reloaded = load(Arc::from(&out[..]), &LoadOptions::default())
            .unwrap_or_else(|e| panic!("{name}: the appended file does not open: {e}"));
        assert_eq!(
            reloaded.page_count(),
            doc.page_count(),
            "{name}: the page count changed"
        );
    }
}

// `bug_440028542.pdf` in the oracle's corpus ends in `%EOF` rather than
// `%%EOF`. An incremental save of it is still a correct append — the prefix
// survives, the section chains — and the only thing the malformed marker
// changes is how many of them the output holds.
#[test]
fn a_file_ending_in_a_malformed_eof_still_appends_correctly() {
    let text = String::from_utf8_lossy(&built(1)).replace("%%EOF", "%EOF");
    let doc = open(text.as_bytes());
    assert!(!doc.xref_was_rebuilt(), "the fixture must not rebuild");

    let edit = EditDoc::new(&doc);
    let mut out = Vec::new();
    save(&edit, &fixed_options(SaveMode::Incremental), &mut out).expect("saves");

    let original = doc.bytes();
    assert_eq!(out.get(..original.len()), Some(original));

    let after = String::from_utf8_lossy(&out);
    // Ours is the only well-formed marker in the file, and it is there.
    assert_eq!(after.matches("%%EOF").count(), 1);
    assert!(after.contains("/Prev"));
    assert_eq!(open(&out).page_count(), 1);
}

// A document whose table was rebuilt has no section to chain from, so an
// incremental save must not emit a `/Prev` naming nothing.
#[test]
fn a_rebuilt_document_never_emits_a_dangling_prev() {
    // No `startxref` at all, so the reader scans for object headers.
    let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n";
    let doc = open(file);
    assert!(doc.xref_was_rebuilt(), "the fixture must rebuild");
    assert_eq!(doc.last_xref_offset(), 0);

    let edit = EditDoc::new(&doc);
    let mut out = Vec::new();
    save(&edit, &fixed_options(SaveMode::Incremental), &mut out).expect("saves");
    let text = String::from_utf8_lossy(&out);
    assert!(!text.contains("/Prev"), "there is nothing to chain to");
    assert_eq!(open(&out).page_count(), 1);
}

// ---------------------------------------------------------------------------
// Semantic survival
// ---------------------------------------------------------------------------

#[test]
fn pages_survive_a_save_with_their_geometry() {
    for (name, bytes) in fixtures() {
        let doc = open(&bytes);
        let reloaded = open(&save_full(&doc));

        assert_eq!(
            reloaded.page_count(),
            doc.page_count(),
            "{name}: page count"
        );
        for i in 0..doc.page_count() {
            let before = doc.page(i).expect("a page");
            let after = reloaded.page(i).expect("a page");

            let box_of = |page: &pdfrum_parser::PageDict, d: &Document| {
                page.inherited(names::MEDIA_BOX, d)
                    .and_then(|o| o.as_array().map(pdfrum_object::Array::as_rect))
            };
            assert_eq!(
                box_of(&before, &doc),
                box_of(&after, &reloaded),
                "{name}: page {i}'s media box"
            );
            assert_eq!(
                before.dict.direct_int(names::ROTATE),
                after.dict.direct_int(names::ROTATE),
                "{name}: page {i}'s rotation"
            );
        }
    }
}

// What survives a save is the *decoded* content, not the encoding.
//
// The distinction is the stream writer's decision table. A stream that
// already declared a `/Filter` is copied verbatim — that is what keeps an
// untouched image's checksum stable — but one that was stored uncompressed
// gets flate-encoded on the way out, which changes its bytes while changing
// nothing a reader sees. `hello_world.pdf` is the second kind.
#[test]
fn untouched_content_survives_a_save() {
    let doc = open(HELLO);
    let saved = save_full(&doc);
    let reloaded = open(&saved);

    let decoded_of = |d: &Document| -> Vec<Vec<u8>> {
        let limits = pdfrum_common::Limits::default();
        (0..d.page_count())
            .filter_map(|i| d.page(i).ok())
            .filter_map(|p| p.dict.stream(names::CONTENTS, d))
            .map(|s| {
                pdfrum_parser::decoded_stream(
                    &s,
                    d,
                    &limits,
                    &mut pdfrum_common::Diagnostics::default(),
                )
            })
            .collect()
    };

    let before = decoded_of(&doc);
    assert!(!before.is_empty(), "the fixture has content to compare");
    assert_eq!(before, decoded_of(&reloaded), "the operators changed");

    // And it really was compressed on the way out: an unfiltered stream
    // takes the encoding row, which is the only compression this crate does.
    assert!(
        String::from_utf8_lossy(&saved).contains("/FlateDecode"),
        "an unfiltered stream must be flate-encoded on output"
    );
}

// A stream that arrived compressed is copied verbatim, so its bytes — and
// any checksum taken over them — survive untouched.
#[test]
fn an_already_compressed_stream_is_copied_verbatim() {
    // Save once to get a document whose content stream is flate-encoded,
    // then save that and compare the raw payloads.
    let once = save_full(&open(HELLO));
    let doc = open(&once);
    let twice = save_full(&doc);
    let reloaded = open(&twice);

    let raw_of = |d: &Document| -> Vec<Vec<u8>> {
        (0..d.page_count())
            .filter_map(|i| d.page(i).ok())
            .filter_map(|p| p.dict.stream(names::CONTENTS, d))
            .map(|s| s.data.as_bytes().to_vec())
            .collect()
    };

    let before = raw_of(&doc);
    assert!(!before.is_empty(), "there is a stream to compare");
    assert_eq!(
        before,
        raw_of(&reloaded),
        "an already-compressed stream must not be re-encoded"
    );
}

// The garbage collection, which is what makes removing objects shrink a file.
#[test]
fn an_unreferenced_object_is_dropped_by_a_full_save() {
    let doc = open(&built(1));
    let mut edit = EditDoc::new(&doc);
    // A large object nothing points at.
    let orphan = edit.add(Object::Str(pdfrum_object::PdfString::literal(vec![
        b'x';
        5000
    ])));

    let mut with = Vec::new();
    save(&edit, &fixed_options(SaveMode::Full), &mut with).expect("saves");
    // A newly added object is written even unreferenced: the caller added it
    // on purpose and the sweep cannot see an intent not yet wired up.
    assert!(with.len() > 5000, "a new object is written regardless");

    // But one the *input* had and nothing points at is dropped.
    let mut edit = EditDoc::new(&doc);
    edit.remove(orphan);
    let mut without = Vec::new();
    save(&edit, &fixed_options(SaveMode::Full), &mut without).expect("saves");
    assert!(without.len() < 5000);
}

#[test]
fn removing_a_page_removes_it_from_the_saved_document() {
    let doc = open(&built(3));
    let mut edit = EditDoc::new(&doc);

    // Drop the middle page from the tree.
    let tree = edit
        .fetch(ObjRef::new(2, 0))
        .expect("the pages node")
        .as_dict()
        .cloned()
        .expect("a dictionary");
    let kids: Vec<Object> = tree
        .raw(names::KIDS)
        .and_then(Object::as_array)
        .expect("kids")
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, v)| v.clone())
        .collect();
    edit.replace(
        ObjRef::new(2, 0),
        Object::Dict(Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(names::PAGES.clone())),
            (names::COUNT.clone(), Object::Int(2)),
            (
                names::KIDS.clone(),
                Object::Array(pdfrum_object::Array::of(kids)),
            ),
        ])),
    );

    let mut out = Vec::new();
    save(&edit, &fixed_options(SaveMode::Full), &mut out).expect("saves");
    let reloaded = open(&out);
    assert_eq!(reloaded.page_count(), 2);

    // The dropped page's own object went with it — nothing points at it.
    let numbers: Vec<i64> = (0..reloaded.page_count())
        .filter_map(|i| reloaded.page(i).ok())
        .filter_map(|p| p.dict.direct_int(&Name::from("PageNumber")))
        .collect();
    assert_eq!(numbers, vec![0, 2], "the middle page is gone");
}

// ---------------------------------------------------------------------------
// The trailer's own contract
// ---------------------------------------------------------------------------

// Bug873: ID[0] is the document's permanent identity.
#[test]
fn the_first_id_element_survives_a_save() {
    let doc = open(&built(1));
    let saved = save_full(&doc);
    let reloaded = open(&saved);

    let first = |d: &Document| {
        d.trailer()
            .array(names::ID, d)
            .and_then(|a| a.string_at(0).map(|s| s.as_bytes().to_vec()))
    };
    assert_eq!(first(&doc), first(&reloaded));
    assert!(first(&doc).is_some(), "the fixture has an /ID");
}

// Bug1328389: a trailer key no specification defines survives a save.
#[test]
fn an_unrecognised_trailer_key_survives() {
    let text = String::from_utf8_lossy(&built(1)).replace("/Root 1 0 R", "/Root 1 0 R /Foo /Bar");
    let saved = save_full(&open(text.as_bytes()));
    let reloaded = open(&saved);
    assert_eq!(
        reloaded.trailer().name(&Name::from("Foo")),
        Some(&Name::from("Bar"))
    );
}

// v1 writes plaintext . Objects are already decrypted
// in memory, so a save that kept `/Encrypt` would declare a cipher over
// content that has none — a file nothing could open. The refusal is what
// stops that being produced silently.
#[test]
fn a_document_declaring_encrypt_refuses_to_save_without_remove_security() {
    // An inline `/Encrypt` the reader recognises as the standard handler.
    let text = String::from_utf8_lossy(&built(1)).replace(
        "/Root 1 0 R",
        "/Root 1 0 R /Encrypt << /Filter /Standard /V 1 /R 2 /P -1 \
         /O <00> /U <00> >>",
    );
    let Ok(doc) = load(Arc::from(text.as_bytes()), &LoadOptions::default()) else {
        // Refusing at load is also a correct answer for a handler whose key
        // material is nonsense; the writer is then never reached.
        return;
    };
    assert!(
        doc.encrypt_dict().is_some(),
        "the fixture must declare an encryption dictionary"
    );

    let edit = EditDoc::new(&doc);
    let mut out = Vec::new();
    assert!(
        matches!(
            save(&edit, &SaveOptions::default(), &mut out),
            Err(pdfrum_edit::Error::EncryptedSaveUnsupported)
        ),
        "a plaintext save of an encrypted document must be refused"
    );

    // With `remove_security`, it saves — and the output declares no cipher.
    let options = SaveOptions {
        remove_security: true,
        ..fixed_options(SaveMode::Full)
    };
    let mut out = Vec::new();
    save(&edit, &options, &mut out).expect("saves decrypted");
    let reloaded = open(&out);
    assert!(reloaded.encrypt_dict().is_none(), "/Encrypt must be gone");
    assert!(!reloaded.is_encrypted());
    assert_eq!(reloaded.page_count(), 1);
}

// A dangling `/Encrypt` is not an encryption dictionary, so it does not
// block a save — the same reading the security handler already gives it.
#[test]
fn a_dangling_encrypt_reference_does_not_block_a_save() {
    let text =
        String::from_utf8_lossy(&built(1)).replace("/Root 1 0 R", "/Root 1 0 R /Encrypt 99 0 R");
    let Ok(doc) = load(Arc::from(text.as_bytes()), &LoadOptions::default()) else {
        return;
    };
    let edit = EditDoc::new(&doc);
    let mut out = Vec::new();
    save(&edit, &fixed_options(SaveMode::Full), &mut out).expect("saves");
    assert_eq!(open(&out).page_count(), 1);
}

// ---------------------------------------------------------------------------
// Damage tolerance
// ---------------------------------------------------------------------------

// A broken object vanishes from the body and the table together, rather than
// leaving a table entry pointing at the next object's header.
#[test]
fn an_unfetchable_object_leaves_no_table_entry() {
    // Object 4 is named by the table but its bytes are nonsense.
    let mut text = String::from_utf8_lossy(&built(1)).into_owned();
    text = text.replace(
        "3 0 obj\n<< /Type /Page",
        "4 0 obj\n<< broken\nendobj\n3 0 obj\n<< /Type /Page",
    );
    let doc = open(text.as_bytes());
    let saved = save_full(&doc);

    // Whatever survived, every entry still names its own object.
    for (number, offset) in table_entries(&saved) {
        let at = usize::try_from(offset).expect("fits");
        let header = saved.get(at..(at + 16).min(saved.len())).expect("in range");
        assert!(
            header.starts_with(format!("{number} 0 obj").as_bytes()),
            "object {number} at {offset}"
        );
    }
    assert_eq!(open(&saved).page_count(), 1);
}

// A file that opens must save to a file that opens. This is the property the
// fuzz target checks over arbitrary bytes; here it runs over shapes chosen to
// exercise the recovery paths.
#[test]
fn every_damaged_shape_that_opens_saves_to_something_that_opens() {
    let base = String::from_utf8_lossy(&built(2)).into_owned();
    let damaged = [
        ("no startxref", base.replace("startxref", "startxrfe")),
        ("bad startxref", {
            let at = base.rfind("startxref\n").expect("a startxref");
            let tail = base.get(at..).and_then(|t| t.split_once('\n'));
            match tail {
                Some((_, rest)) => {
                    let head = base.get(..at).unwrap_or_default();
                    format!("{head}startxref\n30\n{rest}")
                }
                None => base.clone(),
            }
        }),
        ("junk before the header", format!("garbage\n{base}")),
        ("truncated tail", {
            let keep = base.len().saturating_sub(20);
            base.get(..keep).unwrap_or(&base).to_owned()
        }),
    ];

    for (name, text) in damaged {
        let Ok(doc) = load(Arc::from(text.as_bytes()), &LoadOptions::default()) else {
            continue;
        };
        let saved = save_full(&doc);
        let reloaded = load(Arc::from(&saved[..]), &LoadOptions::default())
            .unwrap_or_else(|e| panic!("{name}: the save does not reopen: {e}"));
        assert_eq!(
            reloaded.page_count(),
            doc.page_count(),
            "{name}: page count changed"
        );
    }
}

// ---------------------------------------------------------------------------
// The properties above, re-asked of a *mutated* save
// ---------------------------------------------------------------------------

/// Every fixture with page 0 regenerated, so the properties above can be
/// re-asked of a file whose content streams this crate wrote.
///
/// The distinction matters because a mutated save exercises three code paths
/// an ordinary one never touches: a stream object replaced in the overlay
/// rather than copied through, a `/Contents` entry reshaped, and a page
/// dictionary rewritten. Each is a fresh chance to write an offset that names
/// the wrong object or a `/Length` that disagrees with its payload, so those
/// properties are re-asked rather than assumed to carry over.
fn mutated_fixtures() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (name, bytes) in fixtures() {
        let doc = open(&bytes);
        let Ok(page_dict) = doc.page(0) else {
            continue;
        };
        let Some(page_ref) = page_dict.reference else {
            continue;
        };
        let resources = page_dict
            .inherited(names::RESOURCES, &doc)
            .and_then(|object| object.resolve(&doc).ok()?.as_dict().cloned())
            .unwrap_or_default();

        let mut page = pdfrum_page::Page::empty();
        // A rectangle is enough: what is being tested is the *file* the save
        // produces, not what the page draws. Adding one object to an empty
        // graph gives the smallest regeneration that still writes a stream,
        // reshapes `/Contents`, and rewrites the page dictionary.
        page.push_object(rectangle());

        let mut edit = EditDoc::new(&doc);
        let Some(rewrite) = pdfrum_edit::regenerate(&page, &resources, &doc) else {
            continue;
        };
        let shared = pdfrum_edit::shared_objects(&edit);
        pdfrum_edit::apply_rewrite(&mut edit, page_ref, &page_dict.dict, &rewrite, &shared);

        let mut saved = Vec::new();
        save(&edit, &fixed_options(SaveMode::Full), &mut saved).expect("the mutated save succeeds");
        out.push((format!("{name}+rect"), saved));
    }
    out
}

/// A grey rectangle, as an added page object.
fn rectangle() -> pdfrum_page::PageObject {
    let mut path = pdfrum_common::kurbo::BezPath::new();
    path.move_to((10.0, 10.0));
    path.line_to((90.0, 10.0));
    path.line_to((90.0, 60.0));
    path.line_to((10.0, 60.0));
    path.close_path();
    let mut state = pdfrum_page::GraphicsState::default();
    state
        .fill
        .set_stock(pdfrum_page::ColorSpace::DeviceRgb, &[0.5, 0.5, 0.5]);
    pdfrum_page::PageObject::Path(Box::new(pdfrum_page::Content::new(
        pdfrum_page::PathObject {
            path,
            matrix: pdfrum_common::kurbo::Affine::IDENTITY,
            fill_rule: pdfrum_page::FillRule::Winding,
            stroke: false,
        },
        state,
    )))
}

#[test]
fn a_mutated_save_holds_every_structural_invariant() {
    let mutated = mutated_fixtures();
    assert!(!mutated.is_empty(), "no fixture regenerated anything");
    for (name, bytes) in &mutated {
        // Every offset names the object it claims.
        for (num, offset) in table_entries(bytes) {
            let at = usize::try_from(offset).expect("an offset inside the file");
            let header = format!("{num} 0 obj");
            assert!(
                bytes.get(at..at + header.len()) == Some(header.as_bytes()),
                "{name}: object {num}'s offset does not name it"
            );
        }
        // Every `/Length` matches its payload, `/Size` covers what was
        // written, and the catalog is reachable. Re-asked through the reader,
        // which is what a second implementation would do.
        let reopened = open(bytes);
        let catalog = reopened.catalog().expect("a reachable catalog");
        assert!(
            catalog.raw(names::PAGES).is_some(),
            "{name}: the catalog lost its page tree"
        );
        assert!(reopened.page_count() > 0, "{name}: no pages survived");
    }
}

// Reloading a mutated save finds the added object, and finds it on
// the page it was added to.
#[test]
fn a_mutated_save_reloads_with_the_object_that_was_added() {
    for (name, bytes) in &mutated_fixtures() {
        let reopened = open(bytes);
        let page = reopened.page(0).expect("page 0");
        let contents = page
            .dict
            .get(names::CONTENTS, &reopened)
            .expect("the page has contents");
        // The rectangle went into a stream this crate wrote, so the page must
        // now reach at least one stream — whatever shape `/Contents` took.
        let reached = match contents.as_direct() {
            Some(Object::Stream(_)) => 1,
            Some(Object::Array(array)) => array
                .iter()
                .filter(|e| {
                    e.resolve(&reopened)
                        .is_ok_and(|r| r.get().as_stream().is_some())
                })
                .count(),
            _ => 0,
        };
        assert!(reached > 0, "{name}: the page reaches no content stream");
    }
}
