//! Inline images — `BI … ID … EI`. Input is also run with a `BI` prefix.
//!
//! Property: never panics; a declared size past the remaining bytes clamps.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::parse_content;

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();

    let _ = parse_content(data, &limits, &mut diags);

    let mut prefixed = Vec::with_capacity(data.len() + 3);
    prefixed.extend_from_slice(b"BI ");
    prefixed.extend_from_slice(data);
    let mut diags = pdfrum_fuzz::diags();
    let _ = parse_content(&prefixed, &limits, &mut diags);
});
