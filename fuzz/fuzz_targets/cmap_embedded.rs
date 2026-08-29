//! `parse_embedded` — a CMap program from a font's `/Encoding` stream, then
//! decoding text through whatever came out.
//!
//! One target covers both what the status doc calls `cmap_embedded` and
//! `cmap_lexer`: the lexer has no public entry point separate from the
//! parser that consumes it, and running the resulting CMap over the same
//! bytes checks the second property the doc names — that decoding terminates
//! and every code stays inside the input.
//!
//! Property: parsing never panics, decoding always terminates (each step
//! advances), and the offsets a decode reports never run past the input.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let text = split.take();
    let program = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    let cmap = pdfrum_cmap::parse_embedded(program, &limits, &mut diags);

    // Termination: `next_char` must move the offset forward every call, or
    // the document-side loop over a string spins forever.
    let mut offset = 0usize;
    while offset < text.len() {
        let before = offset;
        let code = cmap.next_char(text, &mut offset);
        assert!(offset > before, "next_char did not advance at {before}");
        assert!(offset <= text.len(), "next_char ran past the input");
        // The round trip the crate documents as total for a decoded code.
        let cid = cmap.cid(code);
        let _ = cmap.charcode_from_cid(cid);
        let _ = cmap.char_size(code);
    }

    // The iterator form must agree with the manual walk on how many codes
    // the string holds.
    assert_eq!(cmap.decode(text).count(), cmap.count_chars(text));
});
