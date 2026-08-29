//! `parse_content` — the content-stream tokenizer and operand ring over
//! arbitrary bytes.
//!
//! The entry point is **infallible by contract** (SPEC.md §7): every bad
//! operator becomes a diagnostic and is skipped. So there is no error to
//! check — the property is simply that it returns, and that the operator
//! list it returns is stable under a second parse of the same bytes.
//!
//! Property: never panics, always terminates, and is a pure function of the
//! input. The two sub-loops that bypass the operand ring — the `m`-triggered
//! path run and the `BI` inline-image scan — are the ones that could spin,
//! and both rewind rather than advancing on anything they do not handle.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::parse_content;

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();

    let mut diags = pdfrum_fuzz::diags();
    let first = parse_content(data, &limits, &mut diags);

    // Purity: the same bytes must give the same operators, whatever state a
    // previous parse left behind — there is none, and this proves it.
    let mut diags = pdfrum_fuzz::diags();
    let second = parse_content(data, &limits, &mut diags);
    assert_eq!(first, second, "parse_content must be a pure function");
});
