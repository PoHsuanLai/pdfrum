//! PDF string objects (ISO 32000-1 §7.3.4) and the text codecs around them.
//!
//! A string object is raw bytes: the lexer has already resolved `\` escapes
//! and paired hex digits, and no encoding is implied by the syntax. Text
//! *meaning* is a separate question answered by [`decode_text`], which picks
//! between `PDFDocEncoding` and three byte-order-marked Unicode encodings.

use std::borrow::Cow;

use crate::name::hex_pair;

/// `PDFDocEncoding` (ISO 32000-1 Annex D.2) as a byte-to-code-point table.
///
/// Identity with Latin-1 except for the eight accent characters at
/// `0x18..=0x1F`, the typographic block at `0x80..=0x9E`, the euro at `0xA0`,
/// and three undefined positions (`0x7F`, `0x9F`, `0xAD`) that map to
/// U+0000 rather than being dropped.
#[rustfmt::skip]
pub const PDF_DOC_ENCODING: [u16; 256] = [
    0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007,
    0x0008, 0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x000e, 0x000f,
    0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x0015, 0x0016, 0x0017,
    0x02d8, 0x02c7, 0x02c6, 0x02d9, 0x02dd, 0x02db, 0x02da, 0x02dc,
    0x0020, 0x0021, 0x0022, 0x0023, 0x0024, 0x0025, 0x0026, 0x0027,
    0x0028, 0x0029, 0x002a, 0x002b, 0x002c, 0x002d, 0x002e, 0x002f,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037,
    0x0038, 0x0039, 0x003a, 0x003b, 0x003c, 0x003d, 0x003e, 0x003f,
    0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004a, 0x004b, 0x004c, 0x004d, 0x004e, 0x004f,
    0x0050, 0x0051, 0x0052, 0x0053, 0x0054, 0x0055, 0x0056, 0x0057,
    0x0058, 0x0059, 0x005a, 0x005b, 0x005c, 0x005d, 0x005e, 0x005f,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067,
    0x0068, 0x0069, 0x006a, 0x006b, 0x006c, 0x006d, 0x006e, 0x006f,
    0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007a, 0x007b, 0x007c, 0x007d, 0x007e, 0x0000,
    0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044,
    0x2039, 0x203a, 0x2212, 0x2030, 0x201e, 0x201c, 0x201d, 0x2018,
    0x2019, 0x201a, 0x2122, 0xfb01, 0xfb02, 0x0141, 0x0152, 0x0160,
    0x0178, 0x017d, 0x0131, 0x0142, 0x0153, 0x0161, 0x017e, 0x0000,
    0x20ac, 0x00a1, 0x00a2, 0x00a3, 0x00a4, 0x00a5, 0x00a6, 0x00a7,
    0x00a8, 0x00a9, 0x00aa, 0x00ab, 0x00ac, 0x0000, 0x00ae, 0x00af,
    0x00b0, 0x00b1, 0x00b2, 0x00b3, 0x00b4, 0x00b5, 0x00b6, 0x00b7,
    0x00b8, 0x00b9, 0x00ba, 0x00bb, 0x00bc, 0x00bd, 0x00be, 0x00bf,
    0x00c0, 0x00c1, 0x00c2, 0x00c3, 0x00c4, 0x00c5, 0x00c6, 0x00c7,
    0x00c8, 0x00c9, 0x00ca, 0x00cb, 0x00cc, 0x00cd, 0x00ce, 0x00cf,
    0x00d0, 0x00d1, 0x00d2, 0x00d3, 0x00d4, 0x00d5, 0x00d6, 0x00d7,
    0x00d8, 0x00d9, 0x00da, 0x00db, 0x00dc, 0x00dd, 0x00de, 0x00df,
    0x00e0, 0x00e1, 0x00e2, 0x00e3, 0x00e4, 0x00e5, 0x00e6, 0x00e7,
    0x00e8, 0x00e9, 0x00ea, 0x00eb, 0x00ec, 0x00ed, 0x00ee, 0x00ef,
    0x00f0, 0x00f1, 0x00f2, 0x00f3, 0x00f4, 0x00f5, 0x00f6, 0x00f7,
    0x00f8, 0x00f9, 0x00fa, 0x00fb, 0x00fc, 0x00fd, 0x00fe, 0x00ff,
];

/// How the string was spelled in the file.
///
/// PDF has two string syntaxes; a parsed string remembers which one it came
/// from so that rewriting a file reproduces it. The choice carries no
/// semantics — the bytes are identical either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringSyntax {
    /// `(parenthesized)`, with backslash escapes.
    Literal,
    /// `<48657820>`, hexadecimal digit pairs.
    Hex,
}

/// A PDF string object: raw bytes plus the syntax they were written in.
///
/// ```
/// use pdfrum_object::PdfString;
///
/// let s = PdfString::literal(b"A simple test");
/// assert_eq!(s.as_text(), "A simple test");
/// assert!(!s.hex);
///
/// // A UTF-16BE byte-order mark selects the Unicode reading.
/// let unicode = PdfString::hex(b"\xFE\xFF\x03\x30\x03\x31");
/// assert_eq!(unicode.as_text(), "\u{0330}\u{0331}");
/// assert!(unicode.hex);
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PdfString {
    /// The string's bytes, escapes already resolved.
    pub bytes: Box<[u8]>,
    /// Whether the source syntax was `<hex>` rather than `(literal)`.
    /// Round-tripped by the writer; never affects meaning.
    pub hex: bool,
}

impl PdfString {
    /// A string spelled `(like this)`.
    #[must_use]
    pub fn literal(bytes: impl AsRef<[u8]>) -> Self {
        Self {
            bytes: bytes.as_ref().into(),
            hex: false,
        }
    }

    /// A string spelled `<6C696B652074686973>`.
    #[must_use]
    pub fn hex(bytes: impl AsRef<[u8]>) -> Self {
        Self {
            bytes: bytes.as_ref().into(),
            hex: true,
        }
    }

    /// A string with an explicit syntax.
    #[must_use]
    pub fn new(bytes: impl AsRef<[u8]>, syntax: StringSyntax) -> Self {
        Self {
            bytes: bytes.as_ref().into(),
            hex: matches!(syntax, StringSyntax::Hex),
        }
    }

    /// Interpret the bytes as text — see [`decode_text`].
    #[must_use]
    pub fn as_text(&self) -> Cow<'_, str> {
        decode_text(&self.bytes)
    }

    /// Re-spell the string in its original syntax, ready to write into a file.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        if self.hex {
            encode_string_hex(&self.bytes)
        } else {
            encode_string_literal(&self.bytes)
        }
    }
}

impl std::fmt::Debug for PdfString {
    /// Prints the string the way a file spells it, so an object dump reads
    /// like the PDF it came from rather than like a byte array.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.encode()))
    }
}

/// Drop the language-code regions ISO 32000-1 §7.9.2.2 allows inside text
/// strings: U+001B opens a region and the next U+001B closes it; an
/// unterminated region runs to the end.
fn strip_language_codes(text: &str) -> Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for c in text.chars() {
        if c == '\u{1b}' {
            inside = !inside;
        } else if !inside {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Decode UTF-16 code units, replacing unpaired surrogates with U+FFFD.
fn decode_utf16(units: impl Iterator<Item = u16>) -> String {
    char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Read a text string's bytes as text (ISO 32000-1 §7.9.2.2).
///
/// A leading `FE FF` selects UTF-16BE and `EF BB BF` selects UTF-8; `FF FE`
/// selects UTF-16LE, which is an extension beyond the specification.
/// Anything else is `PDFDocEncoding`, one byte per character. In the marked
/// encodings, language-code regions are stripped.
///
/// Unpaired surrogates and invalid UTF-8 become U+FFFD (Rust's `str` cannot
/// carry either), and a trailing odd byte in a UTF-16 payload is ignored.
///
/// ```
/// use pdfrum_object::decode_text;
///
/// assert_eq!(decode_text(b"the quick\tfox"), "the quick\tfox");
/// assert_eq!(decode_text(b"\xFE\xFF\xD8\x3C\xDF\xA8"), "\u{1F3A8}");
/// assert_eq!(decode_text(b"\xEF\xBB\xBF\xCC\xB0"), "\u{0330}");
/// // 0x80 is a bullet in PDFDocEncoding, not a Latin-1 control.
/// assert_eq!(decode_text(b"\x80"), "\u{2022}");
/// ```
#[must_use]
pub fn decode_text(bytes: &[u8]) -> Cow<'_, str> {
    if let Some(payload) = bytes.strip_prefix(b"\xFE\xFF") {
        let units = payload
            .chunks_exact(2)
            .filter_map(|p| Some(u16::from_be_bytes([*p.first()?, *p.get(1)?])));
        return Cow::Owned(strip_language_codes(&decode_utf16(units)).into_owned());
    }
    if let Some(payload) = bytes.strip_prefix(b"\xFF\xFE") {
        let units = payload
            .chunks_exact(2)
            .filter_map(|p| Some(u16::from_le_bytes([*p.first()?, *p.get(1)?])));
        return Cow::Owned(strip_language_codes(&decode_utf16(units)).into_owned());
    }
    if let Some(payload) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        let text = String::from_utf8_lossy(payload);
        return Cow::Owned(strip_language_codes(&text).into_owned());
    }

    // PDFDocEncoding. Pure-ASCII input (minus DEL, which the table redefines)
    // is already its own decoding, so borrow it.
    if bytes.iter().all(|b| *b < 0x18 || (0x20..0x7F).contains(b))
        && let Ok(s) = std::str::from_utf8(bytes)
    {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        bytes
            .iter()
            .map(|b| {
                let cp = PDF_DOC_ENCODING.get(usize::from(*b)).copied().unwrap_or(0);
                char::from_u32(u32::from(cp)).unwrap_or(char::REPLACEMENT_CHARACTER)
            })
            .collect(),
    )
}

/// Write text back out as a text string's bytes (ISO 32000-1 §7.9.2.2).
///
/// `PDFDocEncoding` when every character has a byte in the table (the first
/// matching byte wins, so U+0000 encodes as `0x00`), otherwise `FE FF`
/// followed by UTF-16BE.
///
/// ```
/// use pdfrum_object::encode_text;
///
/// assert_eq!(encode_text("the quick\tfox"), b"the quick\tfox");
/// assert_eq!(encode_text("\u{0330}\u{0331}"), b"\xFE\xFF\x03\x30\x03\x31");
/// ```
#[must_use]
pub fn encode_text(text: &str) -> Vec<u8> {
    let mut pdf_doc = Vec::with_capacity(text.len());
    let mut representable = true;
    for c in text.chars() {
        let Ok(cp) = u16::try_from(u32::from(c)) else {
            representable = false;
            break;
        };
        let Some(byte) = PDF_DOC_ENCODING
            .iter()
            .position(|e| *e == cp)
            .and_then(|i| u8::try_from(i).ok())
        else {
            representable = false;
            break;
        };
        pdf_doc.push(byte);
    }
    if representable {
        return pdf_doc;
    }

    let mut out = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}

/// Spell bytes as a literal string: `(…)` with `\n`, `\r`, `\(`, `\)` and
/// `\\` escaped and every other byte verbatim.
///
/// ```
/// use pdfrum_object::encode_string_literal;
///
/// assert_eq!(encode_string_literal(b"a(b)c"), b"(a\\(b\\)c)");
/// assert_eq!(encode_string_literal(b"line\n"), b"(line\\n)");
/// ```
#[must_use]
pub fn encode_string_literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'(');
    for b in bytes {
        match b {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(*b);
            }
            _ => out.push(*b),
        }
    }
    out.push(b')');
    out
}

/// Spell bytes as a hexadecimal string: `<…>` with uppercase digit pairs.
///
/// ```
/// use pdfrum_object::encode_string_hex;
///
/// assert_eq!(encode_string_hex(b"\x12\xac"), b"<12AC>");
/// assert_eq!(encode_string_hex(b""), b"<>");
/// ```
#[must_use]
pub fn encode_string_hex(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * bytes.len() + 2);
    out.push(b'<');
    for b in bytes {
        out.extend_from_slice(&hex_pair(*b));
    }
    out.push(b'>');
    out
}

#[cfg(test)]
mod tests {
    use super::{
        PDF_DOC_ENCODING, PdfString, StringSyntax, decode_text, encode_string_hex,
        encode_string_literal, encode_text,
    };

    // From fpdf_parser_decode_unittest.cpp:324-353.
    #[test]
    fn decode_text_picks_the_encoding_from_the_mark() {
        assert_eq!(decode_text(b""), "");
        assert_eq!(decode_text(b"the quick\tfox"), "the quick\tfox");
        assert_eq!(
            decode_text(b"\xEF\xBB\xBF\xCC\xB0\xCC\xB1"),
            "\u{330}\u{331}"
        );
        assert_eq!(decode_text(b"\xFE\xFF\x03\x30\x03\x31"), "\u{330}\u{331}");
        assert_eq!(
            decode_text(
                b"\xFE\xFF\x7F\x51\x98\x75\x00\x20\x56\xFE\x72\x47\x00\
                  \x20\x8D\x44\x8B\xAF\x66\xF4\x59\x1A\x00\x20\x00\xBB"
            ),
            "\u{7F51}\u{9875}\u{20}\u{56FE}\u{7247}\u{20}\
             \u{8D44}\u{8BAF}\u{66F4}\u{591A}\u{20}\u{BB}"
        );
        // Supplementary planes, through both marked encodings.
        assert_eq!(decode_text(b"\xEF\xBB\xBF\xF0\x9F\x8E\xA8"), "\u{1F3A8}");
        assert_eq!(decode_text(b"\xFE\xFF\xD8\x3C\xDF\xA8"), "\u{1F3A8}");
    }

    #[test]
    fn decode_text_reads_utf16le_as_a_pdfium_extension() {
        assert_eq!(decode_text(b"\xFF\xFE\x30\x03\x31\x03"), "\u{330}\u{331}");
    }

    // From fpdf_parser_decode_unittest.cpp:355-374.
    #[test]
    fn decode_text_strips_language_code_regions() {
        assert_eq!(
            decode_text(b"\xEF\xBB\xBF\x1B\x6A\x61\x1B\x20\xE5\x8D\xB0\xE5\x88\xB7"),
            "\u{20}\u{5370}\u{5237}"
        );
        assert_eq!(
            decode_text(b"\xFE\xFF\x00\x1B\x6A\x61\x00\x1B\x00\x20\x53\x70\x52\x37"),
            "\u{20}\u{5370}\u{5237}"
        );
        // A trailing odd byte in the UTF-16 payload is ignored.
        assert_eq!(
            decode_text(b"\xFE\xFF\x00\x1B\x6A\x61\x00\x1B\x00\x20\x53\x70\x52\x37\x29"),
            "\u{20}\u{5370}\u{5237}"
        );
        assert_eq!(
            decode_text(b"\xFE\xFF\x00\x1B\x6A\x61\x4A\x50\x00\x1B\x00\x20\x53\x70\x52\x37"),
            "\u{20}\u{5370}\u{5237}"
        );
        assert_eq!(
            decode_text(b"\xFE\xFF\x00\x20\x00\x1B\x6A\x61\x4A\x50\x00\x1B\x52\x37"),
            "\u{20}\u{5237}"
        );
    }

    // From fpdf_parser_decode_unittest.cpp:376-384.
    #[test]
    fn decode_text_tolerates_broken_language_code_regions() {
        assert_eq!(decode_text(b"\xEF\xBB\xBF\x1B\x1B"), "");
        assert_eq!(decode_text(b"\xFE\xFF\x00\x1B\x00\x1B"), "");
        // Unterminated region strips to the end.
        assert_eq!(decode_text(b"\xFE\xFF\x00\x1B\x00\x1B\x20"), "");
        assert_eq!(decode_text(b"\xEF\xBB\xBF\x1B\x1B\x20"), "\u{20}");
        assert_eq!(decode_text(b"\xFE\xFF\x00\x1B\x00\x1B\x00\x20"), "\u{20}");
    }

    // From fpdf_parser_decode_unittest.cpp:386-395, adjusted for divergence #5
    // in docs/design/pdfrum-object.md: Rust `str` cannot hold a lone
    // surrogate, so each becomes U+FFFD.
    #[test]
    fn decode_text_replaces_unpaired_surrogates() {
        assert_eq!(decode_text(b"\xFE\xFF\xD8\x00"), "\u{FFFD}");
        assert_eq!(decode_text(b"\xFE\xFF\xDC\x00"), "\u{FFFD}");
        assert_eq!(
            decode_text(b"\xFE\xFF\xD8\x00\xD8\x3C\xDF\xA8"),
            "\u{FFFD}\u{1F3A8}"
        );
        assert_eq!(
            decode_text(b"\xFE\xFF\xD8\x3C\xDF\xA8\xDC\x00"),
            "\u{1F3A8}\u{FFFD}"
        );
    }

    #[test]
    fn decode_text_uses_the_pdfdoc_table_without_a_mark() {
        // The accent block and the typographic block are not Latin-1.
        assert_eq!(decode_text(b"\x18\x19\x1A"), "\u{2D8}\u{2C7}\u{2C6}");
        assert_eq!(decode_text(b"\x80\x8A\x9E"), "\u{2022}\u{2212}\u{17E}");
        assert_eq!(decode_text(b"\xA0"), "\u{20AC}");
        assert_eq!(decode_text(b"\xA1\xFF"), "\u{A1}\u{FF}");
        // The three undefined positions become U+0000 and are kept.
        assert_eq!(decode_text(b"\x7F\x9F\xAD"), "\0\0\0");
        // Literal NULs survive, as in the C++ wide string.
        assert_eq!(decode_text(b"a\0b"), "a\0b");
    }

    // From fpdf_parser_decode_unittest.cpp:397-416.
    #[test]
    fn encode_text_prefers_pdfdoc_then_falls_back_to_utf16() {
        assert_eq!(encode_text(""), b"");
        assert_eq!(encode_text("the quick\tfox"), b"the quick\tfox");
        assert_eq!(encode_text("\u{330}\u{331}"), b"\xFE\xFF\x03\x30\x03\x31");
        assert_eq!(
            encode_text(
                "\u{7F51}\u{9875}\u{20}\u{56FE}\u{7247}\u{20}\
                 \u{8D44}\u{8BAF}\u{66F4}\u{591A}\u{20}\u{BB}"
            ),
            b"\xFE\xFF\x7F\x51\x98\x75\x00\x20\x56\xFE\x72\x47\x00\
              \x20\x8D\x44\x8B\xAF\x66\xF4\x59\x1A\x00\x20\x00\xBB"
        );
        assert_eq!(encode_text("\u{1F3A8}"), b"\xFE\xFF\xD8\x3C\xDF\xA8");
    }

    // From fpdf_parser_decode_unittest.cpp:418-434: every byte round-trips
    // except the three PDFDocEncoding leaves undefined, which collapse to NUL.
    #[test]
    fn text_round_trips_every_byte() {
        for code in 0u16..256 {
            #[expect(clippy::cast_possible_truncation, reason = "loop bound is 256")]
            let original = [code as u8];
            let reencoded = encode_text(&decode_text(&original));
            match code {
                0x7F | 0x9F | 0xAD => assert_eq!(reencoded, b"\0", "undefined at {code:#04x}"),
                _ => assert_eq!(reencoded, original, "PDFDocEncoding {code:#04x}"),
            }
        }
    }

    #[test]
    fn pdfdoc_table_is_the_annex_d_table() {
        assert_eq!(PDF_DOC_ENCODING.len(), 256);
        assert_eq!(PDF_DOC_ENCODING[0x41], 0x0041);
        assert_eq!(PDF_DOC_ENCODING[0x18], 0x02D8);
        assert_eq!(PDF_DOC_ENCODING[0x7F], 0x0000);
        assert_eq!(PDF_DOC_ENCODING[0x80], 0x2022);
        assert_eq!(PDF_DOC_ENCODING[0x9F], 0x0000);
        assert_eq!(PDF_DOC_ENCODING[0xA0], 0x20AC);
        assert_eq!(PDF_DOC_ENCODING[0xAD], 0x0000);
        assert_eq!(PDF_DOC_ENCODING[0xFF], 0x00FF);
    }

    #[test]
    fn string_syntax_round_trips_through_the_encoder() {
        let literal = PdfString::new(b"a(b)\\c\n", StringSyntax::Literal);
        assert!(!literal.hex);
        assert_eq!(literal.encode(), b"(a\\(b\\)\\\\c\\n)");

        let hex = PdfString::new(b"\x12\xAC", StringSyntax::Hex);
        assert!(hex.hex);
        assert_eq!(hex.encode(), b"<12AC>");
    }

    #[test]
    fn string_escaping_leaves_other_control_bytes_alone() {
        assert_eq!(encode_string_literal(b"\x00\x07\t"), b"(\x00\x07\t)");
        assert_eq!(encode_string_literal(b""), b"()");
        assert_eq!(encode_string_hex(b"\x00\xFF"), b"<00FF>");
    }
}
