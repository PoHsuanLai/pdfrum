//! `pdfa::check` — the PDF/A conformance walk over a whole document.
//!
//! Property: never panics, and always terminates. The checker recurses
//! through `/Kids` and through a form `XObject`'s own `/Resources`, neither
//! of which the parser bounds, because each level is a fresh fetch of an
//! already-parsed object. A stack overflow aborts the process rather than
//! unwinding, so the guards on those two walks are what this target exists
//! to exercise.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfrum_doc::pdfa::{Level, check};
use pdfrum_parser::{LoadOptions, load};

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();

    let opts = LoadOptions {
        password: None,
        limits: limits.clone(),
    };
    let Ok(doc) = load(Arc::from(data), &opts) else {
        return;
    };
    let Ok(catalog) = doc.catalog() else {
        return;
    };
    let trailer = doc.trailer();

    // Both levels: A1b forbids transparency, so it takes branches A2b never
    // reaches over the same resource dictionaries.
    for level in [Level::A1b, Level::A2b] {
        let mut diags = pdfrum_fuzz::diags();
        let report = check(level, &catalog, trailer, &doc, &limits, &mut diags);
        let _ = report.conforms();
        let _ = report.clauses();
    }
});
