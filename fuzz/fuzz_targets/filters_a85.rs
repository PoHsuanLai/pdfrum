//! `decode_ascii85` — `/ASCII85Decode`.
//!
//! Property: never panics; the consumed count never runs past the input, and
//! base-85 never expands past four output bytes per five input ones (with
//! `z` — one input byte for four output — as the worst case).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((out, consumed)) = pdfrum_filters::decode_ascii85(data) {
        assert!(consumed <= data.len());
        // `z` is the densest spelling: one byte in, four out.
        assert!(out.len() <= data.len().saturating_mul(4));
    }
});
