//! Charsets, code pages, and the pitch-family bits.
//!
//! Windows vocabulary that PDFium carries everywhere, because the
//! font-selection API it was written against was Windows'. On Linux it
//! survives as the language a font request is phrased in.

use crate::FontFlags;

/// A Windows charset, as `FX_Charset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Charset {
    /// Western European — the default, and where a font with no better claim
    /// lands.
    #[default]
    Ansi,
    /// No charset stated. Distinct from `Ansi`: it makes every installed face
    /// eligible rather than only the ANSI ones.
    Default,
    /// Symbolic, meaning the font's own character set.
    Symbol,
    /// Japanese.
    ShiftJis,
    /// Korean (Wansung).
    Hangul,
    /// Simplified Chinese.
    ChineseSimplified,
    /// Traditional Chinese.
    ChineseTraditional,
    /// Korean (Johab). Notably **not** CJK for the purpose of §1.12 step 8.
    Johab,
    /// Greek.
    Greek,
    /// Turkish.
    Turkish,
    /// Vietnamese.
    Vietnamese,
    /// Hebrew.
    Hebrew,
    /// Arabic.
    Arabic,
    /// Baltic.
    Baltic,
    /// Cyrillic.
    Cyrillic,
    /// Thai.
    Thai,
    /// Central and Eastern European.
    EasternEuropean,
    /// OEM.
    Oem,
}

impl Charset {
    /// Is this one of the four charsets the substitution ladder treats as CJK?
    ///
    /// **Johab is excluded**, even though it is Korean, and so are the
    /// Macintosh CJK charsets. The set is exactly what `FX_CharSetIsCJK`
    /// names, and it decides whether Branch A of §1.12 takes its CJK arm.
    #[must_use]
    pub(crate) fn is_cjk(self) -> bool {
        matches!(
            self,
            Self::ChineseSimplified | Self::ChineseTraditional | Self::Hangul | Self::ShiftJis
        )
    }

    /// The charset a code page implies.
    #[must_use]
    pub(crate) fn from_code_page(cp: CodePage) -> Self {
        match cp {
            CodePage::ShiftJis => Self::ShiftJis,
            CodePage::ChineseSimplified => Self::ChineseSimplified,
            CodePage::Hangul => Self::Hangul,
            CodePage::ChineseTraditional => Self::ChineseTraditional,
            CodePage::DefAnsi | CodePage::Utf16Le => Self::Ansi,
        }
    }
}

/// The code pages a CID collection maps to (`kCharsetCodePages`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CodePage {
    /// The system default, 0.
    #[default]
    DefAnsi,
    /// 932, Japanese.
    ShiftJis,
    /// 936, Simplified Chinese.
    ChineseSimplified,
    /// 949, Korean.
    Hangul,
    /// 950, Traditional Chinese.
    ChineseTraditional,
    /// 1200, UTF-16LE — the Adobe-Identity collection's.
    Utf16Le,
}

impl CodePage {
    /// The code page a CID collection is written in.
    #[must_use]
    pub fn for_cid_set(set: pdfrum_cmap::CidSet) -> Self {
        use pdfrum_cmap::CidSet;
        match set {
            CidSet::Unknown => Self::DefAnsi,
            CidSet::Gb1 => Self::ChineseSimplified,
            CidSet::Cns1 => Self::ChineseTraditional,
            CidSet::Japan1 => Self::ShiftJis,
            CidSet::Korea1 => Self::Hangul,
            CidSet::Unicode => Self::Utf16Le,
        }
    }
}

/// The pitch-family bits a font request carries.
///
/// A bit set rather than an enum: a request can be both serif and fixed-pitch,
/// and the scoring function tests each bit separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PitchFamily(pub u32);

impl PitchFamily {
    /// Every glyph the same width.
    pub const FIXED: u32 = 1;
    /// Serifed.
    pub const ROMAN: u32 = 16;
    /// Cursive.
    pub const SCRIPT: u32 = 64;

    /// Is the bit set?
    #[must_use]
    pub const fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    /// Derive from `/Flags` (`GetPitchFamilyFromFlags`).
    #[must_use]
    pub fn from_flags(flags: FontFlags) -> Self {
        let mut p = 0;
        if flags.contains(FontFlags::SERIF) {
            p |= Self::ROMAN;
        }
        if flags.contains(FontFlags::SCRIPT) {
            p |= Self::SCRIPT;
        }
        if flags.contains(FontFlags::FIXED_PITCH) {
            p |= Self::FIXED;
        }
        Self(p)
    }

    /// Derive from a standard-font index (`GetPitchFamilyFromBaseFont`).
    ///
    /// The four Couriers are fixed; Times, Symbol and ZapfDingbats are Roman;
    /// the Helveticas are neither.
    #[must_use]
    pub fn from_standard_font(f: super::StandardFont) -> Self {
        let i = f.index();
        if i < 4 {
            Self(Self::FIXED)
        } else if i >= 8 {
            Self(Self::ROMAN)
        } else {
            Self(0)
        }
    }
}

/// Guess a charset from a single character (`GetCharSetFromUnicode`).
///
/// An **ordered** first-hit ladder, and the order has consequences: General
/// Punctuation is claimed by Simplified Chinese, and everything below U+007F
/// is ANSI so that ASCII never drags in a CJK face.
// The ASCII arm shares the wildcard's body but not its meaning, and it cannot
// be folded into it: it has to be *first* so that ASCII is claimed before the
// General Punctuation and half-width ranges below can take it.
#[allow(clippy::match_same_arms)]
#[must_use]
pub fn charset_from_unicode(u: u32) -> Charset {
    match u {
        // "Avoid a CJK font to show ASCII" — the C++'s own comment.
        0..=0x7E => Charset::Ansi,
        0x4E00..=0x9FA5 | 0xE7C7..=0xE7F3 | 0x3000..=0x303F | 0x2000..=0x206F => {
            Charset::ChineseSimplified
        }
        0x3040..=0x309F | 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF00..=0xFFEF => Charset::ShiftJis,
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => Charset::Hangul,
        0x0E00..=0x0E7F => Charset::Thai,
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Charset::Greek,
        0x0600..=0x06FF | 0xFB50..=0xFEFC => Charset::Arabic,
        0x0590..=0x05FF => Charset::Hebrew,
        0x0400..=0x04FF => Charset::Cyrillic,
        0x0100..=0x024F => Charset::EasternEuropean,
        0x1E00..=0x1EFF => Charset::Vietnamese,
        _ => Charset::Ansi,
    }
}

#[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
/// The `OS/2` code-page-range bit a charset corresponds to, for reading a
/// face's charsets out of its own tables (`cfx_folderfontinfo.cpp`).
#[must_use]
pub fn charset_for_code_page_bit(bit: u32) -> Option<Charset> {
    Some(match bit {
        1 => Charset::EasternEuropean,
        2 => Charset::Cyrillic,
        3 => Charset::Greek,
        4 => Charset::Turkish,
        5 => Charset::Hebrew,
        6 => Charset::Arabic,
        7 => Charset::Baltic,
        8 => Charset::Vietnamese,
        16 => Charset::Thai,
        17 => Charset::ShiftJis,
        18 => Charset::ChineseSimplified,
        19 => Charset::Hangul,
        20 => Charset::ChineseTraditional,
        21 => Charset::Johab,
        30 => Charset::Oem,
        31 => Charset::Symbol,
        _ => return None,
    })
}

/// A default face name per charset (`kDefaultTTFMap`), taking the Linux
/// spellings the oracle's build uses.
#[must_use]
#[cfg(test)]
pub fn default_face_name(charset: Charset) -> &'static str {
    match charset {
        Charset::Ansi => "Helvetica",
        Charset::ChineseSimplified => "SimSun",
        Charset::ChineseTraditional => "MingLiU",
        Charset::ShiftJis => "MS Gothic",
        Charset::Hangul => "Batang",
        Charset::Cyrillic | Charset::EasternEuropean | Charset::Arabic => "Arial",
        _ => "Arial Unicode MS",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_four_charsets_count_as_cjk() {
        for c in [
            Charset::ChineseSimplified,
            Charset::ChineseTraditional,
            Charset::Hangul,
            Charset::ShiftJis,
        ] {
            assert!(c.is_cjk(), "{c:?}");
        }
        // Johab is Korean and deliberately excluded.
        assert!(!Charset::Johab.is_cjk());
        for c in [
            Charset::Ansi,
            Charset::Symbol,
            Charset::Thai,
            Charset::Default,
        ] {
            assert!(!c.is_cjk(), "{c:?}");
        }
    }

    #[test]
    fn each_cid_collection_maps_to_its_code_page() {
        use pdfrum_cmap::CidSet;
        assert_eq!(CodePage::for_cid_set(CidSet::Unknown), CodePage::DefAnsi);
        assert_eq!(
            CodePage::for_cid_set(CidSet::Gb1),
            CodePage::ChineseSimplified
        );
        assert_eq!(
            CodePage::for_cid_set(CidSet::Cns1),
            CodePage::ChineseTraditional
        );
        assert_eq!(CodePage::for_cid_set(CidSet::Japan1), CodePage::ShiftJis);
        assert_eq!(CodePage::for_cid_set(CidSet::Korea1), CodePage::Hangul);
        assert_eq!(CodePage::for_cid_set(CidSet::Unicode), CodePage::Utf16Le);
    }

    #[test]
    fn the_charset_ladder_is_ordered_and_ascii_wins_first() {
        assert_eq!(charset_from_unicode(0x41), Charset::Ansi);
        assert_eq!(charset_from_unicode(0x7E), Charset::Ansi);
        // 0x7F itself falls out of the first arm and lands on the default.
        assert_eq!(charset_from_unicode(0x7F), Charset::Ansi);
        assert_eq!(charset_from_unicode(0x4E00), Charset::ChineseSimplified);
        // General Punctuation is claimed by Simplified Chinese, not by any
        // Latin charset — an ordering consequence, not an obvious rule.
        assert_eq!(charset_from_unicode(0x2014), Charset::ChineseSimplified);
        assert_eq!(charset_from_unicode(0x3042), Charset::ShiftJis);
        assert_eq!(charset_from_unicode(0xAC00), Charset::Hangul);
        assert_eq!(charset_from_unicode(0x0E01), Charset::Thai);
        assert_eq!(charset_from_unicode(0x03B1), Charset::Greek);
        assert_eq!(charset_from_unicode(0x0627), Charset::Arabic);
        assert_eq!(charset_from_unicode(0x05D0), Charset::Hebrew);
        assert_eq!(charset_from_unicode(0x0410), Charset::Cyrillic);
        assert_eq!(charset_from_unicode(0x0100), Charset::EasternEuropean);
        assert_eq!(charset_from_unicode(0x1E00), Charset::Vietnamese);
        assert_eq!(charset_from_unicode(0x10000), Charset::Ansi);
    }

    #[test]
    fn pitch_families_derive_from_both_sources() {
        use super::super::StandardFont;
        assert_eq!(
            PitchFamily::from_standard_font(StandardFont::Courier).0,
            PitchFamily::FIXED
        );
        assert_eq!(
            PitchFamily::from_standard_font(StandardFont::Helvetica).0,
            0
        );
        assert_eq!(
            PitchFamily::from_standard_font(StandardFont::Times).0,
            PitchFamily::ROMAN
        );
        // Symbol and Dingbats are at index 12 and 13, so both are Roman.
        assert_eq!(
            PitchFamily::from_standard_font(StandardFont::Symbol).0,
            PitchFamily::ROMAN
        );

        let flags = FontFlags::SERIF | FontFlags::FIXED_PITCH;
        let p = PitchFamily::from_flags(flags);
        assert!(p.has(PitchFamily::ROMAN));
        assert!(p.has(PitchFamily::FIXED));
        assert!(!p.has(PitchFamily::SCRIPT));
    }

    #[cfg(all(feature = "system-fonts", not(target_arch = "wasm32")))]
    #[test]
    fn code_page_bits_map_to_charsets() {
        assert_eq!(charset_for_code_page_bit(17), Some(Charset::ShiftJis));
        assert_eq!(charset_for_code_page_bit(31), Some(Charset::Symbol));
        assert_eq!(charset_for_code_page_bit(0), None);
        assert_eq!(charset_for_code_page_bit(29), None);
    }

    #[test]
    fn default_face_names_match_the_linux_table() {
        assert_eq!(default_face_name(Charset::Ansi), "Helvetica");
        assert_eq!(default_face_name(Charset::ChineseSimplified), "SimSun");
        assert_eq!(default_face_name(Charset::ShiftJis), "MS Gothic");
        assert_eq!(default_face_name(Charset::Symbol), "Arial Unicode MS");
    }
}
