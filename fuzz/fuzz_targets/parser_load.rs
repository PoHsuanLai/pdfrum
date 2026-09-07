//! `load` — a whole document.
//!
//! Property: never panics. Unopenable files are `LoadError`; a `Document`
//! that loaded answers its API without panicking.

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

    let _ = doc.version();
    let _ = doc.header_offset();
    let _ = doc.xref_was_rebuilt();
    let _ = doc.is_encrypted();
    let _ = doc.permissions();
    let _ = doc.owner_permissions();
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

    assert!(doc.page(doc.page_count()).is_err());
});
