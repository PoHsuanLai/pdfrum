//! `load` — a whole document, from header search through xref, object
//! streams, incremental updates and the page tree.
//!
//! The M1 exit criterion is "100% of corpus+resources load without crash",
//! and this is the target that criterion is really about: it reaches every
//! other crate in the ring through the paths a real file takes. It is seeded
//! with the oracle's own `testing/resources` PDFs.
//!
//! Property: never panics. A file this reader cannot open must come back as
//! `LoadError`, and a `Document` that loaded must answer every question its
//! API exposes without panicking either.

#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfrum_object::NoResolve;
use pdfrum_parser::{LoadOptions, load};

fuzz_target!(|data: &[u8]| {
    let opts = LoadOptions {
        password: None,
        limits: pdfrum_fuzz::limits(),
    };

    let Ok(doc) = load(Arc::from(data), &opts) else {
        return;
    };

    // Everything the facade will ask a freshly loaded document.
    let _ = doc.version();
    let _ = doc.header_offset();
    let _ = doc.xref_was_rebuilt();
    let _ = doc.is_encrypted();
    let _ = doc.permissions(false);
    let _ = doc.permissions(true);
    let _ = doc.trailer_object_number();
    let _ = doc.catalog();

    // The page walk, which is where inherited attributes, `/Kids` cycles and
    // the depth cap live. Capped so one document cannot dominate the run.
    let pages = doc.page_count().min(64);
    for index in 0..pages {
        if let Ok(page) = doc.page(index) {
            for key in [
                pdfrum_object::names::RESOURCES,
                pdfrum_object::names::MEDIA_BOX,
                pdfrum_object::names::CROP_BOX,
                pdfrum_object::names::ROTATE,
            ] {
                let _ = page.inherited(key, &NoResolve);
            }
        }
    }

    // Reading past the last page must be an error, never a panic.
    assert!(doc.page(doc.page_count()).is_err());
});
