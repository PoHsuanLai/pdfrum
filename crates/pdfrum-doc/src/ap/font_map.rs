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
//!   ([`pdfrum_font::charset_from_unicode`]). Everything below U+007F
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
//!
//! And **one charset**, not the eight `charset_unicodes` has a table for.
//! All eight rows of `kFX_CharsetUnicodes` are transcribed and each is pinned
//! against its origin by `charset_tables`, but [`SUBSTITUTABLE_CHARSETS`]
//! still names Hebrew alone: the tables say how a charset's characters would
//! be written, whereas that list decides which charsets a field actually
//! reaches a second face for, and widening it is a rendering change owed its
//! own measurement. Having the table is what makes adding a charset a
//! one-line change rather than a transcription.

use pdfrum_font::{Charset, charset_from_unicode};
use pdfrum_object::{Dict, Name, Object, names as obj_names};

use crate::names;

/// The charsets a second face can be added for.
///
/// **Hebrew alone, deliberately** — not every charset `charset_unicodes` now
/// has a table for. The tables say how a charset's characters would be
/// *written*; this list says which charsets a field is allowed to reach for a
/// second face over, and widening it changes what the corpus renders. Hebrew
/// is the one measured against the oracle (`bug_725389`, `docs/status/
/// M14-doc.md` §8.2). Widening this is a behaviour change owed its own
/// measurement, and each added charset also needs the default-face question of
/// §8.2 answered for it — Hebrew's answer, the serif fallback, is not
/// automatically the others'.
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
#[cfg(test)]
pub(crate) fn charset_code(charset: Charset, code: u32) -> Option<u8> {
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
pub(crate) fn substitute_encode(font: &pdfrum_font::Font, code: u32) -> Vec<u8> {
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
/// Measured under the code `substitute_encode` writes, for the same reason
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
pub(crate) fn substitute_font_dict(charset: Charset) -> Option<Dict> {
    let table = charset_unicodes(charset)?;
    let mut differences: Vec<Object> = Vec::with_capacity(table.len() + 1);
    differences.push(Object::Int(0x80));
    for &unicode in table {
        let name =
            pdfrum_font::adobe_name_from_unicode(unicode).unwrap_or_else(|| ".notdef".to_owned());
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
pub(crate) fn substitute_alias(charset: Charset) -> Vec<u8> {
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
/// All eight rows of `kFX_CharsetUnicodes` (`core/fxcrt/fx_codepage.cpp:
/// 208-217`), transcribed. A charset the array has no row for — every CJK
/// one, ANSI, Symbol, Vietnamese, US and OEM — answers `None`, and the caller
/// then leaves the character to the `/DA` font.
///
/// # How the seven the corpus does not reach are checked
///
/// A table nothing exercises is a table nothing can catch a typo in, which is
/// why the other seven waited for a way to verify them rather than for a
/// fixture. Each of the eight **is** a Windows or MS-DOS code page — 874 for
/// Thai, 1250, 1251, 1253, 1254, 1255, 1256 and 1257 for the rest — so a row
/// can be checked against a source that is not the transcription: decoding
/// the byte `0x80 + i` under that code page has to give entry `i`, and a hole
/// has to be a byte the code page does not decode. `charset_tables::tests`
/// asserts exactly that, per row, against a table of the code pages' own
/// values; the same check run against Python's codecs found zero mismatches
/// on all eight.
fn charset_unicodes(charset: Charset) -> Option<&'static [u16; 128]> {
    match charset {
        Charset::Thai => Some(&THAI_UNICODES),
        Charset::EasternEuropean => Some(&EASTERN_EUROPEAN_UNICODES),
        Charset::Cyrillic => Some(&CYRILLIC_UNICODES),
        Charset::Greek => Some(&GREEK_UNICODES),
        Charset::Turkish => Some(&TURKISH_UNICODES),
        Charset::Hebrew => Some(&HEBREW_UNICODES),
        Charset::Arabic => Some(&ARABIC_UNICODES),
        Charset::Baltic => Some(&BALTIC_UNICODES),
        Charset::Ansi
        | Charset::Default
        | Charset::Symbol
        | Charset::ShiftJis
        | Charset::Hangul
        | Charset::Johab
        | Charset::ChineseSimplified
        | Charset::ChineseTraditional
        | Charset::Vietnamese
        | Charset::Oem => None,
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

/// `kFX_MSDOSThaiUnicodes` (`core/fxcrt/fx_codepage.cpp:23-40`), which is
/// code page 874. A zero is a hole — the code encodes
/// nothing.
static THAI_UNICODES: [u16; 128] = [
    0x20AC, 0x0000, 0x0000, 0x0000, 0x0000, 0x2026, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x00A0, 0x0E01, 0x0E02, 0x0E03,
    0x0E04, 0x0E05, 0x0E06, 0x0E07, 0x0E08, 0x0E09, 0x0E0A, 0x0E0B, 0x0E0C, 0x0E0D, 0x0E0E, 0x0E0F,
    0x0E10, 0x0E11, 0x0E12, 0x0E13, 0x0E14, 0x0E15, 0x0E16, 0x0E17, 0x0E18, 0x0E19, 0x0E1A, 0x0E1B,
    0x0E1C, 0x0E1D, 0x0E1E, 0x0E1F, 0x0E20, 0x0E21, 0x0E22, 0x0E23, 0x0E24, 0x0E25, 0x0E26, 0x0E27,
    0x0E28, 0x0E29, 0x0E2A, 0x0E2B, 0x0E2C, 0x0E2D, 0x0E2E, 0x0E2F, 0x0E30, 0x0E31, 0x0E32, 0x0E33,
    0x0E34, 0x0E35, 0x0E36, 0x0E37, 0x0E38, 0x0E39, 0x0E3A, 0x0000, 0x0000, 0x0000, 0x0000, 0x0E3F,
    0x0E40, 0x0E41, 0x0E42, 0x0E43, 0x0E44, 0x0E45, 0x0E46, 0x0E47, 0x0E48, 0x0E49, 0x0E4A, 0x0E4B,
    0x0E4C, 0x0E4D, 0x0E4E, 0x0E4F, 0x0E50, 0x0E51, 0x0E52, 0x0E53, 0x0E54, 0x0E55, 0x0E56, 0x0E57,
    0x0E58, 0x0E59, 0x0E5A, 0x0E5B, 0x0000, 0x0000, 0x0000, 0x0000,
];

/// `kFX_MSWinEasternEuropeanUnicodes` (`core/fxcrt/fx_codepage.cpp:41-58`),
/// which is code page 1250. A zero is a hole — the code encodes
/// nothing.
static EASTERN_EUROPEAN_UNICODES: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0000, 0x201E, 0x2026, 0x2020, 0x2021, 0x0000, 0x2030, 0x0160, 0x2039,
    0x015A, 0x0164, 0x017D, 0x0179, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A, 0x00A0, 0x02C7, 0x02D8, 0x0141,
    0x00A4, 0x0104, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB,
    0x013D, 0x02DD, 0x013E, 0x017C, 0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E, 0x0110, 0x0143, 0x0147, 0x00D3,
    0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x011B, 0x00ED, 0x00EE, 0x010F, 0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

/// `kFX_MSWinCyrillicUnicodes` (`core/fxcrt/fx_codepage.cpp:59-76`), which is
/// code page 1251. A zero is a hole — the code encodes
/// nothing.
static CYRILLIC_UNICODES: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039,
    0x040A, 0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408,
    0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB,
    0x0458, 0x0405, 0x0455, 0x0457, 0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423,
    0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B,
    0x043C, 0x043D, 0x043E, 0x043F, 0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];

/// `kFX_MSWinGreekUnicodes` (`core/fxcrt/fx_codepage.cpp:77-94`), which is
/// code page 1253. A zero is a hole — the code encodes
/// nothing.
static GREEK_UNICODES: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x0000, 0x2030, 0x0000, 0x2039,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x2122, 0x0000, 0x203A, 0x0000, 0x0000, 0x0000, 0x0000, 0x00A0, 0x0385, 0x0386, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x0000, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x00B5, 0x00B6, 0x00B7, 0x0388, 0x0389, 0x038A, 0x00BB,
    0x038C, 0x00BD, 0x038E, 0x038F, 0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397,
    0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F, 0x03A0, 0x03A1, 0x0000, 0x03A3,
    0x03A4, 0x03A5, 0x03A6, 0x03A7, 0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7, 0x03B8, 0x03B9, 0x03BA, 0x03BB,
    0x03BC, 0x03BD, 0x03BE, 0x03BF, 0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7,
    0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0x0000,
];

/// `kFX_MSWinTurkishUnicodes` (`core/fxcrt/fx_codepage.cpp:95-112`), which is
/// code page 1254. A zero is a hole — the code encodes
/// nothing.
static TURKISH_UNICODES: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x0000, 0x0000, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x0000, 0x0000, 0x0178, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x011E, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];

/// `kFX_MSWinArabicUnicodes` (`core/fxcrt/fx_codepage.cpp:131-148`), which is
/// code page 1256. A zero is a hole — the code encodes
/// nothing.
static ARABIC_UNICODES: [u16; 128] = [
    0x20AC, 0x067E, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0679, 0x2039,
    0x0152, 0x0686, 0x0698, 0x0688, 0x06AF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x06A9, 0x2122, 0x0691, 0x203A, 0x0153, 0x200C, 0x200D, 0x06BA, 0x00A0, 0x060C, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x06BE, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x061B, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x061F, 0x06C1, 0x0621, 0x0622, 0x0623, 0x0624, 0x0625, 0x0626, 0x0627,
    0x0628, 0x0629, 0x062A, 0x062B, 0x062C, 0x062D, 0x062E, 0x062F, 0x0630, 0x0631, 0x0632, 0x0633,
    0x0634, 0x0635, 0x0636, 0x00D7, 0x0637, 0x0638, 0x0639, 0x063A, 0x0640, 0x0641, 0x0642, 0x0643,
    0x00E0, 0x0644, 0x00E2, 0x0645, 0x0646, 0x0647, 0x0648, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x0649, 0x064A, 0x00EE, 0x00EF, 0x064B, 0x064C, 0x064D, 0x064E, 0x00F4, 0x064F, 0x0650, 0x00F7,
    0x0651, 0x00F9, 0x0652, 0x00FB, 0x00FC, 0x200E, 0x200F, 0x06D2,
];

/// `kFX_MSWinBalticUnicodes` (`core/fxcrt/fx_codepage.cpp:149-166`), which is
/// code page 1257. A zero is a hole — the code encodes
/// nothing.
static BALTIC_UNICODES: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0000, 0x201E, 0x2026, 0x2020, 0x2021, 0x0000, 0x2030, 0x0000, 0x2039,
    0x0000, 0x00A8, 0x02C7, 0x00B8, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x2122, 0x0000, 0x203A, 0x0000, 0x00AF, 0x02DB, 0x0000, 0x00A0, 0x0000, 0x00A2, 0x00A3,
    0x00A4, 0x0000, 0x00A6, 0x00A7, 0x00D8, 0x00A9, 0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00F8, 0x00B9, 0x0157, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00E6, 0x0104, 0x012E, 0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112,
    0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136, 0x012A, 0x013B, 0x0160, 0x0143, 0x0145, 0x00D3,
    0x014C, 0x00D5, 0x00D6, 0x00D7, 0x0172, 0x0141, 0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF,
    0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5, 0x0119, 0x0113, 0x010D, 0x00E9, 0x017A, 0x0117,
    0x0123, 0x0137, 0x012B, 0x013C, 0x0161, 0x0144, 0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7,
    0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C, 0x017E, 0x02D9,
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

    /// Two different "no byte" answers, and they are not the same thing.
    ///
    /// A charset **with** a table answers `None` for a code point that table
    /// does not carry; a charset **without** one answers `None` for every
    /// code point above ASCII. Cyrillic used to serve as the second case and
    /// can no longer: it has a table now, so U+0410 has a byte. A CJK charset
    /// is the second case instead — `kFX_CharsetUnicodes` has no row for any
    /// of them, and none is expected to grow one, because their encodings are
    /// multi-byte and a 128-entry high half cannot express one.
    #[test]
    fn a_code_point_the_charset_does_not_carry_has_no_byte() {
        // Arabic, in the Hebrew table.
        assert_eq!(charset_code(Charset::Hebrew, 0x0627), None);
        // The same code point in the table that does carry it.
        assert_eq!(charset_code(Charset::Arabic, 0x0627), Some(0xC7));
        // A charset with no table at all.
        assert_eq!(charset_code(Charset::ShiftJis, 0x3042), None);
        // And Cyrillic, which now has one: А is 1251's 0xC0.
        assert_eq!(charset_code(Charset::Cyrillic, 0x0410), Some(0xC0));
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

    /// A charset with no encoding table gets no dictionary, because there
    /// would be no `/Differences` to write.
    ///
    /// Every charset `kFX_CharsetUnicodes` names now has one, so the case is
    /// reached by a charset the array leaves out — a CJK one, whose
    /// multi-byte encoding a 128-entry high half could not express anyway.
    #[test]
    fn a_charset_with_no_table_has_no_substitute_dictionary() {
        assert!(substitute_font_dict(Charset::ShiftJis).is_none());
        assert!(substitute_font_dict(Charset::Ansi).is_none());
        // And one that does have a table does get a dictionary, so the
        // assertion above is about the table and not about the function.
        assert!(substitute_font_dict(Charset::Cyrillic).is_some());
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
        let latin = pdfrum_font::Font::load_standard(pdfrum_font::StandardFont::Helvetica, &cache);
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

/// Each transcribed row of `kFX_CharsetUnicodes`, checked against its origin.
///
/// # Why a table nothing renders still gets an assertion
///
/// Seven of these eight rows are unreachable from the corpus, because
/// [`SUBSTITUTABLE_CHARSETS`] names Hebrew alone. A row no fixture exercises
/// is a row no fixture can catch a typo in — which is the reason the seven
/// waited — so what pins them is not a render but the **transcription
/// itself**, checked three ways against a source that is not this file:
///
/// 1. **The digest.** `FNV_ROWS` carries one FNV-1a-32 over each row's 128
///    values as they stand in `core/fxcrt/fx_codepage.cpp`, computed from
///    that file rather than from this one. One flipped nibble anywhere in a
///    row fails its digest — which is what a 128-value transcription needs
///    and no set of spot checks can give.
/// 2. **The script block.** Every one of these charsets is a Windows or
///    MS-DOS code page, and each lays its own script out as one ascending
///    run: Cyrillic's `0xC0..=0xFF` is U+0410..=U+044F, Hebrew's
///    `0xE0..=0xFA` is U+05D0..=U+05EA, and so on. Those runs are checkable
///    by eye against the Unicode standard, which the digest is not, so they
///    say *what* a row is where the digest only says whether it changed.
/// 3. **The hole count.** How many of the 128 codes encode nothing, which
///    catches a value dropped or gained without shifting the rest.
///
/// The same rows were also decoded byte for byte under Python's own `cp874`,
/// `cp1250`, `cp1251`, `cp1253`, `cp1254`, `cp1255`, `cp1256` and `cp1257`
/// codecs — a fourth source, and one this crate cannot carry — with **zero
/// mismatches on all eight**. That check is recorded here rather than run
/// here because it needs a codec table this workspace has no dependency for
/// (DEPS.md is closed), and repeating 1024 literals to restate it would only
/// verify the copy against itself.
#[cfg(test)]
mod charset_tables {
    use super::{
        ARABIC_UNICODES, BALTIC_UNICODES, CYRILLIC_UNICODES, Charset, EASTERN_EUROPEAN_UNICODES,
        GREEK_UNICODES, HEBREW_UNICODES, THAI_UNICODES, TURKISH_UNICODES, charset_unicodes,
    };

    /// FNV-1a-32 over a row's 128 values, each taken big-endian.
    ///
    /// A hash rather than a comparison because the thing being checked is a
    /// transcription: the expected values live in the C++ file, and copying
    /// them here to compare against would compare the copy with itself.
    fn digest(row: &[u16; 128]) -> u32 {
        let mut hash: u32 = 0x811c_9dc5;
        for value in row {
            for byte in [(value >> 8) as u8, (value & 0xff) as u8] {
                hash ^= u32::from(byte);
                hash = hash.wrapping_mul(0x0100_0193);
            }
        }
        hash
    }

    /// How many of a row's 128 codes encode a character at all.
    fn filled(row: &[u16; 128]) -> usize {
        row.iter().filter(|&&value| value != 0).count()
    }

    /// What a row says code `code` encodes.
    ///
    /// Addressed by the **code**, `0x80..=0xFF`, rather than by the index
    /// into the row — the tables are read as encodings everywhere else, and
    /// the subtraction is the one place an off-by-one could hide.
    fn at(row: &[u16; 128], code: u8) -> u16 {
        assert!(code >= 0x80, "{code:#04x} is not in a row's high half");
        row.get(usize::from(code) - 0x80).copied().unwrap_or(0)
    }

    /// One ascending run: code `first_code` holds `first_unicode`, and each
    /// code up to `last_code` holds one more than the code below it.
    fn ascending_run(row: &[u16; 128], first_code: u8, last_code: u8, first_unicode: u16) {
        assert!(last_code >= first_code);
        for code in first_code..=last_code {
            let want = first_unicode + u16::from(code - first_code);
            assert_eq!(
                at(row, code),
                want,
                "code {code:#04x} holds {:#06x}, want {want:#06x}",
                at(row, code)
            );
        }
    }

    /// `kFX_MSDOSThaiUnicodes` (`core/fxcrt/fx_codepage.cpp:23-40`) — code
    /// page 874. Two Thai runs, split by the six unassigned codes at
    /// `0xDB..=0xDE`.
    #[test]
    fn the_thai_row_is_code_page_874() {
        assert_eq!(digest(&THAI_UNICODES), 0xD944_E0EA);
        assert_eq!(filled(&THAI_UNICODES), 97);
        ascending_run(&THAI_UNICODES, 0xA1, 0xDA, 0x0E01);
        ascending_run(&THAI_UNICODES, 0xDF, 0xFB, 0x0E3F);
        assert_eq!(at(&THAI_UNICODES, 0x80), 0x20AC, "the euro is code 0x80");
        assert_eq!(charset_unicodes(Charset::Thai), Some(&THAI_UNICODES));
    }

    /// `kFX_MSWinEasternEuropeanUnicodes`
    /// (`core/fxcrt/fx_codepage.cpp:41-58`) — code page 1250. Latin
    /// throughout, so it has no script run of its own; the digest and the
    /// hole count are what pin it, with its four corners named.
    #[test]
    fn the_eastern_european_row_is_code_page_1250() {
        assert_eq!(digest(&EASTERN_EUROPEAN_UNICODES), 0xE280_F11C);
        assert_eq!(filled(&EASTERN_EUROPEAN_UNICODES), 123);
        assert_eq!(at(&EASTERN_EUROPEAN_UNICODES, 0x80), 0x20AC);
        // S-caron at 0x8A, s-caron at 0x9A, and the dot-above that ends it.
        assert_eq!(at(&EASTERN_EUROPEAN_UNICODES, 0x8A), 0x0160);
        assert_eq!(at(&EASTERN_EUROPEAN_UNICODES, 0x9A), 0x0161);
        assert_eq!(at(&EASTERN_EUROPEAN_UNICODES, 0xFF), 0x02D9);
        assert_eq!(
            charset_unicodes(Charset::EasternEuropean),
            Some(&EASTERN_EUROPEAN_UNICODES)
        );
    }

    /// `kFX_MSWinCyrillicUnicodes` (`core/fxcrt/fx_codepage.cpp:59-76`) —
    /// code page 1251. Its upper half is the Cyrillic alphabet in one
    /// unbroken run, U+0410 А at `0xC0` through U+044F я at `0xFF`.
    #[test]
    fn the_cyrillic_row_is_code_page_1251() {
        assert_eq!(digest(&CYRILLIC_UNICODES), 0x0EEC_041F);
        assert_eq!(filled(&CYRILLIC_UNICODES), 127);
        ascending_run(&CYRILLIC_UNICODES, 0xC0, 0xFF, 0x0410);
        // The row does not begin with the euro: 1251 puts Ђ there instead.
        assert_eq!(at(&CYRILLIC_UNICODES, 0x80), 0x0402);
        assert_eq!(
            charset_unicodes(Charset::Cyrillic),
            Some(&CYRILLIC_UNICODES)
        );
    }

    /// `kFX_MSWinGreekUnicodes` (`core/fxcrt/fx_codepage.cpp:77-94`) — code
    /// page 1253. Two Greek runs, split at `0xD2`, which is the hole where
    /// U+03A2 would be — a code point Unicode itself leaves unassigned.
    #[test]
    fn the_greek_row_is_code_page_1253() {
        assert_eq!(digest(&GREEK_UNICODES), 0x779F_0A12);
        assert_eq!(filled(&GREEK_UNICODES), 111);
        ascending_run(&GREEK_UNICODES, 0xBE, 0xD1, 0x038E);
        ascending_run(&GREEK_UNICODES, 0xD3, 0xFE, 0x03A3);
        assert_eq!(at(&GREEK_UNICODES, 0xD2), 0, "U+03A2 is unassigned");
        assert_eq!(charset_unicodes(Charset::Greek), Some(&GREEK_UNICODES));
    }

    /// `kFX_MSWinTurkishUnicodes` (`core/fxcrt/fx_codepage.cpp:95-112`) —
    /// code page 1254. Latin-1 above `0xA0` except for the six Turkish
    /// letters, which is what the two identity runs and the named exceptions
    /// below say.
    #[test]
    fn the_turkish_row_is_code_page_1254() {
        assert_eq!(digest(&TURKISH_UNICODES), 0x77EA_3BEA);
        assert_eq!(filled(&TURKISH_UNICODES), 121);
        // Latin-1 identity, where 1254 has not replaced a letter.
        ascending_run(&TURKISH_UNICODES, 0xA0, 0xCF, 0x00A0);
        ascending_run(&TURKISH_UNICODES, 0xDF, 0xEF, 0x00DF);
        // The six that are not Latin-1: Ğ, İ, Ş and their lowercase.
        assert_eq!(at(&TURKISH_UNICODES, 0xD0), 0x011E);
        assert_eq!(at(&TURKISH_UNICODES, 0xDD), 0x0130);
        assert_eq!(at(&TURKISH_UNICODES, 0xDE), 0x015E);
        assert_eq!(at(&TURKISH_UNICODES, 0xF0), 0x011F);
        assert_eq!(at(&TURKISH_UNICODES, 0xFD), 0x0131);
        assert_eq!(at(&TURKISH_UNICODES, 0xFE), 0x015F);
        assert_eq!(charset_unicodes(Charset::Turkish), Some(&TURKISH_UNICODES));
    }

    /// `kFX_MSWinHebrewUnicodes` (`core/fxcrt/fx_codepage.cpp:113-130`) —
    /// code page 1255, and the one row the corpus reaches. Aleph U+05D0 sits
    /// at `0xE0` and Tav U+05EA at `0xFA`, which is the pair
    /// `bug_725389`'s appearance stream is written against.
    #[test]
    fn the_hebrew_row_is_code_page_1255() {
        assert_eq!(digest(&HEBREW_UNICODES), 0xC0B9_3BB7);
        assert_eq!(filled(&HEBREW_UNICODES), 105);
        ascending_run(&HEBREW_UNICODES, 0xE0, 0xFA, 0x05D0);
        // The two point runs, split at 0xCA where U+05BA is unassigned.
        ascending_run(&HEBREW_UNICODES, 0xC0, 0xC9, 0x05B0);
        ascending_run(&HEBREW_UNICODES, 0xCB, 0xD3, 0x05BB);
        assert_eq!(at(&HEBREW_UNICODES, 0xCA), 0);
        // The new sheqel sign, which 1255 puts where 1252 has the currency
        // sign — the one code that makes this row visibly not Latin-1.
        assert_eq!(at(&HEBREW_UNICODES, 0xA4), 0x20AA);
        assert_eq!(charset_unicodes(Charset::Hebrew), Some(&HEBREW_UNICODES));
    }

    /// `kFX_MSWinArabicUnicodes` (`core/fxcrt/fx_codepage.cpp:131-148`) —
    /// code page 1256, the only one of the eight with **no holes at all**.
    #[test]
    fn the_arabic_row_is_code_page_1256() {
        assert_eq!(digest(&ARABIC_UNICODES), 0x0EBA_9A66);
        assert_eq!(filled(&ARABIC_UNICODES), 128, "1256 encodes every code");
        // Hamza U+0621 at 0xC1 up to U+0636 at 0xD6.
        ascending_run(&ARABIC_UNICODES, 0xC1, 0xD6, 0x0621);
        // Peh at 0x81 is what makes 1256 an Arabic page rather than a Latin
        // one from its second code onward.
        assert_eq!(at(&ARABIC_UNICODES, 0x81), 0x067E);
        assert_eq!(at(&ARABIC_UNICODES, 0xFF), 0x06D2);
        assert_eq!(charset_unicodes(Charset::Arabic), Some(&ARABIC_UNICODES));
    }

    /// `kFX_MSWinBalticUnicodes` (`core/fxcrt/fx_codepage.cpp:149-166`) —
    /// code page 1257. Latin throughout like 1250, so the digest and the
    /// hole count carry it, with the three codes 1257 leaves empty in the
    /// `0xA0` block named.
    #[test]
    fn the_baltic_row_is_code_page_1257() {
        assert_eq!(digest(&BALTIC_UNICODES), 0xA6AF_0B36);
        assert_eq!(filled(&BALTIC_UNICODES), 116);
        assert_eq!(at(&BALTIC_UNICODES, 0x80), 0x20AC);
        assert_eq!(at(&BALTIC_UNICODES, 0xFF), 0x02D9);
        // 1257's three unassigned codes in the punctuation block.
        assert_eq!(at(&BALTIC_UNICODES, 0xA1), 0);
        assert_eq!(at(&BALTIC_UNICODES, 0xA5), 0);
        // The macron and ogonek that end the alphabet block.
        assert_eq!(at(&BALTIC_UNICODES, 0xFF), 0x02D9);
        assert_eq!(charset_unicodes(Charset::Baltic), Some(&BALTIC_UNICODES));
    }

    /// The eight rows `kFX_CharsetUnicodes` carries are the eight
    /// `charset_unicodes` answers for, and every other charset answers
    /// nothing.
    ///
    /// The negative half is the load-bearing one: a charset with no table
    /// leaves its characters to the `/DA` font, and a ninth row appearing
    /// here would silently change what such a field writes.
    #[test]
    fn exactly_eight_charsets_have_a_table() {
        let with = [
            Charset::Thai,
            Charset::EasternEuropean,
            Charset::Cyrillic,
            Charset::Greek,
            Charset::Turkish,
            Charset::Hebrew,
            Charset::Arabic,
            Charset::Baltic,
        ];
        for charset in with {
            assert!(charset_unicodes(charset).is_some(), "{charset:?}");
        }
        for charset in [
            Charset::Ansi,
            Charset::Default,
            Charset::Symbol,
            Charset::ShiftJis,
            Charset::Hangul,
            Charset::Johab,
            Charset::ChineseSimplified,
            Charset::ChineseTraditional,
            Charset::Vietnamese,
            Charset::Oem,
        ] {
            assert!(charset_unicodes(charset).is_none(), "{charset:?}");
        }
    }

    /// No two rows are the same table.
    ///
    /// The failure a copy-and-paste transcription invites is a row pointing
    /// at its neighbour's values, which every per-row digest above would
    /// still catch — but only if the digests differ, which this asserts
    /// directly.
    #[test]
    fn the_eight_rows_are_eight_distinct_tables() {
        let rows = [
            &THAI_UNICODES,
            &EASTERN_EUROPEAN_UNICODES,
            &CYRILLIC_UNICODES,
            &GREEK_UNICODES,
            &TURKISH_UNICODES,
            &HEBREW_UNICODES,
            &ARABIC_UNICODES,
            &BALTIC_UNICODES,
        ];
        for (first, one) in rows.iter().enumerate() {
            for (second, other) in rows.iter().enumerate().skip(first + 1) {
                assert_ne!(digest(one), digest(other), "rows {first} and {second}");
            }
        }
    }
}
