//! The fallback text of a glyph: the font's cmap read backwards. Exact for text whose every
//! glyph is one code point's (Latin without ligatures, CJK); wrong for a ligature (its glyph
//! has no cmap entry, or only a presentation form such as U+FB01), and for shaped scripts
//! (Arabic, Indic) whose contextual glyphs have none. Used only for runs the caller could not
//! give the text of (`RunTexts`).
//!
//! Several code points can share one glyph. CJK fonts map the unified ideograph 一 (U+4E00)
//! and the Kangxi radical ⼀ (U+2F00) to the same outline, and the lowest code point is the
//! radical, so a naive reverse map copies radicals out of Chinese text. The preferred code
//! point is the canonical one: not a radical, a compatibility ideograph, a presentation form
//! or private use.

use skrifa::{FontRef, MetadataProvider as _};
use std::collections::HashMap;

/// How much a code point is to be preferred when several share a glyph; lower wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Standing {
    /// An ordinary character.
    Canonical,
    /// A radical, a compatibility ideograph or a presentation form: a duplicate of some
    /// canonical character, kept in Unicode for round-tripping old encodings.
    Compatibility,
    /// Private use: means nothing outside the font.
    Private,
}

fn standing(code: u32) -> Standing {
    match code {
        0xE000..=0xF8FF | 0xF_0000..=0x10_FFFF => Standing::Private,
        // CJK radicals supplement and Kangxi radicals.
        0x2E80..=0x2FDF
        // CJK compatibility ideographs and their supplement.
        | 0xF900..=0xFAFF
        | 0x2_F800..=0x2_FA1F
        // Alphabetic presentation forms (ﬁ ﬂ), Arabic presentation forms, CJK compatibility
        // forms, half/full-width forms.
        | 0xFB00..=0xFDFF
        | 0xFE30..=0xFE4F
        | 0xFE70..=0xFEFF
        | 0xFF00..=0xFFEF => Standing::Compatibility,
        _ => Standing::Canonical,
    }
}

/// Each glyph's preferred character in `font`.
pub(crate) fn reverse(font: &FontRef<'_>) -> HashMap<u32, char> {
    let mut best: HashMap<u32, char> = HashMap::new();
    for (code, glyph) in font.charmap().mappings() {
        let Some(found) = char::from_u32(code) else {
            continue;
        };
        best.entry(glyph.to_u32())
            .and_modify(|held| {
                if rank(found) < rank(*held) {
                    *held = found;
                }
            })
            .or_insert(found);
    }
    best
}

fn rank(c: char) -> (Standing, u32) {
    (standing(u32::from(c)), u32::from(c))
}

#[cfg(test)]
mod tests {
    use super::{Standing, rank, standing};

    const CASES: &[(char, Standing)] = &[
        ('a', Standing::Canonical),
        ('一', Standing::Canonical),
        ('⼀', Standing::Compatibility),
        ('⺀', Standing::Compatibility),
        ('\u{F900}', Standing::Compatibility),
        ('ﬁ', Standing::Compatibility),
        ('Ａ', Standing::Compatibility),
        ('\u{E000}', Standing::Private),
    ];

    #[test]
    fn code_points_stand_where_the_table_says() {
        for &(c, want) in CASES {
            assert_eq!(standing(u32::from(c)), want, "{c:?}");
        }
    }

    #[test]
    fn the_ideograph_beats_the_radical_it_shares_a_glyph_with() {
        assert!(rank('一') < rank('⼀'));
        assert!(rank(' ') < rank('\u{A0}'));
    }
}
