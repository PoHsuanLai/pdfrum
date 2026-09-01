//! The second face a field reaches for when its own cannot write a character.
//!
//! A widget's `/DA` names one font, and that font is often a Latin one — a
//! bare `/Arial` with no `/Encoding` and no `/Widths`, say. Typing Hebrew into
//! such a field asks it to write code points its encoding has no code for. The
//! field does not decline and it does not draw the wrong glyph: it **adds a
//! second font** that can write them, puts that font in the appearance
//! stream's own `/Resources /Font`, and switches to it with a `Tf` for the
//! characters that need it.
//!
//! # What decides that a second face is needed
//!
//! Not "the font cannot map this character" on its own. The test is a
//! **charset** comparison, made before any mapping is attempted:
//!
//! - Every code point has a charset, from a fixed range ladder
//!   ([`pdfrum_font::subst::charset_from_unicode`]). Everything below U+007F
//!   is ANSI — deliberately, so that ASCII never drags a CJK face in — and
//!   U+0590..U+05FF is Hebrew.
//! - The `/DA` font has a charset too, the one its substitution chose, or ANSI
//!   when it has no substitution to ask.
//! - The `/DA` font is used when the two agree (or when the character's
//!   charset is unknown, or the font's is Symbol) **and** the font can map the
//!   character. Otherwise a second face is added for the character's charset.
//!
//! The two halves matter separately. A Latin field typing `.` keeps its own
//! font because the charsets agree; the same field typing `ב` does not even
//! ask whether it could map it, because Hebrew is not ANSI. That is why a
//! single word can be set in two faces with two different sets of advances,
//! and why the widths cannot be taken from one face for the whole run.
//!
//! # Which face is added
//!
//! The name asked for is the charset's default face, and Hebrew has **no
//! entry** in that table, so the ask is the universal default —
//! `"Arial Unicode MS"` — which the hermetic font set does not have. The ask
//! then falls through to the empty name, and an empty name is what the
//! substitution resolves to the serif face. So a Hebrew run in a field whose
//! `/DA` names a *sans* font is set in a **serif** one, and its advances are
//! that face's.
//!
//! # How the character is written
//!
//! The added font carries a `/Differences` array built from the charset's own
//! 128-entry Unicode table, based on `WinAnsiEncoding`. For Hebrew that table
//! is code page 1255, so `ב` — U+05D1 — is written as the single byte `0xE1`.
//! The table is the encoding; the font's own `cmap` is not consulted for the
//! code, only for the glyph the code then names.
//!
//! # Where this stops
//!
//! One second face, not the *N* upstream's map can accumulate: a field mixing
//! Hebrew and Cyrillic would need two, and no corpus file does. The map here
//! holds the `/DA` font at index zero and at most one substitute at index one,
//! which is the shape `CPVT_VariableText::Provider::GetWordFontIndex`
//! (`core/fpdfdoc/cpvt_variabletext.cpp:69-83`) already assumes when it asks
//! only slots 0 and 1.

use pdfrum_font::subst::{Charset, charset_from_unicode};
use pdfrum_object::{Dict, Name, Object, names as obj_names};

use crate::names;

/// The charsets a second face can be added for.
///
/// Exactly the charsets [`substitute_font_dict`] has an encoding table for —
/// see `charset_unicodes` for why that is one rather than eight.
pub const SUBSTITUTABLE_CHARSETS: &[Charset] = &[Charset::Hebrew];

/// The charset a font's substitution chose, or ANSI when it has none.
///
/// `CPDF_BAFontMap`'s constructor (`core/fpdfdoc/cpdf_bafontmap.cpp:77-93`)
/// reads `GetSubstFontCharset()` and falls back to ANSI — the Wingdings
/// special case above it names four faces no corpus file carries.
#[must_use]
pub fn font_charset(font: &pdfrum_font::Font) -> Charset {
    font.subst().map_or(Charset::Ansi, |subst| subst.charset)
}

/// Whether the `/DA` font is the one that writes `code`.
///
/// The charset gate, then the mapping test — in that order, because the gate
/// is what stops a Latin font from being asked about Hebrew at all.
#[must_use]
pub fn da_font_writes(font: &pdfrum_font::Font, da_charset: Charset, code: u32) -> bool {
    let wanted = charset_from_unicode(code);
    if !(da_charset == Charset::Symbol || wanted == da_charset) {
        return false;
    }
    char::from_u32(code).is_some_and(|ch| font.char_code_from_unicode(ch).is_some())
}

/// The byte a charset's own encoding writes `code` as, if it has one.
///
/// The 128 high codes are the charset's Unicode table; the 128 low ones are
/// ASCII, which every one of these encodings shares with `WinAnsiEncoding`.
#[must_use]
pub fn charset_code(charset: Charset, code: u32) -> Option<u8> {
    if code < 0x80 {
        return u8::try_from(code).ok();
    }
    let table = charset_unicodes(charset)?;
    let index = table.iter().position(|&u| u32::from(u) == code)?;
    u8::try_from(0x80 + index).ok()
}

/// The bytes a substitute font writes `code` as.
///
/// The substitute's encoding is the charset's own table, so the byte comes
/// from that table rather than from the face's `cmap` — which is also what the
/// face's `/Differences` array says, so the two agree. A code the table has no
/// entry for is written as its low byte, which is where
/// `CPWL_EditImpl::GetPDFWordString`'s fallthrough leaves a character no font
/// in the map could take.
#[must_use]
pub fn substitute_encode(font: &pdfrum_font::Font, code: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let byte = char::from_u32(code)
        .and_then(|ch| font.char_code_from_unicode(ch))
        .unwrap_or(pdfrum_font::CharCode(code & 0xff));
    font.append_char(&mut out, byte);
    out
}

/// One code point's advance through a substitute font, in thousandths of an
/// em.
///
/// Measured under the code [`substitute_encode`] writes, for the same reason
/// the `/DA` font's width is: the two have to name the same glyph or the
/// layout advances past characters the stream still contains.
#[must_use]
pub fn substitute_width(font: &pdfrum_font::Font, code: u32) -> i32 {
    let charcode = char::from_u32(code)
        .and_then(|ch| font.char_code_from_unicode(ch))
        .unwrap_or(pdfrum_font::CharCode(code & 0xff));
    #[allow(clippy::cast_possible_truncation)]
    {
        font.char_width(charcode) as i32
    }
}

/// A font dictionary for the face a charset's substitute is set in.
///
/// `CPDF_DocPageData::AddFont` (`core/fpdfapi/page/cpdf_docpagedata.cpp:
/// 545-601`) writes a TrueType font whose `/Encoding` is a dictionary with
/// `/BaseEncoding /WinAnsiEncoding` and a `/Differences` array starting at
/// 128, one Adobe glyph name per entry of the charset's Unicode table, with
/// `.notdef` where the table has a hole.
///
/// `/Widths` is deliberately **not** written. Upstream computes it from the
/// substituted face's own glyph advances, which is the same face our loader
/// substitutes to and the same advances it reports — so writing them would
/// restate what the loader already answers, and getting them from a different
/// place than the one the layout measures with is how a run comes out the
/// wrong length.
#[must_use]
pub fn substitute_font_dict(charset: Charset) -> Option<Dict> {
    let table = charset_unicodes(charset)?;
    let mut differences: Vec<Object> = Vec::with_capacity(table.len() + 1);
    differences.push(Object::Int(0x80));
    for &unicode in table {
        let name = pdfrum_font::encoding::adobe_name_from_unicode(unicode)
            .unwrap_or_else(|| ".notdef".to_owned());
        differences.push(Object::Name(Name::from(name.as_str())));
    }
    let encoding = Dict::from_pairs([
        (
            names::BASE_ENCODING.clone(),
            Object::Name(obj_names::WIN_ANSI_ENCODING.clone()),
        ),
        (
            names::DIFFERENCES.clone(),
            Object::Array(differences.into_iter().collect()),
        ),
    ]);
    Some(Dict::from_pairs([
        (obj_names::TYPE.clone(), Object::Name(names::FONT.clone())),
        (
            obj_names::SUBTYPE.clone(),
            Object::Name(names::TRUE_TYPE.clone()),
        ),
        // The face the ask falls through to. Every charset this table covers
        // is absent from the default-face map, so every one of them asks for
        // the universal default, misses, and lands on the empty name — which
        // is the serif face. Naming that face here is naming the *result* of
        // that walk rather than repeating the walk.
        (
            names::BASE_FONT.clone(),
            Object::Name(Name::from(SUBSTITUTE_BASE_FONT)),
        ),
        (names::ENCODING.clone(), Object::Dict(encoding)),
        // The descriptor exists for **one bit**, and without it the
        // `/Differences` above are never read.
        //
        // A TrueType font's glyph map is a ladder, and its first rung — the
        // one that resolves each code through its glyph *name* — is taken only
        // when the encoding is a plain Latin one with no `/Differences`, or
        // when the font declares itself non-symbolic
        // (`core/fpdfapi/font/cpdf_truetypefont.cpp:60-61`). A font with a
        // `/Differences` array and no descriptor satisfies neither, falls
        // through to the charmap rungs, and is mapped by the face's own Mac
        // Roman table — which answers Latin glyphs for the high codes the
        // `/Differences` meant as Hebrew.
        //
        // `CalculateFlags` (`core/fpdfapi/page/cpdf_docpagedata.cpp:103-131`)
        // sets non-symbolic for every charset but Symbol, which is what puts
        // the font on the first rung. That is the whole of what the descriptor
        // is for here, so it carries that one key.
        (
            names::FONT_DESCRIPTOR.clone(),
            Object::Dict(Dict::from_pairs([(
                names::FLAGS.clone(),
                Object::Int(i64::from(NON_SYMBOLIC)),
            )])),
        ),
    ]))
}

/// The `/Flags` bit that says a font's glyphs are named rather than its
/// codes being its own (`kFontStyleNonSymbolic`, bit 6).
const NON_SYMBOLIC: i32 = 1 << 5;

/// The base font name the substitute is written under.
///
/// An empty ask resolves to the serif face, and `Times` is the name that
/// resolves there through the same substitution every other font on the page
/// goes through. Naming the concrete face the hermetic set holds would tie
/// this to that set.
const SUBSTITUTE_BASE_FONT: &str = "Times-Roman";

/// The resource name a charset's substitute is filed under.
///
/// `EncodeFontAlias` (`core/fpdfdoc/cpdf_bafontmap.cpp:59-63`) is the face
/// name with its spaces removed and the charset appended as two uppercase hex
/// digits. The name the alias is built from is the one the ask ended on, which
/// for every charset here is the **empty** one — so the alias is the charset
/// alone, and a field can carry both `Arial` and `_B1`.
#[must_use]
pub fn substitute_alias(charset: Charset) -> Vec<u8> {
    format!("_{:02X}", charset_byte(charset)).into_bytes()
}

/// A charset's own byte, the value `FX_Charset` gives it.
fn charset_byte(charset: Charset) -> u8 {
    match charset {
        Charset::Ansi => 0,
        Charset::Default => 1,
        Charset::Symbol => 2,
        Charset::ShiftJis => 128,
        Charset::Hangul => 129,
        Charset::Johab => 130,
        Charset::ChineseSimplified => 134,
        Charset::ChineseTraditional => 136,
        Charset::Greek => 161,
        Charset::Turkish => 162,
        Charset::Vietnamese => 163,
        Charset::Hebrew => 177,
        Charset::Arabic => 178,
        Charset::Baltic => 186,
        Charset::Cyrillic => 204,
        Charset::Thai => 222,
        Charset::EasternEuropean => 238,
        Charset::Oem => 255,
    }
}

/// The 128 code points a charset's high half encodes, in code order.
///
/// `kFX_CharsetUnicodes` (`core/fxcrt/fx_codepage.cpp:208-217`) carries eight
/// of these. Only Hebrew is transcribed: it is the one the corpus reaches, and
/// a table nothing exercises is a table nothing can catch a typo in. A charset
/// without one answers `None`, and the caller then leaves the character to the
/// `/DA` font, which is what happened before this module existed.
fn charset_unicodes(charset: Charset) -> Option<&'static [u16; 128]> {
    match charset {
        Charset::Hebrew => Some(&HEBREW_UNICODES),
        _ => None,
    }
}

/// `kFX_MSWinHebrewUnicodes` (`core/fxcrt/fx_codepage.cpp:113-130`), which is
/// code page 1255. A zero is a hole — the code encodes nothing.
static HEBREW_UNICODES: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0000, 0x2039,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0000, 0x203A, 0x0000, 0x0000, 0x0000, 0x0000, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x20AA, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00D7, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00F7, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x05B0, 0x05B1, 0x05B2, 0x05B3, 0x05B4, 0x05B5, 0x05B6, 0x05B7,
    0x05B8, 0x05B9, 0x0000, 0x05BB, 0x05BC, 0x05BD, 0x05BE, 0x05BF, 0x05C0, 0x05C1, 0x05C2, 0x05C3,
    0x05F0, 0x05F1, 0x05F2, 0x05F3, 0x05F4, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7, 0x05D8, 0x05D9, 0x05DA, 0x05DB,
    0x05DC, 0x05DD, 0x05DE, 0x05DF, 0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0x0000, 0x0000, 0x200E, 0x200F, 0x0000,
];

#[cfg(test)]
mod tests {
    use super::{
        Charset, charset_code, charset_from_unicode, substitute_alias, substitute_font_dict,
    };

    #[test]
    fn a_hebrew_letter_is_written_as_its_code_page_1255_byte() {
        // Aleph is the first of the letter block, at 0xE0, and the block runs
        // to Tav at 0xFA.
        assert_eq!(charset_code(Charset::Hebrew, 0x05D0), Some(0xE0));
        assert_eq!(charset_code(Charset::Hebrew, 0x05D1), Some(0xE1));
        assert_eq!(charset_code(Charset::Hebrew, 0x05EA), Some(0xFA));
        // And the two the fixture types beside them.
        assert_eq!(charset_code(Charset::Hebrew, 0x05D7), Some(0xE7));
        assert_eq!(charset_code(Charset::Hebrew, 0x05E8), Some(0xF8));
    }

    #[test]
    fn ascii_is_its_own_byte_under_every_charset() {
        for code in [0x20_u32, 0x2E, 0x41, 0x7E] {
            assert_eq!(
                charset_code(Charset::Hebrew, code),
                u8::try_from(code).ok(),
                "{code:#x}"
            );
        }
    }

    #[test]
    fn a_code_point_the_charset_does_not_carry_has_no_byte() {
        // Arabic, in the Hebrew table.
        assert_eq!(charset_code(Charset::Hebrew, 0x0627), None);
        // A charset with no table at all.
        assert_eq!(charset_code(Charset::Cyrillic, 0x0410), None);
    }

    #[test]
    fn hebrew_code_points_take_the_hebrew_charset_and_ascii_does_not() {
        assert_eq!(charset_from_unicode(0x05D1), Charset::Hebrew);
        assert_eq!(charset_from_unicode(0x2E), Charset::Ansi);
        assert_eq!(charset_from_unicode(0x20), Charset::Ansi);
    }

    /// `EncodeFontAlias`'s two hex digits, uppercase, for the charset the
    /// fixture reaches.
    #[test]
    fn the_substitute_is_filed_under_the_charset_alone() {
        assert_eq!(substitute_alias(Charset::Hebrew), b"_B1".to_vec());
        assert_eq!(substitute_alias(Charset::Ansi), b"_00".to_vec());
    }

    #[test]
    fn the_substitute_dict_is_a_truetype_with_a_differences_encoding() {
        let dict = substitute_font_dict(Charset::Hebrew).expect("a Hebrew substitute");
        assert_eq!(
            dict.name(pdfrum_object::names::SUBTYPE)
                .map(|name| name.as_bytes().to_vec()),
            Some(b"TrueType".to_vec())
        );

        let encoding = dict
            .dict(crate::names::ENCODING, &NoResolve)
            .expect("an encoding dictionary");
        assert_eq!(
            encoding
                .name(crate::names::BASE_ENCODING)
                .map(|name| name.as_bytes().to_vec()),
            Some(b"WinAnsiEncoding".to_vec())
        );
        let differences = encoding
            .array(crate::names::DIFFERENCES, &NoResolve)
            .expect("a differences array");
        // One leading code, then one name per table entry.
        assert_eq!(differences.len(), 129);
        assert_eq!(differences.int_at(0), Some(128));

        let name_at = |code: usize| {
            differences
                .name_at(code - 0x80 + 1)
                .map(|name| name.as_bytes().to_vec())
        };
        // Aleph is at 0xE0.
        assert_eq!(name_at(0xE0), Some(b"afii57664".to_vec()));
        // And a hole in the table is `.notdef` rather than a missing entry,
        // which would shift every name after it by one code.
        assert_eq!(name_at(0x81), Some(b".notdef".to_vec()));
    }

    #[test]
    fn a_charset_with_no_table_has_no_substitute_dictionary() {
        assert!(substitute_font_dict(Charset::Cyrillic).is_none());
    }

    /// The whole point of the descriptor: without it the `/Differences` above
    /// are never read.
    ///
    /// A TrueType font reaches its glyph names only on the first rung of
    /// `LoadGlyphMap` (`core/fpdfapi/font/cpdf_truetypefont.cpp:60-61`), and
    /// that rung wants a plain Latin encoding **without** `/Differences`, or a
    /// non-symbolic declaration. A font carrying `/Differences` only satisfies
    /// the second, and only if it says so.
    #[test]
    fn the_substitute_declares_itself_non_symbolic() {
        let dict = substitute_font_dict(Charset::Hebrew).expect("a Hebrew substitute");
        let flags = dict
            .dict(crate::names::FONT_DESCRIPTOR, &NoResolve)
            .and_then(|desc| desc.int(crate::names::FLAGS, &NoResolve))
            .expect("a descriptor with flags");
        assert_eq!(
            flags & i64::from(super::NON_SYMBOLIC),
            i64::from(super::NON_SYMBOLIC),
            "flags {flags:#x}"
        );
    }

    /// The loaded face writes each Hebrew letter as its code-page byte and
    /// reads that byte back as the letter — the round trip the emitter and
    /// the renderer each take one half of.
    ///
    /// Loaded through the same entry point every other font on a page goes
    /// through, with no substitution directories, so it answers wherever the
    /// checkout is.
    #[test]
    fn the_loaded_substitute_round_trips_a_hebrew_letter_through_its_own_encoding() {
        let cache = pdfrum_font::FontCache::new();
        let options = pdfrum_font::SubstitutionOptions::default();
        let (limits, mut diags) = (
            pdfrum_common::Limits::default(),
            pdfrum_common::Diagnostics::default(),
        );
        let dict = substitute_font_dict(Charset::Hebrew).expect("a Hebrew substitute");
        let font = pdfrum_font::load_with_options(
            &dict, &NoResolve, &cache, &options, &limits, &mut diags,
        )
        .expect("the substitute loads");

        for (code, byte) in [(0x05D0_u32, 0xE0_u8), (0x05D1, 0xE1), (0x05EA, 0xFA)] {
            assert_eq!(
                super::substitute_encode(&font, code),
                vec![byte],
                "U+{code:04X}"
            );
            assert_eq!(
                font.unicode_from_charcode(pdfrum_font::CharCode(u32::from(byte)))
                    .first()
                    .copied(),
                char::from_u32(code),
                "byte {byte:#04X}"
            );
        }
    }

    /// The gate, both ways round: a Latin font is asked to write Latin and
    /// declines Hebrew without ever consulting its own tables.
    #[test]
    fn a_latin_fonts_charset_admits_ascii_and_refuses_hebrew() {
        let cache = pdfrum_font::FontCache::new();
        let latin =
            pdfrum_font::Font::load_standard(pdfrum_font::subst::StandardFont::Helvetica, &cache);
        let charset = super::font_charset(&latin);
        assert_eq!(charset, Charset::Ansi);
        assert!(super::da_font_writes(&latin, charset, u32::from(b'A')));
        assert!(super::da_font_writes(&latin, charset, u32::from(b'.')));
        assert!(
            !super::da_font_writes(&latin, charset, 0x05D1),
            "a Latin font never writes Hebrew, whatever its cmap says"
        );
    }

    use pdfrum_object::NoResolve;
}
