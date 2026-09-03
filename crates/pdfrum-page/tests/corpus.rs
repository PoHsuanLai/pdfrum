//! Interpreting real content streams end to end.
//!
//! The unit tests each pin one decision on a stream written to provoke it.
//! These run the whole pipeline — `/Contents` bytes through
//! [`parse_content`] and [`build_page`] — over files that exist, and check
//! the page-object graph against what the file plainly contains. They catch
//! the failure the unit tests structurally cannot: a rule that is right in
//! isolation and wrong in composition.
//!
//! The corpus lives outside the repository, so every test skips when it is
//! absent rather than failing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Object, names};
use pdfrum_page::{BuildContext, Op, PageObject, Resources, build_page_from_dict, parse_content};
use pdfrum_parser::{Document, LoadOptions, load};

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

/// Where the oracle's test files live, when this checkout has them.
fn resources() -> Option<PathBuf> {
    let path = oracle_checkout().join("testing/resources");
    path.is_dir().then_some(path)
}

/// The corpus of larger real-world files.
fn corpus() -> Option<PathBuf> {
    let path = oracle_checkout().join("testing/corpus/pdfium");
    path.is_dir().then_some(path)
}

/// Open one file, or `None` when the corpus is absent or it will not load.
fn open(dir: &Path, name: &str) -> Option<Document> {
    let bytes: Arc<[u8]> = Arc::from(std::fs::read(dir.join(name)).ok()?);
    load(bytes, &LoadOptions::default()).ok()
}

/// A page's content bytes, concatenating a `/Contents` array with the **one
/// space** between elements that lets an operator straddle two streams.
fn content_bytes(doc: &Document, dict: &pdfrum_object::Dict, diags: &mut Diagnostics) -> Vec<u8> {
    let limits = Limits::default();
    let Some(contents) = dict.get(names::CONTENTS, doc) else {
        // A page with no `/Contents` parses successfully with zero objects.
        return Vec::new();
    };
    match &*contents {
        Object::Stream(stream) => pdfrum_parser::decoded_stream(stream, doc, &limits, diags),
        Object::Array(array) => {
            let mut out = Vec::new();
            for element in array.iter() {
                // A non-stream element contributes zero bytes but still
                // occupies a stream index.
                if let Some(stream) = element
                    .resolve(doc)
                    .ok()
                    .and_then(|o| o.as_stream().cloned())
                {
                    out.extend_from_slice(&pdfrum_parser::decoded_stream(
                        &stream, doc, &limits, diags,
                    ));
                }
                out.push(b' ');
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Build page `index` of `name`, or `None` when the file is absent.
fn build(dir: &Path, name: &str, index: u32) -> Option<(pdfrum_page::Page, Diagnostics)> {
    let doc = open(dir, name)?;
    let page = doc.page(index).ok()?;
    let mut diags = Diagnostics::default();
    let limits = Limits::default();
    let bytes = content_bytes(&doc, &page.dict, &mut diags);
    let ops = parse_content(&bytes, &limits, &mut diags);

    let page_resources = page
        .inherited(names::RESOURCES, &doc)
        .and_then(|o| o.resolve(&doc).ok().and_then(|res| res.as_dict().cloned()));
    let resources = Resources::for_page(page_resources);
    let mut ctx = BuildContext::new();
    let built = build_page_from_dict(
        &ops,
        &page.dict,
        |key| page.inherited(key, &doc),
        &resources,
        &doc,
        &mut ctx,
        &limits,
        &mut diags,
    );
    Some((built, diags))
}

/// How many objects of each kind a page produced.
#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    paths: usize,
    texts: usize,
    images: usize,
    shadings: usize,
    forms: usize,
}

fn count(page: &pdfrum_page::Page) -> Counts {
    let mut counts = Counts::default();
    for object in &page.objects {
        match object {
            PageObject::Path(_) => counts.paths += 1,
            PageObject::Text(_) => counts.texts += 1,
            PageObject::Image(_) => counts.images += 1,
            PageObject::Shading(_) => counts.shadings += 1,
            PageObject::Form(_) => counts.forms += 1,
        }
    }
    counts
}

#[test]
fn a_plain_text_page_produces_text_objects_and_letter_geometry() {
    let Some(dir) = resources() else { return };
    let Some((page, _)) = build(&dir, "hello_world.pdf", 0) else {
        return;
    };
    let counts = count(&page);
    assert!(
        counts.texts >= 2,
        "hello_world.pdf shows two strings, got {counts:?}"
    );
    assert_eq!(counts.images, 0);
    // The file states a 200-point square, so the derivation must **not**
    // reach for the Letter default.
    assert!(
        (page.media_box.width() - 200.0).abs() < 1.0,
        "got {:?}",
        page.media_box
    );
    assert!((page.media_box.height() - 200.0).abs() < 1.0);
    // With no `/CropBox` the crop box is the media box.
    assert_eq!(page.media_box, page.crop_box);
}

#[test]
fn a_page_of_rectangles_produces_path_objects() {
    let Some(dir) = resources() else { return };
    let Some((page, _)) = build(&dir, "rectangles.pdf", 0) else {
        return;
    };
    let counts = count(&page);
    assert!(
        counts.paths >= 4,
        "rectangles.pdf draws several rectangles, got {counts:?}"
    );
    assert_eq!(counts.texts, 0);
}

#[test]
fn an_image_page_produces_an_image_object() {
    let Some(dir) = resources() else { return };
    for name in ["bug_1258634.pdf", "embedded_images.pdf"] {
        let Some((page, _)) = build(&dir, name, 0) else {
            continue;
        };
        let counts = count(&page);
        assert!(
            counts.images > 0 || counts.forms > 0,
            "{name} should paint an image or a form, got {counts:?}"
        );
    }
}

#[test]
fn every_resource_file_builds_without_panicking() {
    let Some(dir) = resources() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut built = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(doc) = open(&dir, name) else {
            continue;
        };
        // The first three pages of each file is plenty of coverage without
        // making the suite slow.
        for index in 0..doc.page_count().min(3) {
            if build(&dir, name, index).is_some() {
                built += 1;
            }
        }
    }
    assert!(
        built > 50,
        "expected to build many pages from the resource corpus, built {built}"
    );
}

#[test]
fn every_corpus_file_builds_without_panicking() {
    let Some(dir) = corpus() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut built = 0usize;
    let mut files = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // A slice is plenty of coverage without making the suite slow; the
        // fuzz ring is what runs the whole corpus.
        files += 1;
        if files > 120 {
            break;
        }
        let Some(doc) = open(&dir, name) else {
            continue;
        };
        for index in 0..doc.page_count().min(2) {
            if build(&dir, name, index).is_some() {
                built += 1;
            }
        }
    }
    assert!(
        built > 10,
        "expected to build pages from the corpus, built {built} from {files} files"
    );
}

#[test]
fn a_page_with_no_contents_builds_successfully_with_no_objects() {
    let Some(dir) = resources() else { return };
    // Whatever file this is, the rule is the same: no content is not an
    // error. Synthesize the case rather than hunting for a file with it.
    let Some(doc) = open(&dir, "hello_world.pdf") else {
        return;
    };
    let Ok(page) = doc.page(0) else { return };
    let mut diags = Diagnostics::default();
    let limits = Limits::default();
    let ops = parse_content(b"", &limits, &mut diags);
    assert!(ops.is_empty());
    let mut ctx = BuildContext::new();
    let built = build_page_from_dict(
        &ops,
        &page.dict,
        |key| page.inherited(key, &doc),
        &Resources::default(),
        &doc,
        &mut ctx,
        &limits,
        &mut diags,
    );
    assert!(built.objects.is_empty());
}

#[test]
fn parsing_a_stream_twice_gives_the_same_operators() {
    let Some(dir) = resources() else { return };
    let Some(doc) = open(&dir, "hello_world.pdf") else {
        return;
    };
    let Ok(page) = doc.page(0) else { return };
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let bytes = content_bytes(&doc, &page.dict, &mut diags);

    let first: Vec<Op> = parse_content(&bytes, &limits, &mut Diagnostics::default());
    let second: Vec<Op> = parse_content(&bytes, &limits, &mut Diagnostics::default());
    assert_eq!(
        first, second,
        "parsing must be a pure function of the bytes"
    );
}

#[test]
fn the_operators_a_real_page_uses_are_all_recognised() {
    let Some(dir) = resources() else { return };
    let Some(doc) = open(&dir, "hello_world.pdf") else {
        return;
    };
    let Ok(page) = doc.page(0) else { return };
    let mut diags = Diagnostics::default();
    let bytes = content_bytes(&doc, &page.dict, &mut diags);
    let ops = parse_content(&bytes, &Limits::default(), &mut diags);
    let unknown: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            Op::Unknown(word) => Some(String::from_utf8_lossy(word).into_owned()),
            _ => None,
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "a well-formed page should use no unrecognised operators, got {unknown:?}"
    );
}
