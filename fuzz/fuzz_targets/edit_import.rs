//! `import_pages` and `n_page_to_one` — copying between two documents.
//!
//! Property: never panics, and the destination stays a document.
//!
//! The copier is the one place in the crate that walks an *untrusted* object
//! graph while building a new one, so it is where a cycle, a dangling
//! reference or a self-referential page tree turns into either a hang or a
//! malformed output. Both source and destination come from the same input
//! here — the second half of the bytes, so the two documents differ — which
//! makes a source that references the destination's numbering reachable.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfrum_edit::{
    EditDoc, IdSource, ImportOptions, NUpOptions, PageRange, SaveMode, SaveOptions, import_pages,
    n_page_to_one, save,
};
use pdfrum_parser::{LoadOptions, load};

/// How many pages an import may ask for. A fuzzer asking for thousands only
/// measures allocation.
const MAX_PAGES: u32 = 32;

fuzz_target!(|data: &[u8]| {
    let opts = LoadOptions {
        password: None,
        limits: pdfrum_fuzz::limits(),
    };

    // Split the input so the two documents are not the same bytes: a source
    // that shares the destination's object numbering is the case where a
    // remap bug would show up as a silently wrong reference.
    let split = data.len() / 2;
    let (a, b) = data.split_at(split);
    let (Ok(src), Ok(dest)) = (
        load(Arc::from(a), &opts),
        load(Arc::from(b.is_empty().then_some(a).unwrap_or(b)), &opts),
    ) else {
        return;
    };

    let pages = PageRange::all(src.page_count().min(MAX_PAGES));
    let saved = |edit: &EditDoc<'_>| {
        let options = SaveOptions {
            mode: SaveMode::Full,
            remove_security: true,
            id_source: IdSource::Fixed([0x7E; 16]),
            ..SaveOptions::default()
        };
        let mut out = Vec::new();
        save(edit, &options, &mut out).ok().map(|()| out)
    };

    // ---- importing pages as pages ----
    let mut edit = EditDoc::new(&dest);
    if import_pages(&mut edit, &src, &pages, &ImportOptions::default()).is_ok()
        && let Some(out) = saved(&edit)
    {
        let reloaded = load(Arc::from(&out[..]), &opts)
            .unwrap_or_else(|err| panic!("an imported document did not reopen: {err:?}"));
        // Nothing is lost: the destination keeps its own pages and gains the
        // source's.
        assert!(
            reloaded.page_count() >= dest.page_count(),
            "importing must not remove a page"
        );
        for i in 0..reloaded.page_count().min(64) {
            let _ = reloaded.page(i);
        }
    }

    // ---- a failed import must leave the destination alone ----
    let mut edit = EditDoc::new(&dest);
    let past_the_end = PageRange::of([src.page_count().saturating_add(1)]);
    if import_pages(&mut edit, &src, &past_the_end, &ImportOptions::default()).is_err()
        && let Some(out) = saved(&edit)
        && let Ok(reloaded) = load(Arc::from(&out[..]), &opts)
    {
        assert_eq!(
            reloaded.page_count(),
            dest.page_count(),
            "a failed import changed the destination"
        );
    }

    // ---- N-up ----
    let mut edit = EditDoc::new(&dest);
    let grid = NUpOptions {
        sheet: (612.0, 792.0),
        grid: (2, 2),
    };
    if n_page_to_one(&mut edit, &src, &pages, &grid).is_ok()
        && let Some(out) = saved(&edit)
    {
        let _ = load(Arc::from(&out[..]), &opts);
    }
});
