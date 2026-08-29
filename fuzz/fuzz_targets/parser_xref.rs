//! `read_xref` — cross-reference tables, cross-reference streams, the
//! `/Prev` chain across incremental updates, and the full-file rebuild that
//! takes over when none of that works.
//!
//! This is the damage-tolerance entry point: almost every input is a broken
//! file, and almost every input must still come back with *something*. The
//! rebuild scanner in particular walks the whole input looking for `obj`
//! keywords, so it sees every byte the fuzzer can produce.
//!
//! Property: never panics, and every entry the table reports names an object
//! number the table itself calls legal.
//!
//! Two things are deliberately *not* asserted, because in both cases the
//! looser behaviour is the damage tolerance rather than a bug:
//!
//! - that an `Entry::Offset` points inside the file. A classic table stores
//!   the offset the file wrote, however wrong; `ObjectStore` is where a bad
//!   one stops mattering, and `parser_load` is the target that covers that.
//! - that the archive of an `InObjStream` entry is still flagged an object
//!   stream. A later incremental section may free that object number, and
//!   freeing drops the flag — so the two can disagree in a file that says
//!   contradictory things about the same object.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_parser::{Entry, read_xref};

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();

    let Ok((xref, _trailer)) = read_xref(data, &limits, &mut diags) else {
        return;
    };

    // Walking the table is what every caller does next; the cap keeps a
    // pathological rebuild from dominating the run without hiding a bug,
    // since a bad entry anywhere is reachable with a shorter input too.
    for num in xref.object_numbers().take(4096) {
        assert!(
            xref.is_valid_object_number(num),
            "table reports object {num}, which it also calls invalid"
        );
        // Every accessor a caller reaches for must answer without panicking,
        // including on the entry shapes only a damaged file produces.
        match xref.entry(num) {
            Some(Entry::Offset(_) | Entry::Free) | None => {}
            Some(Entry::InObjStream { stream, index: _ }) => {
                let _ = xref.is_object_stream(stream.num);
            }
        }
        let _ = xref.generation(num);
    }
});
