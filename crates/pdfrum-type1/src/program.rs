//! Reading a tokenized font program into the fields the rest of the crate
//! needs.
//!
//! Every reader here is a pattern matcher over [`Lexer`] output, never an
//! evaluator: it scans for a `/Key` it recognises and then reads the shape the
//! Type 1 specification fixes for that key. Anything it does not recognise —
//! `/OtherSubrs`, the `RD`/`ND`/`NP` procedure definitions, `/Blend`'s nested
//! private dictionary, the PostScript ceremony around `eexec` — flows past
//! untouched, which is why a font can carry arbitrary extra keys without
//! upsetting the reader.
//!
//! Both halves of the program go through the same reader with different key
//! sets: the cleartext half carries the font's identity, matrix, encoding and
//! Multiple-Master declaration, and the decrypted half carries `/Subrs`,
//! `/CharStrings` and `/lenIV`.

use crate::blend::{AxisKind, Blend, DesignMap};
use crate::eexec;
use crate::encoding::{self, Encoding};
use crate::postscript::{Lexer, Token};
use pdfrum_common::kurbo::{Affine, Rect};

/// The cleartext half's contents.
#[derive(Debug, Clone, Default)]
pub(crate) struct Header {
    pub font_name: Option<Box<str>>,
    pub full_name: Option<Box<str>>,
    pub family_name: Option<Box<str>>,
    pub weight_name: Option<Box<str>>,
    pub italic_angle: f32,
    pub is_fixed_pitch: bool,
    pub font_matrix: Option<Affine>,
    pub font_bbox: Option<Rect>,
    pub encoding: Option<Encoding>,
    /// The four Multiple-Master declarations, each present only for an MM font.
    pub axis_kinds: Vec<AxisKind>,
    pub design_positions: Vec<Vec<f32>>,
    pub design_maps: Vec<DesignMap>,
    pub weight_vector: Vec<f32>,
}

/// The decrypted half's contents.
#[derive(Debug, Clone, Default)]
pub(crate) struct Private {
    /// Subroutines in index order, already decrypted past `lenIV`.
    pub subrs: Vec<Vec<u8>>,
    /// `(glyph name, decrypted charstring)` in the order the dictionary
    /// declared them — which is the glyph order the rest of the crate uses.
    pub charstrings: Vec<(Box<str>, Vec<u8>)>,
}

/// Read the cleartext preamble.
///
/// **First declaration wins, and an empty reading never displaces a good one.**
/// Both rules are load-bearing rather than defensive: a Multiple-Master font's
/// preamble ends with a `makeblendedfont` procedure — PostScript source shipped
/// for a real interpreter's benefit — whose body mentions `/BlendAxisTypes`,
/// `/FontName` and `/WeightVector` as *operands*, not definitions. A reader
/// that took the last occurrence would parse the real declarations correctly
/// and then wipe them, which is exactly what the two Foxit fallback faces would
/// do to a naive implementation. Recognising the procedure would mean
/// interpreting PostScript; taking the first definition is what every real
/// reader does instead.
pub(crate) fn read_header(bytes: &[u8]) -> Header {
    let mut h = Header::default();
    let mut lx = Lexer::new(bytes);
    let mut seen_italic = false;
    let mut seen_pitch = false;
    while let Some(tok) = lx.next() {
        let Token::Literal(key) = tok else { continue };
        match key {
            b"FontName" => take_first(&mut h.font_name, next_name(&mut lx)),
            b"FullName" => take_first(&mut h.full_name, next_string(&mut lx)),
            b"FamilyName" => take_first(&mut h.family_name, next_string(&mut lx)),
            b"Weight" => {
                // `/Weight (Bold)` in `/FontInfo`, but an MM font also names
                // `/Weight` as a blend axis type. Only a string is the name,
                // and `next_string` yields `None` for the axis spelling.
                take_first(&mut h.weight_name, next_string(&mut lx));
            }
            b"ItalicAngle" => {
                if let (false, Some(v)) = (seen_italic, next_number(&mut lx)) {
                    h.italic_angle = v as f32;
                    seen_italic = true;
                }
            }
            b"isFixedPitch" => {
                if let (false, Some(v)) = (seen_pitch, next_bool(&mut lx)) {
                    h.is_fixed_pitch = v;
                    seen_pitch = true;
                }
            }
            b"FontMatrix" => take_first(&mut h.font_matrix, read_matrix(&mut lx)),
            b"FontBBox" => take_first(&mut h.font_bbox, read_bbox(&mut lx)),
            b"Encoding" => take_first(&mut h.encoding, read_encoding(&mut lx)),
            b"BlendAxisTypes" => take_first_vec(&mut h.axis_kinds, read_axis_types(&mut lx)),
            b"BlendDesignPositions" => {
                take_first_vec(&mut h.design_positions, read_number_matrix(&mut lx));
            }
            b"BlendDesignMap" => take_first_vec(&mut h.design_maps, read_design_maps(&mut lx)),
            b"WeightVector" => take_first_vec(&mut h.weight_vector, read_number_array(&mut lx)),
            _ => {}
        }
    }
    h
}

/// Fill a slot only if it is still empty and the new reading produced
/// something.
fn take_first<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() && value.is_some() {
        *slot = value;
    }
}

/// The list-valued counterpart: an empty parse never displaces a filled slot.
fn take_first_vec<T>(slot: &mut Vec<T>, value: Vec<T>) {
    if slot.is_empty() && !value.is_empty() {
        *slot = value;
    }
}

/// Read the decrypted private dictionary.
///
/// `/lenIV` is honoured wherever it appears, including after `/Subrs` — a font
/// that declares it late is malformed, but re-reading with the right skip
/// costs one pass and salvages the glyphs.
pub(crate) fn read_private(bytes: &[u8]) -> Private {
    let len_iv = scan_len_iv(bytes).unwrap_or(eexec::DEFAULT_LEN_IV);
    let skip = eexec::len_iv_skip(len_iv);
    let mut p = Private::default();
    let mut lx = Lexer::new(bytes);
    while let Some(tok) = lx.next() {
        // First declaration wins here too: a Multiple-Master private
        // dictionary carries a nested `/Blend /Private` that restates
        // several keys as per-master arrays.
        match tok {
            Token::Literal(b"Subrs") => take_first_vec(&mut p.subrs, read_subrs(&mut lx, skip)),
            Token::Literal(b"CharStrings") => {
                take_first_vec(&mut p.charstrings, read_charstrings(&mut lx, skip));
            }
            _ => {}
        }
    }
    p
}

/// A cheap pre-pass for `/lenIV`, which governs how the payloads that come
/// later are decrypted and so must be known before them.
fn scan_len_iv(bytes: &[u8]) -> Option<i32> {
    let mut lx = Lexer::new(bytes);
    while let Some(tok) = lx.next() {
        if tok == Token::Literal(b"lenIV")
            && let Some(Token::Number(v)) = lx.next()
        {
            return Some(v as i32);
        }
    }
    None
}

/// A declared element count, capped so a hostile `/CharStrings 4294967295 dict`
/// cannot make us reserve gigabytes before reading a single glyph. The cap is
/// one past the largest glyph index our `Gid` can name.
fn declared_count(n: f64) -> usize {
    payload_len(n).map_or(0, |n| n.min(65_536))
}

/// A non-negative, finite, in-range length from a PostScript number. Anything
/// else — a negative, a NaN, a value past `usize` — yields `None`, which the
/// callers turn into "stop reading here".
fn payload_len(n: f64) -> Option<usize> {
    if !n.is_finite() || !(0.0..=4_294_967_295.0).contains(&n) {
        return None;
    }
    // Sign and range are both checked above, so the cast is exact.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Some(n as usize)
}

/// `/Subrs <n> array` then `dup <i> <len> RD <bytes> NP` repeated.
fn read_subrs(lx: &mut Lexer<'_>, skip: usize) -> Vec<Vec<u8>> {
    let count = match lx.next() {
        Some(Token::Number(n)) => declared_count(n),
        _ => return Vec::new(),
    };
    let mut subrs = vec![Vec::new(); count];
    let mut seen = 0usize;
    // Cloning gives a cursor we can abandon if this turns out not to be a
    // subroutine array after all.
    let mut cursor = lx.clone();
    while seen < count {
        // Look for `dup <index> <length> RD`.
        let Some(tok) = cursor.next() else { break };
        match tok {
            Token::Keyword(b"dup") => {}
            // `/CharStrings` starting means the Subrs array ended early.
            Token::Literal(b"CharStrings") => break,
            _ => continue,
        }
        let (Some(Token::Number(index)), Some(Token::Number(length))) =
            (cursor.next(), cursor.next())
        else {
            continue;
        };
        // The token after the length is the font's own `RD` alias, whatever it
        // is spelled: `RD`, `-|`, or something else the font defined.
        let Some(Token::Keyword(_)) = cursor.next() else {
            continue;
        };
        let Some(payload) = payload_len(length).and_then(|n| cursor.take_binary(n)) else {
            break;
        };
        if let Some(slot) = payload_len(index).and_then(|i| subrs.get_mut(i)) {
            *slot = eexec::decrypt(payload, eexec::CHARSTRING_SEED, skip);
        }
        seen = seen.saturating_add(1);
    }
    *lx = cursor;
    subrs
}

/// `/CharStrings <n> dict dup begin` then `/<name> <len> RD <bytes> ND`
/// repeated, ending at `end`.
fn read_charstrings(lx: &mut Lexer<'_>, skip: usize) -> Vec<(Box<str>, Vec<u8>)> {
    let capacity = match lx.next() {
        Some(Token::Number(n)) => declared_count(n),
        _ => 0,
    };
    let mut out = Vec::with_capacity(capacity);
    while let Some(tok) = lx.next() {
        match tok {
            Token::Keyword(b"end") => break,
            Token::Literal(name) => {
                let Some(Token::Number(length)) = lx.next() else {
                    continue;
                };
                let Some(Token::Keyword(_)) = lx.next() else {
                    continue;
                };
                let Some(payload) = payload_len(length).and_then(|n| lx.take_binary(n)) else {
                    break;
                };
                if let Ok(name) = core::str::from_utf8(name) {
                    out.push((
                        Box::from(name),
                        eexec::decrypt(payload, eexec::CHARSTRING_SEED, skip),
                    ));
                }
            }
            _ => {}
        }
    }
    out
}

/// `/Encoding StandardEncoding def` or a built `dup <code> /<name> put` run.
fn read_encoding(lx: &mut Lexer<'_>) -> Option<Encoding> {
    let mut cursor = lx.clone();
    match cursor.next()? {
        Token::Keyword(b"StandardEncoding") => {
            *lx = cursor;
            Some(Encoding::Standard)
        }
        Token::Keyword(b"ExpertEncoding") => {
            *lx = cursor;
            Some(Encoding::Expert)
        }
        Token::Keyword(b"ISOLatin1Encoding") => {
            *lx = cursor;
            Some(Encoding::IsoLatin1)
        }
        // `256 array` — the built form. Collect `dup <n> /<name> put` until
        // `readonly def` or `def`.
        Token::Number(_) => {
            let mut pairs: Vec<(u8, &[u8])> = Vec::new();
            let mut ended = false;
            // Not `for tok in cursor.by_ref()`: the `dup` arm reads two more
            // tokens of its own, which a borrowing `for` would forbid.
            while let Some(tok) = cursor.next() {
                match tok {
                    Token::Keyword(b"dup") => {
                        let (Some(Token::Number(code)), Some(Token::Literal(name))) =
                            (cursor.next(), cursor.next())
                        else {
                            continue;
                        };
                        if let Ok(code) = u8::try_from(code as i64) {
                            pairs.push((code, name));
                        }
                    }
                    // `readonly def` (or a bare `def`) ends the vector. A `def`
                    // inside the initialising `{…} for` loop cannot appear, so
                    // this is unambiguous.
                    Token::Keyword(b"def") => {
                        ended = true;
                        break;
                    }
                    // The private section starting means the encoding never
                    // terminated.
                    Token::Keyword(b"eexec") | Token::Literal(b"Private") => break,
                    _ => {}
                }
            }
            if ended {
                *lx = cursor;
            }
            Some(encoding::custom_from_pairs(pairs))
        }
        _ => None,
    }
}

fn read_matrix(lx: &mut Lexer<'_>) -> Option<Affine> {
    let v = read_number_array(lx);
    let c = |i: usize| -> Option<f64> { Some(f64::from(v.get(i).copied()?)) };
    Some(Affine::new([c(0)?, c(1)?, c(2)?, c(3)?, c(4)?, c(5)?]))
}

fn read_bbox(lx: &mut Lexer<'_>) -> Option<Rect> {
    let v = read_number_array(lx);
    let c = |i: usize| -> Option<f64> { Some(f64::from(v.get(i).copied()?)) };
    Some(Rect::new(c(0)?, c(1)?, c(2)?, c(3)?))
}

/// Read a bracketed number list. Accepts `[ … ]` and `{ … }`, because
/// `/FontBBox {…}` and `/FontMatrix […]` are both written by real fonts, and
/// tolerates a missing opening bracket.
fn read_number_array(lx: &mut Lexer<'_>) -> Vec<f32> {
    let mut cursor = lx.clone();
    let mut out = Vec::new();
    let mut opened = false;
    for tok in cursor.by_ref() {
        match tok {
            Token::Delim(b'[' | b'{') if !opened => opened = true,
            Token::Number(n) => out.push(n as f32),
            // A closing bracket ends the array; anything else — a keyword
            // where a number belonged, a nested delimiter — means this was
            // not the array we thought, and whatever was read stands.
            _ => break,
        }
    }
    *lx = cursor;
    out
}

/// `[[a b][c d]…]` — used by `/BlendDesignPositions`.
fn read_number_matrix(lx: &mut Lexer<'_>) -> Vec<Vec<f32>> {
    let mut cursor = lx.clone();
    let mut rows = Vec::new();
    let mut row: Option<Vec<f32>> = None;
    let mut depth = 0u32;
    for tok in cursor.by_ref() {
        match tok {
            Token::Delim(b'[') => {
                depth = depth.saturating_add(1);
                if depth == 2 {
                    row = Some(Vec::new());
                }
            }
            Token::Delim(b']') => {
                if depth == 2
                    && let Some(r) = row.take()
                {
                    rows.push(r);
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Token::Number(n) => {
                if let Some(r) = row.as_mut() {
                    r.push(n as f32);
                }
            }
            _ => break,
        }
    }
    *lx = cursor;
    rows
}

/// `[[[d n][d n]…] [[d n]…]]` — one design map per axis, each a knot list.
fn read_design_maps(lx: &mut Lexer<'_>) -> Vec<DesignMap> {
    let mut cursor = lx.clone();
    let mut maps = Vec::new();
    let mut knots: Vec<(f32, f32)> = Vec::new();
    let mut pair: Vec<f32> = Vec::new();
    let mut depth = 0u32;
    for tok in cursor.by_ref() {
        match tok {
            Token::Delim(b'[') => {
                depth = depth.saturating_add(1);
                match depth {
                    2 => knots.clear(),
                    3 => pair.clear(),
                    _ => {}
                }
            }
            Token::Delim(b']') => {
                match depth {
                    3 => {
                        if let (Some(&d), Some(&n)) = (pair.first(), pair.get(1)) {
                            knots.push((d, n));
                        }
                    }
                    2 => maps.push(DesignMap {
                        knots: core::mem::take(&mut knots),
                    }),
                    _ => {}
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Token::Number(n) => {
                if depth == 3 {
                    pair.push(n as f32);
                }
            }
            _ => break,
        }
    }
    *lx = cursor;
    maps
}

/// `[/Weight /Width]`.
fn read_axis_types(lx: &mut Lexer<'_>) -> Vec<AxisKind> {
    let mut cursor = lx.clone();
    let mut out = Vec::new();
    let mut opened = false;
    for tok in cursor.by_ref() {
        match tok {
            Token::Delim(b'[') if !opened => opened = true,
            Token::Literal(name) => out.push(match name {
                b"Weight" => AxisKind::Weight,
                b"Width" => AxisKind::Width,
                other => AxisKind::Other(core::str::from_utf8(other).unwrap_or("?").into()),
            }),
            _ => break,
        }
    }
    *lx = cursor;
    out
}

fn next_name(lx: &mut Lexer<'_>) -> Option<Box<str>> {
    match lx.next()? {
        Token::Literal(n) => core::str::from_utf8(n).ok().map(Box::from),
        _ => None,
    }
}

fn next_string(lx: &mut Lexer<'_>) -> Option<Box<str>> {
    match lx.next()? {
        Token::Str(s) => Some(String::from_utf8_lossy(s).trim().into()),
        _ => None,
    }
}

fn next_number(lx: &mut Lexer<'_>) -> Option<f64> {
    match lx.next()? {
        Token::Number(n) => Some(n),
        _ => None,
    }
}

fn next_bool(lx: &mut Lexer<'_>) -> Option<bool> {
    match lx.next()? {
        Token::Keyword(b"true") => Some(true),
        Token::Keyword(b"false") => Some(false),
        _ => None,
    }
}

/// Turn a header's four Multiple-Master declarations into a [`Blend`], or
/// nothing when the font is not variable.
pub(crate) fn build_blend(h: &Header, diags: &mut pdfrum_common::Diagnostics) -> Option<Blend> {
    if h.weight_vector.is_empty() && h.axis_kinds.is_empty() {
        return None; // Not an MM font; nothing to complain about.
    }
    Blend::new(
        h.axis_kinds.clone(),
        h.design_positions.clone(),
        h.design_maps.clone(),
        h.weight_vector.clone(),
        diags,
    )
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]
mod tests {
    use super::{Header, read_header, read_private};
    use crate::blend::AxisKind;
    use crate::eexec;
    use crate::encoding::Encoding;

    #[test]
    fn header_identity_and_geometry() {
        let h = read_header(
            b"%!PS-AdobeFont-1.0: Demo\n\
              /FontInfo 4 dict dup begin\n\
              /FullName (Demo Regular) readonly def\n\
              /FamilyName (Demo) readonly def\n\
              /Weight (Medium) readonly def\n\
              /ItalicAngle -12.5 def\n\
              /isFixedPitch true def\n\
              end readonly def\n\
              /FontName /DemoRegular def\n\
              /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n\
              /FontBBox {-100 -200 900 800} readonly def\n",
        );
        assert_eq!(h.font_name.as_deref(), Some("DemoRegular"));
        assert_eq!(h.full_name.as_deref(), Some("Demo Regular"));
        assert_eq!(h.family_name.as_deref(), Some("Demo"));
        assert_eq!(h.weight_name.as_deref(), Some("Medium"));
        assert!((h.italic_angle - -12.5).abs() < f32::EPSILON);
        assert!(h.is_fixed_pitch);
        let m = h.font_matrix.expect("matrix").as_coeffs();
        assert!((m[0] - 0.001).abs() < 1e-9);
        let b = h.font_bbox.expect("bbox");
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (-100.0, -200.0, 900.0, 800.0));
    }

    #[test]
    fn a_built_encoding_vector() {
        let h = read_header(
            b"/Encoding 256 array\n\
              0 1 255 {1 index exch /.notdef put} for\n\
              dup 32 /space put\n\
              dup 65 /A put\n\
              dup 66 /B put\n\
              readonly def\n\
              /FontName /X def\n",
        );
        let enc = h.encoding.expect("encoding");
        assert_eq!(enc.glyph_name(32), Some("space"));
        assert_eq!(enc.glyph_name(65), Some("A"));
        assert_eq!(enc.glyph_name(67), None);
        assert!(enc.predefined().is_none());
        // The reader must not have swallowed what follows.
        assert_eq!(h.font_name.as_deref(), Some("X"));
    }

    #[test]
    fn the_predefined_encoding_form() {
        let h = read_header(b"/Encoding StandardEncoding def\n/FontName /Y def\n");
        assert_eq!(h.encoding, Some(Encoding::Standard));
        assert_eq!(h.font_name.as_deref(), Some("Y"));
    }

    #[test]
    fn the_multiple_master_declarations() {
        // Foxit Serif MM's, verbatim.
        let h: Header = read_header(
            b"/BlendDesignPositions [[0 0 ][1 0 ][0 1 ][1 1 ]] def\n\
              /BlendDesignMap [[[110 0 ][790 1 ]][[100 0 ][900 1 ]]] def\n\
              /BlendAxisTypes [/Weight /Width ] def\n\
              /WeightVector [0.2702 0.1048 0.4504 0.1746 ] def\n",
        );
        assert_eq!(h.axis_kinds, vec![AxisKind::Weight, AxisKind::Width]);
        assert_eq!(
            h.design_positions,
            vec![
                vec![0.0, 0.0],
                vec![1.0, 0.0],
                vec![0.0, 1.0],
                vec![1.0, 1.0]
            ]
        );
        assert_eq!(h.design_maps.len(), 2);
        assert_eq!(h.design_maps[0].knots, vec![(110.0, 0.0), (790.0, 1.0)]);
        assert_eq!(h.design_maps[1].knots, vec![(100.0, 0.0), (900.0, 1.0)]);
        assert_eq!(h.weight_vector.len(), 4);
        assert!((h.weight_vector[0] - 0.2702).abs() < 1e-6);
    }

    /// Build a private dictionary with the given `lenIV`.
    fn private_source(len_iv: Option<i32>) -> Vec<u8> {
        let enc = |plain: &[u8], skip: usize| {
            let mut v = vec![0x5A; skip];
            v.extend_from_slice(plain);
            eexec::encrypt(&v, eexec::CHARSTRING_SEED)
        };
        let skip = eexec::len_iv_skip(len_iv.unwrap_or(eexec::DEFAULT_LEN_IV));
        let mut out = Vec::new();
        out.extend_from_slice(b"dup /Private 8 dict dup begin\n");
        if let Some(n) = len_iv {
            out.extend_from_slice(format!("/lenIV {n} def\n").as_bytes());
        }
        out.extend_from_slice(b"/Subrs 2 array\n");
        for (i, body) in [b"sub-zero".as_slice(), b"sub-one"].iter().enumerate() {
            let c = enc(body, skip);
            out.extend_from_slice(format!("dup {i} {} RD ", c.len()).as_bytes());
            out.extend_from_slice(&c);
            out.extend_from_slice(b" NP\n");
        }
        out.extend_from_slice(b"ND\n/CharStrings 2 dict dup begin\n");
        for (name, body) in [(".notdef", b"nd-body".as_slice()), ("A", b"A-body")] {
            let c = enc(body, skip);
            out.extend_from_slice(format!("/{name} {} RD ", c.len()).as_bytes());
            out.extend_from_slice(&c);
            out.extend_from_slice(b" ND\n");
        }
        out.extend_from_slice(b"end\nend\n");
        out
    }

    #[test]
    fn private_dictionary_with_the_default_len_iv() {
        let p = read_private(&private_source(None));
        assert_eq!(p.subrs.len(), 2);
        assert_eq!(p.subrs[0], b"sub-zero");
        assert_eq!(p.subrs[1], b"sub-one");
        assert_eq!(p.charstrings.len(), 2);
        assert_eq!(&*p.charstrings[0].0, ".notdef");
        assert_eq!(p.charstrings[1].1, b"A-body");
    }

    #[test]
    fn len_iv_of_zero_and_eight_are_honoured() {
        for n in [0i32, 8] {
            let p = read_private(&private_source(Some(n)));
            assert_eq!(p.charstrings.len(), 2, "lenIV {n}");
            assert_eq!(p.charstrings[1].1, b"A-body", "lenIV {n}");
            assert_eq!(p.subrs[1], b"sub-one", "lenIV {n}");
        }
    }

    #[test]
    fn a_binary_payload_containing_postscript_syntax_is_not_tokenized() {
        // The payload spells `end` and `/CharStrings`: neither may be seen.
        let body = b"end /CharStrings 9 dict";
        let mut v = vec![0u8; 4];
        v.extend_from_slice(body);
        let c = eexec::encrypt(&v, eexec::CHARSTRING_SEED);
        let mut src = Vec::from(b"/CharStrings 1 dict dup begin\n/A ".as_slice());
        src.extend_from_slice(format!("{} RD ", c.len()).as_bytes());
        src.extend_from_slice(&c);
        src.extend_from_slice(b" ND\nend\n");
        let p = read_private(&src);
        assert_eq!(p.charstrings.len(), 1);
        assert_eq!(p.charstrings[0].1, body);
    }

    #[test]
    fn a_truncated_charstrings_dict_keeps_what_it_read() {
        let mut src = private_source(None);
        src.truncate(src.len() - 40);
        let p = read_private(&src);
        assert!(p.charstrings.len() <= 2);
        assert_eq!(p.subrs.len(), 2);
    }

    #[test]
    fn absent_keys_leave_defaults() {
        let h = read_header(b"nothing to see here");
        assert!(h.font_name.is_none());
        assert!(h.font_matrix.is_none());
        assert!(h.encoding.is_none());
        assert!(h.weight_vector.is_empty());
        let p = read_private(b"");
        assert!(p.subrs.is_empty());
        assert!(p.charstrings.is_empty());
    }
}
