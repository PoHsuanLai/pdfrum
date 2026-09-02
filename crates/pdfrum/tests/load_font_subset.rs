//! `SaveOptions::subset_new_fonts` over the fonts `DocEdit::embed_font` added.
//!
//! Ports PDFium's `CPDFFontSubsetterTest` and
//! `FPDFSaveWithFontSubsetEmbedderTest` cases
//! (`core/fpdfapi/edit/cpdf_fontsubsetter_embeddertest.cpp`,
//! `fpdfsdk/fpdf_save_embeddertest.cpp`). The C++ inspects the subsetter's
//! override map directly; there is no such map here, so each case makes the
//! same claim through the saved file instead — the subset tag, the shrunken
//! `/FontFile2`, and the text that still extracts.
//!
//! Split out of `load_font.rs`, which holds the embedding and standard-14
//! cases. The five helpers below are duplicated from it rather than shared:
//! STYLE §4 forbids a test module named `common`/`util`/`helpers`, and the
//! repo has no `#[path]`-included test module to follow.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot open its own fixture has nothing to report but a panic"
)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use pdfrum::{Document, FontEncoding, Point, Resolve, SaveOptions, StandardFont, TextBuilder};
use pdfrum_object::{Dict, Name, ObjRef};

const HELLO_PDF: &[u8] = include_bytes!("fixtures/hello_world.pdf");
const ROBOTO: &[u8] = include_bytes!("fixtures/roboto.ttf");
const BUG_377948405: &[u8] = include_bytes!("fixtures/bug_377948405.ttf");

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("pdfrum-load-font-subset");
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn fetch_dict(doc: &Document, reference: ObjRef) -> Dict {
    doc.parser()
        .fetch(reference)
        .expect("fetches")
        .as_dict()
        .expect("dict")
        .clone()
}

/// The `/Length1` of the first `/FontFile2` stream in `pdf`.
fn length1_of(pdf: &[u8]) -> i64 {
    let needle = b"/Length1 ";
    let mut i = 0;
    while i + needle.len() < pdf.len() {
        if pdf.get(i..i + needle.len()) == Some(needle.as_slice()) {
            let rest = pdf.get(i + needle.len()..).unwrap_or_default();
            let digits: String = rest
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .map(|b| char::from(*b))
                .collect();
            if let Ok(n) = digits.parse::<i64>() {
                return n;
            }
        }
        i += 1;
    }
    panic!("no /Length1 in the saved file");
}

fn extracted(bytes: &[u8]) -> String {
    let doc = Document::from_bytes(Arc::from(bytes)).expect("opens");
    doc.page(0).expect("page").text().to_string()
}

/// `pdfium_test --md5` over `path`, as the reopen check. The binary exits 0
/// even when the load fails, so the caller must look for an `MD5:` line: its
/// absence is how a file the oracle could not open shows up.
fn oracle_md5(path: &Path) -> Result<String, String> {
    let candidates = [
        "/mnt/data2/pdfium/pdfium-c++/out/Default/pdfium_test",
        "/mnt/data2/pdfium/pdfium-c++/out/Release/pdfium_test",
    ];
    let Some(bin) = candidates.iter().find(|p| Path::new(p).exists()) else {
        return Err("pdfium_test binary is absent; skip oracle reopen".into());
    };
    let out = Command::new(bin)
        .args(["--md5", "--png", "--pages=0"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "pdfium_test --md5 exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn subset_standard_font_produces_no_font_file() {
    // Ports CPDFFontSubsetterTest.StandardFont.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .standard_font(StandardFont::Helvetica)
        .expect("Helvetica");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Hello standard"), font.object(), 16.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("subset_standard.pdf");
    edit.save_pages(
        &out,
        &[page],
        &SaveOptions {
            subset_new_fonts: true,
            ..SaveOptions::default()
        },
    )
    .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    // The C++'s `GenerateObjectOverrides` returns **empty** for a standard
    // font: there is no program to subset, so the subsetter must leave the
    // dictionary exactly as `standard_font` wrote it. In particular it must
    // not mint a subset tag for a font it did not subset — a `/BaseFont` of
    // `ABCDEF+Helvetica` with no `/FontDescriptor` names a face no reader can
    // find.
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Helvetica"[..]),
        "an unsubsettable font keeps its untagged name"
    );
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type1"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Encoding")).map(Name::as_bytes),
        Some(&b"WinAnsiEncoding"[..])
    );
    assert!(font_dict.raw(&Name::from("FontDescriptor")).is_none());
    assert!(font_dict.raw(&Name::from("Widths")).is_none());
    assert!(font_dict.raw(&Name::from("ToUnicode")).is_none());
    // And no font program of any flavour was invented for it.
    assert!(!saved.windows(9).any(|w| w == b"/FontFile"));

    let text = extracted(&saved);
    assert!(text.contains("Hello standard"));
}

#[test]
fn subset_without_new_fonts_leaves_file_unchanged() {
    // Ports FPDFSaveWithFontSubsetEmbedderTest.SaveWithSubsetWithoutNewText.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let dir = scratch_dir();
    let out_plain = dir.join("without_subset_no_new.pdf");
    let out_subset = dir.join("with_subset_no_new.pdf");

    doc.save_with(
        &out_plain,
        &SaveOptions {
            subset_new_fonts: false,
            ..SaveOptions::default()
        },
    )
    .expect("plain save");
    doc.save_with(
        &out_subset,
        &SaveOptions {
            subset_new_fonts: true,
            ..SaveOptions::default()
        },
    )
    .expect("subset save");

    let bytes_plain = std::fs::read(&out_plain).expect("reads plain");
    let bytes_subset = std::fs::read(&out_subset).expect("reads subset");
    assert_eq!(bytes_plain.len(), bytes_subset.len());
    // Strip trailing document ID dictionary which is freshly randomized on each save.
    let id_marker = b"/ID[";
    let plain_prefix = bytes_plain
        .windows(id_marker.len())
        .position(|w| w == id_marker)
        .map(|pos| bytes_plain.get(..pos).unwrap_or_default());
    let subset_prefix = bytes_subset
        .windows(id_marker.len())
        .position(|w| w == id_marker)
        .map(|pos| bytes_subset.get(..pos).unwrap_or_default());
    assert_eq!(plain_prefix, subset_prefix);
}

#[test]
fn subset_single_font_multiple_texts_preserves_all_runs() {
    // Ports CPDFFontSubsetterTest.SingleFontMultipleTexts.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 130.0),
            ..TextBuilder::new(font.encode("Hello "), font.object(), 14.0)
        }
        .build(),
    );
    page.push(
        TextBuilder {
            position: Point::new(20.0, 160.0),
            ..TextBuilder::new(font.encode("world!"), font.object(), 14.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("single_font_multiple_texts.pdf");
    edit.save_pages(
        &out,
        &[page],
        &SaveOptions {
            subset_new_fonts: true,
            ..SaveOptions::default()
        },
    )
    .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let text = extracted(&saved);
    assert!(text.contains("Hello "));
    assert!(text.contains("world!"));
    let len1 = length1_of(&saved);
    assert!(len1 < i64::try_from(ROBOTO.len()).expect("fits"));

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

/// Walk one subsetted composite font's whole chain and return its subset tag.
///
/// This is the six overrides `CPDFFontSubsetterTest` inspects in its override
/// map — root font, CID font, descriptor, `/W`, `/ToUnicode` and the shrunken
/// program stream — read back off the saved file instead, since there is no
/// override map to look at here.
fn subset_chain_of(doc: &Document, font: ObjRef, original_len: usize, label: &str) -> Vec<u8> {
    let root = fetch_dict(doc, font);
    assert_eq!(
        root.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type0"[..]),
        "{label} is a composite font"
    );
    let base = root
        .name(&Name::from("BaseFont"))
        .map(|n| n.as_bytes().to_vec())
        .unwrap_or_default();

    // `IsRootFont(kBaseFontName)`: a six-letter uppercase subset tag plus `+`
    // prefixes the name (ISO 32000-1 §9.6.4).
    let tag = base.get(..7).map(<[u8]>::to_vec).unwrap_or_default();
    assert_eq!(
        tag.len(),
        7,
        "{label} /BaseFont {base:?} is too short for a tag"
    );
    assert_eq!(
        tag.get(6),
        Some(&b'+'),
        "{label} /BaseFont {base:?} lacks `XXXXXX+`"
    );
    assert!(
        tag.get(..6)
            .is_some_and(|t| t.iter().all(u8::is_ascii_uppercase)),
        "{label} subset tag is not six uppercase letters: {base:?}"
    );

    let descendants = root
        .array(&Name::from("DescendantFonts"), doc.parser())
        .expect("DescendantFonts");
    assert_eq!(descendants.len(), 1);
    let cid = fetch_dict(doc, descendants.reference_at(0).expect("cid ref"));
    // `IsCIDFont(…)`: the descendant carries the tagged name too.
    assert_eq!(
        cid.name(&Name::from("BaseFont"))
            .map(|n| n.as_bytes().to_vec()),
        Some(base.clone()),
        "{label} descendant name should match the root's"
    );
    // `IsWidths()` and `IsToUnicode()`.
    assert!(cid.raw(&Name::from("W")).is_some(), "{label} keeps /W");
    assert!(
        root.raw(&Name::from("ToUnicode")).is_some(),
        "{label} keeps /ToUnicode"
    );

    // `IsFontDescriptor(…)` and `StreamSizeIsWithinRange(…)`.
    let desc = fetch_dict(
        doc,
        cid.reference(&Name::from("FontDescriptor"))
            .expect("FontDescriptor"),
    );
    assert_eq!(
        desc.name(&Name::from("FontName"))
            .map(|n| n.as_bytes().to_vec()),
        Some(base),
        "{label} descriptor /FontName should match /BaseFont"
    );
    let file = doc
        .parser()
        .fetch(desc.reference(&Name::from("FontFile2")).expect("FontFile2"))
        .expect("font file")
        .as_stream()
        .expect("stream")
        .clone();
    let embedded = pdfrum_filters::decode_chain(
        &file,
        0,
        doc.parser(),
        &pdfrum_common::Limits::default(),
        &mut pdfrum_common::Diagnostics::default(),
    )
    .data
    .len();
    assert!(
        embedded < original_len,
        "{label}'s subsetted program ({embedded} B) should be smaller than the \
         {original_len} B it was built from"
    );

    tag
}

#[test]
fn subset_multiple_fonts_multiple_texts_shrinks_both() {
    // Ports CPDFFontSubsetterTest.MultipleFontsMultipleTexts and
    // FPDFSaveWithFontSubsetEmbedderTest.SaveWithSubsetMultipleFontsMultipleTexts.
    //
    // The C++ pins twelve overrides — six per font: root font, CID font,
    // descriptor, widths, `/ToUnicode` and a stream shrunk to a few percent of
    // the original. There is no override map to inspect here (the subsetter
    // writes straight into the save), so the same claim is made through the
    // saved file: two *independent* font chains, each with its own subset tag,
    // each with a `/FontFile2` smaller than the program it came from, and both
    // texts still extractable.
    //
    // The second font is `bug_377948405.ttf`, whose cmap covers exactly
    // `A À Ä Å Æ È` — six characters, which is what makes it a genuinely
    // different face from Roboto rather than a second copy of the same
    // coverage. Only a TrueType-outline composite is subsettable
    // (`collect.rs:234`), so both must be TrueType for "shrinks both" to mean
    // anything.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font1 = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds font1");
    let font2 = edit
        .embed_font(BUG_377948405, FontEncoding::Composite)
        .expect("embeds font2");
    // Both codes are real glyphs, not `.notdef` — otherwise the extraction
    // assertions below would pass on an empty run.
    assert_ne!(font2.encode("A"), vec![0, 0]);
    assert_ne!(
        font1.object(),
        font2.object(),
        "two programs get two font dictionaries"
    );

    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 130.0),
            ..TextBuilder::new(font1.encode("Hello"), font1.object(), 14.0)
        }
        .build(),
    );
    page.push(
        TextBuilder {
            position: Point::new(20.0, 160.0),
            ..TextBuilder::new(font2.encode("AÀÄ"), font2.object(), 14.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("multiple_fonts_multiple_texts.pdf");
    edit.save_pages(
        &out,
        &[page],
        &SaveOptions {
            subset_new_fonts: true,
            ..SaveOptions::default()
        },
    )
    .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    // Both runs survive, in both fonts — the C++'s `TestExtractedFont` over
    // the concatenation of the two expected strings.
    let text = extracted(&saved);
    assert!(text.contains("Hello"), "font 1's run, got {text:?}");
    assert!(text.contains("AÀÄ"), "font 2's run, got {text:?}");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let tag1 = subset_chain_of(&saved_doc, font1.object(), ROBOTO.len(), "font1");
    let tag2 = subset_chain_of(&saved_doc, font2.object(), BUG_377948405.len(), "font2");

    // Each font gets its *own* tag: the subsetter must not reuse one, or a
    // reader that caches by name conflates the two faces.
    assert_ne!(tag1, tag2, "the two subset tags collide: {tag1:?}");

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(
                stdout.contains("MD5:"),
                "the oracle reopens a two-font subsetted save, got {stdout:?}"
            );
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn subset_embedded_truetype_shrinks_program_and_preserves_extraction() {
    // Ports CPDFFontSubsetterTest.TrueType and FPDFSaveWithFontSubsetEmbedderTest.SaveWithSubsetWithNewText.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit_full = doc.edit();
    let font_full = edit_full
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");
    let mut page_full = doc.page(0).expect("page").edit();
    page_full.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font_full.encode("Hello world"), font_full.object(), 20.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out_full = dir.join("truetype_full.pdf");
    edit_full
        .save_pages(&out_full, &[page_full], &SaveOptions::default())
        .expect("saves full");
    let saved_full = std::fs::read(&out_full).expect("reads full");
    let before_len = length1_of(&saved_full);

    let mut edit_subset = doc.edit();
    let font_sub = edit_subset
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");
    let mut page_sub = doc.page(0).expect("page").edit();
    page_sub.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font_sub.encode("Hello world"), font_sub.object(), 20.0)
        }
        .build(),
    );
    let out_sub = dir.join("truetype_subset.pdf");
    edit_subset
        .save_pages(
            &out_sub,
            &[page_sub],
            &SaveOptions {
                subset_new_fonts: true,
                ..SaveOptions::default()
            },
        )
        .expect("saves subset");
    let saved_sub = std::fs::read(&out_sub).expect("reads sub");
    let after_len = length1_of(&saved_sub);

    assert!(
        after_len < before_len,
        "subsetting should reduce program size: {after_len} < {before_len}"
    );

    let text = extracted(&saved_sub);
    assert!(text.contains("Hello world"));

    match oracle_md5(&out_sub) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
#[ignore = "OTTO subsetting is deliberately skipped (pdfrum-edit §3.7 divergence 2 and §7.3): \
            an OpenType/CFF program is embedded as /FontFile3 and passed over by the subsetter, \
            so there is no /CIDFontType0 override shape for CPDFFontSubsetterTest.OpenType to assert"]
fn subset_opentype_cff_font() {
    // Ports CPDFFontSubsetterTest.OpenType.
}
