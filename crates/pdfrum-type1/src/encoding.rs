//! The font's built-in encoding: the `/Encoding` vector that maps character
//! codes 0–255 to glyph names.
//!
//! A Type 1 program writes this in one of two ways, and both appear in the
//! wild:
//!
//! - the name of a predefined vector — in practice always `StandardEncoding`;
//! - a built array, `0 1 255 {1 index exch /.notdef put} for` followed by a run
//!   of `dup <code> /<name> put`, terminated by `readonly def`.
//!
//! The predefined tables themselves are `read_fonts::ps::encoding`'s, which is
//! genuine coverage rather than a table we would otherwise copy: they are the
//! Adobe CFF standard/expert/ISO-Latin-1 encodings, identical in both formats.
//! The same crate's Adobe Glyph List answers the name → Unicode question that
//! the synthesized charmap needs.

use read_fonts::ps::{agl, encoding::PredefinedEncoding};

/// The font's built-in character-code → glyph-name mapping.
///
/// The `Custom` case stores names rather than glyph ids so a caller can ask
/// what a code *means* even when the `/CharStrings` dictionary has no such
/// glyph — which is exactly the situation a subsetted font leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoding {
    /// Adobe `StandardEncoding`.
    Standard,
    /// Adobe `ExpertEncoding`.
    Expert,
    /// Adobe `ISOLatin1Encoding`.
    IsoLatin1,
    /// A vector the font built itself. Entries the font left at `.notdef` are
    /// `None`.
    Custom(Box<[Option<Box<str>>; 256]>),
}

impl Encoding {
    /// The glyph name a character code selects, or `None` where the vector is
    /// `.notdef`.
    ///
    /// ```
    /// use pdfrum_type1::Encoding;
    ///
    /// assert_eq!(Encoding::Standard.glyph_name(b'A'), Some("A"));
    /// assert_eq!(Encoding::Standard.glyph_name(b'\''), Some("quoteright"));
    /// assert_eq!(Encoding::Standard.glyph_name(0), None); // .notdef
    /// ```
    #[must_use]
    pub fn glyph_name(&self, code: u8) -> Option<&str> {
        match self {
            Self::Standard => predefined_name(PredefinedEncoding::Standard, code),
            Self::Expert => predefined_name(PredefinedEncoding::Expert, code),
            Self::IsoLatin1 => predefined_name(PredefinedEncoding::IsoLatin1, code),
            Self::Custom(table) => table.get(code as usize)?.as_deref(),
        }
    }

    /// Whether this is one of the three predefined vectors, and which.
    ///
    /// `pdfrum-font` needs this to reproduce PDFium's `UseType1Charmap`
    /// decision, which turns on whether the face carries a *built-in* Adobe
    /// encoding rather than a font-specific one.
    #[must_use]
    pub fn predefined(&self) -> Option<PredefinedEncoding> {
        match self {
            Self::Standard => Some(PredefinedEncoding::Standard),
            Self::Expert => Some(PredefinedEncoding::Expert),
            Self::IsoLatin1 => Some(PredefinedEncoding::IsoLatin1),
            Self::Custom(_) => None,
        }
    }
}

/// `PredefinedEncoding::name` returns `.notdef` — spelled out — for the holes
/// in the table. Callers here want `None` for a hole.
fn predefined_name(enc: PredefinedEncoding, code: u8) -> Option<&'static str> {
    match enc.name(code) {
        "" | ".notdef" => None,
        name => Some(name),
    }
}

/// The name a `seac` component code selects.
///
/// `seac` (standard-encoded accented character) composes two glyphs named by
/// their *`StandardEncoding`* codes regardless of what the font's own
/// `/Encoding` says — that indirection is the whole point of the operator, and
/// getting it wrong silently swaps accents.
#[must_use]
pub fn standard_encoding_name(code: u8) -> Option<&'static str> {
    predefined_name(PredefinedEncoding::Standard, code)
}

/// The Unicode scalar a glyph name denotes, by the Adobe Glyph List plus the
/// `uniXXXX` / `uXXXXXX` conventions.
///
/// Returns `None` for a name that maps to a sequence rather than a single
/// scalar (`ffi`), which is the right answer for a charmap: a `char` → glyph
/// lookup cannot represent it.
#[must_use]
pub fn unicode_from_glyph_name(name: &str) -> Option<char> {
    agl::name_to_char(name)
}

/// Build a `Custom` encoding from the `(code, name)` pairs a font declared.
///
/// Later pairs win, matching PostScript's `put` semantics.
pub(crate) fn custom_from_pairs<'a>(pairs: impl IntoIterator<Item = (u8, &'a [u8])>) -> Encoding {
    // `[None; 256]` needs `Copy`, which `Option<Box<str>>` is not.
    let mut table: Box<[Option<Box<str>>; 256]> =
        Box::new(core::array::from_fn(|_| Option::<Box<str>>::None));
    for (code, name) in pairs {
        let Ok(name) = core::str::from_utf8(name) else {
            continue;
        };
        if name == ".notdef" || name.is_empty() {
            continue;
        }
        if let Some(slot) = table.get_mut(code as usize) {
            *slot = Some(name.into());
        }
    }
    Encoding::Custom(table)
}

#[cfg(test)]
mod tests {
    use super::{Encoding, custom_from_pairs, standard_encoding_name, unicode_from_glyph_name};

    #[test]
    fn standard_encoding_has_the_quirks_that_matter() {
        // The three that separate StandardEncoding from Latin-1 and that a
        // `seac` decomposition depends on.
        assert_eq!(standard_encoding_name(0x27), Some("quoteright"));
        assert_eq!(standard_encoding_name(0x60), Some("quoteleft"));
        assert_eq!(standard_encoding_name(0xC1), Some("grave"));
        assert_eq!(standard_encoding_name(0xC5), Some("macron"));
        // And the holes really are holes.
        assert_eq!(standard_encoding_name(0x00), None);
        assert_eq!(standard_encoding_name(0x80), None);
    }

    #[test]
    fn custom_vector_overrides_and_ignores_notdef() {
        let enc = custom_from_pairs([
            (65u8, b"A".as_slice()),
            (65, b"Alpha"), // later put wins
            (66, b".notdef"),
        ]);
        assert_eq!(enc.glyph_name(65), Some("Alpha"));
        assert_eq!(enc.glyph_name(66), None);
        assert_eq!(enc.glyph_name(200), None);
        assert!(enc.predefined().is_none());
    }

    #[test]
    fn agl_answers_names_and_declines_sequences() {
        assert_eq!(unicode_from_glyph_name("A"), Some('A'));
        assert_eq!(unicode_from_glyph_name("quoteright"), Some('\u{2019}'));
        assert_eq!(unicode_from_glyph_name("uni20AC"), Some('\u{20AC}'));
        assert_eq!(unicode_from_glyph_name("nosuchglyphname"), None);
        // `ffi` has a single-scalar ligature codepoint, so it *is* mappable.
        assert_eq!(unicode_from_glyph_name("ffi"), Some('\u{FB03}'));
        // A name spelling a multi-scalar sequence is not.
        assert_eq!(unicode_from_glyph_name("uni004100420043"), None);
    }

    #[test]
    fn predefined_round_trips() {
        assert!(Encoding::Standard.predefined().is_some());
        assert!(Encoding::Expert.predefined().is_some());
        assert!(Encoding::IsoLatin1.predefined().is_some());
    }
}
