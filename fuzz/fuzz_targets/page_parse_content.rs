//! `parse_content` — content-stream tokenizer. Infallible: bad operators
//! become diagnostics.
//!
//! Property: never panics; a second parse of the same bytes agrees.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::parse_content;

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();

    let mut diags = pdfrum_fuzz::diags();
    let first = parse_content(data, &limits, &mut diags);

    let mut diags = pdfrum_fuzz::diags();
    let second = parse_content(data, &limits, &mut diags);
    assert_eq!(first, second, "parse_content must be a pure function");
});
