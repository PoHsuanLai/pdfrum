//! The content-stream tokenizer (ISO 32000-1 §7.2 as content streams use it).
//!
//! This is deliberately *not* the file lexer from `pdfrum-parser`. PDFium
//! runs a second, simpler tokenizer over content streams, and the two
//! disagree in ways real files depend on (design brief D21):
//!
//! - words truncate at **255** bytes here, 256 in the file lexer;
//! - strings truncate at **32767** bytes, a cap the file lexer does not have;
//! - a dictionary with a non-name key, or with an unparsable value, fails
//!   *entirely* rather than skipping the bad entry;
//! - a nested array inside a top-level array is refused, and the outer array
//!   then swallows its elements (see [`ObjectReader`]);
//! - there are no indirect references at all.
//!
//! The output is an [`Element`] stream: numbers and names stay unboxed
//! because the operand ring materializes them lazily, everything else arrives
//! as an [`Object`].

use pdfrum_object::{Array, Dict, Name, Object, PdfString};

/// Longest word kept; further bytes are consumed and dropped.
pub(crate) const MAX_WORD_LEN: usize = 255;

/// Longest string kept, for both literal and hexadecimal syntax.
pub(crate) const MAX_STRING_LEN: usize = 32767;

/// How deep [`ObjectReader`] recurses before giving up and yielding null.
pub(crate) const MAX_OBJECT_DEPTH: u32 = 64;

/// One token from a content stream.
///
/// Mirrors PDFium's `ElementType`: the tokenizer distinguishes a number and a
/// name from a general object because the operand ring stores those two
/// unboxed, and a keyword from everything else because only a keyword can be
/// an operator.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Element {
    /// A numeric literal.
    Number(f32),
    /// A name, without its leading solidus, already `#`-decoded.
    Name(Name),
    /// A bare word that is none of the above: an operator, or garbage.
    Keyword(Box<[u8]>),
    /// Anything the object grammar produced: strings, arrays, dictionaries,
    /// booleans, nulls. A delimiter that starts nothing (`]`, `)`, `>`, `{`,
    /// `}`) lands here as [`Object::Null`].
    Object(Object),
    /// The stream ended.
    Eof,
}

/// A cursor over content-stream bytes.
///
/// Holds only a position, so rewinding is assigning back a saved
/// [`ContentLexer::pos`] — which the inline-image scan and the `m` fast loop
/// both rely on.
#[derive(Debug, Clone)]
pub(crate) struct ContentLexer<'a> {
    data: &'a [u8],
    pos: usize,
}

/// Whether a byte ends a token.
fn is_ws(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// Whether a byte is one of the eight PDF delimiters.
fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Whether a byte can appear in a numeric literal.
fn is_numeric_char(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.')
}

/// The numeric reading PDFium's `FX_Number` gives a word: a decimal integer
/// or real, with anything unparsable reading as zero.
fn word_to_number(word: &[u8]) -> f32 {
    let mut neg = false;
    let mut int: i64 = 0;
    let mut frac: f32 = 0.0;
    let mut scale: f32 = 0.1;
    let mut seen_dot = false;
    let mut i = 0;
    while let Some(&b) = word.get(i) {
        match b {
            b'-' if i == 0 => neg = true,
            b'+' if i == 0 => {}
            b'.' if !seen_dot => seen_dot = true,
            b'0'..=b'9' => {
                if seen_dot {
                    frac += f32::from(b - b'0') * scale;
                    scale *= 0.1;
                } else {
                    int = int.saturating_mul(10).saturating_add(i64::from(b - b'0'));
                }
            }
            // A second sign or dot, or any other byte, ends the number —
            // matching `FX_Number`'s single forward scan.
            _ => break,
        }
        i += 1;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "matching the C++'s int-to-float widening exactly, including its loss"
    )]
    let v = int as f32 + frac;
    if neg { -v } else { v }
}

impl<'a> ContentLexer<'a> {
    /// A lexer positioned at the start of `data`.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The current byte offset.
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Move to `pos`, clamped to the end of the data.
    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }

    /// The whole buffer, for the inline-image reader's byte-range slicing.
    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Skip whitespace and `%`-comments. A comment runs to a line ending, and
    /// whitespace skipping resumes after it, so comments are invisible.
    fn skip_blanks(&mut self) {
        while let Some(b) = self.peek() {
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    if c == b'\r' || c == b'\n' {
                        break;
                    }
                }
            } else {
                return;
            }
        }
    }

    /// The next element, following `ParseNextElement`'s dispatch exactly.
    pub(crate) fn next_element(&mut self) -> Element {
        self.skip_blanks();
        let Some(first) = self.peek() else {
            return Element::Eof;
        };

        // A delimiter that is not `/` routes into the object grammar. The
        // closers among them (`]`, `)`, `>`, `{`, `}`) fall through it to
        // null, which the interpreter then pushes as a null operand.
        if is_delim(first) && first != b'/' {
            let obj = ObjectReader::new(self).read(false, false, 0);
            return Element::Object(obj);
        }

        // The first byte is taken unconditionally — that is what lets a name
        // begin with `/` even though `/` is itself a delimiter. Only
        // *subsequent* bytes end the word.
        let start = self.pos;
        let mut is_number = true;
        let mut len = 0usize;
        while let Some(b) = self.peek() {
            if !is_numeric_char(b) {
                is_number = false;
            }
            self.pos += 1;
            len += 1;
            match self.peek() {
                Some(next) if is_ws(next) || is_delim(next) => break,
                Some(_) => {}
                None => break,
            }
        }
        // Bytes past the cap are consumed but dropped, so a 300-byte name
        // becomes its first 255 bytes and the rest vanish.
        let word = self
            .data
            .get(start..start + len.min(MAX_WORD_LEN))
            .unwrap_or(&[]);

        if is_number && len > 0 {
            return Element::Number(word_to_number(word));
        }
        if word.first() == Some(&b'/') {
            return Element::Name(Name::decode(word.get(1..).unwrap_or(&[])));
        }
        // The three exact-length keyword checks. Because these become
        // *objects*, `true`, `false` and `null` can never dispatch as
        // operators.
        match word {
            b"true" => Element::Object(Object::Bool(true)),
            b"false" => Element::Object(Object::Bool(false)),
            b"null" => Element::Object(Object::Null),
            _ if word.is_empty() => Element::Eof,
            _ => Element::Keyword(word.into()),
        }
    }

    /// The lower-level scanner `ObjectReader` uses.
    ///
    /// Returns the token bytes and whether every byte was numeric. Unlike
    /// [`Self::next_element`] this splits `<<`/`>>` from `<`/`>` and returns
    /// each other delimiter as a one-byte token.
    fn next_word(&mut self) -> (Vec<u8>, bool) {
        self.skip_blanks();
        let Some(first) = self.peek() else {
            return (Vec::new(), false);
        };
        if is_delim(first) {
            self.pos += 1;
            let mut word = vec![first];
            match first {
                b'/' => {
                    // A name keeps running through regular and numeric bytes.
                    while let Some(b) = self.peek() {
                        if is_ws(b) || is_delim(b) {
                            break;
                        }
                        if word.len() <= MAX_WORD_LEN {
                            word.push(b);
                        }
                        self.pos += 1;
                    }
                    return (word, false);
                }
                b'<' | b'>' => {
                    if self.peek() == Some(first) {
                        self.pos += 1;
                        word.push(first);
                    }
                    return (word, false);
                }
                _ => return (word, false),
            }
        }
        let start = self.pos;
        let mut is_number = true;
        let mut len = 0usize;
        while let Some(b) = self.peek() {
            if is_ws(b) || is_delim(b) {
                break;
            }
            if !is_numeric_char(b) {
                is_number = false;
            }
            self.pos += 1;
            len += 1;
        }
        let word = self
            .data
            .get(start..start + len.min(MAX_WORD_LEN))
            .unwrap_or(&[]);
        (word.to_vec(), is_number && len > 0)
    }

    /// A literal string, from just after the opening parenthesis.
    ///
    /// The same five-state escape machine as the file lexer, plus this
    /// tokenizer's 32767-byte truncation on both the normal and the
    /// end-of-data exit.
    fn read_literal_string(&mut self) -> PdfString {
        let mut out: Vec<u8> = Vec::new();
        let mut level = 0i32;
        let mut state = 0u8; // 0 normal, 1 after backslash, 2..=3 octal, 4 CR
        let mut octal = 0u32;
        let mut digits = 0u8;

        let push = |out: &mut Vec<u8>, b: u8| {
            if out.len() < MAX_STRING_LEN {
                out.push(b);
            }
        };

        while let Some(b) = self.peek() {
            self.pos += 1;
            match state {
                0 => match b {
                    b'\\' => state = 1,
                    b'(' => {
                        level += 1;
                        push(&mut out, b);
                    }
                    b')' => {
                        if level == 0 {
                            return PdfString::literal(out);
                        }
                        level -= 1;
                        push(&mut out, b);
                    }
                    _ => push(&mut out, b),
                },
                1 => {
                    state = 0;
                    match b {
                        b'n' => push(&mut out, b'\n'),
                        b'r' => push(&mut out, b'\r'),
                        b't' => push(&mut out, b'\t'),
                        b'b' => push(&mut out, 0x08),
                        b'f' => push(&mut out, 0x0C),
                        b'0'..=b'7' => {
                            octal = u32::from(b - b'0');
                            digits = 1;
                            state = 2;
                        }
                        b'\r' => state = 4,
                        b'\n' => {}
                        _ => push(&mut out, b),
                    }
                }
                2 | 3 => {
                    if b.is_ascii_digit() && b < b'8' {
                        octal = octal * 8 + u32::from(b - b'0');
                        digits += 1;
                        if digits == 3 {
                            let byte = u8::try_from(octal & 0xFF).unwrap_or(0);
                            push(&mut out, byte);
                            state = 0;
                        } else {
                            state = 3;
                        }
                    } else {
                        let byte = u8::try_from(octal & 0xFF).unwrap_or(0);
                        push(&mut out, byte);
                        state = 0;
                        // Reprocess this byte in the normal state.
                        self.pos -= 1;
                    }
                }
                _ => {
                    // A `\<CR>` line continuation swallows an immediately
                    // following `<LF>` too.
                    state = 0;
                    if b != b'\n' {
                        self.pos -= 1;
                    }
                }
            }
        }
        PdfString::literal(out)
    }

    /// A hexadecimal string, from just after the opening angle bracket.
    ///
    /// Non-hex bytes are skipped silently, `>` or the end of data terminates,
    /// and a trailing lone nibble contributes `nibble * 16`.
    fn read_hex_string(&mut self) -> PdfString {
        let mut out: Vec<u8> = Vec::new();
        let mut nibble: Option<u8> = None;
        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'>' {
                break;
            }
            let Some(v) = (b as char).to_digit(16) else {
                continue;
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a hex digit is always below 16"
            )]
            let v = v as u8;
            match nibble.take() {
                None => nibble = Some(v),
                Some(hi) => {
                    if out.len() < MAX_STRING_LEN {
                        out.push((hi << 4) | v);
                    }
                }
            }
        }
        if let Some(hi) = nibble
            && out.len() < MAX_STRING_LEN
        {
            out.push(hi << 4);
        }
        PdfString::hex(out)
    }
}

/// The object grammar sitting on top of [`ContentLexer`].
///
/// A borrow of the lexer plus nothing else — the recursion depth is a
/// parameter, so the reader holds no state that could desynchronize from the
/// cursor.
struct ObjectReader<'r, 'a> {
    lexer: &'r mut ContentLexer<'a>,
    /// The word the most recent scan produced. The array loop consults this
    /// *after* a nested read has returned nothing, to tell "end of data" and
    /// "a closing bracket" apart from "an element that would not parse" —
    /// without reading another token, which would lose it.
    last_word: Vec<u8>,
}

impl<'r, 'a> ObjectReader<'r, 'a> {
    fn new(lexer: &'r mut ContentLexer<'a>) -> Self {
        Self {
            lexer,
            last_word: Vec::new(),
        }
    }

    /// Scan a word, remembering it.
    fn scan_word(&mut self) -> (Vec<u8>, bool) {
        let (word, is_number) = self.lexer.next_word();
        self.last_word.clone_from(&word);
        (word, is_number)
    }

    /// Read one object.
    ///
    /// `allow_nested_array` and `in_array` together encode PDFium's rule that
    /// an array *directly* inside an operator-level array is refused: a bare
    /// `[` at operator level reads with `allow_nested_array = false`, so a
    /// nested `[` yields null, the array loop does not treat that null as a
    /// terminator, and it spins forward consuming elements until `]`. Net,
    /// `[1 [2] 3]` at operator level is `[1, 3]`. Dictionary *values* read
    /// with `allow_nested_array = true`, so dictionaries do nest arrays.
    fn read(&mut self, allow_nested_array: bool, in_array: bool, depth: u32) -> Object {
        if depth > MAX_OBJECT_DEPTH {
            return Object::Null;
        }
        let (word, is_number) = self.scan_word();
        if word.is_empty() {
            return Object::Null;
        }
        if is_number {
            return match std::str::from_utf8(&word)
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
            {
                Some(i) => Object::Int(i),
                None => Object::Real(word_to_number(&word)),
            };
        }
        match word.first().copied() {
            Some(b'<') if word.len() > 1 => self.read_dict(in_array, depth),
            Some(b'[') => {
                if !allow_nested_array && in_array {
                    return Object::Null;
                }
                self.read_array(allow_nested_array, depth)
            }
            _ => self.read_leaf(&word),
        }
    }

    fn read_dict(&mut self, in_array: bool, depth: u32) -> Object {
        let mut dict = Dict::new();
        loop {
            let (key_word, _) = self.scan_word();
            if key_word.is_empty() {
                return Object::Null;
            }
            // `>>` closes it; a lone `>` does not.
            if key_word.len() == 2 && key_word.first() == Some(&b'>') {
                return Object::Dict(dict);
            }
            // Stricter than the file parser: a key that is not a name fails
            // the whole dictionary rather than being skipped.
            if key_word.first() != Some(&b'/') {
                return Object::Null;
            }
            let key = Name::decode(key_word.get(1..).unwrap_or(&[]));
            // Dictionary values may nest arrays, which is the asymmetry that
            // makes `[1 [2] 3]` flatten at operator level but not here.
            let value = self.read(true, in_array, depth + 1);
            // ... and an unparsable value fails the dictionary too.
            if matches!(value, Object::Null) {
                return Object::Null;
            }
            dict.push(key, value);
        }
    }

    fn read_array(&mut self, allow_nested_array: bool, depth: u32) -> Object {
        let mut array = Array::new();
        loop {
            let element = self.read(allow_nested_array, true, depth + 1);
            if !matches!(element, Object::Null) {
                array.push(element);
                continue;
            }
            // A null ends the array only when the word that produced it was
            // empty (end of data) or a closing bracket. Anything else — a
            // refused nested array, chiefly — means "keep spinning", which is
            // what swallows the refused array's elements.
            if self.last_word.is_empty() || self.last_word.first() == Some(&b']') {
                return Object::Array(array);
            }
        }
    }

    fn read_leaf(&mut self, word: &[u8]) -> Object {
        match word.first().copied() {
            Some(b'/') => Object::Name(Name::decode(word.get(1..).unwrap_or(&[]))),
            Some(b'(') => Object::Str(self.lexer.read_literal_string()),
            Some(b'<') => Object::Str(self.lexer.read_hex_string()),
            _ => match word {
                b"false" => Object::Bool(false),
                b"true" => Object::Bool(true),
                _ => Object::Null,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{ContentLexer, Element, MAX_STRING_LEN, MAX_WORD_LEN};
    use pdfrum_object::Object;

    fn elements(src: &[u8]) -> Vec<Element> {
        let mut lexer = ContentLexer::new(src);
        let mut out = Vec::new();
        loop {
            match lexer.next_element() {
                Element::Eof => return out,
                e => out.push(e),
            }
        }
    }

    #[test]
    fn numbers_names_and_keywords_split() {
        let got = elements(b"1 -2.5 /Name Tj");
        assert_eq!(got.len(), 4);
        assert!(matches!(got[0], Element::Number(n) if (n - 1.0).abs() < 1e-6));
        assert!(matches!(got[1], Element::Number(n) if (n + 2.5).abs() < 1e-6));
        assert!(matches!(&got[2], Element::Name(n) if n.as_bytes() == b"Name"));
        assert!(matches!(&got[3], Element::Keyword(k) if &**k == b"Tj"));
    }

    #[test]
    fn comments_are_invisible() {
        let got = elements(b"1 % this is 99 ignored\n 2 add");
        assert_eq!(got.len(), 3);
        assert!(matches!(got[1], Element::Number(n) if (n - 2.0).abs() < 1e-6));
    }

    #[test]
    fn true_false_null_are_objects_not_keywords() {
        let got = elements(b"true false null");
        assert_eq!(
            got,
            vec![
                Element::Object(Object::Bool(true)),
                Element::Object(Object::Bool(false)),
                Element::Object(Object::Null),
            ]
        );
    }

    #[test]
    fn words_truncate_at_255_bytes() {
        let mut src = vec![b'/'];
        src.extend(std::iter::repeat_n(b'a', 300));
        let got = elements(&src);
        let Some(Element::Name(name)) = got.first() else {
            panic!("expected a name, got {got:?}");
        };
        // The solidus occupies one of the 255 kept bytes.
        assert_eq!(name.as_bytes().len(), MAX_WORD_LEN - 1);
    }

    #[test]
    fn strings_truncate_at_32767_bytes() {
        let mut src = vec![b'('];
        src.extend(std::iter::repeat_n(b'x', 40_000));
        src.push(b')');
        let got = elements(&src);
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(s.bytes.len(), MAX_STRING_LEN);
    }

    #[test]
    fn hex_strings_pad_a_trailing_nibble() {
        // The C++ unittest's five cases, restated.
        let got = elements(b"<1A2b>abcd");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(&*s.bytes, &[0x1a, 0x2b]);

        let got = elements(b"<1A2b");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(&*s.bytes, &[0x1a, 0x2b]);

        let got = elements(b"<1A2>asdf");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(&*s.bytes, &[0x1a, 0x20]);

        let got = elements(b"<>");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert!(s.bytes.is_empty());
    }

    #[test]
    fn nested_array_at_operator_level_is_flattened() {
        // A bare `[` at operator level reads with nesting disallowed, so the
        // inner `[` yields nothing, the loop spins forward, and the inner
        // array's *elements* are absorbed into the outer one. The closing
        // `]` of the inner array then terminates the outer, leaving the `3`
        // to be tokenized as ordinary content.
        let got = elements(b"[1 [2] 3]");
        let Some(Element::Object(Object::Array(a))) = got.first() else {
            panic!("expected an array, got {got:?}");
        };
        assert_eq!(a.len(), 2);
        assert_eq!(a.int_at(0), Some(1));
        assert_eq!(a.int_at(1), Some(2));
        // The trailing `3` and `]` come back as separate elements.
        assert!(got.len() > 1);
    }

    #[test]
    fn dict_values_do_nest_arrays() {
        let got = elements(b"<</K [1 [2] 3]>>");
        let Some(Element::Object(Object::Dict(d))) = got.first() else {
            panic!("expected a dict, got {got:?}");
        };
        let arr = d
            .raw(&"K".into())
            .and_then(Object::as_array)
            .expect("array");
        assert_eq!(arr.len(), 3);
        assert!(matches!(arr.raw_at(1), Some(Object::Array(_))));
    }

    #[test]
    fn a_non_name_dict_key_fails_the_whole_dict() {
        let got = elements(b"<</A 1 2 3>>");
        assert_eq!(got.first(), Some(&Element::Object(Object::Null)));
    }

    #[test]
    fn a_closing_delimiter_alone_reads_as_null() {
        for src in [&b"]"[..], b")", b"}", b"{"] {
            let got = elements(src);
            assert_eq!(
                got.first(),
                Some(&Element::Object(Object::Null)),
                "for {src:?}"
            );
        }
    }

    #[test]
    fn literal_string_escapes_and_nesting() {
        let got = elements(br"(a\(b\)c\n\101\\)");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(&*s.bytes, b"a(b)c\nA\\");

        let got = elements(b"(outer (inner) done)");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(&*s.bytes, b"outer (inner) done");
    }

    #[test]
    fn line_continuations_are_dropped() {
        let got = elements(b"(a\\\r\nb)");
        let Some(Element::Object(Object::Str(s))) = got.first() else {
            panic!("expected a string, got {got:?}");
        };
        assert_eq!(&*s.bytes, b"ab");
    }
}
