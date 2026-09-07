//! Lexer token stream, including the literal and hex string readers.
//!
//! Property: every `next_word` leaves the position no earlier; `Eof` arrives.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_parser::{Delim, Lexer, Token};

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();
    let mut lexer = Lexer::new(data);

    loop {
        let before = lexer.pos();
        // `peek_word` must not move the cursor, and must agree with the
        // token `next_word` then returns.
        let peeked = lexer.peek_word(&limits);
        assert_eq!(lexer.pos(), before, "peek_word moved the cursor");

        let token = lexer.next_word(&limits);
        assert_eq!(token, peeked, "peek_word disagreed with next_word");
        assert!(lexer.pos() >= before, "lexer went backwards");
        assert!(lexer.pos() <= data.len(), "lexer ran past the input");

        match token {
            Token::Eof => break,
            // A string's opening delimiter hands over to the string reader,
            // which is where the escape and nesting scanners live.
            Token::Delim(Delim::StringOpen) => {
                let _ = lexer.read_literal_string();
            }
            Token::Delim(Delim::HexOpen) => {
                let _ = lexer.read_hex_string();
            }
            Token::Number(bytes) => {
                // Both numeric readers are total over any spelling the
                // lexer classified as numeric.
                let _ = pdfrum_parser::atoi64(bytes);
                let _ = pdfrum_parser::atoui(bytes);
                assert!(bytes.len() <= limits.max_word_len);
            }
            Token::Name(bytes) | Token::Keyword(bytes) => {
                assert!(bytes.len() <= limits.max_word_len);
            }
            Token::Delim(_) => {}
        }

        // Termination: with `Eof` handled above, every other token must have
        // consumed at least one byte, or this loop never ends.
        assert!(
            lexer.pos() > before,
            "next_word did not advance at {before}"
        );
    }

    assert!(lexer.at_eof());
});
