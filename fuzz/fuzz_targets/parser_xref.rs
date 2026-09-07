//! `read_xref` — tables, streams, `/Prev` chains, and the full-file rebuild.
//!
//! Property: never panics; every reported object number is one the table
//! calls legal. Not asserted: that an offset is inside the file, or that an
//! `InObjStream` archive is still flagged an object stream — both are damage
//! a later section can write.

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
