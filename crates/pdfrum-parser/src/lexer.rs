//! Byte classification and tokenization of PDF syntax (ISO 32000-1 §7.2).
//!
//! # The classifier is the file format
//!
//! Almost every syntactic decision a PDF reader makes reduces to "what class
//! is this byte": whitespace separates tokens, delimiters end them and start
//! composite objects, and the numeric class decides whether a word can be a
//! number. The table here is not the one ISO 32000-1 §7.2.2 prints. Two
//! bytes differ, and both differences are load-bearing:
//!
//! - `0x80` and `0xFF` count as **whitespace**. Real files separate tokens
//!   with them, and a reader that treats them as name characters reads
//!   different objects out of the same bytes.
//! - `0x0B` (vertical tab) is **not** whitespace, though the specification
//!   lists no such byte either way. A `0x0B` inside a name stays in the name.
//!
//! # Positions, not slices
//!
//! [`Lexer`] is a cursor: a byte slice plus an offset. It hands out tokens
//! that borrow from those bytes and never copies except where an escape
//! sequence forces it (a literal string with a `\n` in it cannot be a
//! subslice of the file). Everything above this module addresses the file by
//! offset, so seeking backwards to re-read a region is normal and cheap.
//!
//! # Truncation
//!
//! A word longer than [`Limits::max_word_len`] bytes keeps its first
//! `max_word_len` bytes and drops the rest, while still consuming the whole
//! run. A *name* keeps one byte fewer, because the slash it opens with
//! occupies the first byte of the same budget. This is observable: two names
//! agreeing on their first 255 payload bytes are the same name, while two
//! keywords need 256 to collide. Files do not do this on purpose, but
//! fuzzers and damaged files do, and the truncation is what decides whether
//! their dictionaries have one key or two.

use std::borrow::Cow;

use pdfrum_common::Limits;

/// What a byte means to the tokenizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    /// Separates tokens and is otherwise ignored.
    Whitespace,
    /// Can appear in a number: `0`–`9`, `+`, `-`, `.`.
    Numeric,
    /// Ends the current token and begins a new one: `%()/<>[]{}`.
    Delimiter,
    /// Everything else — the body of keywords and names.
    Regular,
}

/// The class of one byte.
///
/// The four groups, spelled out rather than tabulated, so the two bytes that
/// deviate from ISO 32000-1 §7.2.2 sit in plain sight.
///
/// ```
/// use pdfrum_parser::{CharClass, class_of};
///
/// assert_eq!(class_of(b' '), CharClass::Whitespace);
/// // Two bytes the specification does not call whitespace, but files do.
/// assert_eq!(class_of(0x80), CharClass::Whitespace);
/// assert_eq!(class_of(0xFF), CharClass::Whitespace);
/// // And one it arguably should: vertical tab is a regular character.
/// assert_eq!(class_of(0x0B), CharClass::Regular);
/// ```
#[must_use]
pub const fn class_of(byte: u8) -> CharClass {
    match byte {
        // NUL, TAB, LF, FF, CR and SPACE — plus the two high bytes files
        // separate tokens with. Note 0x0B (vertical tab) is deliberately
        // absent, and stays a regular character.
        0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20 | 0x80 | 0xFF => CharClass::Whitespace,
        b'0'..=b'9' | b'+' | b'-' | b'.' => CharClass::Numeric,
        b'%' | b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' => CharClass::Delimiter,
        _ => CharClass::Regular,
    }
}

/// Whether a byte separates tokens.
#[must_use]
pub fn is_whitespace(byte: u8) -> bool {
    class_of(byte) == CharClass::Whitespace
}

/// Whether a byte may appear in a number: `0`–`9`, `+`, `-`, `.`.
#[must_use]
pub fn is_numeric(byte: u8) -> bool {
    class_of(byte) == CharClass::Numeric
}

/// Whether a byte ends a token and starts syntax of its own.
#[must_use]
pub fn is_delimiter(byte: u8) -> bool {
    class_of(byte) == CharClass::Delimiter
}

/// Whether a byte ends a line, for the purposes of `stream` data and
/// `%%EOF` scanning.
#[must_use]
pub fn is_line_ending(byte: u8) -> bool {
    byte == b'\r' || byte == b'\n'
}

/// One syntactic token, borrowing from the file.
///
/// The tokenizer is deliberately shallow: it reports *what shape* a run of
/// bytes has, never what it means. Whether `12` is a length, an object
/// number, or an array element is the grammar's business, so a number arrives
/// as [`Token::Number`] carrying its spelling and the value parse happens one
/// layer up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// A word whose every byte is in the numeric class. Spelling is kept
    /// because `--37`, `1.2.3` and `+-.` all reach here, and only the value
    /// parse decides what they are worth.
    Number(&'a [u8]),
    /// A name, without its leading slash and before escape decoding. An
    /// empty slice is the valid empty name `/`.
    Name(&'a [u8]),
    /// A keyword or any other run of regular bytes: `obj`, `endstream`,
    /// `true`, or junk.
    Keyword(&'a [u8]),
    /// A punctuation token: one delimiter, or the paired `<<` and `>>`.
    Delim(Delim),
    /// The file ended before another token began.
    Eof,
}

impl<'a> Token<'a> {
    /// The token's bytes as the tokenizer stored them, for the callers that
    /// compare against literals. Punctuation answers with its spelling.
    #[must_use]
    pub fn bytes(&self) -> &'a [u8] {
        match self {
            Self::Number(b) | Self::Name(b) | Self::Keyword(b) => b,
            Self::Delim(d) => d.as_bytes(),
            Self::Eof => b"",
        }
    }

    /// Whether this is a number-shaped word.
    #[must_use]
    pub fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }

    /// Whether the file ended.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        matches!(self, Self::Eof)
    }

    /// Whether this is the given keyword.
    #[must_use]
    pub fn is_keyword(&self, word: &[u8]) -> bool {
        matches!(self, Self::Keyword(b) if *b == word)
    }
}

/// Punctuation the tokenizer recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delim {
    /// `[`, opening an array.
    ArrayOpen,
    /// `]`, closing an array.
    ArrayClose,
    /// `<<`, opening a dictionary.
    DictOpen,
    /// `>>`, closing a dictionary.
    DictClose,
    /// `<`, opening a hexadecimal string.
    HexOpen,
    /// `>`, unpaired — syntactically meaningless on its own.
    HexClose,
    /// `(`, opening a literal string.
    StringOpen,
    /// `)`, unpaired.
    StringClose,
    /// `{`, which only PostScript calculator functions use.
    BraceOpen,
    /// `}`.
    BraceClose,
    /// `%`, reachable only when a caller tokenizes without skipping
    /// comments first.
    Percent,
}

impl Delim {
    /// The delimiter's spelling.
    #[must_use]
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::ArrayOpen => b"[",
            Self::ArrayClose => b"]",
            Self::DictOpen => b"<<",
            Self::DictClose => b">>",
            Self::HexOpen => b"<",
            Self::HexClose => b">",
            Self::StringOpen => b"(",
            Self::StringClose => b")",
            Self::BraceOpen => b"{",
            Self::BraceClose => b"}",
            Self::Percent => b"%",
        }
    }
}

/// A cursor over the bytes of a PDF file.
///
/// Offsets are relative to whatever slice the lexer was built over. The
/// document layer passes the file *from its header onwards*, so a lexer
/// position and a cross-reference offset mean the same thing.
///
/// ```
/// use pdfrum_common::Limits;
/// use pdfrum_parser::{Lexer, Token};
///
/// let limits = Limits::default();
/// let mut lx = Lexer::new(b"12 0 obj % a comment\n<< /Type /Page >>");
/// assert_eq!(lx.next_word(&limits), Token::Number(b"12"));
/// assert_eq!(lx.next_word(&limits), Token::Number(b"0"));
/// assert_eq!(lx.next_word(&limits), Token::Keyword(b"obj"));
/// // Comments are invisible everywhere except inside strings.
/// assert!(matches!(lx.next_word(&limits), Token::Delim(_)));
/// assert_eq!(lx.next_word(&limits), Token::Name(b"Type"));
/// ```
#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// A lexer positioned at the start of `bytes`.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// A lexer positioned at `pos`, clamped to the end of the input.
    #[must_use]
    pub fn at(bytes: &'a [u8], pos: usize) -> Self {
        Self {
            bytes,
            pos: pos.min(bytes.len()),
        }
    }

    /// The bytes being read.
    #[must_use]
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The current offset.
    #[must_use]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Move to `pos`, clamped to the end of the input.
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.bytes.len());
    }

    /// Whether the cursor is at or past the end.
    #[must_use]
    pub fn at_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    /// The byte at the cursor, without advancing.
    #[must_use]
    pub fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Read one byte and advance.
    fn read_byte(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.pos).copied()?;
        self.pos += 1;
        Some(b)
    }

    /// Step back one byte, if there is one.
    fn unread(&mut self) {
        self.pos = self.pos.saturating_sub(1);
    }

    /// Skip whitespace and comments, leaving the cursor on the first byte of
    /// the next token (or at the end).
    ///
    /// A `%` runs to the next line ending, and the skipping repeats — so a
    /// block of comment lines costs one call.
    pub fn skip_to_word(&mut self) {
        while let Some(b) = self.peek_byte() {
            if is_whitespace(b) {
                self.pos += 1;
            } else if b == b'%' {
                self.skip_comment();
            } else {
                return;
            }
        }
    }

    /// Consume a `%` comment up to but not including its line ending.
    fn skip_comment(&mut self) {
        while let Some(b) = self.peek_byte() {
            if is_line_ending(b) {
                return;
            }
            self.pos += 1;
        }
    }

    /// Move past the next line ending, so the cursor sits on the first byte
    /// of the following line.
    ///
    /// A `\r\n` pair counts as one ending. This is how stream data finds its
    /// first byte after the `stream` keyword (ISO 32000-1 §7.3.8.1).
    pub fn to_next_line(&mut self) {
        while let Some(b) = self.read_byte() {
            if b == b'\n' {
                return;
            }
            if b == b'\r' {
                if self.peek_byte() == Some(b'\n') {
                    self.pos += 1;
                }
                return;
            }
        }
    }

    /// Consume one end-of-line marker if the cursor is on one, and report how
    /// many bytes it took: two for `\r\n`, one for a lone `\r` or `\n`, zero
    /// for anything else.
    pub fn skip_eol_marker(&mut self) -> usize {
        match self.peek_byte() {
            Some(b'\r') => {
                self.pos += 1;
                if self.peek_byte() == Some(b'\n') {
                    self.pos += 1;
                    2
                } else {
                    1
                }
            }
            Some(b'\n') => {
                self.pos += 1;
                1
            }
            _ => 0,
        }
    }

    /// Read the next token, skipping whitespace and comments first.
    ///
    /// Words longer than `limits.max_word_len` are truncated in the returned
    /// token but consumed whole, so the cursor always lands past the run.
    pub fn next_word(&mut self, limits: &Limits) -> Token<'a> {
        self.skip_to_word();
        let Some(first) = self.read_byte() else {
            return Token::Eof;
        };

        if is_delimiter(first) {
            return self.delimiter_token(first, limits);
        }

        // A regular or numeric run, ending at whitespace or a delimiter —
        // and the terminator is **pushed back either way**, whitespace
        // included. That matters: `stream` is followed by the end-of-line
        // that marks where its data begins, and a reader that swallowed the
        // newline as a separator would start the payload one line late.
        let start = self.pos - 1;
        let mut all_numeric = is_numeric(first);
        while let Some(b) = self.read_byte() {
            if is_whitespace(b) || is_delimiter(b) {
                self.unread();
                break;
            }
            all_numeric &= is_numeric(b);
        }
        let word = truncate(self.bytes.get(start..self.pos).unwrap_or_default(), limits);
        if all_numeric {
            Token::Number(word)
        } else {
            Token::Keyword(word)
        }
    }

    /// Tokenize a delimiter that has already been consumed.
    fn delimiter_token(&mut self, first: u8, limits: &Limits) -> Token<'a> {
        match first {
            // A name runs while the bytes stay regular or numeric; both
            // whitespace and any delimiter stop it, and the delimiter is
            // pushed back.
            b'/' => {
                let start = self.pos;
                while let Some(b) = self.peek_byte() {
                    if matches!(class_of(b), CharClass::Regular | CharClass::Numeric) {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                // A name's budget is one byte smaller than a keyword's,
                // because the slash itself occupies the first byte of it.
                // So two names sharing their first 255 payload bytes are the
                // same name, while two keywords need 256 to collide.
                let payload = self.bytes.get(start..self.pos).unwrap_or_default();
                let budget = limits.max_word_len.saturating_sub(1);
                Token::Name(payload.get(..budget).unwrap_or(payload))
            }
            b'<' => {
                if self.peek_byte() == Some(b'<') {
                    self.pos += 1;
                    Token::Delim(Delim::DictOpen)
                } else {
                    Token::Delim(Delim::HexOpen)
                }
            }
            b'>' => {
                if self.peek_byte() == Some(b'>') {
                    self.pos += 1;
                    Token::Delim(Delim::DictClose)
                } else {
                    Token::Delim(Delim::HexClose)
                }
            }
            b'[' => Token::Delim(Delim::ArrayOpen),
            b']' => Token::Delim(Delim::ArrayClose),
            b'(' => Token::Delim(Delim::StringOpen),
            b')' => Token::Delim(Delim::StringClose),
            b'{' => Token::Delim(Delim::BraceOpen),
            b'}' => Token::Delim(Delim::BraceClose),
            _ => Token::Delim(Delim::Percent),
        }
    }

    /// Read the next token and restore the cursor, so a caller can decide
    /// what to do without committing.
    pub fn peek_word(&mut self, limits: &Limits) -> Token<'a> {
        let saved = self.pos;
        let token = self.next_word(limits);
        self.pos = saved;
        token
    }

    /// Read the body of a literal string, the `(` already consumed
    /// (ISO 32000-1 §7.3.4.2).
    ///
    /// Nested parentheses are kept as content and only an unescaped `)` at
    /// depth zero ends the string. An end of file ends it too, silently, with
    /// whatever was read — the recovery scan depends on that, because it uses
    /// this function to skip over string bodies that may well be truncated.
    ///
    /// The result borrows the file when no escape sequence forced a rewrite.
    pub fn read_literal_string(&mut self) -> Cow<'a, [u8]> {
        let start = self.pos;
        let mut out: Option<Vec<u8>> = None;
        let mut depth: u32 = 0;
        // How many bytes from `start` are still a verbatim prefix of the
        // output; once an escape appears, `out` takes over.
        let mut verbatim_end = start;

        while let Some(b) = self.read_byte() {
            match b {
                b'(' => {
                    depth += 1;
                    push(&mut out, verbatim_end, b);
                    verbatim_end = self.pos;
                }
                b')' => {
                    if depth == 0 {
                        return finish(self.bytes, start, verbatim_end, out);
                    }
                    depth -= 1;
                    push(&mut out, verbatim_end, b);
                    verbatim_end = self.pos;
                }
                b'\\' => {
                    // From here the output can no longer be a subslice.
                    let buf = out.get_or_insert_with(|| {
                        self.bytes
                            .get(start..verbatim_end)
                            .unwrap_or_default()
                            .to_vec()
                    });
                    self.read_escape(buf);
                    verbatim_end = self.pos;
                }
                _ => {
                    push(&mut out, verbatim_end, b);
                    verbatim_end = self.pos;
                }
            }
        }
        finish(self.bytes, start, verbatim_end, out)
    }

    /// Handle one backslash escape, the backslash already consumed.
    fn read_escape(&mut self, out: &mut Vec<u8>) {
        let Some(b) = self.read_byte() else { return };
        match b {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            // A backslash before a line ending is a line continuation: the
            // ending vanishes and nothing is emitted.
            b'\r' => {
                if self.peek_byte() == Some(b'\n') {
                    self.pos += 1;
                }
            }
            b'\n' => {}
            b'0'..=b'7' => {
                // Up to three octal digits, wrapping into one byte, so `\777`
                // is 0xFF rather than an error.
                let mut value: u32 = u32::from(b - b'0');
                for _ in 0..2 {
                    match self.peek_byte() {
                        Some(d @ b'0'..=b'7') => {
                            self.pos += 1;
                            value = value * 8 + u32::from(d - b'0');
                        }
                        _ => break,
                    }
                }
                // The wrap is the behavior: `\777` is one byte, 0xFF.
                out.push(u8::try_from(value & 0xFF).unwrap_or(0));
            }
            // Anything else stands for itself, which is how `\(`, `\)` and
            // `\\` reach the output.
            other => out.push(other),
        }
    }

    /// Read the body of a hexadecimal string, the `<` already consumed
    /// (ISO 32000-1 §7.3.4.3).
    ///
    /// Every byte that is neither a hex digit nor `>` is skipped without
    /// comment — whitespace, NULs, letters, anything. A `>` or the end of
    /// file ends the string, and a dangling half byte is padded with a zero
    /// nibble, so `<1A2` reads as `1A 20`.
    pub fn read_hex_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        while let Some(b) = self.read_byte() {
            if b == b'>' {
                break;
            }
            let Some(nibble) = hex_value(b) else { continue };
            match high.take() {
                None => high = Some(nibble),
                Some(h) => out.push((h << 4) | nibble),
            }
        }
        if let Some(h) = high {
            out.push(h << 4);
        }
        out
    }

    /// Search backwards from the cursor for `word` as a whole word, within
    /// `window` bytes, and leave the cursor on its first byte.
    ///
    /// "Whole word" means the neighbouring bytes are not regular or numeric; a
    /// delimiter beside the word is an acceptable boundary. The cursor's own
    /// byte is inside the search, so a match may end at `pos()` rather than
    /// before it. This is how `startxref` is found in a file whose tail is
    /// otherwise junk.
    // That last byte matters: the caller starts nine bytes from the end of the
    // file, so the position only reachable this way is a `startxref` followed
    // by exactly eight bytes of offset and nothing else. A file truncated with
    // no trailing end-of-line or `%%EOF` has precisely that shape, and it is
    // the shape this search exists to rescue.
    pub fn search_back(&mut self, word: &[u8], window: usize) -> bool {
        if word.is_empty() || self.pos + 1 < word.len() {
            return false;
        }
        let limit = self.pos.saturating_sub(window);
        let mut candidate = (self.pos + 1).saturating_sub(word.len());
        loop {
            if self.bytes.get(candidate..candidate + word.len()) == Some(word)
                && is_whole_word(
                    self.bytes,
                    candidate,
                    word.len(),
                    WordBoundary::WhitespaceOrDelimiter,
                )
            {
                self.pos = candidate;
                return true;
            }
            if candidate == 0 || candidate <= limit {
                return false;
            }
            candidate -= 1;
        }
    }
}

/// Append one verbatim byte only once the output has been materialized.
fn push(out: &mut Option<Vec<u8>>, _verbatim_end: usize, b: u8) {
    if let Some(buf) = out {
        buf.push(b);
    }
}

/// Produce the string body, borrowing when nothing forced a copy.
fn finish(bytes: &[u8], start: usize, verbatim_end: usize, out: Option<Vec<u8>>) -> Cow<'_, [u8]> {
    match out {
        Some(buf) => Cow::Owned(buf),
        None => Cow::Borrowed(bytes.get(start..verbatim_end).unwrap_or_default()),
    }
}

/// Keep at most `limits.max_word_len` bytes of a word.
fn truncate<'a>(word: &'a [u8], limits: &Limits) -> &'a [u8] {
    word.get(..limits.max_word_len).unwrap_or(word)
}

/// The value of one hexadecimal digit.
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Which bytes may neighbour a match for it to stand alone as a word.
///
/// The two rules come from real files and each changes which bytes a damaged
/// document yields, so the choice is the caller's and is named at every call
/// site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBoundary {
    /// Only whitespace. The stricter rule, used when scanning for
    /// `endstream`: `>>endstream` does not count as a match.
    WhitespaceOnly,
    /// Whitespace or a delimiter — anything that is neither
    /// [`CharClass::Regular`] nor [`CharClass::Numeric`]. The looser rule,
    /// used for `startxref`, where the keyword may sit against `>>` or `]`.
    WhitespaceOrDelimiter,
}

impl WordBoundary {
    /// Whether `b` may sit beside a match under this rule.
    #[must_use]
    fn accepts(self, b: u8) -> bool {
        match self {
            Self::WhitespaceOnly => is_whitespace(b),
            Self::WhitespaceOrDelimiter => {
                !matches!(class_of(b), CharClass::Regular | CharClass::Numeric)
            }
        }
    }
}

/// Whether the `len` bytes at `pos` stand alone as a word under `rule`.
#[must_use]
pub fn is_whole_word(bytes: &[u8], pos: usize, len: usize, rule: WordBoundary) -> bool {
    let boundary = |b: u8| rule.accepts(b);
    if pos > 0 && !bytes.get(pos - 1).copied().is_some_and(boundary) {
        return false;
    }
    match bytes.get(pos + len) {
        None => true,
        Some(&b) => boundary(b),
    }
}

/// Find `word` at or after `from`, as a whole word under `rule` (see
/// [`is_whole_word`]). Returns the offset of its first byte.
#[must_use]
pub fn find_word(bytes: &[u8], word: &[u8], from: usize, rule: WordBoundary) -> Option<usize> {
    if word.is_empty() || from > bytes.len() {
        return None;
    }
    let last = bytes.len().checked_sub(word.len())?;
    (from..=last).find(|&i| {
        bytes.get(i..i + word.len()) == Some(word) && is_whole_word(bytes, i, word.len(), rule)
    })
}

/// Parse an unsigned decimal the way the C library's conversion does, which
/// is what every count and offset in a cross-reference table goes through.
///
/// Digits accumulate until a non-digit; overflow saturates at [`u32::MAX`]
/// rather than wrapping; and a leading `-` negates in two's complement, so
/// `-1` reads as `4294967295`. Object-stream offsets in the wild rely on
/// exactly that, which is why this is not `str::parse`.
#[must_use]
pub fn atoui(word: &[u8]) -> u32 {
    let (negative, digits) = match word.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, word),
    };
    let mut value: u32 = 0;
    for &b in digits {
        let Some(d) = (b as char).to_digit(10) else {
            break;
        };
        value = match value.checked_mul(10).and_then(|v| v.checked_add(d)) {
            Some(v) => v,
            None => return u32::MAX,
        };
    }
    if negative {
        (!value).wrapping_add(1)
    } else {
        value
    }
}

/// Parse a signed decimal into an `i64`, saturating rather than wrapping.
///
/// Cross-reference offsets and `startxref` targets come through here, so an
/// absurd offset becomes an out-of-range one rather than a small valid one.
#[must_use]
pub fn atoi64(word: &[u8]) -> i64 {
    let (negative, digits) = match word.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, word),
    };
    let mut value: i64 = 0;
    for &b in digits {
        let Some(d) = (b as char).to_digit(10) else {
            break;
        };
        value = match value
            .checked_mul(10)
            .and_then(|v| v.checked_add(i64::from(d)))
        {
            Some(v) => v,
            None => return if negative { i64::MIN } else { i64::MAX },
        };
    }
    if negative { -value } else { value }
}

#[cfg(test)]
mod tests {
    use super::{
        CharClass, Delim, Lexer, Token, WordBoundary, atoi64, atoui, class_of, find_word,
        is_whole_word,
    };
    use pdfrum_common::Limits;

    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn classifies_the_two_pdfium_quirks() {
        assert_eq!(class_of(0x80), CharClass::Whitespace);
        assert_eq!(class_of(0xFF), CharClass::Whitespace);
        assert_eq!(class_of(0x0B), CharClass::Regular);
        assert_eq!(class_of(b'.'), CharClass::Numeric);
        assert_eq!(class_of(b'%'), CharClass::Delimiter);
    }

    #[test]
    fn high_bytes_separate_words() {
        let bytes = [b'a', 0x80, b'b', 0xFF, b'c'];
        let mut lx = Lexer::new(&bytes);
        assert_eq!(lx.next_word(&limits()), Token::Keyword(b"a"));
        assert_eq!(lx.next_word(&limits()), Token::Keyword(b"b"));
        assert_eq!(lx.next_word(&limits()), Token::Keyword(b"c"));
        assert!(lx.next_word(&limits()).is_eof());
    }

    #[test]
    fn vertical_tab_stays_inside_a_name() {
        let bytes = [b'/', b'a', 0x0B, b'b', b' '];
        let mut lx = Lexer::new(&bytes);
        assert_eq!(lx.next_word(&limits()), Token::Name(&[b'a', 0x0B, b'b']));
    }

    #[test]
    fn number_tokens_are_shape_not_value() {
        let mut lx = Lexer::new(b"--37 1.2.3 +-. 12a");
        assert_eq!(lx.next_word(&limits()), Token::Number(b"--37"));
        assert_eq!(lx.next_word(&limits()), Token::Number(b"1.2.3"));
        assert_eq!(lx.next_word(&limits()), Token::Number(b"+-."));
        assert_eq!(lx.next_word(&limits()), Token::Keyword(b"12a"));
    }

    #[test]
    fn delimiters_pair_and_push_back() {
        let mut lx = Lexer::new(b"<</a[1]>>><");
        assert_eq!(lx.next_word(&limits()), Token::Delim(Delim::DictOpen));
        assert_eq!(lx.next_word(&limits()), Token::Name(b"a"));
        assert_eq!(lx.next_word(&limits()), Token::Delim(Delim::ArrayOpen));
        assert_eq!(lx.next_word(&limits()), Token::Number(b"1"));
        assert_eq!(lx.next_word(&limits()), Token::Delim(Delim::ArrayClose));
        assert_eq!(lx.next_word(&limits()), Token::Delim(Delim::DictClose));
        assert_eq!(lx.next_word(&limits()), Token::Delim(Delim::HexClose));
        assert_eq!(lx.next_word(&limits()), Token::Delim(Delim::HexOpen));
    }

    #[test]
    fn a_bare_slash_is_the_empty_name() {
        let mut lx = Lexer::new(b"/ /Name/Other");
        assert_eq!(lx.next_word(&limits()), Token::Name(b""));
        assert_eq!(lx.next_word(&limits()), Token::Name(b"Name"));
        assert_eq!(lx.next_word(&limits()), Token::Name(b"Other"));
    }

    #[test]
    fn comments_vanish() {
        let mut lx = Lexer::new(b"% one\n%two\n  42");
        assert_eq!(lx.next_word(&limits()), Token::Number(b"42"));
    }

    #[test]
    fn two_long_names_collide_one_byte_sooner_than_keywords() {
        // 255 shared payload bytes make two names equal; keywords need 256.
        let name = |tail: u8| {
            let mut v = vec![b'/'];
            v.extend(std::iter::repeat_n(b'a', 255));
            v.push(tail);
            v.push(b' ');
            v
        };
        let (x, y) = (name(b'x'), name(b'y'));
        assert_eq!(
            Lexer::new(&x).next_word(&limits()),
            Lexer::new(&y).next_word(&limits())
        );
    }

    #[test]
    fn words_truncate_at_the_limit() {
        let long = vec![b'a'; 300];
        let mut source = long.clone();
        source.push(b' ');
        source.push(b'z');
        let mut lx = Lexer::new(&source);
        let token = lx.next_word(&limits());
        assert_eq!(token.bytes().len(), 256);
        // The whole run was still consumed.
        assert_eq!(lx.next_word(&limits()), Token::Keyword(b"z"));
    }

    #[test]
    fn names_truncate_one_byte_sooner_than_keywords() {
        // The slash occupies the first byte of a name's budget, so its
        // payload gets 255 where a keyword gets 256.
        let mut source = vec![b'/'];
        source.extend(std::iter::repeat_n(b'x', 300));
        let mut lx = Lexer::new(&source);
        assert_eq!(lx.next_word(&limits()).bytes().len(), 255);
    }

    #[test]
    fn peek_is_position_neutral() {
        let mut lx = Lexer::new(b"  hello world");
        let before = lx.pos();
        assert_eq!(lx.peek_word(&limits()), Token::Keyword(b"hello"));
        assert_eq!(lx.pos(), before);
        assert_eq!(lx.next_word(&limits()), Token::Keyword(b"hello"));
    }

    #[test]
    fn literal_string_escapes() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"abc)", b"abc"),
            (b"a(b)c)", b"a(b)c"),
            (b"\\n\\r\\t\\b\\f)", b"\n\r\t\x08\x0C"),
            (b"\\101)", b"A"),
            (b"\\777)", b"\xFF"),
            (b"\\(\\)\\\\)", b"()\\"),
            (b"a\\\nb)", b"ab"),
            (b"a\\\r\nb)", b"ab"),
            (b"a\\\rb)", b"ab"),
            (b"\\q)", b"q"),
            // End of file ends the string with what was read.
            (b"abc", b"abc"),
        ];
        for (input, expected) in cases {
            let mut lx = Lexer::new(input);
            assert_eq!(&*lx.read_literal_string(), *expected, "input {input:?}");
        }
    }

    #[test]
    fn literal_string_borrows_when_it_can() {
        let mut lx = Lexer::new(b"plain)");
        assert!(matches!(
            lx.read_literal_string(),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn hex_string_skips_everything_it_does_not_understand() {
        // The assertion goldens from the C++ syntax-parser tests.
        let cases: &[(&[u8], &[u8], usize)] = &[
            (b"1A2b>abcd", b"\x1a\x2b", 5),
            (b"1A2>abcd", b"\x1a\x20", 4),
            (b"z12b>abcd", b"\x12\xb0", 5),
            (b"*<&*#$^&@1>abcd", b"\x10", 11),
            (b"\x00z12b>", b"\x12\xb0", 6),
            (b"12&%^*b>", b"\x12\xb0", 8),
            (b"1A2b", b"\x1a\x2b", 4),
            (b"1A2", b"\x1a\x20", 3),
            (b"", b"", 0),
            (b">", b"", 1),
        ];
        for (input, expected, end) in cases {
            let mut lx = Lexer::new(input);
            assert_eq!(&lx.read_hex_string(), expected, "input {input:?}");
            assert_eq!(lx.pos(), *end, "end position for {input:?}");
        }
    }

    #[test]
    fn to_next_line_treats_crlf_as_one() {
        let mut lx = Lexer::new(b"abc\r\ndef");
        lx.to_next_line();
        assert_eq!(lx.pos(), 5);
        let mut lx = Lexer::new(b"abc\rdef");
        lx.to_next_line();
        assert_eq!(lx.pos(), 4);
        let mut lx = Lexer::new(b"abc\ndef");
        lx.to_next_line();
        assert_eq!(lx.pos(), 4);
        // No line ending at all leaves the cursor at the end.
        let mut lx = Lexer::new(b"abc");
        lx.to_next_line();
        assert_eq!(lx.pos(), 3);
    }

    #[test]
    fn eol_markers_count_their_bytes() {
        assert_eq!(Lexer::new(b"\r\nx").skip_eol_marker(), 2);
        assert_eq!(Lexer::new(b"\rx").skip_eol_marker(), 1);
        assert_eq!(Lexer::new(b"\nx").skip_eol_marker(), 1);
        assert_eq!(Lexer::new(b"x").skip_eol_marker(), 0);
    }

    #[test]
    fn whole_word_boundaries_differ_by_strictness() {
        let bytes = b">>endstream ";
        // Under the keyword rule a delimiter is not a boundary.
        assert!(!is_whole_word(bytes, 2, 9, WordBoundary::WhitespaceOnly));
        // Under the loose rule it is.
        assert!(is_whole_word(
            bytes,
            2,
            9,
            WordBoundary::WhitespaceOrDelimiter
        ));
    }

    #[test]
    fn find_word_respects_the_keyword_rule() {
        let bytes = b"x >>endstream y endstream z";
        assert_eq!(
            find_word(bytes, b"endstream", 0, WordBoundary::WhitespaceOnly),
            Some(16)
        );
        assert_eq!(
            find_word(bytes, b"endstream", 0, WordBoundary::WhitespaceOrDelimiter),
            Some(4)
        );
        assert_eq!(
            find_word(bytes, b"nothere", 0, WordBoundary::WhitespaceOnly),
            None
        );
    }

    #[test]
    fn search_back_finds_the_last_occurrence() {
        let bytes = b"startxref 1\nstartxref 2\n";
        let mut lx = Lexer::at(bytes, bytes.len());
        assert!(lx.search_back(b"startxref", 4096));
        assert_eq!(lx.pos(), 12);
    }

    #[test]
    fn search_back_includes_the_byte_under_the_cursor() {
        // The word ends exactly *at* the cursor rather than before it. The
        // reader starts nine bytes from the end of the file, so reaching this
        // position means a `startxref` followed by a separator and a
        // seven-digit offset and nothing else — a file truncated with no
        // trailing end-of-line or `%%EOF`. No well-formed file lands here,
        // which is why only this test holds the boundary.
        let file = b"%PDF-1.7\nstartxref 1234567";
        let start = 9;
        let cursor = file.len() - 9;
        assert_eq!(file.get(start..start + 9), Some(&b"startxref"[..]));
        // The keyword's last byte *is* the cursor's byte.
        assert_eq!(start + 8, cursor);

        let mut lx = Lexer::at(file, cursor);
        assert!(lx.search_back(b"startxref", 4096));
        assert_eq!(lx.pos(), start);
    }

    #[test]
    fn search_back_declines_a_word_that_does_not_fit() {
        let mut lx = Lexer::at(b"xref", 1);
        assert!(!lx.search_back(b"startxref", 4096));
        assert_eq!(lx.pos(), 1);
        // A word exactly as long as the span up to and including the cursor.
        let mut lx = Lexer::at(b"abc", 2);
        assert!(lx.search_back(b"abc", 4096));
        assert_eq!(lx.pos(), 0);
    }

    #[test]
    fn atoui_saturates_and_negates() {
        assert_eq!(atoui(b"0"), 0);
        assert_eq!(atoui(b"42"), 42);
        assert_eq!(atoui(b"4294967295"), u32::MAX);
        assert_eq!(atoui(b"99999999999"), u32::MAX);
        assert_eq!(atoui(b"-1"), u32::MAX);
        assert_eq!(atoui(b"-2"), u32::MAX - 1);
        assert_eq!(atoui(b"12a34"), 12);
        assert_eq!(atoui(b""), 0);
    }

    #[test]
    fn atoi64_saturates() {
        assert_eq!(atoi64(b"-5"), -5);
        assert_eq!(atoi64(b"100940"), 100_940);
        assert_eq!(atoi64(b"999999999999999999999"), i64::MAX);
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        for seed in 0u8..=255 {
            let bytes: Vec<u8> = (0..64u8)
                .map(|i| i.wrapping_mul(7).wrapping_add(seed))
                .collect();
            let mut lx = Lexer::new(&bytes);
            for _ in 0..200 {
                if lx.next_word(&limits()).is_eof() {
                    break;
                }
            }
        }
    }
}
