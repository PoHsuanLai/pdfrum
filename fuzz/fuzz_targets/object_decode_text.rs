//! `decode_text` — PDF text-string bytes (UTF-16BE/LE with BOM, UTF-8 with
//! BOM, else `PDFDocEncoding`) to a Rust string.
//!
//! Property: never panics, and every returned `char` is valid by
//! construction. The interesting inputs are truncated surrogate pairs, odd
//! byte counts after a BOM, and language-code escapes.

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
