//! The word lexer that CMap programs are read with.
//!
//! A CMap stream is a PostScript program, but nothing here interprets
//! PostScript: it is split into whitespace- and delimiter-separated *words*
//! and the CMap operators are recognised among them. This is a different and
//! much simpler tokenizer than the one a content stream or a PDF body needs —
//! it has no concept of a number, a string value, or an escape — so it lives
//! here rather than being shared with `pdfrum-parser`.
//!
//! `pdfrum-font`'s `ToUnicode` parser reads the same shape of program and
//! re-uses [`Words`] directly; that is why [`Words`] is re-exported from the
//! crate root, as `pdfrum_cmap::Words`. The module itself is private — the
//! root re-export block is the crate's surface.
//!
//! The shape [`Words`] hands back is pinned by `a_cid_range_splits_into_five_words`
//! below, which is the doctest this module doc used to carry.

/// PDF whitespace. Note `0x80` and `0xFF`, which the PDF specification does
/// not list: PDFium's character table classifies them as whitespace and real
/// files are tokenized that way, so a `0xFF` between two words separates them
/// rather than joining them.
fn is_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20 | 0x80 | 0xFF)
}

/// PDF delimiters (ISO 32000-1 §7.2.2).
fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'%' | b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}'
    )
}

/// An iterator over the words of a CMap program.
///
/// Iteration ends at the first word the underlying scan reports as empty,
/// which is not always the end of the data: a `/Name` that runs to the end of
/// the stream with no separator after it produces an empty word and therefore
/// silently truncates the program. That is the behavior the CMap parser is
/// built on, so it is the iterator's contract too.
#[derive(Debug, Clone)]
pub struct Words<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Words<'a> {
    /// Start reading words from the beginning of `bytes`.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { data: bytes, at: 0 }
    }

    /// Skip whitespace and `%` comments; return the first significant byte,
    /// leaving `at` just past it.
    fn skip_spaces_and_comments(&mut self) -> Option<u8> {
        loop {
            let mut c = *self.data.get(self.at)?;
            self.at += 1;
            while is_whitespace(c) {
                c = *self.data.get(self.at)?;
                self.at += 1;
            }
            if c != b'%' {
                return Some(c);
            }
            loop {
                let c = *self.data.get(self.at)?;
                self.at += 1;
                if c == b'\r' || c == b'\n' {
                    break;
                }
            }
        }
    }

    fn since(&self, start: usize) -> &'a [u8] {
        self.data.get(start..self.at).unwrap_or_default()
    }

    /// A run of non-delimiter, non-whitespace bytes: an operator such as
    /// `begincidrange`, or a bare number.
    fn regular(&mut self, start: usize) -> &'a [u8] {
        while let Some(&c) = self.data.get(self.at) {
            if is_delimiter(c) || is_whitespace(c) {
                break;
            }
            self.at += 1;
        }
        self.since(start)
    }

    /// `/Name`. A name that reaches the end of the data without a separator
    /// after it yields an **empty** word, which ends the program.
    fn name(&mut self, start: usize) -> &'a [u8] {
        while let Some(&c) = self.data.get(self.at) {
            if is_whitespace(c) || is_delimiter(c) {
                return self.since(start);
            }
            self.at += 1;
        }
        &[]
    }

    /// `<...>` — a hex string, returned **including** both brackets — or the
    /// two-byte token `<<`.
    fn angle_open(&mut self, start: usize) -> &'a [u8] {
        let Some(&first) = self.data.get(self.at) else {
            return self.since(start);
        };
        self.at += 1;
        if first == b'<' {
            return self.since(start);
        }
        let mut c = first;
        while self.at < self.data.len() && c != b'>' {
            c = self.data.get(self.at).copied().unwrap_or(b'>');
            self.at += 1;
        }
        self.since(start)
    }

    /// `>` or `>>`.
    fn angle_close(&mut self, start: usize) -> &'a [u8] {
        if self.data.get(self.at) == Some(&b'>') {
            self.at += 1;
        }
        self.since(start)
    }

    /// `(...)` with balanced nesting and **no escape handling**, so a `\(`
    /// opens a level like any other `(`. The token includes both parentheses.
    fn parens(&mut self, start: usize) -> &'a [u8] {
        let mut level = 1i32;
        while level > 0 {
            let Some(&c) = self.data.get(self.at) else {
                break;
            };
            self.at += 1;
            match c {
                b'(' => level += 1,
                b')' => level -= 1,
                _ => {}
            }
        }
        self.since(start)
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.skip_spaces_and_comments()?;
        let start = self.at.checked_sub(1)?;
        let word = if is_delimiter(first) {
            match first {
                b'/' => self.name(start),
                b'<' => self.angle_open(start),
                b'>' => self.angle_close(start),
                b'(' => self.parens(start),
                // `)`, `[`, `]`, `{`, `}` and `%` are single-byte words.
                _ => self.since(start),
            }
        } else {
            self.regular(start)
        };
        (!word.is_empty()).then_some(word)
    }
}

#[cfg(test)]
mod tests {
    use super::Words;

    fn words(bytes: &[u8]) -> Vec<&[u8]> {
        Words::new(bytes).collect()
    }

    #[test]
    fn splits_operators_and_hex_strings() {
        assert_eq!(
            words(b"1 begincidchar <20> <100> endcidchar"),
            vec![&b"1"[..], b"begincidchar", b"<20>", b"<100>", b"endcidchar"]
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(words(b"").is_empty());
        assert!(words(b"   \n\t").is_empty());
    }

    #[test]
    fn comments_run_to_the_end_of_the_line() {
        assert_eq!(words(b"a % comment here\nb"), vec![&b"a"[..], b"b"]);
        // A comment with no line ending swallows the rest of the program.
        assert_eq!(words(b"a % comment here"), vec![&b"a"[..]]);
        // Comments may be consecutive.
        assert_eq!(words(b"%one\n%two\nz"), vec![&b"z"[..]]);
    }

    #[test]
    fn double_angle_is_its_own_word() {
        assert_eq!(words(b"<</X 1>>"), vec![&b"<<"[..], b"/X", b"1", b">>"]);
        assert_eq!(words(b"<<"), vec![&b"<<"[..]]);
        assert_eq!(words(b">"), vec![&b">"[..]]);
        assert_eq!(words(b">>"), vec![&b">>"[..]]);
    }

    /// A `<` with nothing after it is still a word, and an unterminated hex
    /// string is returned as the remainder.
    #[test]
    fn truncated_hex_strings_survive() {
        assert_eq!(words(b"<"), vec![&b"<"[..]]);
        assert_eq!(words(b"<a1"), vec![&b"<a1"[..]]);
        assert_eq!(words(b"<a1>"), vec![&b"<a1>"[..]]);
    }

    /// Parentheses nest and are not escaped, so `\(` opens a level.
    #[test]
    fn parenthesised_strings_are_returned_whole() {
        assert_eq!(words(b"(a(b)c) x"), vec![&b"(a(b)c)"[..], b"x"]);
        assert_eq!(words(b"(unterminated"), vec![&b"(unterminated"[..]]);
        assert_eq!(words(br"(a\(b) x"), vec![&br"(a\(b) x"[..]]);
        assert_eq!(words(b"(Japan1)"), vec![&b"(Japan1)"[..]]);
    }

    /// The truncation that ends a program: a name at the very end with no
    /// separator after it is dropped, and iteration stops there.
    #[test]
    fn a_name_at_end_of_data_ends_the_program() {
        assert_eq!(words(b"a /Ordering"), vec![&b"a"[..]]);
        assert_eq!(words(b"a /Ordering "), vec![&b"a"[..], b"/Ordering"]);
        assert_eq!(words(b"a /Ordering("), vec![&b"a"[..], b"/Ordering", b"("]);
    }

    #[test]
    fn bracket_delimiters_are_single_byte_words() {
        assert_eq!(
            words(b"[1]{2})"),
            vec![&b"["[..], b"1", b"]", b"{", b"2", b"}", b")"]
        );
    }

    /// 0x80 and 0xFF separate words, matching the character table the oracle
    /// tokenizes with.
    #[test]
    fn high_whitespace_bytes_separate_words() {
        assert_eq!(words(&[b'a', 0x80, b'b']), vec![&b"a"[..], b"b"]);
        assert_eq!(words(&[b'a', 0xFF, b'b']), vec![&b"a"[..], b"b"]);
        // Other high bytes do not.
        assert_eq!(words(&[b'a', 0xFE, b'b']), vec![&[b'a', 0xFE, b'b'][..]]);
    }

    /// Whatever the input, the lexer terminates and never reports a word that
    /// is not a subslice of the input.
    #[test]
    fn tokens_stay_inside_the_input() {
        let inputs: &[&[u8]] = &[
            b"<<<<<<",
            b"((((",
            b"))))",
            b"/",
            b"%",
            b"<>",
            b"><",
            &[0x00, 0xFF, 0x80, 0x25, 0x28],
            &[0x3C; 64],
        ];
        for input in inputs {
            let mut total = 0usize;
            for w in Words::new(input) {
                assert!(!w.is_empty());
                assert!(w.len() <= input.len());
                total += w.len();
                assert!(total <= input.len(), "words overlap in {input:?}");
            }
        }
    }
    /// Was this module's doctest, kept as a unit test now that the module is
    /// private: a `begincidrange` line is five words, and a hex string keeps
    /// its angle brackets.
    #[test]
    fn a_cid_range_splits_into_five_words() {
        let words: Vec<&[u8]> = Words::new(b"begincidrange <20> <7e> 1 endcidrange").collect();
        assert_eq!(words.first().copied(), Some(&b"begincidrange"[..]));
        // The angle brackets are part of the word.
        assert_eq!(words.get(1).copied(), Some(&b"<20>"[..]));
        assert_eq!(words.len(), 5);
    }
}
