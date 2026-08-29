//! The `/DA` default-appearance string: a fragment of content stream naming
//! a font, a size and a colour.
//!
//! It is read by a **third tokenizer**, different from both the file lexer
//! and the content-stream lexer, and the differences matter: a parenthesized
//! string here has nesting but **no backslash escapes**, a name runs to the
//! next delimiter with no length cap, and an unterminated name reads as
//! nothing at all. The three stay separate on purpose.
//!
//! The lookups are backwards from what you would expect. Rather than parsing
//! the string once, each accessor scans **from the beginning** for its
//! operator and then rewinds a fixed number of words to find the operands. A
//! consequence: `"0 g 1 0 0 rg"` reports **grey**, because `g` is tried first
//! and found, and the `rg` after it is never reached.

use crate::color::Color;

/// The six bytes the PDF grammar calls whitespace.
fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

/// The eight bytes that end a token without being part of it.
///
/// Spelled out here rather than borrowed from the file lexer: this tokenizer
/// is deliberately a separate one, and sharing the predicate is how the three
/// would drift into each other.
fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// A tokenizer over a `/DA` string.
#[derive(Debug, Clone)]
pub struct Parser<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    /// Starts at the beginning of `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Parser<'a> {
        Parser { data, at: 0 }
    }

    /// The current byte offset.
    #[must_use]
    pub fn position(&self) -> usize {
        self.at
    }

    /// Moves to a byte offset.
    pub fn seek(&mut self, at: usize) {
        self.at = at;
    }

    /// The next token, or an empty slice at end of input.
    #[must_use]
    pub fn next_word(&mut self) -> &'a [u8] {
        let Some(first) = self.skip_spaces_and_comments() else {
            return b"";
        };
        let start = self.at - 1;
        if !is_delimiter(first) {
            return self.scan_to_delimiter(start);
        }
        match first {
            b'/' => self.scan_name(start),
            b'<' => self.scan_angle_open(start),
            b'>' => self.scan_angle_close(start),
            b'(' => self.scan_parens(start),
            // Any other delimiter is a one-byte token.
            _ => self.slice(start),
        }
    }

    /// Advances past whitespace and `%` comments, returning the first byte of
    /// the token that follows.
    fn skip_spaces_and_comments(&mut self) -> Option<u8> {
        loop {
            let mut byte = *self.data.get(self.at)?;
            self.at += 1;
            while is_whitespace(byte) {
                byte = *self.data.get(self.at)?;
                self.at += 1;
            }
            if byte != b'%' {
                return Some(byte);
            }
            loop {
                let byte = *self.data.get(self.at)?;
                self.at += 1;
                if byte == b'\r' || byte == b'\n' {
                    break;
                }
            }
        }
    }

    fn slice(&self, start: usize) -> &'a [u8] {
        self.data.get(start..self.at).unwrap_or_default()
    }

    fn scan_to_delimiter(&mut self, start: usize) -> &'a [u8] {
        while let Some(byte) = self.data.get(self.at) {
            if is_delimiter(*byte) || is_whitespace(*byte) {
                break;
            }
            self.at += 1;
        }
        self.slice(start)
    }

    /// A name runs to the next delimiter — and an unterminated one, running
    /// to the very end of the input, reads as **nothing**.
    fn scan_name(&mut self, start: usize) -> &'a [u8] {
        while let Some(byte) = self.data.get(self.at) {
            if is_delimiter(*byte) || is_whitespace(*byte) {
                return self.slice(start);
            }
            self.at += 1;
        }
        b""
    }

    fn scan_angle_open(&mut self, start: usize) -> &'a [u8] {
        let Some(byte) = self.data.get(self.at) else {
            return self.slice(start);
        };
        self.at += 1;
        // `<<` is its own token.
        if *byte == b'<' {
            return self.slice(start);
        }
        let mut byte = *byte;
        while self.at < self.data.len() && byte != b'>' {
            byte = self.data.get(self.at).copied().unwrap_or(b'>');
            self.at += 1;
        }
        self.slice(start)
    }

    fn scan_angle_close(&mut self, start: usize) -> &'a [u8] {
        if self.data.get(self.at) == Some(&b'>') {
            self.at += 1;
        }
        self.slice(start)
    }

    /// Parentheses nest, and a backslash is **not** an escape here.
    fn scan_parens(&mut self, start: usize) -> &'a [u8] {
        let mut level = 1;
        while self.at < self.data.len() && level > 0 {
            let byte = self.data.get(self.at).copied().unwrap_or(b')');
            self.at += 1;
            match byte {
                b'(' => level += 1,
                b')' => level -= 1,
                _ => {}
            }
        }
        self.slice(start)
    }
}

/// Finds `token` and rewinds to the `params` words before it.
///
/// Returns whether it was found; on success the parser sits at the first
/// operand, on failure wherever the scan gave up. A token found too early in
/// the string — with fewer than `params` words before it — is **skipped**, and
/// the scan continues looking for a later occurrence.
#[must_use]
pub fn find_tag_param_from_start(parser: &mut Parser<'_>, token: &[u8], params: usize) -> bool {
    // A ring of the last `params + 1` positions, so the rewind target is
    // always the one about to be overwritten.
    let slots = params + 1;
    let mut ring = vec![0usize; slots];
    let mut index = 0;
    let mut filled = 0;

    parser.seek(0);
    loop {
        if let Some(slot) = ring.get_mut(index) {
            *slot = parser.position();
        }
        index = (index + 1) % slots;
        filled = (filled + 1).min(slots);

        let word = parser.next_word();
        if word.is_empty() {
            return false;
        }
        if word == token {
            if filled < slots {
                continue;
            }
            parser.seek(ring.get(index).copied().unwrap_or(0));
            return true;
        }
    }
}

/// The font a `/DA` names.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FontNameAndSize {
    /// The resource name, with its leading slash removed and any `#`
    /// escapes decoded.
    pub name: Vec<u8>,
    /// The size. Zero means "fit the field".
    pub size: f32,
}

/// The `Tf` operand pair.
///
/// Returns `None` **only** for an empty `/DA`. A string with no `Tf` at all
/// answers with an empty name and a size of zero — which is not the same
/// thing, and which callers act on differently: the first means "no default
/// appearance", the second means "one that names no font".
///
/// The name loses its first character whether or not that character is a
/// slash, so a `Tf` operand written without one loses a real letter.
#[must_use]
pub fn font(da: &[u8]) -> Option<FontNameAndSize> {
    if da.is_empty() {
        return None;
    }
    let mut parser = Parser::new(da);
    if !find_tag_param_from_start(&mut parser, b"Tf", 2) {
        return Some(FontNameAndSize::default());
    }
    let name = parser.next_word();
    let size = parser.next_word();
    Some(FontNameAndSize {
        name: pdfrum_object::name_decode(name.get(1..).unwrap_or_default()),
        size: parse_float(size),
    })
}

/// The colour a `/DA` names.
///
/// The three operators are tried **in order** — grey, then RGB, then CMYK —
/// and each scan restarts at the beginning, so the first one present wins
/// wherever it sits. `"1 0 0 rg 0 g"` and `"0 g 1 0 0 rg"` both report grey.
#[must_use]
pub fn color(da: &[u8]) -> Option<Color> {
    if da.is_empty() {
        return None;
    }
    let mut parser = Parser::new(da);
    if find_tag_param_from_start(&mut parser, b"g", 1) {
        return Some(Color::Gray(parse_float(parser.next_word())));
    }
    if find_tag_param_from_start(&mut parser, b"rg", 3) {
        let (r, g, b) = (
            parse_float(parser.next_word()),
            parse_float(parser.next_word()),
            parse_float(parser.next_word()),
        );
        return Some(Color::Rgb(r, g, b));
    }
    if find_tag_param_from_start(&mut parser, b"k", 4) {
        let (c, m, y, k) = (
            parse_float(parser.next_word()),
            parse_float(parser.next_word()),
            parse_float(parser.next_word()),
            parse_float(parser.next_word()),
        );
        return Some(Color::Cmyk(c, m, y, k));
    }
    None
}

/// A number as the content-stream grammar spells it, defaulting to zero.
fn parse_float(word: &[u8]) -> f32 {
    let text = String::from_utf8_lossy(word);
    // Take the longest numeric prefix, which is what a lenient reader does
    // with a token that has trailing junk.
    let end = text
        .char_indices()
        .find(|(index, c)| {
            !(c.is_ascii_digit() || (*index == 0 && (*c == '-' || *c == '+')) || *c == '.')
        })
        .map_or(text.len(), |(index, _)| index);
    text.get(..end)
        .and_then(|prefix| prefix.parse().ok())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::{Parser, color, find_tag_param_from_start, font};
    use crate::color::Color;

    /// Runs one `find_tag_param_from_start` case, reporting the outcome and
    /// where the parser was left.
    fn find(data: &str, token: &str, params: usize) -> (bool, usize) {
        let mut parser = Parser::new(data.as_bytes());
        let found = find_tag_param_from_start(&mut parser, token.as_bytes(), params);
        (found, parser.position())
    }

    #[test]
    fn the_search_reports_where_it_stopped_as_well_as_whether_it_found() {
        assert_eq!(find("", "Tj", 1), (false, 0));
        assert_eq!(find("", "", 1), (false, 0));
        // Three words, none matching: the position is the end of the input.
        assert_eq!(find("  T j", "", 1), (false, 5));
        // Found, but with nothing before it to rewind to.
        assert_eq!(find("Tj", "Tj", 1), (false, 2));
        // An unterminated name reads as empty, which ends the scan.
        assert_eq!(find("(Tj", "Tj", 1), (false, 3));
        assert_eq!(find("\r12\t34  56 78Tj", "Tj", 1), (false, 15));
    }

    #[test]
    fn a_match_rewinds_exactly_as_many_words_as_asked_for() {
        assert_eq!(find("\r\0abd Tj", "Tj", 1), (true, 0));
        assert_eq!(find("12 4 Tj 3 46 Tj", "Tj", 1), (true, 2));
        assert_eq!(find("er^ 2 (34) (5667) Tj", "Tj", 2), (true, 5));
        assert_eq!(find("<344> (232)\t343.4\n12 45 Tj", "Tj", 3), (true, 11));
        assert_eq!(find("1 2 3 4 5 6 7 8 cm", "cm", 6), (true, 3));
    }

    #[test]
    fn an_empty_string_and_a_string_without_a_font_answer_differently() {
        assert_eq!(font(b""), None);
        let no_tf = font(b"0 g").expect("not empty");
        assert!(no_tf.name.is_empty());
        assert!(no_tf.size.abs() < f32::EPSILON);
    }

    #[test]
    fn the_font_operands_read_in_order() {
        let got = font(b"0 0 0 rg /Helv 12 Tf").expect("has a font");
        assert_eq!(got.name, b"Helv");
        assert!((got.size - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_negative_size_survives() {
        let got = font(b"0 0 0 rg /F1 -12 Tf").expect("has a font");
        assert!((got.size + 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_name_operand_loses_its_first_character_whether_or_not_it_is_a_slash() {
        let slashless = font(b"Helv 12 Tf").expect("has a font");
        assert_eq!(slashless.name, b"elv");
    }

    #[test]
    fn a_hash_escape_in_the_name_is_decoded() {
        let got = font(b"/A#20B 8 Tf").expect("has a font");
        assert_eq!(got.name, b"A B");
    }

    #[test]
    fn the_first_colour_operator_wins_wherever_it_sits() {
        // Grey is tried first, so it wins even written second.
        assert_eq!(color(b"1 0 0 rg 0 g"), Some(Color::Gray(0.0)));
        assert_eq!(color(b"0 g 1 0 0 rg"), Some(Color::Gray(0.0)));
        assert_eq!(color(b"1 0 0 rg"), Some(Color::Rgb(1.0, 0.0, 0.0)));
        assert_eq!(
            color(b"0 .25 .5 1 k"),
            Some(Color::Cmyk(0.0, 0.25, 0.5, 1.0))
        );
        assert_eq!(color(b"/Helv 12 Tf"), None);
        assert_eq!(color(b""), None);
    }

    #[test]
    fn the_tokenizer_treats_parentheses_as_nesting_without_escapes() {
        let mut parser = Parser::new(b"(a(b)c) next");
        assert_eq!(parser.next_word(), b"(a(b)c)");
        assert_eq!(parser.next_word(), b"next");

        // A backslash is an ordinary byte here, so this string ends early.
        let mut parser = Parser::new(br"(a\) b");
        assert_eq!(parser.next_word(), br"(a\)");
    }

    #[test]
    fn a_comment_runs_to_the_end_of_its_line() {
        let mut parser = Parser::new(b"% skipped\n/Helv 12 Tf");
        assert_eq!(parser.next_word(), b"/Helv");
        assert_eq!(parser.next_word(), b"12");
        assert_eq!(parser.next_word(), b"Tf");
        assert_eq!(parser.next_word(), b"");
    }

    #[test]
    fn angle_brackets_tokenize_as_hex_strings_and_dictionary_marks() {
        let mut parser = Parser::new(b"<< <41> >>");
        assert_eq!(parser.next_word(), b"<<");
        assert_eq!(parser.next_word(), b"<41>");
        assert_eq!(parser.next_word(), b">>");
    }
}
