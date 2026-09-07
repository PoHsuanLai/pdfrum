//! `decode_jbig2` — arbitrary bytes, not via the image ladder.
//!
//! Property: never panics. The requested bitmap is sized from the caller's
//! dimensions and must be bounded before the codec runs.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::decode_jbig2;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let w = u32::from(split.byte()) * 4;
    let h = u32::from(split.byte()) * 4;
    let globals = split.take();
    let body = split.rest();

    let limits = pdfrum_fuzz::limits();
    let _ = decode_jbig2(None, body, w, h, &limits);
    let _ = decode_jbig2(Some(globals), body, w, h, &limits);
});
