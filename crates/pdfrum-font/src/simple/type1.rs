//! The Type 1 glyph ladder.
//!
//! Three branches, and which one runs is decided before any code is looked up.
//! The governing rule is **name first, charmap second**: a Type 1 font names
//! its glyphs, so a name that resolves is trusted over anything a charmap
//! says (`docs/design/pdfrum-font.md` §1.8).

use super::{LadderContext, char_index, name_index};
use crate::encoding::{FontEncoding, unicode_from_adobe_name};
use crate::glyphs::{Charmap, CharmapId};
use crate::widths::WIDTH_UNSET;

/// The two names not worth a second lookup.
const NOTDEF: &[u8] = b".notdef";
const SPACE: &[u8] = b"space";

/// The prefixes an MS Symbol charmap hides its glyphs behind.
const SYMBOL_PREFIXES: [u32; 4] = [0x00, 0xF0, 0xF1, 0xF2];

/// Fill the glyph and unicode tables (`CPDF_Type1Font::LoadGlyphMap`).
pub(super) fn load_glyph_map(
    ctx: &LadderContext<'_>,
    unicodes: &mut [u16; 256],
    glyph_index: &mut [u16; 256],
) {
    // Branch A — a Type 1 font dict that got substituted with a TrueType
    // system face. The font's names mean nothing to that face, so the ladder
    // works through charmaps instead.
    if !ctx.embedded
        && !ctx
            .base14
            .is_some_and(crate::subst::StandardFont::is_symbolic)
        && ctx.glyphs.is_truetype()
    {
        if try_symbol_prefixes(ctx, glyph_index) {
            return;
        }
        unicode_path(ctx, unicodes, glyph_index);
        return;
    }

    // Which charmap the remaining branches read. `UseType1Charmap` picks the
    // font's **own** encoding vector in preference to the synthesized Unicode
    // one, so a bare `char_index(c)` below means "look `c` up in the font's
    // built-in encoding".
    let builtin = use_type1_charmap(ctx);

    if ctx.flags.is_symbolic() {
        symbolic(ctx, builtin, unicodes, glyph_index);
    } else {
        non_symbolic(ctx, unicodes, glyph_index);
    }
}

/// Branch A1: probe the four MS Symbol prefixes.
///
/// Returns whether *any* code found a glyph — if none did, the caller falls
/// through to the Unicode path rather than accepting an empty table.
fn try_symbol_prefixes(ctx: &LadderContext<'_>, glyph_index: &mut [u16; 256]) -> bool {
    let Some(cm) = find_charmap(ctx, CharmapId::WINDOWS_SYMBOL) else {
        return false;
    };
    let mut any = false;
    for c in 0..256u32 {
        for prefix in SYMBOL_PREFIXES {
            let g = char_index(ctx.glyphs, cm, prefix * 256 + c);
            if g != 0 {
                if let Some(slot) = glyph_index.get_mut(c as usize) {
                    *slot = g;
                }
                any = true;
                break;
            }
        }
    }
    any
}

/// Branch A2: read names through the Adobe Glyph List into a Unicode charmap.
fn unicode_path(ctx: &LadderContext<'_>, unicodes: &mut [u16; 256], glyph_index: &mut [u16; 256]) {
    // A `Builtin` encoding is promoted here, so the predefined table is
    // consulted after all.
    let encoding = if ctx.encoding == FontEncoding::Builtin {
        FontEncoding::Standard
    } else {
        ctx.encoding
    };
    let ctx = LadderContext {
        encoding,
        ..copy_ctx(ctx)
    };
    for c in 0..=255u8 {
        let Some(name) = ctx.char_name(c) else {
            continue;
        };
        let u = unicode_from_adobe_name(name);
        let idx = usize::from(c);
        if let Some(slot) = unicodes.get_mut(idx) {
            *slot = u;
        }
        let mut g = char_index(ctx.glyphs, Charmap::Unicode, u32::from(u));
        if g == 0 && name == NOTDEF {
            // `.notdef` with no glyph borrows the space, which is the one
            // substitution this branch makes.
            if let Some(slot) = unicodes.get_mut(idx) {
                *slot = 0x20;
            }
            g = char_index(ctx.glyphs, Charmap::Unicode, 0x20);
        }
        if let Some(slot) = glyph_index.get_mut(idx) {
            *slot = g;
        }
    }
}

/// Branch B — symbolic.
///
/// A code with a name is looked up **by name**; a code without one falls back
/// to the font's built-in encoding and then reverses the glyph back to a name
/// to recover a Unicode. There is deliberately **no** `.notdef`-to-space
/// rescue here.
fn symbolic(
    ctx: &LadderContext<'_>,
    builtin: Charmap,
    unicodes: &mut [u16; 256],
    glyph_index: &mut [u16; 256],
) {
    for c in 0..=255u8 {
        let idx = usize::from(c);
        if let Some(name) = ctx.char_name(c) {
            let name = name.to_vec();
            if let Some(slot) = unicodes.get_mut(idx) {
                *slot = unicode_from_adobe_name(&name);
            }
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = name_index(ctx.glyphs, &name);
            }
        } else {
            let g = char_index(ctx.glyphs, builtin, u32::from(c));
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = g;
            }
            if g != 0 {
                let u = ctx
                    .glyphs
                    .glyph_name(crate::Gid(g))
                    .map_or(0, |n| unicode_from_adobe_name(n.as_bytes()));
                if let Some(slot) = unicodes.get_mut(idx) {
                    *slot = u;
                }
            }
        }
    }
}

/// Branch C — non-symbolic.
///
/// Name first, charmap second. `.notdef` and `space` are the two names not
/// worth a second lookup: they take Unicode U+0020 and the explicit
/// "draw nothing" sentinel.
fn non_symbolic(ctx: &LadderContext<'_>, unicodes: &mut [u16; 256], glyph_index: &mut [u16; 256]) {
    let has_unicode = ctx.glyphs.charmaps().iter().any(|c| c.is_unicode())
        || matches!(ctx.glyphs, crate::GlyphSource::Type1(_));
    for c in 0..=255u8 {
        let Some(name) = ctx.char_name(c).map(<[u8]>::to_vec) else {
            continue;
        };
        let idx = usize::from(c);
        let u = unicode_from_adobe_name(&name);
        if let Some(slot) = unicodes.get_mut(idx) {
            *slot = u;
        }
        let mut g = name_index(ctx.glyphs, &name);
        if g == 0 {
            if name != NOTDEF && name != SPACE {
                let code = if has_unicode {
                    u32::from(u)
                } else {
                    u32::from(c)
                };
                g = char_index(ctx.glyphs, Charmap::Unicode, code);
            } else {
                if let Some(slot) = unicodes.get_mut(idx) {
                    *slot = 0x20;
                }
                g = WIDTH_UNSET;
            }
        }
        if let Some(slot) = glyph_index.get_mut(idx) {
            *slot = g;
        }
    }
}

/// Select the font's own encoding vector rather than its synthesized Unicode
/// charmap (`UseType1Charmap`).
///
/// A Type 1 face typically exposes charmap 0 as Unicode, synthesized from
/// glyph names, and charmap 1 as the font's real encoding. This deliberately
/// picks the **non**-Unicode one — and a face with exactly one charmap that is
/// Unicode gets nothing, because there is no built-in vector to select.
fn use_type1_charmap(ctx: &LadderContext<'_>) -> Charmap {
    let charmaps = ctx.glyphs.charmaps();
    if charmaps.is_empty() {
        return Charmap::None;
    }
    let first_is_unicode = charmaps.first().is_some_and(|c| c.is_unicode());
    if charmaps.len() == 1 && first_is_unicode {
        return Charmap::None;
    }
    Charmap::Index(usize::from(first_is_unicode))
}

/// The first charmap matching an id, as a selection value.
fn find_charmap(ctx: &LadderContext<'_>, id: CharmapId) -> Option<Charmap> {
    ctx.glyphs
        .charmaps()
        .iter()
        .position(|c| *c == id)
        .map(Charmap::Index)
}

/// Rebuild a context with a different encoding. A struct update would need
/// `LadderContext` to be `Clone`, which its borrowed fields make awkward.
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
    use super::*;
    use crate::GlyphSource;
    use crate::subst::builtin_generic;

    fn ctx_for<'a>(
        glyphs: &'a GlyphSource,
        encoding: FontEncoding,
        differences: &'a [Option<crate::GlyphName>; 256],
        flags: crate::FontFlags,
        embedded: bool,
    ) -> LadderContext<'a> {
        LadderContext {
            glyphs,
            encoding,
            differences,
            flags,
            embedded,
            base14: None,
            to_unicode: None,
            first_char: 0,
        }
    }

    const NO_DIFFS: [Option<crate::GlyphName>; 256] = [const { None }; 256];

    #[test]
    fn the_non_symbolic_branch_prefers_a_name_over_a_charmap() {
        // The Multiple-Master fallback face names its glyphs, so the name path
        // is the one that resolves — and it resolves to the same glyph the
        // charmap would, which is what makes the *precedence* observable only
        // through a face where they disagree.
        let (glyphs, _) = builtin_generic(false);
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Standard,
            &NO_DIFFS,
            crate::FontFlags(crate::FontFlags::NON_SYMBOLIC),
            true,
        );
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        non_symbolic(&ctx, &mut unicodes, &mut glyph_index);

        // `A` is at code 65 in StandardEncoding.
        assert_eq!(unicodes[65], u16::from(b'A'));
        assert_ne!(glyph_index[65], 0);
        assert_ne!(glyph_index[65], WIDTH_UNSET);
        assert_eq!(glyph_index[65], name_index(&glyphs, b"A"));
    }

    #[test]
    fn notdef_and_space_take_the_sentinel_rather_than_a_second_lookup() {
        let (glyphs, _) = builtin_generic(false);
        let mut diffs = NO_DIFFS;
        diffs[10] = Some(crate::GlyphName::from(".notdef"));
        diffs[11] = Some(crate::GlyphName::from("nosuchglyphname"));
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Standard,
            &diffs,
            crate::FontFlags(crate::FontFlags::NON_SYMBOLIC),
            true,
        );
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        non_symbolic(&ctx, &mut unicodes, &mut glyph_index);

        // `.notdef` is not worth a second lookup: unicode becomes a space and
        // the glyph is the explicit "draw nothing" sentinel.
        assert_eq!(unicodes[10], 0x20);
        assert_eq!(glyph_index[10], WIDTH_UNSET);
        // An unknown name *is* worth one, and simply finds nothing.
        assert_ne!(unicodes[11], 0x20);
    }

    #[test]
    fn the_symbolic_branch_reads_the_builtin_encoding_when_there_is_no_name() {
        let (glyphs, _) = builtin_generic(false);
        // A `Builtin` encoding with no differences names nothing at all, so
        // every code takes the built-in-encoding arm.
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            crate::FontFlags(crate::FontFlags::SYMBOLIC),
            true,
        );
        assert!(ctx.char_name(65).is_none());
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        let builtin = use_type1_charmap(&ctx);
        symbolic(&ctx, builtin, &mut unicodes, &mut glyph_index);

        // The face's own encoding maps 65 to `A`, and reversing the glyph
        // recovers the unicode.
        assert_ne!(glyph_index[65], 0);
        assert_eq!(unicodes[65], u16::from(b'A'));
    }

    #[test]
    fn use_type1_charmap_picks_the_non_unicode_charmap() {
        let (glyphs, _) = builtin_generic(false);
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            crate::FontFlags::DEFAULT,
            true,
        );
        // A Type 1 face reports Unicode first and its own encoding second, so
        // index 1 is chosen.
        assert_eq!(use_type1_charmap(&ctx), Charmap::Index(1));
    }

    #[test]
    fn a_face_with_no_charmaps_selects_none() {
        let glyphs = GlyphSource::None;
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Builtin,
            &NO_DIFFS,
            crate::FontFlags::DEFAULT,
            true,
        );
        assert_eq!(use_type1_charmap(&ctx), Charmap::None);
    }

    #[test]
    fn differences_reach_the_ladder_and_win() {
        let (glyphs, _) = builtin_generic(false);
        let mut diffs = NO_DIFFS;
        // Code 39 is `quoteright` in StandardEncoding; name it `A` instead.
        diffs[39] = Some(crate::GlyphName::from("A"));
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Standard,
            &diffs,
            crate::FontFlags(crate::FontFlags::NON_SYMBOLIC),
            true,
        );
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        non_symbolic(&ctx, &mut unicodes, &mut glyph_index);
        assert_eq!(unicodes[39], u16::from(b'A'));
        assert_eq!(glyph_index[39], name_index(&glyphs, b"A"));
    }

    #[test]
    fn a_face_with_no_glyphs_leaves_every_code_unset() {
        let glyphs = GlyphSource::None;
        let ctx = ctx_for(
            &glyphs,
            FontEncoding::Standard,
            &NO_DIFFS,
            crate::FontFlags::DEFAULT,
            false,
        );
        let mut unicodes = [0u16; 256];
        let mut glyph_index = [WIDTH_UNSET; 256];
        load_glyph_map(&ctx, &mut unicodes, &mut glyph_index);
        // Names still resolve to unicodes even with no face...
        assert_eq!(unicodes[65], u16::from(b'A'));
        // ...but no glyph does.
        assert!(glyph_index.iter().all(|&g| g == 0 || g == WIDTH_UNSET));
    }
}
