//! What a caller can ask of a render, and the defaults that reproduce the
//! oracle (`CPDF_RenderOptions`, `cpdf_renderoptions.h:19-84`).
//!
//! `pdfium_test` renders every golden with `flags = FPDF_ANNOT` and nothing
//! else, so the whole option surface collapses to its defaults: colour mode
//! normal, no forced colours, and — the one that surprises — `bClearType
//! = false`, because `RenderPageImpl` overwrites the constructor's `true`
//! from the flag word on every public render call. Subpixel text therefore
//! never runs in conformance.

use kurbo::Affine;

use crate::color::Argb;

/// The four colour modes. Three are reachable without any caller asking:
/// `Alpha` from an alpha-type soft mask and from an uncoloured tiling
/// pattern, `Gray` from `FPDF_GRAYSCALE`, `Forced` from a colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Colours pass through unchanged.
    #[default]
    Normal,
    /// Every path and text colour collapses to its NTSC gray.
    Gray,
    /// Drawing writes *alpha as gray*: the mode a soft mask's alpha group and
    /// an uncoloured tile render in.
    Alpha,
    /// Path and text colours are replaced wholesale; images, shadings and
    /// forms keep their own and are not gray-translated either.
    Forced(ColorScheme),
}

/// The four replacement colours a forced-colour render substitutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorScheme {
    /// Replaces every path fill.
    pub path_fill: Argb,
    /// Replaces every path stroke.
    pub path_stroke: Argb,
    /// Replaces every text fill.
    pub text_fill: Argb,
    /// Replaces every text stroke.
    pub text_stroke: Argb,
}

/// How glyphs are antialiased.
///
/// The oracle's own choice with `FPDF_ANNOT` alone is [`TextAa::Grayscale`]:
/// `bNoTextSmooth` and `bClearType` are both false, so `kAntiAliasing` wins
/// and LCD filtering never runs. [`TextAa::None`] is `bNoTextSmooth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAa {
    /// Grayscale coverage — the oracle's conformance setting.
    #[default]
    Grayscale,
    /// Hard-edged glyph fills.
    None,
}

/// Everything a render is parameterised by.
///
/// Constructed with `..RenderOptions::default()` struct update, per STYLE §4.
/// The defaults are the oracle's: reproducing a golden needs no configuration
/// beyond the target size.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "these are `CPDF_RenderOptions::Options`' independent bit flags \
              one for one; grouping them into sub-structs or an enum would \
              hide which upstream flag each is and make the port unreviewable"
)]
pub struct RenderOptions {
    /// Page space to device space. `render_page` composes this with the
    /// page's own display matrix.
    pub transform: Affine,
    /// The colour mode, normally [`ColorMode::Normal`].
    pub color_mode: ColorMode,
    /// Glyph antialiasing.
    pub text_aa: TextAa,
    /// `bNoPathSmooth`: hard-edge every path fill and stroke.
    pub no_path_smooth: bool,
    /// `bNoImageSmooth`: never interpolate an image, whatever `/Interpolate`
    /// or the size heuristic say. Wins over both.
    pub no_image_smooth: bool,
    /// `bForceHalftone`. Forced on inside type-3 char procs and uncoloured
    /// tile cells, so it reaches a render even with the oracle's bare flags.
    pub force_halftone: bool,
    /// `bRectAA`: antialias axis-aligned rectangle fills instead of
    /// integer-snapping them. Forced on inside type-3 char procs.
    pub rect_aa: bool,
    /// `bConvertFillToStroke`, read only under a forced colour scheme.
    pub convert_fill_to_stroke: bool,
    /// The page background. `None` follows the oracle: opaque white for a
    /// page without transparency, fully transparent for one with it.
    ///
    /// This is load-bearing rather than cosmetic — a white-backed page
    /// composites differently at the edges from a transparent-backed one, and
    /// the engine's single compositing path relies on the white being real
    /// pixels where the oracle would have replayed a white backdrop.
    pub background: Option<peniko::Color>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            transform: Affine::IDENTITY,
            color_mode: ColorMode::Normal,
            text_aa: TextAa::Grayscale,
            no_path_smooth: false,
            no_image_smooth: false,
            force_halftone: false,
            rect_aa: false,
            convert_fill_to_stroke: false,
            background: None,
        }
    }
}

impl RenderOptions {
    /// The background a page with the given transparency renders onto:
    /// `0xFFFFFFFF` white when opaque, `0x00000000` when transparent
    /// (`pdfium_test.cc:1070-1088`), unless overridden.
    #[must_use]
    pub fn background_for(&self, has_transparency: bool) -> peniko::Color {
        self.background.unwrap_or(if has_transparency {
            peniko::Color::TRANSPARENT
        } else {
            peniko::Color::WHITE
        })
    }

    /// Whether path geometry is antialiased under these options.
    #[must_use]
    pub fn path_aa(&self) -> crate::device::AntiAlias {
        if self.no_path_smooth {
            crate::device::AntiAlias::Off
        } else {
            crate::device::AntiAlias::On
        }
    }

    /// Whether glyph outlines are antialiased under these options.
    #[must_use]
    pub fn text_antialias(&self) -> crate::device::AntiAlias {
        match self.text_aa {
            TextAa::Grayscale => crate::device::AntiAlias::On,
            TextAa::None => crate::device::AntiAlias::Off,
        }
    }

    /// The options a type-3 char proc renders under: `bForceHalftone` and
    /// `bRectAA` forced on (`cpdf_renderstatus.cpp:997-998`), which means a
    /// rectangle inside a type-3 glyph *is* antialiased where the same
    /// rectangle on the page would not be.
    #[must_use]
    pub fn for_type3_char_proc(&self) -> Self {
        Self {
            force_halftone: true,
            rect_aa: true,
            ..self.clone()
        }
    }

    /// The options an uncoloured tiling pattern's cell renders under: alpha
    /// colour mode plus `bForceHalftone` (`cpdf_rendertiling.cpp:61-66`).
    #[must_use]
    pub fn for_uncolored_tile(&self) -> Self {
        Self {
            color_mode: ColorMode::Alpha,
            force_halftone: true,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_defaults() {
        let o = RenderOptions::default();
        // The whole point: `pdfium_test` passes FPDF_ANNOT and nothing else,
        // so bClearType is false and text is plain grayscale-antialiased.
        assert_eq!(o.text_aa, TextAa::Grayscale);
        assert_eq!(o.color_mode, ColorMode::Normal);
        assert!(!o.rect_aa);
        assert!(!o.force_halftone);
    }

    #[test]
    fn background_follows_page_transparency() {
        let o = RenderOptions::default();
        assert_eq!(o.background_for(false), peniko::Color::WHITE);
        assert_eq!(o.background_for(true), peniko::Color::TRANSPARENT);
        let forced = RenderOptions {
            background: Some(peniko::Color::BLACK),
            ..RenderOptions::default()
        };
        assert_eq!(forced.background_for(true), peniko::Color::BLACK);
    }

    #[test]
    fn type3_forces_rect_aa_and_halftone() {
        let inner = RenderOptions::default().for_type3_char_proc();
        assert!(inner.rect_aa);
        assert!(inner.force_halftone);
    }

    #[test]
    fn uncolored_tile_renders_in_alpha_mode() {
        let inner = RenderOptions::default().for_uncolored_tile();
        assert_eq!(inner.color_mode, ColorMode::Alpha);
        assert!(inner.force_halftone);
    }
}
