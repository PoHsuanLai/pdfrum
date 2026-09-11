//! What a caller can ask of a render, and the defaults that reproduce the
//! oracle.
//!
//! The conformance corpus renders with annotations on and nothing else, so
//! the whole option surface collapses to its defaults.

// `pdfium_test` renders every golden with `flags = FPDF_ANNOT` and nothing
// else: colour mode normal, no forced colours, and — the one that surprises
// — `bClearType = false`, because `RenderPageImpl` overwrites the
// constructor's `true` from the flag word on every public render call.
//
// **`bClearType = false` does not mean the LCD path is off.** It sets
// `CFX_TextRenderOptions::aliasing_type` to `kAntiAliasing`, and that is a
// different variable from `FontAntiAliasingMode`, which `DrawNormalText`
// derives separately (`cfx_renderdevice.cpp:1165-1206`): on a display device
// at 32 bpp with a smooth aliasing type the mode is `kLcd` *whatever*
// `bClearType` said, and `aliasing_type` only decides `normalize`. It matters
// for exactly one thing — the glyph-origin snap floors in x under `kLcd` and
// rounds under `kMono`.

use kurbo::Affine;

use crate::color::Argb;

/// The four colour modes. Three are reachable without any caller asking:
/// `Alpha` from an alpha-type soft mask and from an uncoloured tiling
/// pattern, `Gray` from a grayscale render, `Forced` from a colour scheme.
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

impl ColorScheme {
    /// A scheme that names each replacement colour.
    #[must_use]
    pub const fn new(
        path_fill: Argb,
        path_stroke: Argb,
        text_fill: Argb,
        text_stroke: Argb,
    ) -> Self {
        Self {
            path_fill,
            path_stroke,
            text_fill,
            text_stroke,
        }
    }

    /// The same colour for every path and text fill and stroke.
    #[must_use]
    pub const fn all(color: Argb) -> Self {
        Self::new(color, color, color, color)
    }
}

/// How glyphs are antialiased.
///
/// [`TextAa::Grayscale`] is the oracle's own choice on an ordinary page: the
/// three subpixel coverages are averaged back to grey. [`TextAa::LcdSubpixel`]
/// is reachable, but only per-draw — see its own note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAa {
    /// Grayscale coverage — the oracle's conformance setting.
    #[default]
    Grayscale,
    /// Hard-edged glyph fills.
    None,
    /// `ClearType`: each pixel's three LCD stripes keep their own coverage and
    /// merge into their own destination channel, so a glyph drawn in one colour
    /// carries colour fringes.
    ///
    /// It is **not** something only a caller asks for: a live edit's text —
    /// and only that text — is drawn with `ClearType` on while the rest of
    /// the page is not, which is why this is a *per-draw* selection rather
    /// than a whole-render one. See [`RenderOptions::text_aa_override`].
    LcdSubpixel,
}

/// Everything a render is parameterised by.
///
/// Constructed with `..RenderOptions::default()` struct update. The defaults
/// are the oracle's: reproducing a golden needs no configuration beyond the
/// target size.
#[derive(Debug, PartialEq)]
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
    /// Glyph antialiasing, for the whole render.
    pub text_aa: TextAa,
    /// Glyph antialiasing for *this draw only*, overriding [`Self::text_aa`].
    ///
    /// Text antialiasing is per-draw, not per-render: on a page whose every
    /// other run is grayscale, a live edit's text is drawn with `ClearType`.
    ///
    /// `None` — the default — means [`Self::text_aa`] decides. A caller
    /// drawing one run differently sets this on a clone of its options for
    /// that run, so the page's own setting stays readable.
    pub text_aa_override: Option<TextAa>,
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
    /// Place each glyph at its true fractional device origin instead of
    /// snapping it to a whole pixel.
    ///
    /// **Default `false`, which is the oracle**: small text is placed on a
    /// grid of whole pixels in y and thirds of a pixel in x. Set this to
    /// `true` when a caller wants text where the PDF actually puts it —
    /// smooth animation, a non-integer device scale, any use where oracle
    /// parity is not the goal — at the cost of matching a golden.
    ///
    /// No effect on large text: above the size threshold glyphs are placed
    /// fractionally either way.
    pub subpixel_text_positioning: bool,
    /// The page background. `None` follows the oracle: opaque white for a
    /// page without transparency, fully transparent for one with it.
    ///
    /// This is load-bearing rather than cosmetic — a white-backed page
    /// composites differently at the edges from a transparent-backed one, and
    /// the engine's single compositing path relies on the white being real
    /// pixels where the oracle would have replayed a white backdrop.
    pub background: Option<peniko::Color>,
}

// Hand-written so that the nested contexts a walk builds — a form's, a char
// proc's, a tile cell's, a soft mask's, and one per *run* of a text clip —
// are countable at a single point. The body is `*self`: every field is
// `Copy`, so deriving `Clone` would produce exactly this code and would leave
// the count unanswerable.
impl Clone for RenderOptions {
    fn clone(&self) -> Self {
        crate::walkprofile::alloc_items(
            crate::walkprofile::Site::OptionsClone,
            1,
            core::mem::size_of::<Self>(),
        );
        Self { ..*self }
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            transform: Affine::IDENTITY,
            color_mode: ColorMode::Normal,
            text_aa: TextAa::Grayscale,
            text_aa_override: None,
            no_path_smooth: false,
            no_image_smooth: false,
            force_halftone: false,
            rect_aa: false,
            convert_fill_to_stroke: false,
            subpixel_text_positioning: false,
            background: None,
        }
    }
}

impl RenderOptions {
    /// The background a page with the given transparency renders onto:
    /// opaque white when the page is opaque, fully transparent when it is
    /// not, unless [`RenderOptions::background`] overrides it.
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

    /// The text antialiasing in force for the draw being made: the per-draw
    /// [`Self::text_aa_override`] when a caller set one, else [`Self::text_aa`].
    #[must_use]
    pub fn effective_text_aa(&self) -> TextAa {
        self.text_aa_override.unwrap_or(self.text_aa)
    }

    /// The options one text run draws under with `aa` forced, leaving the
    /// page's own options untouched.
    ///
    /// This is what a live edit's text draws through.
    #[must_use]
    pub fn for_text_run(&self, aa: TextAa) -> Self {
        Self {
            text_aa_override: Some(aa),
            ..self.clone()
        }
    }

    /// Whether glyph *outlines* are antialiased under these options.
    ///
    /// The outline path has no subpixel spelling, so [`TextAa::LcdSubpixel`]
    /// antialiases here exactly like [`TextAa::Grayscale`]. The subpixel
    /// choice is expressed on the *bitmap* path, the only place the oracle
    /// expresses it either.
    #[must_use]
    pub fn text_antialias(&self) -> crate::device::AntiAlias {
        match self.effective_text_aa() {
            TextAa::Grayscale | TextAa::LcdSubpixel => crate::device::AntiAlias::On,
            TextAa::None => crate::device::AntiAlias::Off,
        }
    }

    /// The options a type-3 char proc renders under: forced halftone and
    /// rectangle antialiasing, which means a rectangle inside a type-3 glyph
    /// *is* antialiased where the same rectangle on the page would not be.
    #[must_use]
    pub fn for_type3_char_proc(&self) -> Self {
        Self {
            force_halftone: true,
            rect_aa: true,
            ..self.clone()
        }
    }

    /// The options an uncoloured tiling pattern's cell renders under: alpha
    /// colour mode plus forced halftone.
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
    fn a_per_draw_override_wins_over_the_render_wide_setting() {
        // The shape the oracle's `DrawTextString` has: one run drawn with
        // ClearType on a page whose every other run is grayscale.
        let page = RenderOptions::default();
        assert_eq!(page.effective_text_aa(), TextAa::Grayscale);
        let run = page.for_text_run(TextAa::LcdSubpixel);
        assert_eq!(run.effective_text_aa(), TextAa::LcdSubpixel);
        // And the page's own setting is untouched, which is what makes this an
        // override rather than a mutation.
        assert_eq!(page.effective_text_aa(), TextAa::Grayscale);
        assert_eq!(run.text_aa, TextAa::Grayscale);
    }

    #[test]
    fn the_subpixel_mode_still_antialiases_outlines() {
        // Above the size threshold the oracle abandons bitmaps for
        // `DrawTextPath`, which reads only `!is_text_smooth` — so ClearType and
        // grayscale fill an outline identically and only `None` hard-edges it.
        let lcd = RenderOptions::default().for_text_run(TextAa::LcdSubpixel);
        assert_eq!(lcd.text_antialias(), crate::device::AntiAlias::On);
        let off = RenderOptions::default().for_text_run(TextAa::None);
        assert_eq!(off.text_antialias(), crate::device::AntiAlias::Off);
    }

    #[test]
    fn uncolored_tile_renders_in_alpha_mode() {
        let inner = RenderOptions::default().for_uncolored_tile();
        assert_eq!(inner.color_mode, ColorMode::Alpha);
        assert!(inner.force_halftone);
    }
}
