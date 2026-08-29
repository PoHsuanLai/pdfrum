//! Inline images — `BI … ID … EI`, the quirkiest corner of content parsing.
//!
//! Seeded with a `BI` prefix so the fuzzer spends its budget inside the
//! dictionary scan, the abbreviation expansion, the length inference and the
//! `EI` resync rather than rediscovering the two bytes that reach them.
//!
//! Property: never panics and always terminates. The resync loop absorbs
//! bytes until a standalone `EI` token or the end of the data, and an
//! inline image whose declared size vastly exceeds what is there must clamp
//! rather than allocate.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::parse_content;

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();

    // The bare input, in case it already contains a `BI`.
    let _ = parse_content(data, &limits, &mut diags);

    // …and the input forced into the inline-image path.
    let mut prefixed = Vec::with_capacity(data.len() + 3);
    prefixed.extend_from_slice(b"BI ");
    prefixed.extend_from_slice(data);
    let mut diags = pdfrum_fuzz::diags();
    let _ = parse_content(&prefixed, &limits, &mut diags);
});
