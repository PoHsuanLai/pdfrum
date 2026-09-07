//! `decode_flate` — `/FlateDecode`.
//!
//! Property: never panics; output never exceeds `max_decoded_stream_len`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    // A four-byte estimate: enough to reach both the "trust it" and the
    // "absurd" ends of the chunk-sizing arithmetic.
    let estimated_size = usize::from(split.byte())
        | (usize::from(split.byte()) << 8)
        | (usize::from(split.byte()) << 16)
        | (usize::from(split.byte()) << 24);
    let input = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    if let Ok(out) = pdfrum_filters::decode_flate(input, estimated_size, &limits, &mut diags) {
        assert!(out.len() <= limits.max_decoded_stream_len);
    }
});
