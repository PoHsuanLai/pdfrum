//! Integration: decode real corpus PDFs and check the Unicode against the
//! oracle's own text extraction.
//!
//! This crate cannot produce `--txt` output on its own — that needs
//! `pdfrum-page` to interpret content streams and `pdfrum-text` to order the
//! result — so these tests isolate the part that *is* ours: for a page whose
//! golden text is known, every character the oracle extracted must be
//! reachable through some font on that page. A mapping bug shows up as a
//! character the fonts cannot produce, which is exactly the failure mode
//! `Font::decode` is responsible for.
//!
//! The corpus lives in the read-only oracle checkout, so these tests skip
//! themselves when it is absent rather than failing.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::{CharCode, Font, FontCache, load};
use pdfrum_object::{Dict, Name};
use pdfrum_parser::{Document, LoadOptions};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The read-only C++ PDFium checkout, resolved the one way every script and
/// test in this repository resolves it: `$PDFRUM_ORACLE_CHECKOUT`, else the
/// sibling `../pdfium-c++` directory README.md and PLAN.md §4 name.
/// `scripts/env.nu` holds the nushell spelling of the same rule.
///
/// Six lines rather than a shared module: STYLE.md §4 forbids a `common`,
/// `util` or `helpers` module name, and an integration test in one crate
/// cannot reach another crate's test code anyway.
fn oracle_checkout() -> PathBuf {
    let checkout = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
        PathBuf::from,
    );
    if !checkout.is_dir() {
        // Said once per test process, so a run where every case below did
        // nothing says so rather than reporting a silent green.
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            eprintln!(
                "skipping the oracle-corpus cases: no checkout at {} \
                 (set PDFRUM_ORACLE_CHECKOUT)",
                checkout.display()
            );
        });
    }
    checkout
}

/// The read-only oracle checkout, when it is present.
fn oracle_root() -> Option<PathBuf> {
    let p = oracle_checkout();
    p.is_dir().then_some(p)
}

/// The golden store.
fn goldens_root() -> PathBuf {
    std::env::var_os("PDFRUM_GOLDENS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/goldens"),
        PathBuf::from,
    )
}

/// Whether the golden store has been generated here.
///
/// `conformance/goldens/` is 68 MB of the oracle's own output and is
/// `.gitignore`d, so it exists only where `conformance generate-goldens` has
/// been run — the primary checkout, and not a fresh worktree. These tests skip
/// on its absence for the same reason they skip on the oracle's: an
/// environment that cannot answer the question is not a failing answer. The
/// distinction that matters is *absent* versus *present but resolving to
/// nothing*; the second is a real defect and still asserts below.
fn goldens_generated() -> bool {
    goldens_root().is_dir()
}

/// What to do about it, printed where a reader will meet it.
const HOW_TO_GENERATE: &str = "skipping: conformance/goldens/ is absent \
    (it is .gitignore'd; generate it with `cargo run -p conformance --release \
    -- generate-goldens`)";

/// One corpus file with a known text extraction.
struct Golden {
    /// The PDF, resolved against the oracle checkout.
    pdf: PathBuf,
    /// The oracle's `--txt` output for page 0.
    text: String,
    /// The manifest's source path, for failure messages.
    source: String,
}

/// Load the goldens named below, skipping any whose source PDF is missing —
/// `.in` templates need a fixup step this crate does not run.
fn goldens(keys: &[&str]) -> Vec<Golden> {
    let Some(oracle) = oracle_root() else {
        return Vec::new();
    };
    let root = goldens_root();
    let mut out = Vec::new();
    for key in keys {
        let dir = root.join(key);
        let Ok(manifest) = std::fs::read_to_string(dir.join("manifest.json")) else {
            continue;
        };
        // A one-line extraction rather than a JSON dependency: after the key,
        // the first quoted run is the first source path.
        let Some(source) = manifest
            .split("\"sources\"")
            .nth(1)
            .and_then(|s| s.split('"').nth(1))
            .map(str::to_owned)
        else {
            continue;
        };
        let pdf = oracle.join("testing").join(&source);
        if !pdf.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read(dir.join("input.pdf.0.txt")) else {
            continue;
        };
        let Ok(text) = String::from_utf8(text) else {
            continue;
        };
        out.push(Golden { pdf, text, source });
    }
    out
}

/// Every font resource reachable from a page dictionary.
fn page_fonts(doc: &Document, page: &Dict, cache: &FontCache) -> Vec<Font> {
    let resources_key = Name::from("Resources");
    let font_key = Name::from("Font");
    let Some(resources) = page.dict(&resources_key, doc) else {
        return Vec::new();
    };
    let Some(fonts) = resources.dict(&font_key, doc) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut diags = Diagnostics::default();
    for (name, _) in fonts.iter() {
        let Some(dict) = fonts.dict(name, doc) else {
            continue;
        };
        if let Some(font) = load(&dict, doc, cache, &Limits::default(), &mut diags) {
            out.push(font);
        }
    }
    out
}

/// Every character the page's fonts can produce, over every one-byte code and
/// — for a composite font — every two-byte one.
fn producible(fonts: &[Font]) -> HashSet<char> {
    let mut chars = HashSet::new();
    for font in fonts {
        let codes: Box<dyn Iterator<Item = u32>> = match font {
            Font::Type0(_) => Box::new(0..=0xffff),
            Font::Simple(_) | Font::Type3(_) => Box::new(0..=0xff),
        };
        for code in codes {
            for c in font.unicode_from_charcode(CharCode(code)) {
                chars.insert(c);
            }
        }
    }
    chars
}

/// A handful of goldens with plain Latin text, chosen so a mapping failure is
/// unambiguous rather than a whitespace-heuristic difference.
const LATIN_GOLDENS: [&str; 8] = [
    "006a87e2f58eb8b4", // annotation_circle.pdf
    "0a3ce2c4528ff5b2", // annotation_polygon.pdf
    "0a4d33143d2394b8", // annotation_square.pdf
    "096a7bb580bb31a3", // annotation_circle_hidden.pdf
    "039217935074883e", // text_form_negative_fontsize.pdf
    "0712aaffc7c5cc3a", // path_10_jd.pdf
    "0e3edbde51d102a7", // tcpdf example_025.pdf
    "00500c3f2d28aee1", // FRC_3.5_CF_Strf_stmf_StdCF.pdf
];

#[test]
fn every_extracted_character_is_reachable_through_some_page_font() {
    let Some(_) = oracle_root() else {
        eprintln!("skipping: the oracle checkout is absent");
        return;
    };
    if !goldens_generated() {
        eprintln!("{HOW_TO_GENERATE}");
        return;
    }
    let goldens = goldens(&LATIN_GOLDENS);
    // With the checkout present the goldens must resolve; a silent skip here
    // would let a mapping regression through unnoticed.
    assert!(
        goldens.len() >= LATIN_GOLDENS.len() - 1,
        "only {} of {} goldens resolved to a corpus file",
        goldens.len(),
        LATIN_GOLDENS.len()
    );
    let mut checked = 0usize;

    for golden in &goldens {
        let Ok(bytes) = std::fs::read(&golden.pdf) else {
            continue;
        };
        let Ok(doc) = pdfrum_parser::load(Arc::from(bytes.as_slice()), &LoadOptions::default())
        else {
            // An encrypted or badly damaged file is the parser's problem, not
            // this crate's.
            continue;
        };
        let Ok(page) = doc.page(0) else { continue };
        let cache = FontCache::new();
        let fonts = page_fonts(&doc, &page.dict, &cache);
        if fonts.is_empty() {
            continue;
        }
        let reachable = producible(&fonts);

        // Whitespace and line breaks are inserted by the *text* layer from
        // geometry, not produced by any font, so they are not this crate's to
        // account for.
        let missing: Vec<char> = golden
            .text
            .chars()
            .filter(|c| !c.is_whitespace() && !reachable.contains(c))
            .collect();
        assert!(
            missing.is_empty(),
            "{}: {} of {} characters unreachable through {} font(s): {missing:?}",
            golden.source,
            missing.len(),
            golden.text.chars().count(),
            fonts.len(),
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "no golden could be checked; the corpus paths may have moved"
    );
    eprintln!("checked {checked} of {} goldens", goldens.len());
}

#[test]
fn decoding_a_corpus_page_never_panics_and_widths_are_finite() {
    let Some(_) = oracle_root() else {
        eprintln!("skipping: the oracle checkout is absent");
        return;
    };
    if !goldens_generated() {
        eprintln!("{HOW_TO_GENERATE}");
        return;
    }
    let goldens = goldens(&LATIN_GOLDENS);
    assert!(!goldens.is_empty(), "no golden resolved to a corpus file");
    for golden in &goldens {
        let Ok(bytes) = std::fs::read(&golden.pdf) else {
            continue;
        };
        let Ok(doc) = pdfrum_parser::load(Arc::from(bytes.as_slice()), &LoadOptions::default())
        else {
            continue;
        };
        for index in 0..doc.page_count().min(4) {
            let Ok(page) = doc.page(index) else { continue };
            let cache = FontCache::new();
            for font in page_fonts(&doc, &page.dict, &cache) {
                // Drive the decoder over a spread of byte patterns, including
                // sequences that are not valid codes under any CMap.
                for probe in [
                    &b"Hello, world!"[..],
                    &[0x00, 0x01, 0x02, 0x03],
                    &[0xff, 0xfe, 0xfd],
                    &[0x80],
                    &[],
                ] {
                    for item in font.decode(probe) {
                        assert!(
                            item.width.is_finite(),
                            "{}: a non-finite width for code {:#x}",
                            golden.source,
                            item.code.0
                        );
                        if let Some(gid) = item.gid {
                            let _ = font.glyph_path(gid);
                        }
                    }
                }
                let _ = font.font_bbox();
                let _ = font.ascent();
                let _ = font.descent();
                let _ = font.is_vertical();
                let _ = font.is_unicode_compatible();
            }
        }
    }
}

#[test]
fn every_corpus_font_loads_without_panicking() {
    // A far wider sweep than the goldens above: every font dictionary in a
    // few hundred corpus files, loaded and driven. This is the closest thing
    // to a fuzz run available before the `fuzz/` workspace exists.
    let Some(oracle) = oracle_root() else {
        eprintln!("skipping: the oracle checkout is absent");
        return;
    };
    let corpus = oracle.join("testing/resources");
    let Ok(entries) = std::fs::read_dir(&corpus) else {
        eprintln!("skipping: {} is not readable", corpus.display());
        return;
    };

    let mut files = 0usize;
    let mut fonts_loaded = 0usize;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();

    for path in paths.iter().take(200) {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(doc) = pdfrum_parser::load(Arc::from(bytes.as_slice()), &LoadOptions::default())
        else {
            continue;
        };
        files += 1;
        let cache = FontCache::new();
        for index in 0..doc.page_count().min(3) {
            let Ok(page) = doc.page(index) else { continue };
            for font in page_fonts(&doc, &page.dict, &cache) {
                fonts_loaded += 1;
                let _: Vec<_> = font.decode(b"\x00\x41\xff\x20").collect();
                let _ = font.char_width(CharCode(65));
                let _ = font.base_font_name().to_vec();
            }
        }
    }

    assert!(
        files > 20,
        "expected to read many corpus files, got {files}"
    );
    eprintln!("loaded {fonts_loaded} fonts across {files} files");
}

/// Every font in the corpus that declares a `/ToUnicode` must actually answer
/// through it.
///
/// The strongest structural claim this crate can make on its own: a font whose
/// dictionary carries a Unicode map and which reports itself Unicode-compatible
/// must map *something*, because a map that parsed to nothing is exactly what a
/// count-check bug or a lexer bug produces — and it would then be invisible
/// until text extraction lands.
#[test]
fn every_unicode_compatible_corpus_font_maps_at_least_one_code() {
    let Some(oracle) = oracle_root() else {
        eprintln!("skipping: the oracle checkout is absent");
        return;
    };
    let corpus = oracle.join("testing/corpus/pdfium");
    let Ok(entries) = std::fs::read_dir(&corpus) else {
        eprintln!("skipping: {} is not readable", corpus.display());
        return;
    };

    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "pdf") {
            paths.push(path);
        } else if let Ok(inner) = std::fs::read_dir(&path) {
            // One level down only — the corpus groups files by feature, and a
            // deeper walk would take far longer than the sweep is worth.
            paths.extend(
                inner
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "pdf")),
            );
        }
    }
    paths.sort();

    let mut compatible = 0usize;
    for path in paths.iter().take(150) {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(doc) = pdfrum_parser::load(Arc::from(bytes.as_slice()), &LoadOptions::default())
        else {
            continue;
        };
        let cache = FontCache::new();
        for index in 0..doc.page_count().min(2) {
            let Ok(page) = doc.page(index) else { continue };
            for font in page_fonts(&doc, &page.dict, &cache) {
                if !font.is_unicode_compatible() {
                    continue;
                }
                let mapped = (0..=0xffu32)
                    .filter(|c| !font.unicode_from_charcode(CharCode(*c)).is_empty())
                    .count();
                assert!(
                    mapped > 0,
                    "{}: a Unicode-compatible font mapped no code at all",
                    path.display()
                );
                compatible += 1;
            }
        }
    }
    assert!(
        compatible > 10,
        "expected many Unicode-compatible fonts, found {compatible}"
    );
    eprintln!("checked {compatible} Unicode-compatible fonts");
}
