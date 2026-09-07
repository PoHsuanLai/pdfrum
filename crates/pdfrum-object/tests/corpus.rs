//! `clone_direct` over real files, which is where it first broke.
//!
//! The unit tests build the panicking shape by hand. This one loads it out of
//! a file that actually contains it: `embedded_images.pdf`'s page carries a
//! `/Resources` whose `/XObject` entries are indirect image streams, so
//! flattening it stores six streams as direct dictionary values. That tripped
//! a `Dict::push` assertion which claimed §7.3.8.1 as an in-memory invariant.
//! It is not one — it constrains what a *file* may say, and the writer is
//! what honours it. See the note on [`pdfrum_object::Dict`].
//!
//! The corpus lives outside the repository, so every test skips when it is
//! absent rather than failing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfrum_object::{Name, Object, names};
use pdfrum_parser::{Document, LoadOptions, load};

/// The read-only C++ PDFium checkout, resolved the one way every script and
/// test in this repository resolves it: `$PDFRUM_ORACLE_CHECKOUT`, else the
/// sibling `../pdfium-c++` directory README.md names.
///
/// Six lines rather than a shared module: forbids a `common`,
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

/// Open one file, or `None` when the corpus is absent or it will not load.
fn open(name: &str) -> Option<Document> {
    let bytes: Arc<[u8]> = Arc::from(std::fs::read(resources()?.join(name)).ok()?);
    load(bytes, &LoadOptions::default()).ok()
}

/// Every page dictionary in the document, in order.
fn page_dicts(doc: &Document) -> Vec<pdfrum_object::Dict> {
    (0..doc.page_count())
        .filter_map(|i| doc.page(i).ok().map(|p| p.dict))
        .collect()
}

/// The file whose page resources hold indirect `/XObject` streams — the
/// exact shape that panicked. Flattening must keep every one of them.
#[test]
fn cloning_a_resources_dictionary_with_indirect_xobject_streams() {
    let Some(doc) = open("embedded_images.pdf") else {
        return;
    };
    let pages = page_dicts(&doc);
    assert!(!pages.is_empty(), "the file has a page");

    let mut streams_seen = 0;
    for page in &pages {
        let Some(resources) = page.dict(names::RESOURCES, &doc) else {
            continue;
        };
        // The precondition: the file really does hold indirect streams here.
        let xobject_refs = resources.dict(&Name::from("XObject"), &doc).map_or(0, |x| {
            x.iter().filter(|(_, v)| v.as_ref_id().is_some()).count()
        });
        assert!(
            xobject_refs > 0,
            "embedded_images.pdf's /XObject entries are indirect"
        );

        // `clone_direct` must not panic on an indirect stream.
        let flattened = Object::Dict(resources).clone_direct(&doc);

        let xobject = flattened
            .as_dict()
            .and_then(|d| d.raw(&Name::from("XObject")))
            .and_then(Object::as_dict)
            .expect("the /XObject sub-dictionary survives flattening");
        // Each reference became the stream itself, stored directly — not
        // dropped, and not left as a reference.
        for (key, value) in xobject.iter() {
            let stream = value.as_stream().unwrap_or_else(|| {
                panic!(
                    "/{} flattened to {value:?}, not a stream",
                    key.as_str().unwrap_or("?")
                )
            });
            assert!(!stream.data.is_empty(), "the stream kept its raw bytes");
            streams_seen += 1;
        }
        assert_eq!(
            xobject.len(),
            xobject_refs,
            "no /XObject entry was lost to the flattening"
        );
    }
    assert!(streams_seen > 0, "at least one image stream was flattened");
}

/// The same operation over a broad sweep: flattening any page's resources
/// must terminate and never panic, whatever the file says.
#[test]
fn flattening_page_resources_never_panics() {
    let Some(dir) = resources() else {
        return;
    };
    let mut files = 0;
    let mut flattened = 0;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "pdf") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(doc) = load(Arc::from(bytes), &LoadOptions::default()) else {
            continue;
        };
        files += 1;
        for page in page_dicts(&doc) {
            // The whole page dictionary, not just its resources: this is the
            // deepest flattening a caller can ask for.
            let _ = Object::Dict(page).clone_direct(&doc);
            flattened += 1;
        }
    }
    assert!(files > 0, "the resource corpus was found and read");
    assert!(flattened > 0, "at least one page was flattened");
}
