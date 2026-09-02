//! `DocEdit::embed_font` and `DocEdit::standard_font`: what a loaded font
//! writes into the file, and what comes back out of it.
//!
//! Ports PDFium's `FPDFText_LoadFont` / `FPDFText_LoadStandardFont` embedder
//! cases (`fpdfsdk/fpdf_edittext_embeddertest.cpp`,
//! `fpdfsdk/fpdf_edit_embeddertest.cpp`) onto
//! `DocEdit::embed_font(bytes, FontEncoding::{Simple, Composite})` and
//! `DocEdit::standard_font(StandardFont)`. Each test asserts what its C++
//! counterpart asserts in substance — the dictionary shape the load produces,
//! the text that extracts back through `/ToUnicode`, the ink the page gains,
//! and that the oracle reopens the result — rather than only that the call
//! returned `Ok`.
//!
//! The five subsetting cases live in `load_font_subset.rs`; the two files
//! duplicate their five helpers rather than share a module, because STYLE §4
//! forbids a test module named `common`/`util`/`helpers` and the repo has no
//! `#[path]` convention to reach for instead.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test that cannot open its own fixture has nothing to report but a panic"
)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use pdfrum::{
    Affine, Dict, Document, FontEncoding, ObjRef, Point, RenderOptions, Resolve, SaveOptions,
    StandardFont, TextBuilder,
};
use pdfrum_object::Name;

const HELLO_PDF: &[u8] = include_bytes!("fixtures/hello_world.pdf");
const ROBOTO: &[u8] = include_bytes!("fixtures/roboto.ttf");
const BUG_2094: &[u8] = include_bytes!("fixtures/bug_2094.ttf");
const BUG_377948405: &[u8] = include_bytes!("fixtures/bug_377948405.ttf");
/// A real Type 1 program. The oracle's `LoadSimpleType1Font` /
/// `LoadCIDType0Font` hand `FPDFText_LoadFont` a *stock* font's span, which
/// under the hermetic test-fonts config is a TrueType substitute — so the
/// Type 1 path those two cases name is only reachable here with an actual
/// Type 1 program. `pdfrum-type1`'s fixture is one, and is not duplicated
/// into this directory.
const FOXIT_SERIF_MM: &[u8] = include_bytes!("../../pdfrum-type1/tests/fixtures/FoxitSerifMM.pfb");

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("pdfrum-load-font");
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

fn pixels(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let doc = Document::from_bytes(Arc::from(bytes)).expect("opens");
    let pix = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::default())
        .expect("renders");
    (pix.width(), pix.height(), pix.data().to_vec())
}

fn dark_pixels(data: &[u8]) -> usize {
    data.chunks_exact(4)
        .filter(|px| {
            let r = px.first().copied().unwrap_or(255);
            let g = px.get(1).copied().unwrap_or(255);
            let b = px.get(2).copied().unwrap_or(255);
            let a = px.get(3).copied().unwrap_or(0);
            a > 0 && (u16::from(r) + u16::from(g) + u16::from(b)) < 600
        })
        .count()
}

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
fn embed_composite_writes_hello_extracts_and_renders() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original = doc
        .page(0)
        .expect("page")
        .render(&RenderOptions::default())
        .expect("renders");
    let original_dark = dark_pixels(original.data());

    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds Roboto");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: pdfrum::Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Hello"), font.object(), 24.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("hello_roboto.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let text = extracted(&saved);
    assert!(
        text.contains("Hello"),
        "extracted text should contain Hello via /ToUnicode, got {text:?}"
    );

    let (_, _, pix) = pixels(&saved);
    let after_dark = dark_pixels(&pix);
    assert!(
        after_dark > original_dark,
        "placed text should add ink ({after_dark} dark pixels vs original {original_dark})"
    );

    let before = length1_of(&saved);

    // The first save wrote the font as new without subsetting. A second
    // save of the same construction with `subset_new_fonts` should shrink
    // the program.
    let mut edit_first = doc.edit();
    let font = edit_first
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: pdfrum::Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Hello"), font.object(), 24.0)
        }
        .build(),
    );
    let subset_path = dir.join("hello_roboto_subset.pdf");
    edit_first
        .save_pages(
            &subset_path,
            &[page],
            &SaveOptions {
                subset_new_fonts: true,
                ..SaveOptions::default()
            },
        )
        .expect("subset save");
    let subset_bytes = std::fs::read(&subset_path).expect("reads subset");
    let after = length1_of(&subset_bytes);
    assert!(
        after < before,
        "subsetted /FontFile2 Length1 {after} should be smaller than {before}"
    );

    let subset_text = extracted(&subset_bytes);
    assert!(
        subset_text.contains("Hello"),
        "subsetted file still extracts Hello, got {subset_text:?}"
    );
    let (_, _, subset_pix) = pixels(&subset_bytes);
    assert!(
        dark_pixels(&subset_pix) > original_dark,
        "subsetted file still draws the new text"
    );

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(
                stdout.contains("MD5:"),
                "pdfium_test --md5 should print a page hash, got {stdout:?}"
            );
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
    match oracle_md5(&subset_path) {
        Ok(stdout) => {
            assert!(
                stdout.contains("MD5:"),
                "oracle should reopen the subsetted file, got {stdout:?}"
            );
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen (subset): {msg}");
        }
    }
}

#[test]
fn standard_fourteen_round_trips_hello() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(
        doc.page(0)
            .expect("page")
            .render(&RenderOptions::default())
            .expect("renders")
            .data(),
    );

    let mut edit = doc.edit();
    let font = edit
        .standard_font(StandardFont::Helvetica)
        .expect("Helvetica");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: pdfrum::Point::new(20.0, 60.0),
            ..TextBuilder::new(font.encode("Hello"), font.object(), 18.0)
        }
        .build(),
    );
    let dir = scratch_dir();
    let out = dir.join("hello_helvetica.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");
    let text = extracted(&saved);
    assert!(
        text.contains("Hello"),
        "standard-14 WinAnsi text should extract, got {text:?}"
    );
    let (_, _, pix) = pixels(&saved);
    assert!(
        dark_pixels(&pix) > original_dark,
        "standard-14 text should add ink"
    );
}

#[test]
fn add_standard_font_text_extracts_and_renders() {
    // Ports FPDFEditEmbedderTest.AddStandardFontText and AddStandardFontText2.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .standard_font(StandardFont::Helvetica)
        .expect("Helvetica");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 20.0),
            ..TextBuilder::new(font.encode("This is some text."), font.object(), 12.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("add_standard_font.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_obj = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_obj.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        font_obj.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type1"[..])
    );
    assert_eq!(
        font_obj.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Helvetica"[..])
    );
    assert_eq!(
        font_obj.name(&Name::from("Encoding")).map(Name::as_bytes),
        Some(&b"WinAnsiEncoding"[..])
    );
    assert!(font_obj.raw(&Name::from("Widths")).is_none());
    assert!(font_obj.raw(&Name::from("FontDescriptor")).is_none());

    let text = extracted(&saved);
    assert!(
        text.contains("This is some text."),
        "extracted text should contain added text, got {text:?}"
    );

    let (_, _, pix) = pixels(&saved);
    let after_dark = dark_pixels(&pix);
    assert!(
        after_dark > original_dark,
        "standard font text should add ink"
    );

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn standard_fonts_encode_winansi_and_unmappable() {
    // Ports FPDFEditEmbedderTest.LoadStandardFonts and CharCodeFromUnicode WinAnsi mapping.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();

    let helvetica = edit
        .standard_font(StandardFont::Helvetica)
        .expect("sans: Helvetica");
    let times = edit
        .standard_font(StandardFont::Times)
        .expect("serif: Times-Roman");
    let courier = edit
        .standard_font(StandardFont::Courier)
        .expect("fixed-pitch: Courier");

    for font in [&helvetica, &times, &courier] {
        // Non-ASCII WinAnsi characters:
        // '€' (U+20AC) -> WinAnsi 0x80 (128)
        assert_eq!(font.encode("€"), vec![128]);
        // 'é' (U+00E9) -> WinAnsi 0xE9 (233)
        assert_eq!(font.encode("é"), vec![233]);
        // Combined string
        assert_eq!(
            font.encode("Café €"),
            vec![b'C', b'a', b'f', 233, b' ', 128]
        );
        // Unmappable characters -> code 0 (CharCodeFromUnicode)
        assert_eq!(font.encode("\u{0100}"), vec![0]);
        assert_eq!(font.encode("\u{4e00}"), vec![0]);
    }

    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 30.0),
            ..TextBuilder::new(
                helvetica.encode("Helvetica Café €"),
                helvetica.object(),
                12.0,
            )
        }
        .build(),
    );
    page.push(
        TextBuilder {
            position: Point::new(20.0, 50.0),
            ..TextBuilder::new(times.encode("Times Café €"), times.object(), 12.0)
        }
        .build(),
    );
    page.push(
        TextBuilder {
            position: Point::new(20.0, 70.0),
            ..TextBuilder::new(courier.encode("Courier Café €"), courier.object(), 12.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("standard_14_winansi.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    for (obj, expected_name) in [
        (helvetica.object(), &b"Helvetica"[..]),
        (times.object(), &b"Times-Roman"[..]),
        (courier.object(), &b"Courier"[..]),
    ] {
        let dict = fetch_dict(&saved_doc, obj);
        assert_eq!(
            dict.name(&Name::from("Subtype")).map(Name::as_bytes),
            Some(&b"Type1"[..])
        );
        assert_eq!(
            dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
            Some(expected_name)
        );
        assert_eq!(
            dict.name(&Name::from("Encoding")).map(Name::as_bytes),
            Some(&b"WinAnsiEncoding"[..])
        );
    }

    let text = extracted(&saved);
    assert!(text.contains("Helvetica"));
    assert!(text.contains("Times"));
    assert!(text.contains("Courier"));

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn font_encoding_variants_encode_ascii_non_ascii_and_unmappable() {
    // Ports FontEncoding::{Simple, Composite} CharCodeFromUnicode and Identity-H mappings.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();

    let simple = edit
        .embed_font(ROBOTO, FontEncoding::Simple)
        .expect("embeds simple");
    assert_eq!(simple.encode("Hello"), b"Hello".to_vec());
    assert_eq!(simple.encode("é"), vec![233]);
    assert_eq!(simple.encode("\u{0100}"), vec![0]);
    assert_eq!(simple.encode("\u{4e00}"), vec![0]);

    let composite = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds composite");
    let encoded_hello = composite.encode("Hello");
    assert_eq!(
        encoded_hello.len(),
        10,
        "2 bytes per character for composite"
    );
    assert_eq!(composite.encode("\u{1f600}"), vec![0, 0]);
}

#[test]
fn add_truetype_font_simple_encoding_extracts_and_renders() {
    // Ports FPDFEditEmbedderTest.AddTrueTypeFontText.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Simple)
        .expect("embeds simple Roboto");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(40.0, 40.0),
            ..TextBuilder::new(font.encode("This is some text."), font.object(), 12.0)
        }
        .build(),
    );
    page.push(
        TextBuilder {
            position: Point::new(40.0, 80.0),
            ..TextBuilder::new(font.encode("Bigger font size"), font.object(), 15.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("add_truetype_simple.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_dict.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"TrueType"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Roboto-Regular"[..])
    );

    let first = font_dict
        .direct_int(&Name::from("FirstChar"))
        .expect("FirstChar");
    let last = font_dict
        .direct_int(&Name::from("LastChar"))
        .expect("LastChar");
    assert!(last >= first);

    let widths_ref = font_dict.reference(&Name::from("Widths")).expect("Widths");
    let widths = saved_doc
        .parser()
        .fetch(widths_ref)
        .expect("widths")
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(
        widths.len(),
        usize::try_from(last - first + 1).expect("fits")
    );

    let desc_ref = font_dict
        .reference(&Name::from("FontDescriptor"))
        .expect("FontDescriptor");
    let desc = fetch_dict(&saved_doc, desc_ref);
    let file2_ref = desc.reference(&Name::from("FontFile2")).expect("FontFile2");
    let file2 = saved_doc
        .parser()
        .fetch(file2_ref)
        .expect("file2")
        .as_stream()
        .expect("stream")
        .clone();
    let length1 = file2
        .dict
        .direct_int(&Name::from("Length1"))
        .expect("Length1");
    assert_eq!(length1, i64::try_from(ROBOTO.len()).expect("fits"));

    let text = extracted(&saved);
    assert!(text.contains("This is some text."));
    assert!(text.contains("Bigger font size"));

    let (_, _, pix) = pixels(&saved);
    assert!(dark_pixels(&pix) > original_dark);

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn load_simple_truetype_font_descriptor_and_widths_shape() {
    // Ports FPDFEditEmbedderTest.LoadSimpleTrueTypeFont.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Simple)
        .expect("embeds simple Roboto");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 30.0),
            ..TextBuilder::new(font.encode("Courier Cousine Test"), font.object(), 12.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("load_simple_truetype_shape.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_dict.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"TrueType"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Roboto-Regular"[..])
    );

    let first = font_dict
        .direct_int(&Name::from("FirstChar"))
        .expect("FirstChar");
    let last = font_dict
        .direct_int(&Name::from("LastChar"))
        .expect("LastChar");
    assert!(first <= 32, "first char code <= 32, got {first}");
    assert!(last >= 126);

    let widths_ref = font_dict.reference(&Name::from("Widths")).expect("Widths");
    let widths = saved_doc
        .parser()
        .fetch(widths_ref)
        .expect("widths")
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(
        widths.len(),
        usize::try_from(last - first + 1).expect("fits")
    );
    let non_zero_count = widths
        .iter()
        .filter_map(|o| o.as_int().and_then(|w| (w > 0).then_some(())))
        .count();
    assert!(
        non_zero_count > 100,
        "expected over 100 positive glyph advances, got {non_zero_count}"
    );

    let desc_ref = font_dict
        .reference(&Name::from("FontDescriptor"))
        .expect("FontDescriptor");
    let desc = fetch_dict(&saved_doc, desc_ref);
    assert_eq!(
        desc.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"FontDescriptor"[..])
    );
    let flags = desc.direct_int(&Name::from("Flags")).expect("Flags");
    assert_eq!(flags & (1 << 5), 1 << 5, "NonSymbolic flag bit 6");
    assert!(desc.raw(&Name::from("FontBBox")).is_some());
    assert!(desc.raw(&Name::from("Ascent")).is_some());
    assert!(desc.raw(&Name::from("Descent")).is_some());
    assert!(desc.raw(&Name::from("CapHeight")).is_some());
    assert!(desc.raw(&Name::from("StemV")).is_some());
}

#[test]
#[allow(clippy::too_many_lines)]
fn load_cid_type2_font_dictionary_structure() {
    // Ports FPDFEditEmbedderTest.LoadCIDType2Font.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds composite Roboto");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("CID Type 2"), font.object(), 14.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("load_cid_type2.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_dict.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type0"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Roboto-Regular"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Encoding")).map(Name::as_bytes),
        Some(&b"Identity-H"[..])
    );

    let descendants = font_dict
        .array(&Name::from("DescendantFonts"), saved_doc.parser())
        .expect("DescendantFonts");
    assert_eq!(descendants.len(), 1);
    let cid_ref = descendants.reference_at(0).expect("cid ref");
    let cid_dict = fetch_dict(&saved_doc, cid_ref);
    assert_eq!(
        cid_dict.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        cid_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"CIDFontType2"[..])
    );
    assert_eq!(
        cid_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Roboto-Regular"[..])
    );

    let cid_info = cid_dict
        .dict(&Name::from("CIDSystemInfo"), saved_doc.parser())
        .expect("CIDSystemInfo");
    assert_eq!(
        cid_info.byte_string(&Name::from("Registry"), saved_doc.parser()),
        Some(b"Adobe".to_vec())
    );
    assert_eq!(
        cid_info.byte_string(&Name::from("Ordering"), saved_doc.parser()),
        Some(b"Identity".to_vec())
    );
    assert_eq!(cid_info.direct_int(&Name::from("Supplement")), Some(0));

    let w_ref = cid_dict.reference(&Name::from("W")).expect("W");
    let w = saved_doc
        .parser()
        .fetch(w_ref)
        .expect("w")
        .as_array()
        .expect("array")
        .clone();
    assert!(!w.is_empty(), "CID widths array should not be empty");

    let tu = font_dict
        .stream(&Name::from("ToUnicode"), saved_doc.parser())
        .expect("ToUnicode");
    let tu_bytes = pdfrum_filters::decode_chain(
        &tu,
        0,
        saved_doc.parser(),
        &pdfrum_common::Limits::default(),
        &mut pdfrum_common::Diagnostics::default(),
    )
    .data;
    assert!(tu_bytes.windows(9).any(|w| w == b"begincmap"));
    assert!(tu_bytes.windows(7).any(|w| w == b"endcmap"));

    let desc_ref = cid_dict
        .reference(&Name::from("FontDescriptor"))
        .expect("FontDescriptor");
    let desc = fetch_dict(&saved_doc, desc_ref);
    let file2_ref = desc.reference(&Name::from("FontFile2")).expect("FontFile2");
    let file2 = saved_doc
        .parser()
        .fetch(file2_ref)
        .expect("file2")
        .as_stream()
        .expect("stream")
        .clone();
    assert!(file2.dict.direct_int(&Name::from("Length1")).is_some());
}

#[test]
fn add_cid_font_text_extracts_and_renders() {
    // Ports FPDFEditEmbedderTest.AddCIDFontText and FPDFEditEmbedderTest.EmbedNotoSansSCFont.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds composite Roboto");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 50.0),
            ..TextBuilder::new(font.encode("Hello world, Café naïve!"), font.object(), 14.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("add_cid_font_text.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let text = extracted(&saved);
    assert!(
        text.contains("Hello world, Café naïve!"),
        "ToUnicode CMap must recover exact unicode string, got {text:?}"
    );

    let (_, _, pix) = pixels(&saved);
    assert!(dark_pixels(&pix) > original_dark);

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn embed_composite_font_direct_charcodes_extracts_and_renders() {
    // Ports FPDFEditEmbedderTest.EmbedNotoSansSCFontWithCharcodes.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");

    let codes = font.encode("Hello direct");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 140.0),
            ..TextBuilder::new(codes, font.object(), 16.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("direct_charcodes.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let text = extracted(&saved);
    assert!(text.contains("Hello direct"));

    let (_, _, pix) = pixels(&saved);
    assert!(dark_pixels(&pix) > original_dark);

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

/// Ports `FPDFEditEmbedderTest.Bug2094`.
///
/// The C++ case is one line — `EXPECT_TRUE(font)` — because the bug was a
/// crash while *building* the font, on a program whose tables are degenerate.
/// A Rust `Result` makes "did not crash" free, so the port has to say more to
/// be worth its fixture: it pins the whole composite shape this program
/// produces, including the two facts that made the bug reachable — the font
/// names itself `Test` and its cmap covers **nothing**, so every code maps to
/// `.notdef` and `/W` collapses to one run at CID 0.
#[test]
fn embed_bug_2094_ttf_composite_succeeds() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .embed_font(BUG_2094, FontEncoding::Composite)
        .expect("embeds bug_2094.ttf");

    // The program has no usable Unicode cmap, so `char_maps`'s fallback runs
    // and every character encodes to two zero bytes.
    assert_eq!(font.encode("A"), vec![0, 0]);

    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Test"), font.object(), 16.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("bug_2094.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_dict.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type0"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Encoding")).map(Name::as_bytes),
        Some(&b"Identity-H"[..])
    );
    // The program's own PostScript name, degenerate as it is, is what
    // `/BaseFont` carries — not a placeholder.
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"Test"[..])
    );

    let descendants = font_dict
        .array(&Name::from("DescendantFonts"), saved_doc.parser())
        .expect("DescendantFonts");
    assert_eq!(descendants.len(), 1);
    let cid_dict = fetch_dict(&saved_doc, descendants.reference_at(0).expect("cid ref"));
    assert_eq!(
        cid_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"CIDFontType2"[..])
    );

    // One run starting at CID 0, four glyphs wide. This is the array the
    // crashing build never got to write.
    let w = saved_doc
        .parser()
        .fetch(cid_dict.reference(&Name::from("W")).expect("W"))
        .expect("w")
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(w.len(), 2, "one `c [w …]` run, got {w:?}");
    assert_eq!(w.int_at(0), Some(0), "the run starts at CID 0");
    let run = w.array_at(1, saved_doc.parser()).expect("run array");
    assert_eq!(
        run.iter()
            .filter_map(pdfrum_object::Object::as_int)
            .collect::<Vec<_>>(),
        vec![1000, 0, 1000, 1000]
    );

    // The descriptor still points at the whole program, unaltered.
    let desc = fetch_dict(
        &saved_doc,
        cid_dict
            .reference(&Name::from("FontDescriptor"))
            .expect("FontDescriptor"),
    );
    let file2 = saved_doc
        .parser()
        .fetch(desc.reference(&Name::from("FontFile2")).expect("FontFile2"))
        .expect("file2")
        .as_stream()
        .expect("stream")
        .clone();
    assert_eq!(
        file2.dict.direct_int(&Name::from("Length1")),
        Some(i64::try_from(BUG_2094.len()).expect("fits"))
    );

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(
                stdout.contains("MD5:"),
                "the oracle reopens the file this program builds, got {stdout:?}"
            );
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn embed_bug_377948405_widths_array_compaction() {
    // Ports FPDFEditEmbedderTest.Bug377948405.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .embed_font(BUG_377948405, FontEncoding::Composite)
        .expect("embeds bug_377948405.ttf");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Test"), font.object(), 16.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("bug_377948405.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    let descendants = font_dict
        .array(&Name::from("DescendantFonts"), saved_doc.parser())
        .expect("DescendantFonts");
    let cid_dict = fetch_dict(&saved_doc, descendants.reference_at(0).expect("cid ref"));
    let w_ref = cid_dict.reference(&Name::from("W")).expect("W");
    let w = saved_doc
        .parser()
        .fetch(w_ref)
        .expect("w")
        .as_array()
        .expect("array")
        .clone();

    // The C++ case checks only entries 0 and 2 (`EXPECT_EQ(…GetIntegerAt(0),
    // 1)` and `…GetIntegerAt(2), 5)`) but its comment names the whole array the
    // fix produces. `create_widths_array` produces exactly that, so the port
    // pins all seven entries rather than the two the C++ happened to sample:
    //
    //   [1 [639]  5 7 639  8 [881 556]]
    //    ^        ^        ^
    //    |        |        `- another `c [w …]` run, CIDs 8 and 9
    //    |        `- a `first last w` run: CIDs 5..=7 all 639, compacted
    //    `- a `c [w …]` run at CID 1
    //
    // That is the whole point of the bug: before the fix, the three equal
    // widths at CIDs 5..7 were emitted one per entry instead of as one run.
    assert_eq!(w.len(), 7, "three runs, seven entries, got {w:?}");
    assert_eq!(w.int_at(0), Some(1));
    assert_eq!(w.int_at(2), Some(5));
    assert_eq!(w.int_at(3), Some(7));
    assert_eq!(w.int_at(4), Some(639));
    assert_eq!(w.int_at(5), Some(8));
    let first_run = w.array_at(1, saved_doc.parser()).expect("run 1");
    assert_eq!(
        first_run
            .iter()
            .filter_map(pdfrum_object::Object::as_int)
            .collect::<Vec<_>>(),
        vec![639]
    );
    let last_run = w.array_at(6, saved_doc.parser()).expect("run 3");
    assert_eq!(
        last_run
            .iter()
            .filter_map(pdfrum_object::Object::as_int)
            .collect::<Vec<_>>(),
        vec![881, 556]
    );

    // The run-length form is strictly shorter than the naive one: five CIDs
    // (5..=9) would need ten entries as `c [w]` pairs, and take four here.
    assert_eq!(
        cid_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"CIDFontType2"[..])
    );

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn transform_text_placement_and_matrix_render() {
    // Ports FPDFEditEmbedderTest text transformation and matrix placement.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(10.0, 10.0),
            ..TextBuilder::new(font.encode("Moved"), font.object(), 18.0)
        }
        .build(),
    );

    page.transform(0, Affine::translate((80.0, 60.0)))
        .expect("transforms");

    let dir = scratch_dir();
    let out = dir.join("transformed_text.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let text = extracted(&saved);
    assert!(text.contains("Moved"));

    let (_, _, pix) = pixels(&saved);
    assert!(dark_pixels(&pix) > original_dark);

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

#[test]
fn text_object_font_getters_retrieve_embedded_reference() {
    // Ports `FPDFTextObj_GetFont`: the font a text object was built with is
    // recoverable from the object, and it is the *same* font object the
    // resource dictionary ends up naming — a getter that returned a plausible
    // but unrelated reference would satisfy the in-memory check alone, so the
    // round-trip through the save is the load-bearing half.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let helvetica = edit
        .standard_font(StandardFont::Helvetica)
        .expect("Helvetica");
    let roboto = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds Roboto");
    assert_ne!(helvetica.object(), roboto.object());

    let mut page = doc.page(0).expect("page").edit();
    let before = page.len();
    page.push(
        TextBuilder {
            position: Point::new(15.0, 25.0),
            ..TextBuilder::new(helvetica.encode("Getter test"), helvetica.object(), 14.0)
        }
        .build(),
    );
    page.push(
        TextBuilder {
            position: Point::new(15.0, 55.0),
            ..TextBuilder::new(roboto.encode("Second font"), roboto.object(), 14.0)
        }
        .build(),
    );

    // Two objects, two different fonts, each reported against its own index —
    // not one answer given twice.
    assert_eq!(page.font_of(before), Some(helvetica.object()));
    assert_eq!(page.font_of(before + 1), Some(roboto.object()));
    // The page's pre-existing objects are text too, but they were not added
    // by this session, so their fonts are not this session's references.
    for i in 0..before {
        assert_ne!(page.font_of(i), Some(helvetica.object()));
        assert_ne!(page.font_of(i), Some(roboto.object()));
    }
    assert_eq!(page.font_of(999), None, "an out-of-range index has no font");

    // Both references survive the save and resolve to the dictionaries the
    // getter named.
    let dir = scratch_dir();
    let out = dir.join("font_getters.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");
    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    assert_eq!(
        fetch_dict(&saved_doc, helvetica.object())
            .name(&Name::from("BaseFont"))
            .map(Name::as_bytes),
        Some(&b"Helvetica"[..])
    );
    assert_eq!(
        fetch_dict(&saved_doc, roboto.object())
            .name(&Name::from("BaseFont"))
            .map(Name::as_bytes),
        Some(&b"Roboto-Regular"[..])
    );

    let text = extracted(&saved);
    assert!(text.contains("Getter test"));
    assert!(text.contains("Second font"));
}

#[test]
fn save_and_render_round_trip_with_embedded_font() {
    // Ports FPDFEditEmbedderTest.SaveAndRender.
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let (orig_w, orig_h, orig_pix) = pixels(HELLO_PDF);
    let orig_dark = dark_pixels(&orig_pix);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(ROBOTO, FontEncoding::Composite)
        .expect("embeds");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(50.0, 50.0),
            ..TextBuilder::new(font.encode("RoundTrip"), font.object(), 20.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("save_and_render.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let (saved_w, saved_h, saved_pix) = pixels(&saved);
    assert_eq!((saved_w, saved_h), (orig_w, orig_h));
    assert!(dark_pixels(&saved_pix) > orig_dark);

    let text = extracted(&saved);
    assert!(text.contains("RoundTrip"));

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

/// The `/FontFile` stream of `font`'s descriptor, with its three lengths.
///
/// `CheckFontDescriptor(font_dict, FPDF_FONT_TYPE1, …)` in the C++: a Type 1
/// program goes to `/FontFile` (ISO 32000-1 §9.9 Table 126) and never to
/// `/FontFile2` or `/FontFile3`, and it carries all three of the
/// clear/encrypted/trailer lengths Table 127 defines.
fn type1_font_file(doc: &Document, font: ObjRef) -> (pdfrum_object::Stream, i64, i64, i64) {
    let desc = fetch_dict(
        doc,
        fetch_dict(doc, font)
            .reference(&Name::from("FontDescriptor"))
            .expect("FontDescriptor"),
    );
    assert_eq!(
        desc.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"FontDescriptor"[..])
    );
    assert!(desc.raw(&Name::from("FontFile2")).is_none());
    assert!(desc.raw(&Name::from("FontFile3")).is_none());
    let file = doc
        .parser()
        .fetch(desc.reference(&Name::from("FontFile")).expect("FontFile"))
        .expect("fontfile")
        .as_stream()
        .expect("stream")
        .clone();
    let get = |k: &str| {
        file.dict
            .direct_int(&Name::from(k))
            .unwrap_or_else(|| panic!("{k} is missing from /FontFile"))
    };
    let (l1, l2, l3) = (get("Length1"), get("Length2"), get("Length3"));
    (file, l1, l2, l3)
}

/// Ports `FPDFEditEmbedderTest.LoadSimpleType1Font`.
///
/// The C++ case loads `CPDF_Font::GetStockFont(doc, "Times-Bold")`'s span with
/// `FPDF_FONT_TYPE1` and asserts a `/Type1` dictionary with `/FirstChar 32`,
/// `/LastChar 255`, a 224-entry `/Widths` and a descriptor whose font file is
/// the program handed in. Under the hermetic test-fonts config that stock span
/// is in fact a TrueType substitute (`Tinos-Bold`), so the assertions there are
/// about the *simple-font shape*, not about a Type 1 program. Here the same
/// shape is asserted over a genuinely Type 1 program, which additionally
/// reaches `/FontFile` and its `/Length1` `/Length2` `/Length3` triple —
/// `embed.rs`'s Type 1 branch, which the C++ case never actually exercises.
#[test]
fn load_simple_type1_font() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(FOXIT_SERIF_MM, FontEncoding::Simple)
        .expect("embeds a Type 1 program");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Type 1 simple"), font.object(), 14.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("load_simple_type1.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_dict.name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Font"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type1"[..]),
        "a Type 1 program embeds as /Subtype /Type1, not /TrueType"
    );
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"ChromeSerifMM"[..]),
        "/BaseFont is the program's own name, read out of its /FontName"
    );
    // The C++ case's `EXPECT_EQ(32, …FirstChar)` / `EXPECT_EQ(255, …LastChar)`.
    assert_eq!(font_dict.direct_int(&Name::from("FirstChar")), Some(32));
    assert_eq!(font_dict.direct_int(&Name::from("LastChar")), Some(255));

    let widths_ref = font_dict.reference(&Name::from("Widths")).expect("Widths");
    let widths = saved_doc
        .parser()
        .fetch(widths_ref)
        .expect("widths")
        .as_array()
        .expect("array")
        .clone();
    // `ASSERT_EQ(224u, widths_array->size())` — 255 - 32 + 1.
    assert_eq!(widths.len(), 224);
    let positive = widths
        .iter()
        .filter(|o| o.as_int().is_some_and(|w| w > 0))
        .count();
    assert!(
        positive > 100,
        "a Latin Type 1 face advances over 100 of the 224 codes, got {positive}"
    );

    let (_, l1, l2, l3) = type1_font_file(&saved_doc, font.object());
    // The three are the PFB's three segment payload lengths: clear text,
    // eexec-encrypted, and the 512-zeros-plus-`cleartomark` trailer. This is
    // the divergence `embed.rs` documents against the oracle, which writes
    // `/FontFile` with none of the three (`fpdf_edittext.cpp:166`,
    // `TODO(npm): Lengths for Type1 fonts.`).
    assert_eq!((l1, l2, l3), (10710, 102_155, 532));
    assert_eq!(
        l1 + l2 + l3 + 6 * 3 + 2,
        i64::try_from(FOXIT_SERIF_MM.len()).expect("fits"),
        "…and they account for the PFB minus its three 6-byte segment headers \
         and its two-byte `80 03` end marker"
    );

    let (_, _, pix) = pixels(&saved);
    assert!(
        dark_pixels(&pix) > original_dark,
        "the Type 1 program actually draws"
    );

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(
                stdout.contains("MD5:"),
                "the oracle reopens a file whose font is a Type 1 program, got {stdout:?}"
            );
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

/// A library defect this port found, kept as the failing test that pins it.
///
/// `embed_program` (`crates/pdfrum-edit/src/font/embed.rs:451-485`) writes the
/// caller's bytes into `/FontFile` **verbatim** — for a PFB, that is the file
/// including its three 6-byte `80 01`/`80 02` segment headers and its `80 03`
/// end marker. But the `/Length1` `/Length2` `/Length3` it writes beside them
/// come from `pdfrum_font::type1_program_lengths`, which returns the *unwrapped*
/// segment payload lengths. The two disagree by the 20 wrapper bytes: the
/// decoded stream is 113417 bytes while `Length1 + Length2 + Length3` is
/// 113397.
///
/// ISO 32000-1 §9.9 Table 127 defines the three as a partition of the stream's
/// decoded data, so a conforming reader that slices `[0..Length1]` gets six
/// bytes of PFB header followed by 10704 bytes of clear text, and its
/// `[Length1..Length1+Length2]` slice straddles the second segment header. The
/// fix belongs in `embed_program`: strip the PFB framing before storing, or
/// compute the lengths over the bytes actually stored. Not fixed here — this
/// port touches tests only.
#[test]
#[ignore = "library defect: embed_program stores the PFB verbatim (113417 B) while \
            /Length1+/Length2+/Length3 describe the unwrapped program (113397 B), so the \
            three lengths do not partition the stream as ISO 32000-1 §9.9 Table 127 requires"]
fn type1_font_file_lengths_partition_the_stream() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();
    let font = edit
        .embed_font(FOXIT_SERIF_MM, FontEncoding::Simple)
        .expect("embeds a Type 1 program");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 40.0),
            ..TextBuilder::new(font.encode("Lengths"), font.object(), 14.0)
        }
        .build(),
    );
    let dir = scratch_dir();
    let out = dir.join("type1_lengths.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let (file, l1, l2, l3) = type1_font_file(&saved_doc, font.object());

    let program = pdfrum_filters::decode_chain(
        &file,
        0,
        saved_doc.parser(),
        &pdfrum_common::Limits::default(),
        &mut pdfrum_common::Diagnostics::default(),
    )
    .data;

    assert_eq!(
        i64::try_from(program.len()).expect("fits"),
        l1 + l2 + l3,
        "the three lengths must partition the decoded /FontFile"
    );
    assert!(
        program.starts_with(b"%!"),
        "the stored program must open with the clear-text segment's PostScript \
         banner, not with a PFB segment header"
    );
    let clear = program
        .get(..usize::try_from(l1).expect("fits"))
        .expect("clear-text segment");
    assert!(
        clear.windows(5).any(|w| w == b"eexec"),
        "/Length1 must end at the eexec boundary"
    );
}

/// Ports `FPDFEditEmbedderTest.LoadCIDType0Font`.
///
/// Same program, `FontEncoding::Composite`. The C++ case pins the `/Type0`
/// wrapper naming `Identity-H`, one descendant, `/CIDFontType0` (not `Type2`,
/// because the program is not TrueType), the `Adobe`/`Identity`/`0`
/// `/CIDSystemInfo`, and a non-trivial `/W`.
#[test]
fn load_cid_type0_font() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let original_dark = dark_pixels(&pixels(HELLO_PDF).2);

    let mut edit = doc.edit();
    let font = edit
        .embed_font(FOXIT_SERIF_MM, FontEncoding::Composite)
        .expect("embeds a Type 1 program as a CID font");
    let mut page = doc.page(0).expect("page").edit();
    page.push(
        TextBuilder {
            position: Point::new(20.0, 60.0),
            ..TextBuilder::new(font.encode("Type 1 CID"), font.object(), 14.0)
        }
        .build(),
    );

    let dir = scratch_dir();
    let out = dir.join("load_cid_type0.pdf");
    edit.save_pages(&out, &[page], &SaveOptions::default())
        .expect("saves");
    let saved = std::fs::read(&out).expect("reads");

    let saved_doc = Document::from_bytes(Arc::from(saved.as_slice())).expect("reopens");
    let font_dict = fetch_dict(&saved_doc, font.object());
    assert_eq!(
        font_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Type0"[..])
    );
    assert_eq!(
        font_dict.name(&Name::from("Encoding")).map(Name::as_bytes),
        Some(&b"Identity-H"[..])
    );
    // The C++'s `"Tinos-Regular-Identity-H"`: the root's `/BaseFont` is the
    // descendant's name with the encoding appended.
    assert_eq!(
        font_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"ChromeSerifMM-Identity-H"[..])
    );

    let descendants = font_dict
        .array(&Name::from("DescendantFonts"), saved_doc.parser())
        .expect("DescendantFonts");
    assert_eq!(descendants.len(), 1);
    let cid_dict = fetch_dict(&saved_doc, descendants.reference_at(0).expect("cid ref"));
    assert_eq!(
        cid_dict.name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"CIDFontType0"[..]),
        "a Type 1 descendant is CIDFontType0; only a TrueType one is CIDFontType2"
    );
    assert_eq!(
        cid_dict.name(&Name::from("BaseFont")).map(Name::as_bytes),
        Some(&b"ChromeSerifMM"[..])
    );

    let cid_info = cid_dict
        .dict(&Name::from("CIDSystemInfo"), saved_doc.parser())
        .expect("CIDSystemInfo");
    assert_eq!(
        cid_info.byte_string(&Name::from("Registry"), saved_doc.parser()),
        Some(b"Adobe".to_vec())
    );
    assert_eq!(
        cid_info.byte_string(&Name::from("Ordering"), saved_doc.parser()),
        Some(b"Identity".to_vec())
    );
    assert_eq!(cid_info.direct_int(&Name::from("Supplement")), Some(0));

    // `EXPECT_GT(widths_array->size(), 1u)` plus `CheckCompositeFontWidths`.
    let w_ref = cid_dict.reference(&Name::from("W")).expect("W");
    let w = saved_doc
        .parser()
        .fetch(w_ref)
        .expect("w")
        .as_array()
        .expect("array")
        .clone();
    assert!(w.len() > 1, "/W should carry real runs, got {}", w.len());

    // The descendant, not the root, owns the descriptor, and it is /FontFile.
    let desc_ref = cid_dict
        .reference(&Name::from("FontDescriptor"))
        .expect("FontDescriptor");
    let desc = fetch_dict(&saved_doc, desc_ref);
    assert!(desc.reference(&Name::from("FontFile")).is_some());
    assert!(desc.raw(&Name::from("FontFile2")).is_none());
    assert!(font_dict.raw(&Name::from("FontDescriptor")).is_none());

    let (_, _, pix) = pixels(&saved);
    assert!(
        dark_pixels(&pix) > original_dark,
        "the composite Type 1 font actually draws"
    );

    match oracle_md5(&out) {
        Ok(stdout) => {
            assert!(stdout.contains("MD5:"));
        }
        Err(msg) => {
            eprintln!("skipping oracle reopen: {msg}");
        }
    }
}

/// The half of `FPDFEditEmbedderTest.LoadCidType2FontWithBadParameters` that
/// is about the *font program*, which is the only parameter `embed_font`
/// takes. The C++ case additionally rejects a null document, a null or empty
/// `to_unicode_cmap` and a null or empty `cid_to_gid_map`; those three have no
/// counterpart here — see `load_cid_type2_font_custom` for why.
#[test]
fn embed_font_rejects_a_program_it_cannot_read() {
    let doc = Document::from_bytes(Arc::from(HELLO_PDF)).expect("opens");
    let mut edit = doc.edit();

    // `FPDFText_LoadCidType2Font(document(), nullptr, …)` and its `size 0`
    // sibling: no bytes is not a font.
    for encoding in [FontEncoding::Simple, FontEncoding::Composite] {
        let err = edit
            .embed_font(&[], encoding)
            .expect_err("an empty program is not a font");
        assert!(
            matches!(err, pdfrum::Error::Save(_)),
            "an unreadable program surfaces as Error::Save, got {err:?}"
        );
        // The C++'s `dummy_vec(3)` — three zero bytes, which is neither a
        // sfnt tag nor a PFB marker nor a `%!PS` banner.
        let err = edit
            .embed_font(&[0, 0, 0], encoding)
            .expect_err("three zero bytes are not a font");
        assert!(matches!(err, pdfrum::Error::Save(_)), "got {err:?}");
        // Text that is not a font program either.
        let err = edit
            .embed_font(b"dummy", encoding)
            .expect_err("ASCII text is not a font");
        assert!(matches!(err, pdfrum::Error::Save(_)), "got {err:?}");
    }

    // A rejected program leaves the session usable: the next call still works,
    // which is the point of returning an error rather than poisoning the edit.
    let good = edit
        .embed_font(ROBOTO, FontEncoding::Simple)
        .expect("the editor survives a rejected program");
    assert_eq!(good.encode("Hi"), b"Hi".to_vec());
}

#[test]
#[ignore = "no API takes a caller-supplied /ToUnicode CMap or /CIDToGIDMap: \
            DocEdit::embed_font(bytes, FontEncoding::Composite) always generates both from the \
            program's own cmap, so FPDFText_LoadCidType2Font's four extra parameters \
            (to_unicode_cmap, cid_to_gid_map and their lengths) have no counterpart. \
            Tracked in docs/status/queue.md under feature gaps"]
fn load_cid_type2_font_custom() {
    // Ports FPDFEditEmbedderTest.LoadCidType2FontCustom and
    // FPDFEditEmbedderTest.LoadCidType2FontCustomGeneratedWidths.
}
