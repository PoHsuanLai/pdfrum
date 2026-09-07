//! `decode_lzw` — `/LZWDecode`, both `/EarlyChange` settings.
//!
//! Property: never panics; output never exceeds `max_decoded_stream_len`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let early_change = split.byte() & 1 == 1;
    let input = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    if let Ok(out) = pdfrum_filters::decode_lzw(input, early_change, &limits, &mut diags) {
        assert!(out.len() <= limits.max_decoded_stream_len);
    }
});
