//! `decode_ascii85` — `/ASCII85Decode`.
//!
//! Property: never panics; consumed stays inside the input; output ≤ 4×
//! (`z` is the densest spelling).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((out, consumed)) = pdfrum_filters::decode_ascii85(data) {
        assert!(consumed <= data.len());
        // `z` is the densest spelling: one byte in, four out.
        assert!(out.len() <= data.len().saturating_mul(4));
    }
});
