//! `decode_text` — PDF text-string bytes to a Rust string.
//!
//! Property: never panics; every returned `char` is valid.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = pdfrum_object::decode_text(data);
    // Force the result to be walked so a lazily-built `Cow` cannot be
    // optimised away, and assert the one invariant the signature promises.
    let mut chars = 0usize;
    for _ in text.chars() {
        chars = chars.wrapping_add(1);
    }
    assert!(chars <= text.len());
});
