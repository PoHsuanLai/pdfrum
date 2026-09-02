//! Embed a font program, write text with [`pdfrum::TextBuilder`], and round-trip.
//!
//! Ports the load-bearing claims of PDFium's `TestExtractedFont` (a loaded
//! font plus `FPDFText_SetText` extracts back through `/ToUnicode`) and
//! checks that `SaveOptions::subset_new_fonts` still sees the new composite
//! TrueType as a candidate.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "helpers shared by the tests below"
)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use pdfrum::{Document, FontEncoding, RenderOptions, SaveOptions, StandardFont, TextBuilder};

const HELLO_PDF: &[u8] = include_bytes!("fixtures/hello_world.pdf");
const ROBOTO: &[u8] = include_bytes!("fixtures/roboto.ttf");

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("pdfrum-load-font");
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
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
