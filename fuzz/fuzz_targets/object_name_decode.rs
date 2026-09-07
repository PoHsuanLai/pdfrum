//! `name_decode` / `name_encode`.
//!
//! Property: never panics, never grows; re-encoding a decoded name round-trips.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let decoded = pdfrum_object::name_decode(data);
    // Every `#xx` triple collapses to one byte; nothing else is consumed.
    assert!(decoded.len() <= data.len());

    let reencoded = pdfrum_object::name_encode(&decoded);
    assert_eq!(pdfrum_object::name_decode(&reencoded), decoded);
});
