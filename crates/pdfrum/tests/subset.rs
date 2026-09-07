//! `SaveOptions::subset_new_fonts`, end to end (ISO 32000-1 §9.9, ).
//!
//! The option only ever fires on fonts a save writes as **new**, so every
//! test here imports a page carrying an embedded CID TrueType font into a
//! second document first — that import is what makes the font new.
//!
//! The load-bearing claim is the one at the bottom: a subsetted save renders
//! **pixel-identically** to the same save without it. That is a stronger
//! assertion than the SSIM floor the conformance harness uses, and it is
//! available here for the reason the whole design rests on — subsetting
//! rewrites `/CIDToGIDMap` and nothing else that names a glyph, so the same
//! outlines land in the same places and no content stream is regenerated.
//!
//! **The oracle agrees.** `pdfium_test --md5`
//! over the two files this fixture produces returns the same digest for each
//! of the two pages — `ab72bc73…` and `1f931859…` — so PDFium's own
//! rasterizer cannot tell a subsetted save from an unsubsetted one either.
//! That is `save-round-trip`'s two-step check (we save, the oracle reopens
//! and renders) run by hand, because `pdfrum-tool` has no switch for the
//! option; `examples/subset_dump.rs` is what writes the two files.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "helpers shared by the tests below; a panic in one is a failure, \
              and `allow-panic-in-tests` recognises test functions rather than \
              the helpers they call"
)]

use std::sync::Arc;

use pdfrum::{Document, RenderOptions, VelloCpuBackend};
use pdfrum_common::PageIndex;
use pdfrum_edit::{
    EditDoc, IdSource, ImportOptions, PageRange, SaveMode, SaveOptions, import_pages, save,
};
use pdfrum_object::{Name, ObjRef, Object, Resolve};
use pdfrum_parser::{LoadOptions, load};

/// A one-page document whose text is drawn with an embedded `CIDFontType2`
/// `Roboto-Regular`, `/CIDToGIDMap /Identity`, with `/W` and `/ToUnicode`.
const CID_TRUETYPE: &[u8] = include_bytes!("fixtures/latin_extended.pdf");
/// The destination the page is imported into.
const DEST: &[u8] = include_bytes!("fixtures/hello_world.pdf");

/// Import the fixture's page into a fresh document and save it.
///
/// `IdSource::Fixed` so the six-letter subset tag is reproducible; without it
/// the name would differ between the two saves a test compares.
fn import_and_save(subset_new_fonts: bool) -> Vec<u8> {
    let src = load(Arc::from(CID_TRUETYPE), &LoadOptions::default()).expect("source opens");
    let dest = load(Arc::from(DEST), &LoadOptions::default()).expect("destination opens");
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::of([0u32]),
        &ImportOptions {
            at: PageIndex::new(0),
            viewer_preferences: false,
        },
    )
    .expect("imports");

    let mut out = Vec::new();
    save(
        &edit,
        &SaveOptions {
            subset_new_fonts,
            id_source: IdSource::Fixed([7; 16]),
            ..SaveOptions::default()
        },
        &mut out,
    )
    .expect("saves");
    out
}

/// The one embedded font program in `bytes`, found by its `/Length1`, with
/// that declared uncompressed length beside it.
fn font_program(bytes: &[u8]) -> (Vec<u8>, i64) {
    let doc = load(Arc::from(bytes), &LoadOptions::default()).expect("reopens");
    for num in 1..=doc.xref().last_object_number() {
        let Ok(object) = doc.fetch(ObjRef::new(num, 0)) else {
            continue;
        };
        let Some(stream) = object.as_stream() else {
            continue;
        };
        let Some(Object::Int(length1)) = stream.dict.raw(&Name::from("Length1")) else {
            continue;
        };
        return (stream.data.as_bytes().to_vec(), *length1);
    }
    panic!("no embedded font program in the saved file");
}

/// Every font-related dictionary of the saved file, as `(object number,
/// dict)`: the `/Type0` font, its descendant `CIDFont` — both of which carry
/// `/BaseFont` — and the `/FontDescriptor`, which carries `/FontName`.
fn font_dicts(bytes: &[u8]) -> Vec<(u32, pdfrum_object::Dict)> {
    let doc = load(Arc::from(bytes), &LoadOptions::default()).expect("reopens");
    let mut out = Vec::new();
    for num in 1..=doc.xref().last_object_number() {
        if let Ok(object) = doc.fetch(ObjRef::new(num, 0))
            && let Some(dict) = object.as_dict()
            && (dict.raw(&Name::from("BaseFont")).is_some()
                || dict.raw(&Name::from("FontName")).is_some())
        {
            out.push((num, dict.clone()));
        }
    }
    out
}

/// The first page's pixels, at one pixel per point.
fn render(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let doc = Document::from_bytes(Arc::from(bytes)).expect("opens");
    let page = doc.page(0).expect("has a page");
    let pixmap = page
        .render(&VelloCpuBackend::new(), &RenderOptions::default())
        .expect("renders");
    (pixmap.width(), pixmap.height(), pixmap.data().to_vec())
}

/// The text the first page extracts to.
fn extracted(bytes: &[u8]) -> String {
    let doc = Document::from_bytes(Arc::from(bytes)).expect("opens");
    doc.page(0).expect("has a page").text().slice(..)
}

// ---- the option is off by default, and off means untouched ----

#[test]
fn off_leaves_the_font_program_byte_identical() {
    let plain = import_and_save(false);
    let (imported, imported_length1) = font_program(&plain);

    // The source's own program, for comparison. `import` copies a stream with
    // its filter intact, so these are the same bytes it started as.
    let (original, original_length1) = font_program(CID_TRUETYPE);
    assert_eq!(
        imported, original,
        "an unsubsetted save must not touch the embedded program"
    );
    assert_eq!(imported_length1, original_length1);
    assert_eq!(original_length1, 35636, "the fixture's program");
}

#[test]
fn off_is_the_default() {
    assert!(!SaveOptions::default().subset_new_fonts);
}

#[test]
fn off_leaves_the_base_font_name_untagged() {
    let mut seen = 0;
    for (_, dict) in font_dicts(&import_and_save(false)) {
        for key in ["BaseFont", "FontName"] {
            let Some(name) = dict.name(&Name::from(key)) else {
                continue;
            };
            if name.as_bytes().ends_with(b"Roboto-Regular") {
                assert_eq!(name.as_bytes(), b"Roboto-Regular", "no tag was minted");
                seen += 1;
            }
        }
    }
    // The `/Type0` font, its descendant, and the descriptor.
    assert_eq!(seen, 3);
}

// ---- on: the program shrinks, and every dictionary that named it follows ----

#[test]
fn the_embedded_program_is_smaller() {
    let (_, before) = font_program(&import_and_save(false));
    let (_, after) = font_program(&import_and_save(true));
    assert!(
        after < before,
        "subsetted program is {after} bytes uncompressed, unsubsetted {before}"
    );
    // The fixture draws most of Latin Extended, so the saving is real but not
    // dramatic. Recorded rather than bounded tightly: a `subsetter` upgrade
    // that keeps a different set of tables moves this number without being a
    // regression.
    assert_eq!((before, after), (35636, 20424));
}

#[test]
fn base_font_and_font_name_carry_a_six_letter_tag() {
    let tagged: Vec<Vec<u8>> = font_dicts(&import_and_save(true))
        .into_iter()
        .filter_map(|(_, dict)| {
            let name = dict.name(&Name::from("BaseFont"))?.as_bytes().to_vec();
            name.ends_with(b"+Roboto-Regular").then_some(name)
        })
        .collect();

    // The `/Type0` font and its descendant `CIDFont`, both renamed
    // (`cpdf_fontsubsetter.cpp:167-183`).
    assert_eq!(tagged.len(), 2, "both font dictionaries are renamed");
    for name in &tagged {
        let tag = name.get(..6).expect("six bytes of tag");
        assert!(
            tag.iter().all(u8::is_ascii_uppercase),
            "{} is not six uppercase letters",
            String::from_utf8_lossy(tag)
        );
        assert_eq!(name.get(6), Some(&b'+'));
        // Exactly one `+`: an existing tag is replaced, never stacked.
        assert!(!name.get(7..).is_some_and(|rest| rest.contains(&b'+')));
    }
    assert_eq!(tagged.first(), tagged.get(1), "one name, used twice");

    // The descriptor names it too (`:203-207`).
    let descriptor_names: Vec<Vec<u8>> = font_dicts(&import_and_save(true))
        .into_iter()
        .filter_map(|(_, dict)| Some(dict.name(&Name::from("FontName"))?.as_bytes().to_vec()))
        .filter(|n| n.ends_with(b"+Roboto-Regular"))
        .collect();
    assert_eq!(descriptor_names.len(), 1);
    assert_eq!(descriptor_names.first(), tagged.first());
}

#[test]
fn a_fixed_id_source_makes_the_tag_reproducible() {
    assert_eq!(import_and_save(true), import_and_save(true));
}

// ---- the renumbering lands in `/CIDToGIDMap`, and nowhere else ----

#[test]
fn the_cid_font_gains_a_cid_to_gid_stream() {
    let saved = import_and_save(true);
    let doc = load(Arc::from(&saved[..]), &LoadOptions::default()).expect("reopens");

    let cid_font = font_dicts(&saved)
        .into_iter()
        .find_map(|(_, dict)| {
            (dict.name(&Name::from("Subtype"))?.as_bytes() == b"CIDFontType2").then_some(dict)
        })
        .expect("a CIDFontType2 descendant");

    // The source declared `/CIDToGIDMap /Identity`; the subset replaces it
    // with the table the renumbering needs.
    let map = cid_font
        .stream(&Name::from("CIDToGIDMap"), &doc)
        .expect("a /CIDToGIDMap stream, not a name");
    let table = pdfrum_filters::decode_chain(
        &map,
        0,
        &doc,
        &pdfrum_common::Limits::default(),
        &mut pdfrum_common::Diagnostics::default(),
    )
    .data;
    assert!(!table.is_empty());
    assert_eq!(table.len() % 2, 0, "a big-endian u16 per CID");
    // Every entry is a glyph in the *subset*, which is far smaller than the
    // original's 1294 glyphs.
    let highest = table
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .max()
        .expect("a non-empty table");
    assert!(highest < 512, "glyph {highest} is outside the subset");
}

#[test]
fn widths_and_to_unicode_are_carried_through_untouched() {
    // The divergence that matters most, stated as a test: PDFium prunes `/W`
    // and rebuilds `/ToUnicode` because its retained-GID subset lets it; ours
    // leaves both alone because the CID space they are keyed by never moved.
    let off = import_and_save(false);
    let on = import_and_save(true);

    for name in ["W", "ToUnicode"] {
        let before = entry(&off, name);
        let after = entry(&on, name);
        assert_eq!(
            before, after,
            "/{name} changed under subsetting; the CID space is supposed to be fixed"
        );
        assert!(before.is_some(), "the fixture has a /{name} to compare");
    }
}

/// The first `key` found on any font dictionary in `bytes`, fully resolved.
fn entry(bytes: &[u8], key: &str) -> Option<String> {
    let doc = load(Arc::from(bytes), &LoadOptions::default()).expect("reopens");
    let key = Name::from(key);
    font_dicts(bytes).into_iter().find_map(|(_, dict)| {
        let value = dict.get(&key, &doc)?.into_owned();
        Some(match value {
            Object::Stream(s) => format!(
                "{:?}",
                pdfrum_filters::decode_chain(
                    &s,
                    0,
                    &doc,
                    &pdfrum_common::Limits::default(),
                    &mut pdfrum_common::Diagnostics::default(),
                )
                .data
            ),
            other => format!("{other:?}"),
        })
    })
}

#[test]
fn exactly_five_objects_change_or_appear() {
    // `TrueType` (`cpdf_fontsubsetter_embeddertest.cpp:398-428`) asserts six
    // overrides. Ours are five and they are not the same five: `/W` is not
    // rewritten, and the `/CIDToGIDMap` is minted. See
    // §3.7.
    let off = objects(&import_and_save(false));
    let on = objects(&import_and_save(true));

    let appeared: Vec<u32> = on
        .keys()
        .filter(|n| !off.contains_key(n))
        .copied()
        .collect();
    assert_eq!(
        appeared.len(),
        1,
        "the `/CIDToGIDMap` and nothing else is minted: {appeared:?}"
    );
    assert!(
        off.keys().all(|n| on.contains_key(n)),
        "subsetting removes no object"
    );

    let changed: Vec<u32> = off
        .iter()
        .filter(|(num, before)| on.get(num).is_some_and(|after| after != *before))
        .map(|(num, _)| *num)
        .collect();
    assert_eq!(
        changed.len(),
        4,
        "the program, the `/Type0` font, its descendant and the descriptor: {changed:?}"
    );
}

/// Every object of the saved file by number, spelled for comparison.
///
/// Keyed rather than positional: the subsetter *adds* an object, so a
/// position-by-position walk would report every object after it as changed.
///
/// A stream is spelled as its dictionary plus its **decoded** bytes rather
/// than through `Debug`, whose `ByteSpan` carries the range the object landed
/// at in the file. Adding one object moves every later one, so a `Debug`
/// comparison would call every stream after the insertion changed.
fn objects(bytes: &[u8]) -> std::collections::BTreeMap<u32, String> {
    let doc = load(Arc::from(bytes), &LoadOptions::default()).expect("reopens");
    (1..=doc.xref().last_object_number())
        .filter_map(|num| {
            let object = doc.fetch(ObjRef::new(num, 0)).ok()?;
            if object.is_null() {
                return None;
            }
            let spelled = match object.as_stream() {
                Some(stream) => format!(
                    "{:?} {:?}",
                    stream.dict,
                    pdfrum_filters::decode_chain(
                        stream,
                        0,
                        &doc,
                        &pdfrum_common::Limits::default(),
                        &mut pdfrum_common::Diagnostics::default(),
                    )
                    .data
                ),
                None => format!("{object:?}"),
            };
            Some((num, spelled))
        })
        .collect()
}

// ---- the proof: nothing a reader can see changed ----

#[test]
fn a_subsetted_save_renders_identically() {
    let (w_off, h_off, off) = render(&import_and_save(false));
    let (w_on, h_on, on) = render(&import_and_save(true));

    assert_eq!((w_off, h_off), (w_on, h_on));
    // Not "close": *identical*. The glyph outlines, the character codes and
    // the text matrices are all untouched, so there is nothing left for the
    // renderer to disagree about. An SSIM floor would be the weaker claim.
    let differing = off.iter().zip(&on).filter(|(a, b)| a != b).count();
    assert_eq!(
        differing,
        0,
        "{differing} of {} bytes differ between the subsetted and unsubsetted renders",
        off.len()
    );
}

#[test]
fn a_subsetted_save_extracts_the_same_text() {
    // Round-trip obligation R15: `/ToUnicode` must still cover the codes the
    // page shows. Ours does trivially, since neither moved.
    assert_eq!(
        extracted(&import_and_save(false)),
        extracted(&import_and_save(true))
    );
    assert!(!extracted(&import_and_save(true)).is_empty());
}

#[test]
fn a_subsetted_save_reopens() {
    let saved = import_and_save(true);
    let doc = Document::from_bytes(Arc::from(&saved[..])).expect("reopens");
    assert_eq!(
        doc.page_count(),
        2,
        "the destination's page plus the import"
    );
}

// ---- a document with no new fonts pays nothing ----

#[test]
fn a_save_with_no_new_fonts_is_unchanged_by_the_option() {
    let doc = load(Arc::from(DEST), &LoadOptions::default()).expect("opens");
    let edit = EditDoc::new(&doc);
    let mut off = Vec::new();
    let mut on = Vec::new();
    let options = |subset| SaveOptions {
        subset_new_fonts: subset,
        id_source: IdSource::Fixed([1; 16]),
        ..SaveOptions::default()
    };
    save(&edit, &options(false), &mut off).expect("saves");
    save(&edit, &options(true), &mut on).expect("saves");
    assert_eq!(off, on, "nothing new to subset means byte-identical output");
}

// ---- an incremental save subsets too, and the minted object lands in the
// delta table rather than only in the body ----

#[test]
fn an_incremental_save_subsets_and_stays_readable() {
    let src = load(Arc::from(CID_TRUETYPE), &LoadOptions::default()).expect("source opens");
    let dest = load(Arc::from(DEST), &LoadOptions::default()).expect("destination opens");
    let mut edit = EditDoc::new(&dest);
    import_pages(
        &mut edit,
        &src,
        &PageRange::of([0u32]),
        &ImportOptions {
            at: PageIndex::new(0),
            viewer_preferences: false,
        },
    )
    .expect("imports");

    let mut out = Vec::new();
    save(
        &edit,
        &SaveOptions {
            mode: SaveMode::Incremental,
            subset_new_fonts: true,
            id_source: IdSource::Fixed([7; 16]),
            ..SaveOptions::default()
        },
        &mut out,
    )
    .expect("saves");

    // The original bytes are still the file's prefix (invariant R7), and the
    // appended section reads back — which it only can if the `/CIDToGIDMap`
    // the subsetter minted got a cross-reference entry of its own.
    assert!(out.starts_with(DEST));
    let doc = Document::from_bytes(Arc::from(&out[..])).expect("reopens");
    assert_eq!(doc.page_count(), 2);
    let (_, length1) = font_program(&out);
    assert_eq!(length1, 20424, "the appended program is the subset");
}
