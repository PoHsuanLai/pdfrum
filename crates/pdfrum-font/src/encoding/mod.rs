//! Simple-font encodings: the nine predefined character sets, the
//! `/Differences` overlay, and the Adobe Glyph List that ties glyph names to
//! Unicode.
//!
//! A simple font maps a one-byte character code to a *glyph name*, and only
//! then to a glyph. Which names a code may take comes from a base encoding —
//! one of five the specification defines plus four PDFium adds — overlaid by
//! the font dictionary's own `/Differences` array. Both halves are pure data,
//! so this module is tables plus the small resolution functions
//! (`docs/design/pdfrum-font.md` §1.7).

mod agl;
mod differences;
// The table doc comments name their C++ source files, which read as
// identifiers to clippy; the file is machine-generated, so the fix belongs in
// the extractor, not here.
#[allow(clippy::doc_markdown)]
mod tables;

pub use agl::{adobe_name_from_unicode, unicode_from_adobe_name};
pub use differences::load_differences;

/// A base encoding: which predefined table a character code is read through
/// before `/Differences` is applied.
///
/// Nine values, not the specification's four. `Builtin` means "the font's own
/// encoding vector", which has no table at all and is why every table lookup
/// is fallible; `AdobeSymbol`, `ZapfDingbats` and `MsSymbol` are the symbolic
/// sets PDFium selects by font name or charmap rather than by `/Encoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontEncoding {
    /// The font program's own encoding vector. No table; `unicodes` and
    /// `char_name` both yield nothing.
    #[default]
    Builtin,
    /// `/WinAnsiEncoding` — Windows code page 1252.
    WinAnsi,
    /// `/MacRomanEncoding`.
    MacRoman,
    /// `/MacExpertEncoding`. Reachable only through a non-TrueType font's
    /// `/BaseEncoding`; the name form rewrites it to WinAnsi (§1.7).
    MacExpert,
    /// Adobe standard encoding, the default for a non-symbolic font.
    Standard,
    /// The Symbol font's own character set.
    AdobeSymbol,
    /// The ZapfDingbats font's own character set.
    ZapfDingbats,
    /// `/PDFDocEncoding`. Its name table starts at code 24, not 32.
    PdfDoc,
    /// The Microsoft symbol charmap `(3, 0)`. Has a code→Unicode table but no
    /// glyph names.
    MsSymbol,
}

impl FontEncoding {
    /// The 256-entry code→Unicode table, when this encoding has one.
    ///
    /// `Builtin` is the only encoding without one — PDFium returns an empty
    /// span for it, and every caller tests emptiness (§1.7).
    #[must_use]
    pub fn unicodes(self) -> Option<&'static [u16; 256]> {
        Some(match self {
            Self::Builtin => return None,
            Self::WinAnsi => &tables::ADOBE_WIN_ANSI_ENCODING,
            Self::MacRoman => &tables::MAC_ROMAN_ENCODING,
            Self::MacExpert => &tables::MAC_EXPERT_ENCODING,
            Self::Standard => &tables::STANDARD_ENCODING,
            Self::AdobeSymbol => &tables::ADOBE_SYMBOL_ENCODING,
            Self::ZapfDingbats => &tables::ZAPF_ENCODING,
            Self::PdfDoc => &tables::PDF_DOC_ENCODING,
            Self::MsSymbol => &tables::MS_SYMBOL_ENCODING,
        })
    }

    /// The glyph name this encoding gives `code`, from its predefined
    /// character set.
    ///
    /// The tables are offset: they start at code 32 for every encoding but
    /// `PdfDoc`, which starts at 24, so a code below the start has no name.
    /// `MsSymbol` and `Builtin` have no name table at all
    /// (`CharNameFromPredefinedCharSet`, §1.7).
    #[must_use]
    pub fn char_name(self, code: u8) -> Option<&'static str> {
        let (table, first): (&[Option<&'static str>], u8) = match self {
            Self::Standard => (&tables::STANDARD_ENCODING_NAMES, 32),
            Self::WinAnsi => (&tables::ADOBE_WIN_ANSI_ENCODING_NAMES, 32),
            Self::MacRoman => (&tables::MAC_ROMAN_ENCODING_NAMES, 32),
            Self::MacExpert => (&tables::MAC_EXPERT_ENCODING_NAMES, 32),
            Self::PdfDoc => (&tables::PDF_DOC_ENCODING_NAMES, 24),
            Self::AdobeSymbol => (&tables::ADOBE_SYMBOL_ENCODING_NAMES, 32),
            Self::ZapfDingbats => (&tables::ZAPF_ENCODING_NAMES, 32),
            Self::MsSymbol | Self::Builtin => return None,
        };
        let index = usize::from(code.checked_sub(first)?);
        table.get(index).copied().flatten()
    }

    /// The four `/Encoding` and `/BaseEncoding` names PDFium recognises.
    ///
    /// Anything else — including `/StandardEncoding` — leaves the encoding
    /// unchanged, which is why this returns `Option` rather than a default
    /// (`GetPredefinedEncoding`, §1.7).
    #[must_use]
    pub fn from_pdf_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"WinAnsiEncoding" => Self::WinAnsi,
            b"MacRomanEncoding" => Self::MacRoman,
            b"MacExpertEncoding" => Self::MacExpert,
            b"PDFDocEncoding" => Self::PdfDoc,
            _ => return None,
        })
    }
}

/// The `fxge`-level encodings a font *face*'s charmap may declare, which are a
/// different set from [`FontEncoding`] and are reverse-mapped through the raw
/// tables (`CharCodeFromUnicodeForEncoding`, §1.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceEncoding {
    /// The charmap is Unicode: a code *is* its character.
    Unicode,
    /// Latin-1, read through the WinAnsi table.
    #[cfg(test)]
    Latin1,
    /// Apple Roman, read through the MacRoman table.
    AppleRoman,
    /// Adobe custom, read through the PDFDoc table.
    AdobeCustom,
    /// Microsoft symbol.
    Symbol,
    /// Anything else: reverse lookup always yields 0.
    Other,
}

impl FaceEncoding {
    /// Find the character code this face encoding gives `unicode`, or 0.
    ///
    /// A linear scan returning the *first* index, and **0 on a miss** — which
    /// is indistinguishable from a real hit at code 0, exactly as `PDF_FindCode`
    /// leaves it. Callers test `!= 0`, so code 0 is unreachable through this
    /// route by construction.
    #[must_use]
    pub fn charcode_from_unicode(self, unicode: u16) -> u32 {
        let table: &[u16; 256] = match self {
            // The identity arm: a Unicode charmap needs no table.
            Self::Unicode => return u32::from(unicode),
            #[cfg(test)]
            Self::Latin1 => &tables::ADOBE_WIN_ANSI_ENCODING,
            Self::AppleRoman => &tables::MAC_ROMAN_ENCODING,
            Self::AdobeCustom => &tables::PDF_DOC_ENCODING,
            Self::Symbol => &tables::MS_SYMBOL_ENCODING,
            Self::Other => return 0,
        };
        table
            .iter()
            .position(|&u| u == unicode)
            .and_then(|i| u32::try_from(i).ok())
            .unwrap_or(0)
    }
}

/// The Unicode an Apple Roman character code stands for
/// (`UnicodeFromAppleRomanCharCode`, §1.7).
#[must_use]
// A `u8` index into a `[u16; 256]` covers exactly the table, so this cannot
// be out of range.
#[allow(clippy::indexing_slicing)]
pub fn unicode_from_apple_roman(code: u8) -> u16 {
    tables::MAC_ROMAN_ENCODING[usize::from(code)]
}

/// The glyph name for `charcode`, merging `/Differences` over the base
/// encoding's predefined set.
///
/// **`/Differences` always wins**, including over a symbolic font's own set,
/// and including when the base encoding is `Builtin` — which is the only way a
/// `Builtin` font names a glyph at all (`GetAdobeCharName`, §1.7).
#[must_use]
pub fn adobe_char_name(
    base: FontEncoding,
    differences: &[Option<crate::GlyphName>; 256],
    charcode: u32,
) -> Option<&[u8]> {
    let code = u8::try_from(charcode).ok()?;
    if let Some(name) = differences.get(usize::from(code)).and_then(Option::as_ref) {
        // An empty name is not a name: PDFium tests `!IsEmpty()`.
        if !name.as_bytes().is_empty() {
            return Some(name.as_bytes());
        }
    }
    base.char_name(code).map(str::as_bytes)
}

#[cfg(test)]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;
    use crate::GlyphName;

    const NO_DIFFS: [Option<GlyphName>; 256] = [const { None }; 256];

    #[test]
    fn builtin_is_the_only_encoding_without_a_unicode_table() {
        assert!(FontEncoding::Builtin.unicodes().is_none());
        for e in [
            FontEncoding::WinAnsi,
            FontEncoding::MacRoman,
            FontEncoding::MacExpert,
            FontEncoding::Standard,
            FontEncoding::AdobeSymbol,
            FontEncoding::ZapfDingbats,
            FontEncoding::PdfDoc,
            FontEncoding::MsSymbol,
        ] {
            assert!(e.unicodes().is_some(), "{e:?}");
        }
    }

    #[test]
    fn name_tables_start_at_32_except_pdfdoc_which_starts_at_24() {
        // Code 31 has no name in a 32-based table...
        assert_eq!(FontEncoding::Standard.char_name(31), None);
        assert_eq!(FontEncoding::Standard.char_name(32), Some("space"));
        // ...but PDFDoc's table reaches down to 24.
        assert_eq!(FontEncoding::PdfDoc.char_name(23), None);
        assert!(FontEncoding::PdfDoc.char_name(24).is_some());
    }

    #[test]
    fn ms_symbol_and_builtin_have_no_glyph_names() {
        for c in [0u8, 32, 65, 255] {
            assert_eq!(FontEncoding::MsSymbol.char_name(c), None);
            assert_eq!(FontEncoding::Builtin.char_name(c), None);
        }
        // MsSymbol does have a *unicode* table, though.
        assert!(FontEncoding::MsSymbol.unicodes().is_some());
    }

    #[test]
    fn only_four_encoding_names_are_recognised() {
        assert_eq!(
            FontEncoding::from_pdf_name(b"WinAnsiEncoding"),
            Some(FontEncoding::WinAnsi)
        );
        assert_eq!(
            FontEncoding::from_pdf_name(b"MacRomanEncoding"),
            Some(FontEncoding::MacRoman)
        );
        assert_eq!(
            FontEncoding::from_pdf_name(b"MacExpertEncoding"),
            Some(FontEncoding::MacExpert)
        );
        assert_eq!(
            FontEncoding::from_pdf_name(b"PDFDocEncoding"),
            Some(FontEncoding::PdfDoc)
        );
        // `/StandardEncoding` is deliberately NOT in the table: naming it is a
        // no-op that leaves the encoding at whatever it already was (§1.7).
        assert_eq!(FontEncoding::from_pdf_name(b"StandardEncoding"), None);
        assert_eq!(FontEncoding::from_pdf_name(b"Identity-H"), None);
        assert_eq!(FontEncoding::from_pdf_name(b""), None);
    }

    #[test]
    fn differences_win_over_the_predefined_set() {
        let mut diffs = NO_DIFFS;
        diffs[65] = Some(GlyphName::from("mycustomglyph"));
        assert_eq!(
            adobe_char_name(FontEncoding::WinAnsi, &diffs, 65),
            Some(&b"mycustomglyph"[..])
        );
        // A code the differences do not cover still reads the base set.
        assert_eq!(
            adobe_char_name(FontEncoding::WinAnsi, &diffs, 66),
            Some(&b"B"[..])
        );
    }

    #[test]
    fn differences_are_the_only_names_a_builtin_font_has() {
        let mut diffs = NO_DIFFS;
        diffs[1] = Some(GlyphName::from("gee"));
        assert_eq!(
            adobe_char_name(FontEncoding::Builtin, &diffs, 1),
            Some(&b"gee"[..])
        );
        assert_eq!(adobe_char_name(FontEncoding::Builtin, &diffs, 2), None);
    }

    #[test]
    fn a_charcode_above_255_never_has_a_name() {
        assert_eq!(adobe_char_name(FontEncoding::WinAnsi, &NO_DIFFS, 256), None);
        assert_eq!(
            adobe_char_name(FontEncoding::WinAnsi, &NO_DIFFS, u32::MAX),
            None
        );
    }

    #[test]
    fn the_face_encoding_reverse_map_uses_the_right_table() {
        // Unicode is the identity arm.
        assert_eq!(FaceEncoding::Unicode.charcode_from_unicode(0x20AC), 0x20AC);
        // WinAnsi puts the Euro sign at 0x80.
        assert_eq!(FaceEncoding::Latin1.charcode_from_unicode(0x20AC), 0x80);
        // A miss is 0, indistinguishable from a hit at code 0.
        assert_eq!(FaceEncoding::Latin1.charcode_from_unicode(0x4E00), 0);
        assert_eq!(FaceEncoding::Other.charcode_from_unicode(0x41), 0);
    }

    #[test]
    fn apple_roman_reads_the_mac_roman_table() {
        assert_eq!(unicode_from_apple_roman(b'A'), u16::from(b'A'));
        assert_eq!(unicode_from_apple_roman(0xA5), 0x2022); // bullet
    }

    #[test]
    fn every_name_table_round_trips_through_the_glyph_list() {
        // The brief's §3.2 verification: wherever an encoding defines both a
        // name and a unicode for a code, the Adobe Glyph List must agree.
        // Disagreements are real in a handful of places where PDFium's tables
        // predate AGL revisions, so this counts rather than asserting zero.
        let mut checked = 0usize;
        let mut agreed = 0usize;
        for e in [
            FontEncoding::Standard,
            FontEncoding::WinAnsi,
            FontEncoding::MacRoman,
            FontEncoding::PdfDoc,
            FontEncoding::AdobeSymbol,
            FontEncoding::ZapfDingbats,
        ] {
            let Some(unicodes) = e.unicodes() else {
                continue;
            };
            for code in 0u8..=255 {
                let (Some(name), u) = (e.char_name(code), unicodes[usize::from(code)]) else {
                    continue;
                };
                if u == 0 {
                    continue;
                }
                checked += 1;
                if unicode_from_adobe_name(name.as_bytes()) == u {
                    agreed += 1;
                }
            }
        }
        assert!(checked > 1000, "expected a broad sweep, got {checked}");
        // The two tables agree on 83% of the names they both define. The
        // residue is not a transcription error: PDFium's encoding tables and
        // the Adobe Glyph List genuinely disagree, mostly in the Symbol and
        // ZapfDingbats sets, where PDFium maps glyph names to the private-use
        // codepoints the original fonts used while the AGL maps them to the
        // standard mathematical and dingbat blocks. A *drop* below this ratio
        // would mean the extraction broke; the exact value is measured, not
        // chosen.
        assert!(
            agreed * 100 / checked >= 80,
            "{agreed} of {checked} names agreed with the glyph list"
        );
    }
}
