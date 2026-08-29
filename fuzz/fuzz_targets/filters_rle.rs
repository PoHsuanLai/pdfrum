//! `decode_run_length` — `/RunLengthDecode`.
//!
//! Property: never panics; the consumed count stays inside the input and the
//! output stays under the filter's own 20 MiB cap, which is behaviourally
//! load-bearing (PDFium truncates there) rather than a fuzz convenience.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut diags = pdfrum_fuzz::diags();
    if let Ok((out, consumed)) = pdfrum_filters::decode_run_length(data, &mut diags) {
        assert!(consumed <= data.len());
        assert!(out.len() as u64 <= pdfrum_filters::RUN_LENGTH_MAX_OUTPUT);
    }
});
