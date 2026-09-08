//! The TrueType glyph ladder.
//!
//! Five rungs, each with its own "did anything map?" test, tried in order
//! until one succeeds. Unlike the Type 1 ladder this one is charmap-first: a
//! TrueType font's glyph names are optional and often absent, so the `cmap`
//! table is the primary route and names are the rescue.

use super::{LadderContext, char_index, name_index};
use crate::encoding::{
    FaceEncoding, FontEncoding, unicode_from_adobe_name, unicode_from_apple_roman,
};
use crate::glyphs::{Charmap, CharmapId};
use crate::widths::WIDTH_UNSET;

const NOTDEF: &[u8] = b".notdef";
const SYMBOL_PREFIXES: [u32; 4] = [0x00, 0xF0, 0xF1, 0xF2];

/// Which charmap the name-driven rung reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharmapType {
    /// A Unicode charmap: look the name's Unicode up directly.
    MsUnicode(Charmap),
    /// An MS Symbol charmap: probe the four private-use prefixes.
    MsSymbol(Charmap),
    /// A Mac Roman charmap: convert the Unicode back to a Mac code first.
    MacRoman(Charmap),
    /// Nothing usable.
    Other,
}

/// Fill the glyph and unicode tables.
pub(super) fn load_glyph_map(
    ctx: &LadderContext<'_>,
    unicodes: &mut [u16; 256],
    glyph_index: &mut [u16; 256],
) {
    let base = determine_encoding(ctx);

    // Rung 1 — the name-driven path, taken when the encoding is a known Latin
    // one with no `/Differences`, or the font declares itself non-symbolic.
    let rung1 = (matches!(base, FontEncoding::WinAnsi | FontEncoding::MacRoman)
        && !ctx.has_differences())
        || ctx.flags.is_non_symbolic();
    if rung1 {
        // 1a — a face with names and *no charmaps at all* gets a synthetic
        // identity mapping from `/FirstChar` upward.
        if ctx.glyphs.has_glyph_names() && ctx.glyphs.charmaps().is_empty() {
            set_glyph_indices_from_first_char(ctx, glyph_index);
            return;
        }
        name_driven(ctx, base, unicodes, glyph_index);
        return;
    }

    // Rung 2 — the whole table from an MS Symbol charmap.
    if let Some(cm) = find_charmap(ctx, CharmapId::WINDOWS_SYMBOL) {
        for c in 0..256u32 {
            if let Some(slot) = glyph_index.get_mut(c as usize) {
                *slot = symbol_glyph(ctx, cm, c);
            }
        }
        if has_any_glyph(glyph_index) {
            if base != FontEncoding::Builtin {
                for c in 0..=255u8 {
                    if let Some(name) = ctx.char_name(c)
                        && let Some(slot) = unicodes.get_mut(usize::from(c))
                    {
                        *slot = unicode_from_adobe_name(name);
                    }
                }
            } else if find_charmap(ctx, CharmapId::MAC_ROMAN).is_some() {
                for c in 0..=255u8 {
                    if let Some(slot) = unicodes.get_mut(usize::from(c)) {
                        *slot = unicode_from_apple_roman(c);
                    }
                }
            }
            return;
        }
        // Nothing mapped: fall through with the table already zeroed. The next
        // rung overwrites every entry, so the damage is invisible.
    }

    // Rung 3 — the whole table from a Mac Roman charmap.
    if let Some(cm) = find_charmap(ctx, CharmapId::MAC_ROMAN) {
        for c in 0..=255u8 {
            let idx = usize::from(c);
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = char_index(ctx.glyphs, cm, u32::from(c));
            }
            if let Some(slot) = unicodes.get_mut(idx) {
                *slot = unicode_from_apple_roman(c);
            }
        }
        // An **embedded** font stops here even if nothing mapped.
        if ctx.embedded || has_any_glyph(glyph_index) {
            return;
        }
    }

    // Rung 4 — a Unicode charmap.
    if let Some(cm) = find_unicode_charmap(ctx) {
        for c in 0..=255u8 {
            let idx = usize::from(c);
            let u = if ctx.embedded {
                // An embedded font's codes *are* its Unicodes here.
                u16::from(c)
            } else {
                // `Builtin` as the encoding argument means only `/Differences`
                // can name a glyph — the predefined table is skipped.
                match ctx.differences.get(idx).and_then(Option::as_ref) {
                    Some(name) => unicode_from_adobe_name(name.as_bytes()),
                    None => base
                        .unicodes()
                        .map_or(0, |t| t.get(idx).copied().unwrap_or(0)),
                }
            };
            if let Some(slot) = unicodes.get_mut(idx) {
                *slot = u;
            }
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = char_index(ctx.glyphs, cm, u32::from(u));
            }
        }
        if has_any_glyph(glyph_index) {
            return;
        }
    }

    // Rung 5 — identity. Every code is its own glyph.
    for c in 0..256u16 {
        if let Some(slot) = glyph_index.get_mut(usize::from(c)) {
            *slot = c;
        }
    }
}

/// Resolve a declared Latin encoding against what the face actually supports
/// (`DetermineEncoding`).
///
/// Only an **embedded, symbolic** font with a WinAnsi or MacRoman encoding is
/// second-guessed; everything else keeps what it declared.
fn determine_encoding(ctx: &LadderContext<'_>) -> FontEncoding {
    if !ctx.embedded
        || !ctx.flags.is_symbolic()
        || !matches!(ctx.encoding, FontEncoding::WinAnsi | FontEncoding::MacRoman)
    {
        return ctx.encoding;
    }
    let charmaps = ctx.glyphs.charmaps();
    if charmaps.is_empty() {
        return ctx.encoding;
    }
    // Platform 0 (Apple Unicode) counts toward Windows support, not Mac.
    let support_win = charmaps.iter().any(|c| c.platform == 0 || c.platform == 3);
    let support_mac = charmaps.iter().any(|c| c.platform == 1);
    match ctx.encoding {
        FontEncoding::WinAnsi if !support_win => {
            if support_mac {
                FontEncoding::MacRoman
            } else {
                FontEncoding::Builtin
            }
        }
        FontEncoding::MacRoman if !support_mac => {
            if support_win {
                FontEncoding::WinAnsi
            } else {
                FontEncoding::Builtin
            }
        }
        other => other,
    }
}

/// Which charmap the name-driven rung drives (`DetermineCharmapType`).
///
/// The symbolic flag **flips** the preference between the `(3,0)` symbol
/// charmap and the `(1,0)` Mac Roman one, which is the whole reason this is a
/// separate decision rather than a fixed order.
fn determine_charmap_type(ctx: &LadderContext<'_>) -> CharmapType {
    if let Some(cm) = find_unicode_charmap(ctx) {
        return CharmapType::MsUnicode(cm);
    }
    let symbol = find_charmap(ctx, CharmapId::WINDOWS_SYMBOL);
    let mac = find_charmap(ctx, CharmapId::MAC_ROMAN);
    if ctx.flags.is_non_symbolic() {
        if let Some(cm) = mac {
            return CharmapType::MacRoman(cm);
        }
        if let Some(cm) = symbol {
            return CharmapType::MsSymbol(cm);
        }
    } else {
        if let Some(cm) = symbol {
            return CharmapType::MsSymbol(cm);
        }
        if let Some(cm) = mac {
            return CharmapType::MacRoman(cm);
        }
    }
    CharmapType::Other
}

/// Rung 1b — the main name-driven loop.
fn name_driven(
    ctx: &LadderContext<'_>,
    base: FontEncoding,
    unicodes: &mut [u16; 256],
    glyph_index: &mut [u16; 256],
) {
    let charmap_type = determine_charmap_type(ctx);
    let has_to_unicode = ctx.to_unicode.is_some();
    let base_ctx = LadderContext {
        encoding: base,
        ..copy_ctx(ctx)
    };

    for c in 0..=255u8 {
        let idx = usize::from(c);
        let Some(name) = base_ctx.char_name(c).map(<[u8]>::to_vec) else {
            // No name: an embedded font tries the raw code, and a substituted
            // one gives up outright.
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = if ctx.embedded {
                    char_index(ctx.glyphs, Charmap::Unicode, u32::from(c))
                } else {
                    WIDTH_UNSET
                };
            }
            continue;
        };

        let u = unicode_from_adobe_name(&name);
        if let Some(slot) = unicodes.get_mut(idx) {
            *slot = u;
        }

        let mut g = match charmap_type {
            CharmapType::MsSymbol(cm) => symbol_glyph(ctx, cm, u32::from(c)),
            CharmapType::MsUnicode(cm) if u != 0 => char_index(ctx.glyphs, cm, u32::from(u)),
            CharmapType::MacRoman(cm) if u != 0 => {
                let maccode = FaceEncoding::AppleRoman.charcode_from_unicode(u);
                if maccode == 0 {
                    name_index(ctx.glyphs, &name)
                } else {
                    char_index(ctx.glyphs, cm, maccode)
                }
            }
            // `Other`, or a name with no Unicode: leave the slot alone.
            _ => glyph_index.get(idx).copied().unwrap_or(WIDTH_UNSET),
        };

        if g != 0 && g != WIDTH_UNSET {
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = g;
            }
            continue;
        }
        if name == NOTDEF {
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = char_index(ctx.glyphs, Charmap::Unicode, 0x20);
            }
            continue;
        }
        g = name_index(ctx.glyphs, &name);
        if g != 0 || !has_to_unicode {
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = g;
            }
            continue;
        }
        // The last resort, and the reason glyph selection can depend on
        // `/ToUnicode`: a name that resolved to nothing by any other route
        // asks the Unicode map what the code means, and looks *that* up.
        let from_tu = ctx
            .to_unicode
            .map(|tu| tu.lookup(crate::CharCode(u32::from(c))))
            .and_then(|chars| chars.first().copied());
        if let Some(ch) = from_tu {
            g = char_index(ctx.glyphs, Charmap::Unicode, ch as u32);
            if let Some(slot) = unicodes.get_mut(idx) {
                *slot = ch as u16;
            }
        }
        if let Some(slot) = glyph_index.get_mut(idx) {
            *slot = g;
        }
    }
}

/// Rung 1a — a synthetic identity mapping for a face with names and no
/// charmaps (`SetGlyphIndicesFromFirstChar`).
///
/// Glyphs are numbered from **3**, skipping the classic `.notdef`, `.null` and
/// carriage-return glyphs a TrueType font conventionally puts first.
fn set_glyph_indices_from_first_char(ctx: &LadderContext<'_>, glyph_index: &mut [u16; 256]) {
    let Ok(first) = usize::try_from(ctx.first_char) else {
        return;
    };
    if first > 255 {
        return;
    }
    for (i, slot) in glyph_index.iter_mut().enumerate() {
        *slot = if i < first {
            0
        } else {
            u16::try_from(i - first + 3).unwrap_or(WIDTH_UNSET)
        };
    }
}

/// Probe the four MS Symbol prefixes (`GetGlyphIndexForMSSymbol`).
fn symbol_glyph(ctx: &LadderContext<'_>, cm: Charmap, code: u32) -> u16 {
    for prefix in SYMBOL_PREFIXES {
        let g = char_index(ctx.glyphs, cm, prefix * 256 + code);
        if g != 0 {
            return g;
        }
    }
    0
}

/// Whether any entry is nonzero.
///
/// Note `WIDTH_UNSET` is nonzero, so a table still full of the "no glyph"
/// sentinel counts as having glyphs — harmless in practice, because every rung
/// that asks has already written real values.
fn has_any_glyph(glyph_index: &[u16; 256]) -> bool {
    glyph_index.iter().any(|&g| g != 0)
}

/// The Unicode charmap to use (`UseTTCharmapUnicode`).
///
/// An exact `(3,1)` wins outright. Otherwise **any** Unicode-encoded charmap
/// serves — including a `(0,3)` Apple one — but only if the face declares no
/// `(3,0)` symbol charmap at all, because a font carrying one is signalling
/// that its Unicode charmap is not the interesting one.
fn find_unicode_charmap(ctx: &LadderContext<'_>) -> Option<Charmap> {
    let charmaps = ctx.glyphs.charmaps();
    let mut unicode_index = None;
    let mut symbol_found = false;
    for (i, id) in charmaps.iter().enumerate() {
        if *id == CharmapId::WINDOWS_UNICODE {
            return Some(Charmap::Index(i));
        }
        if *id == CharmapId::WINDOWS_SYMBOL {
            symbol_found = true;
            continue;
        }
        if unicode_index.is_none() && id.is_unicode() {
            unicode_index = Some(i);
        }
    }
    match unicode_index {
        Some(i) if !symbol_found => Some(Charmap::Index(i)),
        _ => None,
    }
}

fn find_charmap(ctx: &LadderContext<'_>, id: CharmapId) -> Option<Charmap> {
    ctx.glyphs
        .charmaps()
        .iter()
        .position(|c| *c == id)
        .map(Charmap::Index)
}

fn copy_ctx<'a>(ctx: &LadderContext<'a>) -> LadderContext<'a> {
    LadderContext {
        glyphs: ctx.glyphs,
        encoding: ctx.encoding,
        differences: ctx.differences,
        flags: ctx.flags,
        embedded: ctx.embedded,
        base14: ctx.base14,
        to_unicode: ctx.to_unicode,
        first_char: ctx.first_char,
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;
    use crate::glyphs::Face;
    use crate::ids::GlyphName;
    use crate::{FontFlags, GlyphSource};
    use std::sync::Arc;

    const NO_DIFFS: [Option<GlyphName>; 256] = [const { None }; 256];

    fn ctx<'a>(
        glyphs: &'a GlyphSource,
        encoding: FontEncoding,
        differences: &'a [Option<GlyphName>; 256],
        flags: FontFlags,
        embedded: bool,
        first_char: i64,
    ) -> LadderContext<'a> {
        LadderContext {
            glyphs,
            encoding,
            differences,
            flags,
            embedded,
            base14: None,
            to_unicode: None,
            first_char,
        }
    }

    fn tt(name: &str) -> GlyphSource {
        let bytes = crate::testfonts::load(name);
        let face = Face::new(Arc::from(bytes.as_slice()), 0)
            .unwrap_or_else(|| panic!("{name} must parse"));
        GlyphSource::Fontations(face)
    }

    /// A `(0, 3)` charmap must be honoured exactly as a `(3, 1)` one is:
    /// both are Unicode, and neither is preferred over the other.
    // Pins `cpdf_truetypefont_unittest.cpp`'s `AllUnicodeCmapsTreatedEqually`.
    #[test]
    fn a_zero_three_charmap_is_as_good_as_a_three_one() {
        // Two fonts differing only in cmap platform and encoding, each mapping
        // U+002E to glyph 1. Both must reach glyph 1.
        for name in ["tt_unicode_31.ttf", "tt_unicode_03.ttf"] {
            let glyphs = tt(name);
            let mut to_unicode = crate::tounicode::ToUnicode::default();
            let _ = &mut to_unicode;
            let c = ctx(
                &glyphs,
                FontEncoding::MacRoman,
                &NO_DIFFS,
                FontFlags::NON_SYMBOLIC,
                true,
                0,
            );
            let mut unicodes = [0u16; 256];
            let mut glyph_index = [WIDTH_UNSET; 256];
            load_glyph_map(&c, &mut unicodes, &mut glyph_index);
            assert_eq!(glyph_index[0x2E], 1, "{name}");
        }
    }

    #[test]
    fn a_symbol_charmap_suppresses_a_non_exact_unicode_one() {
        // `(0,3)` plus `(3,0)` means the Unicode charmap is *not* chosen —
        // the whole point of `UseTTCharmapUnicode`'s `mssymbol_found` flag.
        let glyphs = tt("tt_unicode_03_and_symbol.ttf");
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::DEFAULT,
            true,
            0,
        );
        assert_eq!(find_unicode_charmap(&c), None);
    }

    #[test]
    fn an_exact_three_one_wins_even_beside_a_symbol_charmap() {
        let glyphs = tt("tt_unicode_31_and_symbol.ttf");
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::DEFAULT,
            true,
            0,
        );
        assert!(find_unicode_charmap(&c).is_some());
    }

    #[test]
    fn rung_1a_numbers_glyphs_from_three() {
        let glyphs = GlyphSource::None;
        let c = ctx(
            &glyphs,
            FontEncoding::Standard,
            &NO_DIFFS,
            FontFlags::DEFAULT,
            true,
            65,
        );
        let mut glyph_index = [WIDTH_UNSET; 256];
        set_glyph_indices_from_first_char(&c, &mut glyph_index);
        assert_eq!(glyph_index[64], 0);
        // The magic 3 skips `.notdef`, `.null` and CR.
        assert_eq!(glyph_index[65], 3);
        assert_eq!(glyph_index[66], 4);
        assert_eq!(glyph_index[255], 193);
    }

    #[test]
    fn rung_1a_declines_a_first_char_outside_the_byte_range() {
        let glyphs = GlyphSource::None;
        for first in [256, 1000, -1] {
            let c = ctx(
                &glyphs,
                FontEncoding::Standard,
                &NO_DIFFS,
                FontFlags::DEFAULT,
                true,
                first,
            );
            let mut glyph_index = [WIDTH_UNSET; 256];
            set_glyph_indices_from_first_char(&c, &mut glyph_index);
            assert!(
                glyph_index.iter().all(|&g| g == WIDTH_UNSET),
                "first {first}"
            );
        }
    }

    #[test]
    fn determine_encoding_only_second_guesses_an_embedded_symbolic_font() {
        let glyphs = tt("tt_macroman_10.ttf");
        // Embedded and symbolic with a WinAnsi encoding, and only a Mac
        // charmap: the encoding is rewritten.
        let c = ctx(
            &glyphs,
            FontEncoding::WinAnsi,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        assert_eq!(determine_encoding(&c), FontEncoding::MacRoman);

        // Not embedded: left alone.
        let c = ctx(
            &glyphs,
            FontEncoding::WinAnsi,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            false,
            0,
        );
        assert_eq!(determine_encoding(&c), FontEncoding::WinAnsi);

        // Not symbolic: left alone.
        let c = ctx(
            &glyphs,
            FontEncoding::WinAnsi,
            &NO_DIFFS,
            FontFlags::NON_SYMBOLIC,
            true,
            0,
        );
        assert_eq!(determine_encoding(&c), FontEncoding::WinAnsi);

        // Not a Latin encoding: left alone.
        let c = ctx(
            &glyphs,
            FontEncoding::Standard,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        assert_eq!(determine_encoding(&c), FontEncoding::Standard);
    }

    #[test]
    fn determine_encoding_falls_to_builtin_when_neither_platform_is_supported() {
        // A face with only a `(4,0)` charmap supports neither.
        let glyphs = tt("tt_custom_40.ttf");
        let c = ctx(
            &glyphs,
            FontEncoding::WinAnsi,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        assert_eq!(determine_encoding(&c), FontEncoding::Builtin);
    }

    #[test]
    fn the_symbolic_flag_flips_the_charmap_preference() {
        let glyphs = tt("tt_symbol_and_macroman.ttf");
        // Symbolic: the `(3,0)` charmap is preferred.
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        assert!(matches!(
            determine_charmap_type(&c),
            CharmapType::MsSymbol(_)
        ));
        // Non-symbolic: `(1,0)` is.
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::NON_SYMBOLIC,
            true,
            0,
        );
        assert!(matches!(
            determine_charmap_type(&c),
            CharmapType::MacRoman(_)
        ));
    }

    #[test]
    fn the_symbol_prefix_probe_finds_a_private_use_glyph() {
        // `tt_symbol_30.ttf` maps 0xF041 to glyph 5, which the 0xF0 prefix
        // reaches from charcode 0x41.
        let glyphs = tt("tt_symbol_30.ttf");
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        let cm = find_charmap(&c, CharmapId::WINDOWS_SYMBOL).expect("has a (3,0) charmap");
        assert_eq!(symbol_glyph(&c, cm, 0x41), 5);
        assert_eq!(symbol_glyph(&c, cm, 0x42), 0);
    }

    #[test]
    fn rung_5_is_the_identity_when_nothing_else_maps() {
        // A face with only a `(3,0)` charmap that maps nothing at all: rung 2
        // fails its own test, rung 3's charmap is absent, rung 4's is too.
        let glyphs = tt("tt_symbol_empty.ttf");
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        load_glyph_map(&c, &mut unicodes, &mut glyph_index);
        for (c, &g) in glyph_index.iter().enumerate() {
            assert_eq!(usize::from(g), c, "code {c}");
        }
    }

    #[test]
    fn rung_3_returns_for_an_embedded_font_even_with_nothing_mapped() {
        let glyphs = tt("tt_macroman_empty.ttf");
        let c = ctx(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            FontFlags::SYMBOLIC,
            true,
            0,
        );
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        load_glyph_map(&c, &mut unicodes, &mut glyph_index);
        // Every glyph is zero — rung 3 accepted the empty result rather than
        // falling through to the identity of rung 5.
        assert!(glyph_index.iter().all(|&g| g == 0));
        // But the unicodes it wrote are Apple Roman's.
        assert_eq!(unicodes[0x41], u16::from(b'A'));
    }

    #[test]
    fn has_any_glyph_counts_the_unset_sentinel_as_present() {
        // A quirk worth pinning: `0xFFFF` is nonzero.
        let table = [WIDTH_UNSET; 256];
        assert!(has_any_glyph(&table));
        assert!(!has_any_glyph(&[0u16; 256]));
    }
}
