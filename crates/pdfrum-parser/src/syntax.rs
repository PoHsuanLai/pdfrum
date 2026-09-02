//! The object grammar (ISO 32000-1 §7.3): tokens in, [`Object`]s out.
//!
//! # Two strictnesses, one grammar
//!
//! PDF files disagree with the specification constantly, and a reader that
//! insists on the grammar opens fewer of them. So the grammar runs in two
//! modes. [`Strictness::Loose`] keeps whatever it managed to read — a partial
//! array, a dictionary missing its `>>` — while [`Strictness::Strict`] fails
//! the whole object instead. The choice is not stylistic: the recovery scan
//! parses **strictly**, because there a body that only half-parses is
//! evidence the offset was wrong, while ordinary fetches parse **loosely**,
//! because there the file is all the reader has.
//!
//! Nested values are always parsed loosely regardless of the caller's mode,
//! which is why a strict parse of `<< /A [1 2 >>` still yields a dictionary
//! whose `/A` is the partial array `[1 2]`.
//!
//! # Where `/Length` is a suggestion
//!
//! A stream's declared length is checked, never trusted: the bytes it points
//! at must be followed by `endstream`, and when they are not the reader
//! throws the number away and searches for the keyword instead. That single
//! repair is why so many damaged files still render, and [`read_stream`]
//! implements it in full — including the case where `/Length` is an indirect
//! reference to an object whose own parse needs this stream, which the
//! store's cycle guard turns into a missing length rather than a hang.

use std::sync::Arc;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, name_decode, names,
};

use crate::error::Error;
use crate::lexer::{
    Delim, Lexer, Token, WordBoundary, atoui, find_word, is_line_ending, is_whitespace,
};

/// How much malformed syntax an object parse tolerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Keep what parsed: a partial array, a dictionary that skipped a bad
    /// key. This is how the document's own objects are read.
    Loose,
    /// Fail the object when anything about it is malformed. The recovery
    /// scan uses this, because there a half-parsed body means the offset was
    /// wrong rather than the file being damaged.
    Strict,
}

/// Everything the grammar needs from the rest of the reader.
///
/// Bundled into one record because it threads through every recursive call,
/// and because the store is genuinely optional: parsing an object out of an
/// object stream has no store to chase a `/Length` reference through, and
/// must not pretend otherwise.
pub struct Context<'r, R: Resolve + ?Sized> {
    /// Caps on nesting and word length.
    pub limits: &'r Limits,
    /// Where repairs are recorded.
    pub diags: &'r mut Diagnostics,
    /// The whole file, for cutting stream payloads out of without copying.
    pub file: Option<&'r Arc<[u8]>>,
    /// The object store, used only to resolve an indirect `/Length`.
    pub store: Option<&'r R>,
}

impl<R: Resolve + ?Sized> Context<'_, R> {
    /// Record a repair at `at`.
    fn note(&mut self, severity: Severity, what: DiagKind, at: usize) {
        self.diags.record(severity, what, Some(at as u64));
    }
}

/// Parse one object body at the lexer's position.
///
/// This is the entry point every other layer uses; `depth` is the nesting
/// already spent, which the caller threads so that an object stream's members
/// share the file parse's budget rather than getting a fresh one.
///
/// # Errors
///
/// [`Error::NoObject`] when the bytes are not an object at all — which is
/// also how composite parses learn they have reached their closing bracket —
/// and [`Error::TooDeep`] when this object is itself nested past
/// `limits.max_object_nesting`. A composite *containing* something too deep
/// does not fail: it drops what it could not read.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_object::{NoResolve, Object};
/// use pdfrum_parser::{Lexer, Strictness, parse_object};
///
/// let limits = Limits::default();
/// let mut diags = Diagnostics::default();
/// let mut lx = Lexer::new(b"<< /Type /Page /Count 3 >>");
/// let obj = parse_object(&mut lx, &limits, &mut diags, Strictness::Loose, &NoResolve)?;
/// let dict = obj.as_dict().expect("a dictionary");
/// assert_eq!(dict.int(pdfrum_object::names::COUNT, &NoResolve), Some(3));
/// # Ok::<(), pdfrum_parser::Error>(())
/// ```
pub fn parse_object<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    limits: &Limits,
    diags: &mut Diagnostics,
    strictness: Strictness,
    store: &R,
) -> Result<Object, Error> {
    let mut ctx = Context {
        limits,
        diags,
        file: None,
        store: Some(store),
    };
    body(lx, &mut ctx, strictness, 0)
}

/// Parse one object body, carrying the reader's full context.
///
/// The grammar proper. `depth` counts composite nesting already entered.
pub(crate) fn body<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    ctx: &mut Context<'_, R>,
    strictness: Strictness,
    depth: u32,
) -> Result<Object, Error> {
    // `depth` counts the composites already entered, so the outermost body
    // arrives at zero and a budget of 64 admits exactly 64 nested composites.
    // Note where the refusal lands: a composite too deep to enter reports
    // failure to its *parent*, which treats it as an element that would not
    // parse and closes normally. So a file nested past the cap loses its
    // innermost contents and keeps everything above them.
    if depth >= ctx.limits.max_object_nesting {
        return Err(Error::TooDeep(ctx.limits.max_object_nesting));
    }
    let start = lx.pos();
    let token = lx.next_word(ctx.limits);

    match token {
        Token::Number(word) => number_or_reference(lx, ctx, word),
        Token::Name(payload) => Ok(Object::Name(Name::decode(payload))),
        Token::Keyword(b"true") => Ok(Object::Bool(true)),
        Token::Keyword(b"false") => Ok(Object::Bool(false)),
        Token::Keyword(b"null") => Ok(Object::Null),
        Token::Delim(Delim::StringOpen) => Ok(Object::Str(PdfString::literal(
            lx.read_literal_string().as_ref(),
        ))),
        Token::Delim(Delim::HexOpen) => Ok(Object::Str(PdfString::hex(lx.read_hex_string()))),
        Token::Delim(Delim::ArrayOpen) => array(lx, ctx, strictness, depth),
        Token::Delim(Delim::DictOpen) => dictionary(lx, ctx, strictness, depth),
        // A `>>` belongs to an enclosing dictionary: give it back and fail,
        // which is exactly how a dictionary's value loop learns it is done.
        Token::Delim(Delim::DictClose) => {
            lx.seek(start);
            Err(Error::NoObject(start as u64))
        }
        // The end of the file, and `]`, `endobj`, `}`, junk — for the latter
        // the position stays past the word so the caller can see what
        // stopped it.
        _ => Err(Error::NoObject(start as u64)),
    }
}

/// A number token, which may turn out to be the head of `N G R`.
fn number_or_reference<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    ctx: &mut Context<'_, R>,
    word: &[u8],
) -> Result<Object, Error> {
    let after_number = lx.pos();
    let second = lx.next_word(ctx.limits);
    if second.is_number() {
        let generation = second.bytes();
        if lx.next_word(ctx.limits).is_keyword(b"R") {
            let num = atoui(word);
            let generation = u16::try_from(atoui(generation)).unwrap_or(u16::MAX);
            let reference = ObjRef::new(num, generation);
            // The all-ones object number is the "no object" marker, so a
            // reference spelling it is not an object at all.
            if reference.is_invalid() {
                return Err(Error::NoObject(after_number as u64));
            }
            return Ok(Object::Ref(reference));
        }
    }
    // Not a reference after all: give back everything but the first number.
    lx.seek(after_number);
    Ok(parse_number(word))
}

/// Turn a number-shaped word into an `Int` or a `Real`.
///
/// The token layer already established every byte is in the numeric class,
/// which admits spellings like `--37` and `1.2.3`. A word containing `.`
/// reads as a real, everything else as an integer, and a spelling neither
/// can represent reads as zero rather than failing.
fn parse_number(word: &[u8]) -> Object {
    let text = String::from_utf8_lossy(word);
    if word.contains(&b'.') {
        Object::Real(parse_real(&text))
    } else {
        Object::Int(parse_int(&text))
    }
}

/// Split off at most **one** leading sign.
///
/// Exactly one: a second sign is not a sign but the end of the digits, so
/// `--37` has no digits at all and is worth zero. The token layer admits such
/// spellings because every byte is in the numeric class, and this is where
/// they stop meaning anything.
fn split_sign(text: &str) -> (bool, &str) {
    match text.as_bytes().first() {
        Some(b'-') => (true, text.get(1..).unwrap_or_default()),
        Some(b'+') => (false, text.get(1..).unwrap_or_default()),
        _ => (false, text),
    }
}

/// Read a decimal integer, folding anything unrepresentable to zero.
///
/// The digits accumulate in a **`u32`**, not a wider type, and that width is
/// load-bearing rather than an implementation detail: it is what makes
/// `/P -1` and `/P 4294967295` name the same permission bits, and it is the
/// range [`INT_RANGE`](pdfrum_object::INT_RANGE) promises every
/// [`Object::Int`] a parse can produce will lie in. Two separate ceilings
/// follow from it, and a spelling past either is worth **zero** — not the
/// nearest representable value, which would silently turn an absurd
/// `/Columns 99999999999999999999` into a plausible one:
///
/// - An **unsigned** spelling may reach `u32::MAX`; overflowing the
///   accumulator itself folds to zero.
/// - A **signed** spelling — one that led with `+` or `-` — may only reach
///   `i32::MAX`, or `i32::MAX + 1` when negative so that `-2147483648`
///   spells itself. Past that it folds to zero too, even though the
///   accumulator held the value fine.
fn parse_int(text: &str) -> i64 {
    let signed = matches!(text.as_bytes().first(), Some(b'-' | b'+'));
    let (negative, digits) = split_sign(text);

    // Overflow here is not saturation: a token wider than the accumulator
    // stops meaning anything at all.
    let mut magnitude: Option<u32> = Some(0);
    for byte in digits.bytes().take_while(u8::is_ascii_digit) {
        magnitude = magnitude
            .and_then(|acc| acc.checked_mul(10))
            .and_then(|acc| acc.checked_add(u32::from(byte - b'0')));
    }
    let magnitude = magnitude.unwrap_or(0);

    if !signed {
        return i64::from(magnitude);
    }

    let limit = i64::from(i32::MAX) + i64::from(negative);
    let magnitude = i64::from(magnitude);
    if magnitude > limit {
        return 0;
    }
    if negative { -magnitude } else { magnitude }
}

/// Read a real, keeping only the first decimal point so `1.2.3` is 1.2.
fn parse_real(text: &str) -> f32 {
    let (negative, digits) = split_sign(text);
    let mut cleaned = String::with_capacity(digits.len());
    let mut seen_dot = false;
    for c in digits.chars() {
        match c {
            '.' if seen_dot => break,
            '.' => {
                seen_dot = true;
                cleaned.push('.');
            }
            '0'..='9' => cleaned.push(c),
            _ => break,
        }
    }
    let magnitude = cleaned.parse::<f32>().unwrap_or(0.0);
    if negative { -magnitude } else { magnitude }
}

/// Read array elements until one fails to parse (ISO 32000-1 §7.3.6).
///
/// Termination is by failure, not by looking for `]`: the element parse falls
/// through every grammar case on a `]` and errors, and *that* is the signal.
/// A strict parse then insists the token that stopped it really was `]`;
/// a loose one keeps the elements it has, so an array left open by a
/// truncated file still yields its contents.
fn array<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    ctx: &mut Context<'_, R>,
    strictness: Strictness,
    depth: u32,
) -> Result<Object, Error> {
    let mut out = Array::new();
    loop {
        let before = lx.pos();
        match body(lx, ctx, Strictness::Loose, depth + 1) {
            Ok(Object::Stream(_)) => {
                // A stream may not be an array element; it is dropped and
                // the array carries on (ISO 32000-1 §7.3.8.1).
                ctx.note(
                    Severity::Recovered,
                    DiagKind::StreamInCompositeDropped,
                    before,
                );
            }
            Ok(value) => {
                if out.len() >= ctx.limits.max_array_len {
                    return Ok(Object::Array(out));
                }
                out.push(value);
            }
            // Running out of nesting budget is not special: like any element
            // that will not parse, it ends the array, which then closes with
            // what it has. So a file nested past the cap loses its innermost
            // contents rather than its outermost object.
            Err(_) => {
                // What stopped us? Re-read the token the failed parse
                // consumed, from where it started.
                let mut probe = Lexer::at(lx.bytes(), before);
                let stopper = probe.next_word(ctx.limits);
                let closed = matches!(stopper, Token::Delim(Delim::ArrayClose));
                if !closed {
                    ctx.note(Severity::Suspicious, DiagKind::MalformedArray, before);
                    if strictness == Strictness::Strict {
                        return Err(Error::NoObject(before as u64));
                    }
                }
                return Ok(Object::Array(out));
            }
        }
    }
}

/// Read dictionary entries until `>>`, and then decide whether a stream
/// follows (ISO 32000-1 §7.3.7).
///
/// Three repairs live here, all of them things real files do: an `endobj`
/// where the `>>` should be ends the dictionary rather than eating the rest
/// of the file; a key that is not a name is skipped along with nothing else,
/// so the value that follows becomes the *next* key; and a value that will
/// not parse drops its pair.
fn dictionary<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    ctx: &mut Context<'_, R>,
    strictness: Strictness,
    depth: u32,
) -> Result<Object, Error> {
    let mut dict = Dict::new();
    loop {
        let key_start = lx.pos();
        let token = lx.next_word(ctx.limits);
        match token {
            // End of file inside a dictionary loses the whole thing.
            Token::Eof => return Err(Error::NoObject(key_start as u64)),
            Token::Delim(Delim::DictClose) => break,
            Token::Keyword(b"endobj") => {
                // The `>>` is missing. Hand `endobj` back so the indirect
                // frame above sees it and take what we have.
                ctx.note(Severity::Suspicious, DiagKind::MalformedDict, key_start);
                lx.seek(key_start);
                break;
            }
            Token::Name(payload) => {
                let key = name_decode(payload);
                let value_start = lx.pos();
                match body(lx, ctx, Strictness::Loose, depth + 1) {
                    Ok(Object::Stream(_)) => {
                        ctx.note(
                            Severity::Recovered,
                            DiagKind::StreamInCompositeDropped,
                            value_start,
                        );
                    }
                    Ok(value) => {
                        // A key that decodes to nothing is dropped: the file
                        // said `/` and meant nothing nameable.
                        if key.is_empty() {
                            ctx.note(Severity::Suspicious, DiagKind::MalformedDict, key_start);
                        } else {
                            dict.push(Name::new(key), value);
                        }
                    }
                    // As in an array, exhausting the nesting budget is just
                    // a value that would not parse: the pair is dropped and
                    // the dictionary carries on.
                    Err(_) => {
                        ctx.note(Severity::Suspicious, DiagKind::MalformedDict, value_start);
                        if strictness == Strictness::Strict {
                            lx.to_next_line();
                            return Err(Error::NoObject(value_start as u64));
                        }
                    }
                }
            }
            // Junk between entries: skipped in both modes. A file with a
            // stray keyword in a dictionary is common enough that failing
            // here would cost more than it saves.
            _ => {
                ctx.note(Severity::Suspicious, DiagKind::MalformedDict, key_start);
            }
        }
    }

    // A `stream` keyword now means this dictionary describes one.
    let after_dict = lx.pos();
    if lx.next_word(ctx.limits).is_keyword(b"stream") {
        return read_stream(lx, ctx, dict);
    }
    lx.seek(after_dict);
    Ok(Object::Dict(dict))
}

/// Read a stream's payload, the `stream` keyword already consumed
/// (ISO 32000-1 §7.3.8).
///
/// The declared `/Length` is a hypothesis, tested by looking for `endstream`
/// where it says the data ends. When the test fails — or there was no usable
/// length in the first place — the reader scans forward for the keyword and
/// believes the file's layout over its arithmetic.
pub(crate) fn read_stream<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    ctx: &mut Context<'_, R>,
    dict: Dict,
) -> Result<Object, Error> {
    // `/Length` is one of the few keys read *through* a reference: files
    // routinely put it in a separate object.
    let declared = declared_length(&dict, ctx);

    lx.to_next_line();
    let data_start = lx.pos();
    let file_len = lx.bytes().len();

    let mut length = declared.filter(|&n| {
        // A length running to or past the end of the file is not a length.
        n == 0 || data_start.checked_add(n).is_some_and(|end| end < file_len)
    });

    if let Some(n) = length {
        lx.seek(data_start.saturating_add(n));
        lx.skip_eol_marker();
        let word = lx.next_word(ctx.limits);
        // A prefix compare, not equality: files write `endstreamXYZ`, and
        // PDFium accepts them.
        if !word.bytes().starts_with(b"endstream") {
            ctx.note(Severity::Recovered, DiagKind::LengthMismatch, data_start);
            length = None;
            lx.seek(data_start);
        }
    } else if declared.is_some() {
        ctx.note(Severity::Recovered, DiagKind::LengthMismatch, data_start);
    }

    let length = match length {
        Some(n) => n,
        None => match scan_for_end(lx.bytes(), data_start) {
            Some(end) => {
                ctx.note(Severity::Recovered, DiagKind::KeywordResync, data_start);
                end.saturating_sub(data_start)
            }
            None => return Err(Error::NoObject(data_start as u64)),
        },
    };

    let data_end = data_start.saturating_add(length).min(file_len);
    let data = match ctx.file {
        Some(file) => ByteSpan::new(Arc::clone(file), data_start..data_end)
            .unwrap_or_else(|_| ByteSpan::empty()),
        // Parsing out of a detached buffer (an object stream's decoded
        // bytes) — copy, since there is no shared file to point into.
        None => ByteSpan::from(
            lx.bytes()
                .get(data_start..data_end)
                .unwrap_or_default()
                .to_vec(),
        ),
    };

    lx.seek(data_end);
    resync_after_stream(lx, ctx);
    Ok(Object::Stream(Stream::new(dict, data)))
}

/// Read `/Length`, following one reference if that is what it holds.
fn declared_length<R: Resolve + ?Sized>(dict: &Dict, ctx: &mut Context<'_, R>) -> Option<usize> {
    let raw = dict.raw(names::LENGTH)?;
    let value = match raw {
        Object::Ref(r) => {
            // The store's in-progress guard turns a self-referential
            // `/Length` into a miss rather than a hang.
            let store = ctx.store?;
            store.fetch(*r).ok()?.as_int()?
        }
        other => other.as_number()?.as_int()?,
    };
    usize::try_from(value).ok()
}

/// Find where the payload ends by looking for the keywords that follow it.
///
/// Whichever of `endstream` and `endobj` comes first wins, and the end-of-line
/// bytes immediately before it belong to the file's formatting rather than to
/// the stream, so they are given back.
fn scan_for_end(bytes: &[u8], start: usize) -> Option<usize> {
    let endstream = find_word(bytes, b"endstream", start, WordBoundary::WhitespaceOnly);
    let endobj = find_word(bytes, b"endobj", start, WordBoundary::WhitespaceOnly);
    let keyword = match (endstream, endobj) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let end = trim_trailing_eol(bytes, keyword);
    (end >= start).then_some(end)
}

/// Step back over the one end-of-line sequence before `pos`.
fn trim_trailing_eol(bytes: &[u8], pos: usize) -> usize {
    match bytes.get(pos.wrapping_sub(1)) {
        Some(b'\n') => {
            if bytes.get(pos.wrapping_sub(2)) == Some(&b'\r') {
                pos.saturating_sub(2)
            } else {
                pos.saturating_sub(1)
            }
        }
        Some(b'\r') => pos.saturating_sub(1),
        _ => pos,
    }
}

/// Consume the keyword that follows the payload, unless it turns out to be
/// the `endobj` belonging to the enclosing object.
///
/// A file missing its `endstream` writes `endobj` there instead. Swallowing
/// it would leave the indirect frame above looking for one that is gone, so
/// it is put back.
fn resync_after_stream<R: Resolve + ?Sized>(lx: &mut Lexer<'_>, ctx: &mut Context<'_, R>) {
    let before_keyword = lx.pos();
    let word = lx.next_word(ctx.limits);
    // Exactly `endobj`, not a word starting with it: this is the one place a
    // whole-word match matters, since resyncing on `endobjects` would hand
    // the frame above a keyword that is not there.
    if word.bytes() != b"endobj" {
        return;
    }

    // Spaces and tabs may sit between the keyword and the line ending, and a
    // file that writes them still means the object ended here.
    let mut probe = Lexer::at(lx.bytes(), lx.pos());
    while probe
        .peek_byte()
        .is_some_and(|b| is_whitespace(b) && !is_line_ending(b))
    {
        probe.seek(probe.pos() + 1);
    }
    // A line ending has to follow. At the end of the file there is none, and
    // the keyword stays consumed.
    if probe.skip_eol_marker() > 0 {
        ctx.note(Severity::Recovered, DiagKind::KeywordResync, before_keyword);
        lx.seek(before_keyword);
    }
}

/// One indirect object: its number, generation, and body.
#[derive(Debug, Clone, PartialEq)]
pub struct Indirect {
    /// The object number from the `N G obj` header.
    pub num: u32,
    /// The generation number from the header.
    pub generation: u16,
    /// The parsed body.
    pub object: Object,
}

/// Parse `N G obj … endobj` at the lexer's position.
///
/// The header must be exactly two number tokens and the keyword `obj`;
/// anything else rewinds to where the parse started and fails, which is what
/// lets a caller probe an offset without losing its place.
///
/// # Errors
///
/// [`Error::NoObject`] when the header or body will not parse.
pub fn parse_indirect_object<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    limits: &Limits,
    diags: &mut Diagnostics,
    strictness: Strictness,
    store: &R,
) -> Result<Indirect, Error> {
    let mut ctx = Context {
        limits,
        diags,
        file: None,
        store: Some(store),
    };
    indirect(lx, &mut ctx, strictness, 0)
}

/// Parse an indirect object with the reader's full context.
pub(crate) fn indirect<R: Resolve + ?Sized>(
    lx: &mut Lexer<'_>,
    ctx: &mut Context<'_, R>,
    strictness: Strictness,
    depth: u32,
) -> Result<Indirect, Error> {
    let start = lx.pos();
    let fail = |lx: &mut Lexer<'_>| {
        lx.seek(start);
        Err(Error::NoObject(start as u64))
    };

    let Token::Number(num_word) = lx.next_word(ctx.limits) else {
        return fail(lx);
    };
    let Token::Number(gen_word) = lx.next_word(ctx.limits) else {
        return fail(lx);
    };
    if !lx.next_word(ctx.limits).is_keyword(b"obj") {
        return fail(lx);
    }

    let object = match body(lx, ctx, strictness, depth) {
        Ok(o) => o,
        Err(_) if strictness == Strictness::Loose => Object::Null,
        Err(e) => return Err(e),
    };

    Ok(Indirect {
        num: atoui(num_word),
        generation: u16::try_from(atoui(gen_word)).unwrap_or(u16::MAX),
        object,
    })
}

#[cfg(test)]
mod tests {
    use super::{Context, Strictness, body, indirect, parse_object};
    use crate::error::Error;
    use crate::lexer::Lexer;
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::{NoResolve, ObjRef, Object, Resolve, names};
    use std::sync::Arc;

    fn parse(input: &[u8]) -> Result<Object, Error> {
        let mut diags = Diagnostics::default();
        parse_object(
            &mut Lexer::new(input),
            &Limits::default(),
            &mut diags,
            Strictness::Loose,
            &NoResolve,
        )
    }

    fn parse_strict(input: &[u8]) -> Result<Object, Error> {
        let mut diags = Diagnostics::default();
        parse_object(
            &mut Lexer::new(input),
            &Limits::default(),
            &mut diags,
            Strictness::Strict,
            &NoResolve,
        )
    }

    /// A map-backed store, for the one behavior that needs a resolver here:
    /// a `/Length` that lives in another object.
    struct Store(std::collections::HashMap<u32, Arc<Object>>);

    impl Resolve for Store {
        fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
            self.0
                .get(&r.num)
                .cloned()
                .ok_or(pdfrum_object::Error::UnresolvedRef(r))
        }
    }

    /// Parse against a file-backed context so streams get real spans.
    fn parse_in_file<R: Resolve + ?Sized>(
        input: &[u8],
        store: &R,
        diags: &mut Diagnostics,
    ) -> Result<Object, Error> {
        let file: Arc<[u8]> = Arc::from(input);
        let limits = Limits::default();
        let mut ctx = Context {
            limits: &limits,
            diags,
            file: Some(&file),
            store: Some(store),
        };
        let mut lx = Lexer::new(&file);
        body(&mut lx, &mut ctx, Strictness::Loose, 0)
    }

    #[test]
    fn scalars() {
        assert_eq!(parse(b"true"), Ok(Object::Bool(true)));
        assert_eq!(parse(b"false"), Ok(Object::Bool(false)));
        assert_eq!(parse(b"null"), Ok(Object::Null));
        assert_eq!(parse(b"42"), Ok(Object::Int(42)));
        assert_eq!(parse(b"-17"), Ok(Object::Int(-17)));
        assert_eq!(parse(b"3.5"), Ok(Object::Real(3.5)));
        assert_eq!(parse(b"-0.25"), Ok(Object::Real(-0.25)));
        // Token-level numbers the value parse has to make sense of.
        assert_eq!(parse(b"1.2.3"), Ok(Object::Real(1.2)));
        assert_eq!(parse(b"--37"), Ok(Object::Int(0)));
    }

    /// Every integer a parse can produce lies in `pdfrum_object::INT_RANGE`,
    /// because the accumulator is a `u32` and a spelling that does not fit
    /// one is worth zero.
    ///
    /// Found by the `filters_chain` and `crypt_encrypt_dict` fuzz targets: a
    /// `/Columns 999999999999999999999999` reached `narrow_to_signed32`, whose
    /// `debug_assert!` on that range is the contract this pins. The bug was
    /// an `i64` accumulator that *saturated* — turning an absurd token into
    /// `i64::MAX` rather than into nothing.
    #[test]
    fn integers_outside_the_c_int_range_are_zero() {
        use pdfrum_object::INT_RANGE;

        // An unsigned spelling reaches u32::MAX and stops.
        assert_eq!(parse(b"4294967295"), Ok(Object::Int(4_294_967_295)));
        assert_eq!(parse(b"4294967296"), Ok(Object::Int(0)));
        assert_eq!(parse(b"99999999999999999999999999"), Ok(Object::Int(0)));

        // A signed spelling only reaches i32::MAX...
        assert_eq!(parse(b"+2147483647"), Ok(Object::Int(2_147_483_647)));
        assert_eq!(parse(b"+2147483648"), Ok(Object::Int(0)));
        assert_eq!(parse(b"+4294967295"), Ok(Object::Int(0)));
        // ...except negatively, where i32::MIN must be spellable.
        assert_eq!(parse(b"-2147483648"), Ok(Object::Int(-2_147_483_648)));
        assert_eq!(parse(b"-2147483649"), Ok(Object::Int(0)));
        assert_eq!(parse(b"-99999999999999999999"), Ok(Object::Int(0)));

        // The invariant itself, over every boundary spelling.
        for spelling in [
            &b"0"[..],
            b"-0",
            b"+0",
            b"2147483647",
            b"2147483648",
            b"4294967295",
            b"4294967296",
            b"-2147483648",
            b"-2147483649",
            b"18446744073709551616",
            b"999999999999999999999999999999",
            b"-999999999999999999999999999999",
        ] {
            let Ok(Object::Int(v)) = parse(spelling) else {
                panic!(
                    "{} did not parse as an integer",
                    String::from_utf8_lossy(spelling)
                );
            };
            assert!(
                INT_RANGE.contains(&v),
                "{} parsed to {v}, outside INT_RANGE",
                String::from_utf8_lossy(spelling)
            );
            // The accessor whose debug_assert the fuzzer tripped.
            let _ = pdfrum_object::narrow_to_signed32(v);
        }
    }

    #[test]
    fn names_decode_escapes() {
        assert_eq!(
            parse(b"/A#20B"),
            Ok(Object::Name(pdfrum_object::Name::from("A B")))
        );
        assert_eq!(parse(b"/"), Ok(Object::Name(pdfrum_object::Name::from(""))));
    }

    #[test]
    fn references_and_the_invalid_one() {
        assert_eq!(parse(b"12 0 R"), Ok(Object::Ref(ObjRef::new(12, 0))));
        assert_eq!(parse(b"3 7 R"), Ok(Object::Ref(ObjRef::new(3, 7))));
        // Object number zero is a legal spelling; the fetch is what fails.
        assert_eq!(parse(b"0 0 R"), Ok(Object::Ref(ObjRef::new(0, 0))));
        // The all-ones object number is not an object.
        assert!(parse(b"4294967295 0 R").is_err());
        // Not a reference: only the first number is consumed.
        let mut lx = Lexer::new(b"12 0 X");
        let mut diags = Diagnostics::default();
        let obj = parse_object(
            &mut lx,
            &Limits::default(),
            &mut diags,
            Strictness::Loose,
            &NoResolve,
        );
        assert_eq!(obj, Ok(Object::Int(12)));
        assert_eq!(lx.pos(), 2);
    }

    #[test]
    fn arrays() {
        let obj = parse(b"[1 2 3]").expect("array");
        let array = obj.as_array().expect("array");
        assert_eq!(array.len(), 3);
        assert_eq!(array.int_at(2), Some(3));
        assert!(parse(b"[]").expect("array").as_array().is_some());
    }

    #[test]
    fn a_loose_array_keeps_what_it_read() {
        let mut diags = Diagnostics::default();
        let obj = parse_object(
            &mut Lexer::new(b"[1 2 endobj"),
            &Limits::default(),
            &mut diags,
            Strictness::Loose,
            &NoResolve,
        )
        .expect("partial array");
        assert_eq!(obj.as_array().expect("array").len(), 2);
        assert!(diags.contains(&DiagKind::MalformedArray));
    }

    #[test]
    fn a_strict_array_needs_its_bracket() {
        assert!(parse_strict(b"[1 2 endobj").is_err());
        assert!(parse_strict(b"[1 2]").is_ok());
    }

    #[test]
    fn dictionaries() {
        let obj = parse(b"<< /Type /Page /Count 3 >>").expect("dict");
        let dict = obj.as_dict().expect("dict");
        assert_eq!(dict.name(names::TYPE), Some(names::PAGE));
        assert_eq!(dict.direct_int(names::COUNT), Some(3));
    }

    #[test]
    fn a_later_duplicate_key_wins() {
        let obj = parse(b"<< /Size 3 /Size -1 >>").expect("dict");
        assert_eq!(
            obj.as_dict().expect("dict").direct_int(names::SIZE),
            Some(-1)
        );
    }

    #[test]
    fn endobj_closes_an_unterminated_dictionary() {
        let mut diags = Diagnostics::default();
        let mut lx = Lexer::new(b"<< /A 1 endobj");
        let obj = parse_object(
            &mut lx,
            &Limits::default(),
            &mut diags,
            Strictness::Loose,
            &NoResolve,
        )
        .expect("dict");
        assert_eq!(obj.as_dict().expect("dict").len(), 1);
        assert!(diags.contains(&DiagKind::MalformedDict));
        // `endobj` was put back for the frame above.
        assert_eq!(lx.next_word(&Limits::default()).bytes(), b"endobj");
    }

    #[test]
    fn junk_keys_are_skipped_and_shift_the_pairs() {
        let obj = parse(b"<< junk /A 1 >>").expect("dict");
        let dict = obj.as_dict().expect("dict");
        assert_eq!(dict.len(), 1);
        assert_eq!(dict.direct_int(&pdfrum_object::Name::from("A")), Some(1));
    }

    #[test]
    fn a_bare_slash_key_is_dropped() {
        let obj = parse(b"<< / 1 /A 2 >>").expect("dict");
        assert_eq!(obj.as_dict().expect("dict").len(), 1);
    }

    #[test]
    fn nesting_past_the_budget_loses_the_middle_not_the_object() {
        // Depth of the parsed result, which is what the cap actually bounds.
        fn depth_of(o: &Object) -> usize {
            match o {
                Object::Array(a) => 1 + a.iter().map(depth_of).max().unwrap_or(0),
                _ => 0,
            }
        }
        let nested = |n: usize| -> Vec<u8> {
            let mut v: Vec<u8> = std::iter::repeat_n(b'[', n).collect();
            v.extend(std::iter::repeat_n(b']', n));
            v
        };

        // Everything up to the budget survives intact.
        assert_eq!(depth_of(&parse(&nested(63)).expect("array")), 63);
        assert_eq!(depth_of(&parse(&nested(64)).expect("array")), 64);
        // Past it the object still parses — the arrays too deep to enter
        // simply come back empty, so the damage is innermost, not outermost.
        assert_eq!(depth_of(&parse(&nested(65)).expect("array")), 64);
        assert_eq!(depth_of(&parse(&nested(500)).expect("array")), 64);
    }

    #[test]
    fn a_stream_reads_its_declared_length() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 5 >>\nstream\nHELLO\nendstream\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(!diags.contains(&DiagKind::LengthMismatch));
    }

    #[test]
    fn a_wrong_length_falls_back_to_the_keyword() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 2 >>\nstream\nHELLO\nendstream\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(diags.contains(&DiagKind::LengthMismatch));
        assert!(diags.contains(&DiagKind::KeywordResync));
    }

    #[test]
    fn a_length_past_the_file_falls_back() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 9999 >>\nstream\nHELLO\nendstream\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
    }

    #[test]
    fn endstream_is_matched_by_prefix() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 5 >>\nstream\nHELLO\nendstreamXYZ\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(!diags.contains(&DiagKind::LengthMismatch));
    }

    #[test]
    fn a_space_before_the_newline_still_resyncs() {
        // The keyword standing in for `endstream` may be followed by spaces
        // before its line ending, and it is still the object's end.
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 99 >>\nstream\nHELLO\nendobj  \n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(diags.contains(&DiagKind::KeywordResync));
    }

    #[test]
    fn a_missing_endstream_resyncs_on_endobj() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 99 >>\nstream\nHELLO\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(diags.contains(&DiagKind::KeywordResync));
    }

    #[test]
    fn a_delimiter_disqualifies_an_endstream_match() {
        // `>>endstream` is not a whole-word match under the keyword rule, so
        // the scan keeps going to the real one.
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 999 >>\nstream\nA>>endstream\nB\nendstream\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"A>>endstream\nB");
    }

    #[test]
    fn a_zero_length_stream_is_legal() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 0 >>\nstream\nendstream\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert!(obj.as_stream().expect("stream").data.is_empty());
    }

    #[test]
    fn an_indirect_length_is_chased() {
        let store = Store([(9u32, Arc::new(Object::Int(5)))].into_iter().collect());
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 9 0 R >>\nstream\nHELLO\nendstream\nendobj\n",
            &store,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(!diags.contains(&DiagKind::LengthMismatch));
    }

    #[test]
    fn an_unresolvable_length_falls_back_to_the_scan() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"<< /Length 9 0 R >>\nstream\nHELLO\nendstream\nendobj\n",
            &NoResolve,
            &mut diags,
        )
        .expect("stream");
        assert_eq!(&*obj.as_stream().expect("stream").data, b"HELLO");
        assert!(diags.contains(&DiagKind::KeywordResync));
    }

    #[test]
    fn streams_are_dropped_from_composites() {
        let mut diags = Diagnostics::default();
        let obj = parse_in_file(
            b"[ 1 << /Length 5 >>\nstream\nHELLO\nendstream\n 2 ]",
            &NoResolve,
            &mut diags,
        )
        .expect("array");
        let array = obj.as_array().expect("array");
        assert_eq!(array.len(), 2);
        assert_eq!(array.int_at(0), Some(1));
        assert_eq!(array.int_at(1), Some(2));
        assert!(diags.contains(&DiagKind::StreamInCompositeDropped));
    }

    #[test]
    fn indirect_frames_need_their_header() {
        let file: Arc<[u8]> = Arc::from(&b"7 0 obj << /A 1 >> endobj"[..]);
        let limits = Limits::default();
        let mut diags = Diagnostics::default();
        let mut ctx = Context {
            limits: &limits,
            diags: &mut diags,
            file: Some(&file),
            store: Some(&NoResolve),
        };
        let mut lx = Lexer::new(&file);
        let parsed = indirect(&mut lx, &mut ctx, Strictness::Loose, 0).expect("indirect");
        assert_eq!(parsed.num, 7);
        assert_eq!(parsed.generation, 0);
        assert!(parsed.object.as_dict().is_some());

        // A missing `obj` keyword rewinds.
        let mut lx = Lexer::new(b"7 0 <<>>");
        assert!(
            indirect(
                &mut lx,
                &mut Context {
                    limits: &limits,
                    diags: &mut Diagnostics::default(),
                    file: None,
                    store: Some(&NoResolve),
                },
                Strictness::Loose,
                0,
            )
            .is_err()
        );
        assert_eq!(lx.pos(), 0);
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        let seeds: &[&[u8]] = &[
            b"<<<<<<<<",
            b">>>>>>>>",
            b"[[[[[[[[",
            b"((((((((",
            b"<<<</Length -1>>stream",
            b"0 0 obj<</Length 99999999999999999999>>stream\n",
            b"<</A",
            b"/",
            b"\xff\xfe\x00\x80",
            b"1 0 R 2 0 R",
            b"<</Length 1 0 R>>stream\nx",
        ];
        for seed in seeds {
            let _ = parse(seed);
            let _ = parse_strict(seed);
            let _ = parse_in_file(seed, &NoResolve, &mut Diagnostics::default());
        }
    }
}
