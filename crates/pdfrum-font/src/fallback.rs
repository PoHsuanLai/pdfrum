//! Per-glyph Arial fallback (`ShouldUseFont` / `GetCharPosList`).
//!
//! PDFium does not stop at "no glyph → empty cell". After
//! `GlyphFromCharCode`, `GetCharPosList` can swap the **entire face** for a
//! lazily created Arial substitute and look the character's Unicode up there.
//! The original font still supplies the advance, so the rest of the run stays
//! put while those cells become Arial ink instead of `.notdef` or nothing.
//!
//! Two distinct triggers, transcribed from `cpdf_font.cpp:368-389`:
//!
//! - `gid == -1` (`None` here) — **any** simple or CID font, embedded or not.
//! - `gid == 0` and no `/ToUnicode` — **non-embedded TrueType only**.
//!
//! Type 3 fonts never take this path: they have no glyph indices, and
//! `ProcessType3Text` does not call `GetCharPosList`.

use std::sync::OnceLock;

use crate::glyphs::{Charmap, GlyphSource, SynthGlyph};
use crate::ids::{CharCode, FontFlags, FontId, Gid};
use crate::subst::{self, CodePage, FontRequest, SubstFont, SubstitutionOptions};
use pdfrum_common::Diagnostics;
use pdfrum_common::kurbo::BezPath;

/// The Arial stand-in `LoadSubstFace("Arial", …)` builds on first miss.
#[derive(Debug)]
pub struct GlyphFallback {
    /// The substitute face.
    pub(crate) glyphs: GlyphSource,
    /// The synthetic skew and embolden that follow from the request.
    pub(crate) subst: SubstFont,
    /// A font identity distinct from the host's, so a glyph-cache entry for
    /// Arial's gid 5 cannot be confused with the host's gid 5.
    pub(crate) id: FontId,
}

impl GlyphFallback {
    /// This stand-in's identity, for glyph-cache keys.
    #[must_use]
    pub fn id(&self) -> FontId {
        self.id
    }

    /// The glyph Arial draws for this character, or `None` when Arial has
    /// none either (`FallbackGlyphFromCharcode` returning `-1`).
    ///
    /// Unicode comes from the host's `UnicodeFromCharCode`; an empty mapping
    /// falls back to the raw character code, which is what the C++ passes to
    /// `GetCharIndex`.
    #[must_use]
    pub fn gid(&self, unicode: &[char], code: CharCode) -> Option<Gid> {
        let u = unicode.first().copied().map_or(code.0, u32::from);
        let gid = self.glyphs.char_index(Charmap::Unicode, u);
        (gid != 0).then_some(Gid(gid))
    }

    /// A glyph's outline, grid-fitted at 64 ppem, for the bitmap path.
    #[must_use]
    pub fn hinted_path(&self, gid: Gid) -> Option<BezPath> {
        self.glyphs.hinted_outline(gid)
    }

    /// The face's own advance for this glyph, in 1000/em units. The spacing
    /// heuristic compares this to the host font's `/Widths`.
    #[must_use]
    pub fn advance(&self, gid: Gid) -> i32 {
        self.glyphs
            .advance(gid, crate::glyphs::GlyphParams::default())
    }

    /// The synthetic italic and embolden the bitmap path applies, resolved
    /// against the device matrix. Arial is never a CID font.
    #[must_use]
    pub fn render_synth(&self, xx: i32, xy: i32, vertical: bool) -> Option<SynthGlyph> {
        let level = self.subst.embolden_level_for_render(false, xx, xy)?;
        Some(SynthGlyph {
            skew: self.subst.effective_skew(false),
            vertical,
            embolden: f64::from(level) / 64.0,
        })
    }
}

/// Whether this character's glyph is drawn from the host font
/// (`ShouldUseFont`).
///
/// `is_truetype` is the PDF `/Subtype`, not the face: a `CIDFontType2` is not
/// a TrueType font in this test, so `.notdef` stays `.notdef`.
#[must_use]
pub(crate) fn should_use_own_glyph(
    embedded: bool,
    is_truetype: bool,
    has_to_unicode: bool,
    gid: Option<Gid>,
) -> bool {
    let Some(gid) = gid else {
        return false;
    };
    if embedded {
        return true;
    }
    if !is_truetype {
        return true;
    }
    gid != Gid(0) || has_to_unicode
}

/// Materialize the Arial stand-in on first miss.
///
/// Weight is `stem_v * 5`, saturating to 400 on overflow — `FX_SAFE_INT32`
/// plus `kFontWeightNormal`. The high bit of `host_id` distinguishes this
/// face from the host without needing the document's [`FontCache`].
pub(crate) fn ensure(
    slot: &OnceLock<Option<GlyphFallback>>,
    host_id: FontId,
    is_truetype: bool,
    flags: FontFlags,
    stem_v: i32,
    italic_angle: i32,
    vertical: bool,
) -> Option<&GlyphFallback> {
    slot.get_or_init(|| {
        let weight = i32::try_from(i64::from(stem_v).saturating_mul(5)).unwrap_or(400);
        let request = FontRequest {
            name: b"Arial".to_vec(),
            is_truetype,
            flags,
            weight,
            italic_angle,
            code_page: CodePage::DefAnsi,
            vertical,
        };
        let resolved = subst::resolve_with_options(
            &request,
            &SubstitutionOptions::default(),
            &mut Diagnostics::with_limit(0),
        );
        if !resolved.glyphs.is_some() {
            return None;
        }
        Some(GlyphFallback {
            glyphs: resolved.glyphs,
            subst: resolved.subst,
            id: FontId(host_id.0 | (1 << 63)),
        })
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_glyph_always_falls_back() {
        assert!(!should_use_own_glyph(true, true, true, None));
        assert!(!should_use_own_glyph(false, false, false, None));
    }

    #[test]
    fn embedded_notdef_is_kept() {
        assert!(should_use_own_glyph(true, true, false, Some(Gid(0))));
    }

    #[test]
    fn non_embedded_truetype_notdef_without_tounicode_falls_back() {
        assert!(!should_use_own_glyph(false, true, false, Some(Gid(0))));
    }

    #[test]
    fn tounicode_keeps_a_truetype_notdef() {
        assert!(should_use_own_glyph(false, true, true, Some(Gid(0))));
    }

    #[test]
    fn type1_notdef_is_kept() {
        assert!(should_use_own_glyph(false, false, false, Some(Gid(0))));
    }

    #[test]
    fn a_real_glyph_is_kept() {
        assert!(should_use_own_glyph(false, true, false, Some(Gid(1))));
    }
}
