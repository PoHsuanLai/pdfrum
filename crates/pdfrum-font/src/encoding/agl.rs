//! The Adobe Glyph List: glyph name ↔ Unicode.
//!
//! PDFium compiles FreeType's `pstables.h` into `fxge` and wraps it in two
//! functions. We take the same table from `read-fonts`' `agl` feature, which
//! is the choice PDFium's own skrifa bridge makes. Both the synthetic
//! `uniXXXX` / `uXXXX`–`uXXXXXX` spellings and the `.variant` suffix strip are
//! covered there, so neither is reimplemented here.

use read_fonts::ps::agl;

/// The Unicode a glyph name stands for, or **0** when the name is unknown.
///
/// Zero rather than `Option` because the whole ladder in §1.8 and §1.9 tests
/// `!= 0` and stores the result in a `[u16; 256]` table where 0 is already the
/// "unmapped" value; introducing an `Option` here would only be unwrapped
/// immediately at every call site.
///
/// PDFium masks FreeType's result with `& 0x7FFFFFFF`, stripping a variant
/// bit, so `"A.swash"` resolves to `A`. `read-fonts` strips the `.`-variant
/// itself, reaching the same answer by a cleaner route.
///
#[must_use]
pub fn unicode_from_adobe_name(name: &[u8]) -> u16 {
    let Ok(name) = std::str::from_utf8(name) else {
        return 0;
    };
    // A name outside the BMP cannot be stored in PDFium's `[u16; 256]`
    // encoding table either; it truncates the same way, so we do too.
    agl::name_to_char(name).map_or(0, |c| c as u16)
}

/// The canonical glyph name for a Unicode value, or `None`.
///
/// PDFium walks its table with an O(table) depth-first search into a 64-byte
/// buffer; `read-fonts` answers directly. The name is returned owned because
/// the underlying API writes into a caller buffer.
///
/// ```
/// use pdfrum_font::adobe_name_from_unicode;
///
/// assert_eq!(adobe_name_from_unicode(0x00F7).as_deref(), Some("divide"));
/// assert_eq!(adobe_name_from_unicode(0x0141).as_deref(), Some("Lslash"));
/// assert_eq!(adobe_name_from_unicode(0x0000), None);
/// ```
#[must_use]
pub fn adobe_name_from_unicode(unicode: u16) -> Option<String> {
    // U+0000 has no glyph name in PDFium's table either — its wrapper returns
    // the empty string, which every caller reads as absence.
    if unicode == 0 {
        return None;
    }
    let mut buf = [0u8; agl::MAX_NAME_LEN];
    agl::char_to_name(u32::from(unicode), &mut buf).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fx_font_unittest.cpp`'s `UnicodeFromAdobeName`, all seven rows.
    #[test]
    fn unicode_from_adobe_name_matches_the_oracle() {
        for (name, expected) in [
            (&b"nonesuch"[..], 0x0000),
            (b"", 0x0000),
            (b"paragraph", 0x00B6),
            (b"Oacute", 0x00D3),
            (b"thorn", 0x00FE),
            (b"tonos", 0x0384),
            (b"bullet", 0x2022),
        ] {
            assert_eq!(
                unicode_from_adobe_name(name),
                expected,
                "{:?}",
                std::str::from_utf8(name)
            );
        }
    }

    /// `fx_font_unittest.cpp`'s `AdobeNameFromUnicode`, all seven rows.
    #[test]
    fn adobe_name_from_unicode_matches_the_oracle() {
        for (unicode, expected) in [
            (0x0000u16, None),
            (0x00F7, Some("divide")),
            (0x0141, Some("Lslash")),
            (0x0384, Some("tonos")),
            (0x0691, Some("afii57513")),
            (0x0E5A, Some("angkhankhuthai")),
            (0x20AC, Some("Euro")),
        ] {
            assert_eq!(
                adobe_name_from_unicode(unicode).as_deref(),
                expected,
                "U+{unicode:04X}"
            );
        }
    }

    #[test]
    fn synthetic_spellings_resolve() {
        // `uniXXXX`, exactly four hex digits.
        assert_eq!(unicode_from_adobe_name(b"uni0041"), 0x0041);
        assert_eq!(unicode_from_adobe_name(b"uni20AC"), 0x20AC);
        // `uXXXX` through `uXXXXXX`.
        assert_eq!(unicode_from_adobe_name(b"u0041"), 0x0041);
        assert_eq!(unicode_from_adobe_name(b"u00041"), 0x0041);
    }

    #[test]
    fn the_variant_suffix_is_stripped() {
        assert_eq!(unicode_from_adobe_name(b"A.swash"), 0x0041);
        assert_eq!(unicode_from_adobe_name(b"A.alt"), 0x0041);
        assert_eq!(unicode_from_adobe_name(b"A"), 0x0041);
    }

    #[test]
    fn non_utf8_names_map_to_nothing_rather_than_panicking() {
        assert_eq!(unicode_from_adobe_name(&[0xff, 0xfe, 0x00]), 0);
    }

    #[test]
    fn the_two_directions_agree_on_the_names_that_have_one() {
        for u in [0x41u16, 0xF7, 0x141, 0x384, 0x20AC, 0x2022] {
            let name = adobe_name_from_unicode(u).expect("has a name");
            assert_eq!(unicode_from_adobe_name(name.as_bytes()), u);
        }
    }
}
