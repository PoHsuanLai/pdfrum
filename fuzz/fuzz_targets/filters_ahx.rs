//! `decode_ascii_hex` — `/ASCIIHexDecode`.
//!
//! Property: never panics, never grows, and stops at `>` — the consumed
//! count is where the next filter in the chain resumes, so it must stay
//! inside the input.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (out, consumed) = pdfrum_filters::decode_ascii_hex(data);
    assert!(consumed <= data.len());
    // Two hex digits per byte, and a trailing odd digit is padded to one.
    assert!(out.len() <= data.len().div_ceil(2));
});
