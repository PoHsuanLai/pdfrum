//! A tokenizer for the PostScript-shaped text of a Type 1 font program.
//!
//! This is *not* a PostScript interpreter and must not become one. A Type 1
//! font program is written in PostScript, but every reader in existence
//! recognises it by pattern rather than by executing it: the shapes that carry
//! data (`/Key value def`, `dup <n> /name put`, `/name <len> RD <bytes> ND`)
//! are fixed by the specification, and the surrounding procedure bodies are
//! boilerplate that exists for a real PostScript printer's benefit.
//!
//! So the tokenizer's job is to hand out lexical atoms — names, numbers,
//! delimiters, literal strings, and *binary payloads* whose length the
//! preceding number gave — and the field readers in [`crate::program`] match on
//! sequences of them.

/// One lexical atom of a font program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token<'a> {
    /// `/Name` — a literal name, without the slash.
    Literal(&'a [u8]),
    /// A bare keyword or operator: `def`, `dup`, `RD`, `array`, `readonly`.
    Keyword(&'a [u8]),
    /// A number. Type 1 writes integers and reals; both arrive as `f64`
    /// because `/BlueScale 0.03963` and `/Subrs 9` sit side by side and the
    /// consumers coerce.
    Number(f64),
    /// `(text)`, with escapes resolved by the caller if it cares — the two
    /// consumers here (`/FullName`, `/FamilyName`) want the bytes.
    Str(&'a [u8]),
    /// One of `[ ] { } << >>`.
    Delim(u8),
}

/// A cursor over font-program text.
///
/// Cheap to clone, which the field readers rely on: several of them peek ahead
/// by cloning rather than by buffering.
#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Lexer<'a> {
    /// Start at the beginning of `bytes`.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Skip whitespace and `%` comments.
    fn skip_blanks(&mut self) {
        loop {
            match self.bytes.get(self.at) {
                Some(b) if b.is_ascii_whitespace() => self.at = self.at.saturating_add(1),
                Some(b'%') => {
                    while !matches!(self.bytes.get(self.at), None | Some(b'\r' | b'\n')) {
                        self.at = self.at.saturating_add(1);
                    }
                }
                _ => return,
            }
        }
    }

    /// Read `n` raw bytes from the current position, skipping exactly one
    /// separator first.
    ///
    /// This is the `<len> RD <bytes>` escape hatch: the byte after `RD` is a
    /// single space by convention, and the payload that follows is arbitrary
    /// binary that must not be tokenized. Returns `None` if the payload runs
    /// past the end.
    pub fn take_binary(&mut self, n: usize) -> Option<&'a [u8]> {
        // Exactly one separator, per the specification. Being lenient here
        // would eat a payload byte that happens to be 0x20.
        if self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at = self.at.saturating_add(1);
        }
        let end = self.at.checked_add(n)?;
        let out = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(out)
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        self.skip_blanks();
        let first = self.bytes.get(self.at).copied()?;
        match first {
            b'/' => {
                self.at = self.at.saturating_add(1);
                let start = self.at;
                self.advance_over_regular();
                Some(Token::Literal(
                    self.bytes.get(start..self.at).unwrap_or_default(),
                ))
            }
            b'[' | b']' | b'{' | b'}' => {
                self.at = self.at.saturating_add(1);
                Some(Token::Delim(first))
            }
            b'<' | b'>' => {
                // `<<` and `>>` are dictionary delimiters; a lone `<` opens a
                // hex string, which no Type 1 field we read uses, so it is
                // reported as a delimiter and skipped by the consumers.
                self.at = self.at.saturating_add(1);
                if self.bytes.get(self.at) == Some(&first) {
                    self.at = self.at.saturating_add(1);
                }
                Some(Token::Delim(first))
            }
            b'(' => {
                self.at = self.at.saturating_add(1);
                let start = self.at;
                let mut depth = 1usize;
                while depth > 0 {
                    match self.bytes.get(self.at).copied() {
                        None => break,
                        Some(b'\\') => self.at = self.at.saturating_add(1),
                        Some(b'(') => depth = depth.saturating_add(1),
                        Some(b')') => depth = depth.saturating_sub(1),
                        Some(_) => {}
                    }
                    self.at = self.at.saturating_add(1);
                }
                // `self.at` is one past the closing paren (or at the end).
                let end = self.at.saturating_sub(usize::from(depth == 0));
                Some(Token::Str(
                    self.bytes.get(start..end.max(start)).unwrap_or_default(),
                ))
            }
            _ => {
                let start = self.at;
                self.advance_over_regular();
                let word = self.bytes.get(start..self.at).unwrap_or_default();
                if word.is_empty() {
                    // A byte that is neither regular nor a recognised
                    // delimiter (only `)` reaches here). Consume it so the
                    // iterator always makes progress.
                    self.at = self.at.saturating_add(1);
                    return self.next();
                }
                Some(number(word).map_or(Token::Keyword(word), Token::Number))
            }
        }
    }
}

impl Lexer<'_> {
    /// Advance over a run of "regular" characters — anything that is not
    /// whitespace and not one of PostScript's delimiters.
    fn advance_over_regular(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| !b.is_ascii_whitespace() && !is_delimiter(*b))
        {
            self.at = self.at.saturating_add(1);
        }
    }
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Parse a PostScript number, which is a subset of Rust's float syntax plus
/// radix literals (`16#FF`) that Type 1 fonts do not in practice use for the
/// fields we read.
fn number(word: &[u8]) -> Option<f64> {
    if word.is_empty() {
        return None;
    }
    // Reject anything that is not sign/digit/dot/exponent up front, so a
    // keyword like `NaN` or `inf` (both of which `str::parse` accepts) does
    // not become a number.
    let mut has_digit = false;
    for (i, b) in word.iter().enumerate() {
        match b {
            b'0'..=b'9' => has_digit = true,
            b'+' | b'-' => {
                // Only leading, or immediately after an exponent marker.
                let prev = i.checked_sub(1).and_then(|p| word.get(p)).copied();
                if !matches!(prev, None | Some(b'e' | b'E')) {
                    return None;
                }
            }
            b'.' => {}
            b'e' | b'E' if has_digit => {}
            _ => return None,
        }
    }
    if !has_digit {
        return None;
    }
    core::str::from_utf8(word).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{Lexer, Token};

    fn toks(s: &[u8]) -> Vec<Token<'_>> {
        Lexer::new(s).collect()
    }

    #[test]
    fn splits_a_typical_definition() {
        assert_eq!(
            toks(b"/FontMatrix [ 0.001 0 0 0.001 0 0 ] readonly def"),
            vec![
                Token::Literal(b"FontMatrix"),
                Token::Delim(b'['),
                Token::Number(0.001),
                Token::Number(0.0),
                Token::Number(0.0),
                Token::Number(0.001),
                Token::Number(0.0),
                Token::Number(0.0),
                Token::Delim(b']'),
                Token::Keyword(b"readonly"),
                Token::Keyword(b"def"),
            ]
        );
    }

    #[test]
    fn comments_and_strings() {
        assert_eq!(
            toks(b"% a /comment (with parens)\n/FullName (Chrome Sans MM) readonly def"),
            vec![
                Token::Literal(b"FullName"),
                Token::Str(b"Chrome Sans MM"),
                Token::Keyword(b"readonly"),
                Token::Keyword(b"def"),
            ]
        );
    }

    #[test]
    fn a_slash_ends_the_previous_token() {
        // `/A/B` is two literals: `/` is a delimiter, so no whitespace needed.
        assert_eq!(
            toks(b"/A/B"),
            vec![Token::Literal(b"A"), Token::Literal(b"B")]
        );
    }

    #[test]
    fn binary_payload_is_taken_verbatim() {
        let src = b"/g 5 RD \x00\x20/\x7dabc ND";
        let mut lx = Lexer::new(src);
        assert_eq!(lx.next(), Some(Token::Literal(b"g")));
        assert_eq!(lx.next(), Some(Token::Number(5.0)));
        assert_eq!(lx.next(), Some(Token::Keyword(b"RD")));
        // The payload contains a space, a slash and a brace: none tokenized.
        assert_eq!(lx.take_binary(5), Some(b"\x00\x20/\x7da".as_slice()));
        assert_eq!(lx.next(), Some(Token::Keyword(b"bc")));
    }

    #[test]
    fn keywords_that_look_like_numbers_are_not() {
        // `NaN` and `inf` parse as floats in Rust and must not here; `1e` is
        // a truncated exponent, which is not a number either.
        assert_eq!(
            toks(b"NaN inf -- 1e 1e3 -2.5"),
            vec![
                Token::Keyword(b"NaN"),
                Token::Keyword(b"inf"),
                Token::Keyword(b"--"),
                Token::Keyword(b"1e"),
                Token::Number(1000.0),
                Token::Number(-2.5),
            ]
        );
    }

    #[test]
    fn unterminated_string_yields_the_rest() {
        assert_eq!(toks(b"(no close"), vec![Token::Str(b"no close")]);
    }

    #[test]
    fn a_stray_close_paren_does_not_stall() {
        assert_eq!(toks(b") def"), vec![Token::Keyword(b"def")]);
    }
}
