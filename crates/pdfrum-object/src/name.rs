//! PDF name objects (ISO 32000-1 §7.3.5) and the `#xx` escape codec.
//!
//! Names are stored *decoded*: `/A#42` and `/AB` are the same name, and
//! equality compares the decoded bytes. Encoding back to file syntax happens
//! only in the writer.

use std::borrow::Cow;

use pdfrum_common::hex_digit;

use crate::string::decode_text;

/// A PDF name, holding its decoded bytes without the leading `/`.
///
/// Names are almost always ASCII identifiers, but the syntax permits any byte
/// through `#xx` escapes, so the storage is bytes rather than a `String`.
/// Specification-defined keys are `'static` and cost nothing to name; parsed
/// ones own their bytes.
///
/// ```
/// use pdfrum_object::{Name, names};
///
/// let n = Name::new(b"Length".to_vec());
/// assert_eq!(n.as_str(), Some("Length"));
/// assert_eq!(&n, names::LENGTH);
///
/// // `#xx` escapes are resolved on the way in, so spellings unify.
/// assert_eq!(Name::decode(b"A#42"), Name::new(b"AB".to_vec()));
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Cow<'static, [u8]>);

impl Name {
    /// A name from bytes that are already decoded.
    #[must_use]
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(Cow::Owned(bytes.into()))
    }

    /// A name from a `'static` spelling, without copying. `bytes` must
    /// already be decoded, which every specification-defined key is.
    #[must_use]
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self(Cow::Borrowed(bytes))
    }

    /// A name from the raw token following `/` in a file, resolving `#xx`
    /// escapes — see [`name_decode`].
    #[must_use]
    pub fn decode(raw: &[u8]) -> Self {
        Self(Cow::Owned(name_decode(raw)))
    }

    /// The decoded bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name as UTF-8, or `None` for the rare name that is not.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// The name read as a text string, for dumps that print names as text.
    #[must_use]
    pub fn as_text(&self) -> Cow<'_, str> {
        decode_text(&self.0)
    }

    /// The file syntax for this name, including the leading `/` —
    /// see [`name_encode`].
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![b'/'];
        out.extend_from_slice(&name_encode(&self.0));
        out
    }
}

impl std::fmt::Debug for Name {
    /// Prints the name the way a file spells it (`/Length`), so an object
    /// dump reads like the PDF it came from rather than like a byte array.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.encode()))
    }
}

impl AsRef<[u8]> for Name {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for Name {
    fn from(bytes: &[u8]) -> Self {
        Self::new(bytes.to_vec())
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self::new(s.as_bytes().to_vec())
    }
}

// PDFium's classifier also calls `0x80` and `0xFF` whitespace; both are
// already at or above `0x80`, so they escape either way and the distinction
// is invisible to `name_encode`.
/// Bytes the PDF grammar treats as whitespace (ISO 32000-1 table 1).
const fn is_pdf_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// Bytes the PDF grammar treats as delimiters (ISO 32000-1 table 2).
const fn is_pdf_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'%' | b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}'
    )
}

/// Resolve `#xx` escapes in a raw name token.
///
/// An escape needs **both** following bytes to be present *and* another byte
/// after them, so a `#` in the last two positions stays literal: `#4` decodes
/// to `#4`, while `#41` decodes to `A` and `#411` to `A1`. A non-hexadecimal
/// byte inside an escape counts as zero rather than aborting the escape.
///
/// ```
/// use pdfrum_object::name_decode;
///
/// assert_eq!(name_decode(b"#41"), b"A");
/// assert_eq!(name_decode(b"#4"), b"#4");
/// assert_eq!(name_decode(b"#411"), b"A1");
/// ```
#[must_use]
pub fn name_decode(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while let Some(&b) = raw.get(i) {
        if let (b'#', Some(&hi), Some(&lo)) = (b, raw.get(i + 1), raw.get(i + 2)) {
            // A non-hexadecimal byte counts as zero, matching the permissive
            // classifier a malformed `#xx` escape falls through to.
            let digit = |b: u8| hex_digit(b).unwrap_or(0);
            out.push(digit(hi).wrapping_mul(16).wrapping_add(digit(lo)));
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

/// Spell a name's bytes as file syntax (without the leading `/`).
///
/// Bytes at or above `0x80`, whitespace, delimiters, and `#` itself each
/// become `#` plus two uppercase hexadecimal digits.
///
/// ```
/// use pdfrum_object::name_encode;
///
/// assert_eq!(name_encode(b"A"), b"A");
/// assert_eq!(name_encode(b"#"), b"#23");
/// assert_eq!(name_encode(b" "), b"#20");
/// assert_eq!(name_encode(b"f\xc2\xa5"), b"f#C2#A5");
/// ```
#[must_use]
pub fn name_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for b in bytes {
        if *b >= 0x80 || is_pdf_whitespace(*b) || *b == b'#' || is_pdf_delimiter(*b) {
            out.push(b'#');
            out.extend_from_slice(&hex_pair(*b));
        } else {
            out.push(*b);
        }
    }
    out
}

/// The two uppercase hexadecimal digits spelling one byte.
pub(crate) const fn hex_pair(b: u8) -> [u8; 2] {
    const fn digit(nibble: u8) -> u8 {
        match nibble {
            0..=9 => b'0' + nibble,
            _ => b'A' + nibble - 10,
        }
    }
    [digit(b >> 4), digit(b & 0x0F)]
}

/// Declare PDF name constants: one table, no desyncing spellings.
///
/// Each entry names a Rust constant and the exact bytes the specification
/// spells the key with — a table that would otherwise be written twice, in
/// the constants module and at every use site.
///
/// ```
/// pdfrum_object::names! {
///     /// The stream's declared byte length.
///     LENGTH = "Length";
/// }
/// assert_eq!(LENGTH.as_str(), Some("Length"));
/// ```
// One of the two sanctioned macros in the project.
#[macro_export]
macro_rules! names {
    ($($(#[$meta:meta])* $konst:ident = $spelling:literal;)*) => {
        $(
            $(#[$meta])*
            pub const $konst: &$crate::Name =
                &$crate::Name::from_static($spelling.as_bytes());
        )*
    };
}

#[cfg(test)]
mod tests {
    use super::{Name, name_decode, name_encode};

    // From fpdf_parser_utility_unittest.cpp:23-30.
    #[test]
    fn name_decode_needs_a_full_escape() {
        assert_eq!(name_decode(b""), b"");
        assert_eq!(name_decode(b"A"), b"A");
        assert_eq!(name_decode(b"#"), b"#");
        assert_eq!(name_decode(b"#4"), b"#4");
        assert_eq!(name_decode(b"#41"), b"A");
        assert_eq!(name_decode(b"#411"), b"A1");
    }

    #[test]
    fn name_decode_treats_non_hex_as_zero() {
        assert_eq!(name_decode(b"#zz9"), b"\x009");
        assert_eq!(name_decode(b"#4z9"), b"\x409");
    }

    // From fpdf_parser_utility_unittest.cpp:32-41.
    #[test]
    fn name_encode_escapes_the_grammar_bytes() {
        assert_eq!(name_encode(b""), b"");
        assert_eq!(name_encode(b"A"), b"A");
        assert_eq!(name_encode(b"#"), b"#23");
        assert_eq!(name_encode(b" "), b"#20");
        assert_eq!(
            name_encode(b"!@#$%^&*()<>[]"),
            b"!@#23$#25^&*#28#29#3C#3E#5B#5D"
        );
        assert_eq!(name_encode(b"\xc2"), b"#C2");
        assert_eq!(name_encode(b"f\xc2\xa5"), b"f#C2#A5");
    }

    #[test]
    fn spellings_unify_after_decoding() {
        assert_eq!(Name::decode(b"A#42"), Name::decode(b"AB"));
        assert_eq!(Name::decode(b"Lengt#68").as_str(), Some("Length"));
    }

    #[test]
    fn static_and_owned_names_compare_equal() {
        assert_eq!(Name::from_static(b"Type"), Name::from("Type"));
    }

    #[test]
    fn encode_round_trips_through_decode() {
        for name in [&b"Length"[..], b"a b", b"#", b"\xFF\x00", b"(x)"] {
            let encoded = name_encode(name);
            assert_eq!(name_decode(&encoded), name, "round trip of {name:?}");
        }
    }

    #[test]
    fn name_encode_includes_the_slash() {
        assert_eq!(Name::from("Length").encode(), b"/Length");
        assert_eq!(Name::from("a b").encode(), b"/a#20b");
    }
}
