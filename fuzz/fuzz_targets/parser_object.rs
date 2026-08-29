//! `parse_object` — the PDF object grammar over arbitrary bytes, in both
//! strictness modes.
//!
//! Objects are parsed in a loop rather than once, because the recursion
//! guard, the array/dictionary length caps and the recovery paths only get
//! interesting once the lexer is somewhere in the middle of the input.
//!
//! Property: never panics and never fails to terminate. Nesting is capped at
//! `max_object_nesting`, so a deeply nested input must come back as
//! `Error::TooDeep`, not as a blown stack.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_object::NoResolve;
use pdfrum_parser::{Lexer, Strictness, parse_object};

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let strictness = if split.byte() & 1 == 0 {
        Strictness::Loose
    } else {
        Strictness::Strict
    };
    let body = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    let mut lexer = Lexer::new(body);

    while !lexer.at_eof() {
        let before = lexer.pos();
        let result = parse_object(&mut lexer, &limits, &mut diags, strictness, &NoResolve);
        assert!(lexer.pos() <= body.len(), "parse ran past the input");
        if result.is_err() {
            break;
        }
        // A successful parse that consumed nothing would spin forever here,
        // and would mean a caller reading a sequence of objects could not
        // make progress either.
        if lexer.pos() == before {
            break;
        }
    }
});
