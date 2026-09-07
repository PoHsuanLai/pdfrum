//! `decode_ascii_hex` — `/ASCIIHexDecode`.
//!
//! Property: never panics, never grows; consumed (the next filter's start)
//! stays inside the input.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (out, consumed) = pdfrum_filters::decode_ascii_hex(data);
    assert!(consumed <= data.len());
    // Two hex digits per byte, and a trailing odd digit is padded to one.
    assert!(out.len() <= data.len().div_ceil(2));
});
