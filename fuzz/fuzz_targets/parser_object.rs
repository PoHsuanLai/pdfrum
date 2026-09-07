//! `parse_object` in both strictness modes.
//!
//! Property: never panics; nesting past `max_object_nesting` is
//! `Error::TooDeep`, not a blown stack.

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
