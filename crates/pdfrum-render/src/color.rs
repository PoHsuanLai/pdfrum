//! Turning a page object's `ColorValue` into the 32-bit colour a device draws
//! with (`GetFillArgb`/`GetStrokeArgb`, `cpdf_renderstatus.cpp:466-532`).
//!
//! Four details in that pipeline are pixel-visible and none of them is what a
//! re-derivation would produce: a resolved colour of white-as-`0xFFFFFFFF` is
//! the "no colour" sentinel and collapses to *transparent*; the alpha is
//! truncated from `alpha * 255`, not rounded; the transfer function applies to
//! the colour before any grayscale translation; and a missing colour inherits
//! from the enclosing render state rather than defaulting to black.

use pdfrum_page::{ColorValue, Rgb};

use crate::options::{ColorMode, RenderOptions};
use crate::pixmap::alpha_byte_truncating;
use crate::transfer::TransferFunc;

/// Which kind of object a colour is being resolved for. A forced-colour
/// scheme replaces path and text colours only; images, shadings and forms
/// keep their own and are then not gray-translated either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// A path object, filled or stroked.
    Path,
    /// A text object.
    Text,
    /// An image, shading or form — colour passes through untouched.
    Other,
}

/// A resolved draw colour: straight (non-premultiplied) RGB plus an alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argb {
    /// Alpha, `0` fully transparent.
    pub a: u8,
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Argb {
    /// Fully transparent — also the `0xFFFFFFFF` sentinel's result.
    pub const TRANSPARENT: Self = Self {
        a: 0,
        r: 0,
        g: 0,
        b: 0,
    };

    /// Opaque black.
    pub const BLACK: Self = Self {
        a: 255,
        r: 0,
        g: 0,
        b: 0,
    };

    /// An opaque colour from RGB bytes.
    #[must_use]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { a: 255, r, g, b }
    }

    /// Whether anything would be painted. PDFium tests the whole 32-bit word
    /// (`if (fill_color || stroke_color)`), so a colour of exactly
    /// `0x00000000` skips the draw — which is the same predicate.
    #[must_use]
    pub fn is_invisible(self) -> bool {
        self.a == 0 && self.r == 0 && self.g == 0 && self.b == 0
    }

    /// As a `peniko::Color`, the vocabulary both backends speak.
    #[must_use]
    pub fn to_peniko(self) -> peniko::Color {
        peniko::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    /// The same colour with a different alpha.
    #[must_use]
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

/// `FXRGB2GRAY(r, g, b) = (b*11 + g*59 + r*30) / 100` (`fx_dib.h:210`).
///
/// Integer, truncating, **NTSC weights on a 0..100 scale** — not Rec. 709 and
/// not floating point. Both rasterizers ship a luminance helper using BT.709
/// coefficients; neither may be used where this formula is meant.
#[must_use]
pub fn rgb_to_gray(r: u8, g: u8, b: u8) -> u8 {
    // The weights sum to 100, so the maximum numerator is 255 * 100 and the
    // truncating divide lands in 0..=255: the narrowing is exact.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "weights summing to 100 bound the quotient to 0..=255"
    )]
    let gray = ((u32::from(b) * 11 + u32::from(g) * 59 + u32::from(r) * 30) / 100) as u8;
    gray
}

/// `CPDF_RenderOptions::TranslateColor`: identity in normal and alpha modes,
/// a gray collapse otherwise.
#[must_use]
fn translate_color(mode: ColorMode, c: Argb) -> Argb {
    match mode {
        ColorMode::Normal | ColorMode::Alpha => c,
        ColorMode::Gray | ColorMode::Forced(_) => {
            let g = rgb_to_gray(c.r, c.g, c.b);
            Argb {
                a: c.a,
                r: g,
                g,
                b: g,
            }
        }
    }
}

/// `TranslateObjectFillColor` / `TranslateObjectStrokeColor`.
#[must_use]
fn translate_object_color(opts: &RenderOptions, c: Argb, kind: ObjectKind, stroking: bool) -> Argb {
    let ColorMode::Forced(scheme) = opts.color_mode else {
        return translate_color(opts.color_mode, c);
    };
    let replacement = match (kind, stroking) {
        (ObjectKind::Path, false) => scheme.path_fill,
        (ObjectKind::Path, true) => scheme.path_stroke,
        (ObjectKind::Text, false) => scheme.text_fill,
        (ObjectKind::Text, true) => scheme.text_stroke,
        (ObjectKind::Other, _) => return c,
    };
    Argb {
        a: c.a,
        ..replacement
    }
}

/// PDFium's `FX_COLORREF` "no colour resolvable" sentinel: `0x00FFFFFF`, i.e.
/// pure white in the packed BGR word. `GetFillArgb` returns ARGB `0` for it,
/// so such an object is *invisible*, not white. Uncoloured tiling patterns use
/// `0x00BFBFBF` precisely so they do not land on it.
#[must_use]
fn is_sentinel(rgb: Rgb) -> bool {
    rgb.to_bytes() == [255, 255, 255]
}

/// The colour ref a pattern colour gets when its operands resolve to nothing
/// (`CPDF_ColorState::SetPattern`, `cpdf_colorstate.cpp:133-144`).
///
/// A pattern is normally *drained* out of the ordinary draw and painted by the
/// pattern machinery, so this colour rarely reaches a pixel — but it does in
/// the one place the drain does not happen: a **type-3 text object**, whose
/// `GetFillArgbForType3` runs before the pattern check and establishes the
/// colour every uncoloured operation inside its glyph procedures then takes.
///
/// The two fallbacks differ, and the difference is the whole point. A
/// **coloured tiling** pattern gets mid grey, which is visible; everything
/// else — a shading pattern, an uncoloured tiling one — gets white, which is
/// the invisibility sentinel and therefore paints *nothing*. Reading that as
/// "no colour, so inherit" instead makes a shading-patterned type-3 run paint
/// solid black glyphs where the oracle paints none at all.
#[must_use]
fn pattern_fallback(value: &ColorValue) -> Option<Rgb> {
    let pattern = value.pattern.as_ref()?;
    let colored_tiling = matches!(
        pattern.loaded.as_deref(),
        Some(pdfrum_page::Pattern::Tiling(t)) if t.colored
    );
    Some(if colored_tiling {
        Rgb {
            r: 191.0 / 255.0,
            g: 191.0 / 255.0,
            b: 191.0 / 255.0,
        }
    } else {
        // `0xFFFFFFFF`, which `is_sentinel` then turns into transparent.
        Rgb {
            r: 1.0,
            g: 1.0,
            b: 1.0,
        }
    })
}

/// Resolve a page object's colour into the ARGB a device paints with.
///
/// `inherited` is the enclosing render state's colour, which a form `XObject`
/// with no colour operators of its own picks up (`initial_states_`).
/// `imposed` is a type-3 char proc's caller-imposed colour, which wins outright
/// for an uncoloured glyph procedure.
#[must_use]
pub fn resolve_argb(
    value: &ColorValue,
    alpha: f32,
    transfer: Option<&TransferFunc>,
    inherited: Option<Argb>,
    opts: &RenderOptions,
    kind: ObjectKind,
    stroking: bool,
) -> Argb {
    let resolved = value.to_rgb().or_else(|| pattern_fallback(value));
    let base = match resolved {
        Some(rgb) if !is_sentinel(rgb) => {
            let [r, g, b] = rgb.to_bytes();
            Argb { a: 255, r, g, b }
        }
        Some(_) => return Argb::TRANSPARENT,
        // "MissingFillColor": no colour of its own, so inherit. With nothing
        // to inherit the C++ reads a zeroed colour ref, which is black.
        None => inherited.unwrap_or(Argb::BLACK),
    };

    let a = alpha_byte_truncating(alpha);
    let transferred = match transfer {
        Some(tf) if !tf.is_identity() => tf.translate(base),
        _ => base,
    };
    translate_object_color(opts, transferred.with_alpha(a), kind, stroking)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfrum_page::ColorSpace;
    use smallvec::SmallVec;

    use super::*;

    fn gray(v: f32) -> ColorValue {
        let mut c = ColorValue::default();
        c.set_stock(ColorSpace::DeviceGray, &[v]);
        c
    }

    #[test]
    fn a_shading_pattern_colour_is_the_invisibility_sentinel() {
        // A pattern is normally drained out of the draw, but a type-3 text
        // object establishes its colour before the drain — and a shading
        // pattern's colour ref is `0xFFFFFFFF`, which is transparent, not
        // black. Reading it as "no colour, so inherit" paints solid glyphs.
        let mut c = ColorValue::default();
        c.set_space(Arc::new(ColorSpace::Pattern(Box::default())));
        c.set_pattern(pdfrum_object::Name::from("P0"), &[], None);
        let argb = resolve_argb(
            &c,
            1.0,
            None,
            None,
            &RenderOptions::default(),
            ObjectKind::Text,
            false,
        );
        assert!(argb.is_invisible(), "got {argb:?}");
    }

    #[test]
    fn a_pattern_colour_that_resolves_keeps_its_colour() {
        // With a base space the operands resolve normally and no fallback
        // applies at all.
        let mut c = ColorValue::default();
        c.set_space(Arc::new(ColorSpace::Pattern(Box::new(
            pdfrum_page::color::PatternSpace {
                base: Some(Box::new(ColorSpace::DeviceRgb)),
            },
        ))));
        c.set_pattern(pdfrum_object::Name::from("P0"), &[1.0, 0.0, 0.0], None);
        let argb = resolve_argb(
            &c,
            1.0,
            None,
            None,
            &RenderOptions::default(),
            ObjectKind::Text,
            false,
        );
        assert_eq!(argb, Argb::opaque(255, 0, 0));
    }

    #[test]
    fn sentinel_white_is_transparent() {
        // cpdf_renderstatus.cpp:502 — colorref 0xFFFFFFFF returns ARGB 0.
        let opts = RenderOptions::default();
        let c = resolve_argb(&gray(1.0), 1.0, None, None, &opts, ObjectKind::Path, false);
        assert_eq!(c, Argb::TRANSPARENT);
        assert!(c.is_invisible());
    }

    #[test]
    fn alpha_truncates() {
        // `(int32)(alpha * 255)`, with upstream's own `// not rounded.`
        let opts = RenderOptions::default();
        let c = resolve_argb(&gray(0.0), 0.5, None, None, &opts, ObjectKind::Path, false);
        assert_eq!(c.a, 127);
    }

    #[test]
    fn missing_color_inherits_from_the_enclosing_state() {
        let opts = RenderOptions::default();
        let empty = ColorValue {
            space: None,
            components: SmallVec::new(),
            ..Default::default()
        };
        let inherited = Argb::opaque(10, 20, 30);
        let c = resolve_argb(
            &empty,
            1.0,
            None,
            Some(inherited),
            &opts,
            ObjectKind::Path,
            false,
        );
        assert_eq!((c.r, c.g, c.b), (10, 20, 30));
    }

    #[test]
    fn gray_mode_uses_ntsc_weights() {
        let opts = RenderOptions {
            color_mode: ColorMode::Gray,
            ..RenderOptions::default()
        };
        let mut red = ColorValue::default();
        red.set_stock(ColorSpace::DeviceRgb, &[1.0, 0.0, 0.0]);
        let c = resolve_argb(&red, 1.0, None, None, &opts, ObjectKind::Path, false);
        // 255*30/100 = 76, not Rec.709's 54.
        assert_eq!((c.r, c.g, c.b), (76, 76, 76));
    }

    #[test]
    fn forced_scheme_leaves_images_alone() {
        let scheme = crate::options::ColorScheme {
            path_fill: Argb::opaque(1, 2, 3),
            path_stroke: Argb::opaque(4, 5, 6),
            text_fill: Argb::opaque(7, 8, 9),
            text_stroke: Argb::opaque(10, 11, 12),
        };
        let opts = RenderOptions {
            color_mode: ColorMode::Forced(scheme),
            ..RenderOptions::default()
        };
        let value = {
            let mut c = ColorValue::default();
            c.set_space(Arc::new(ColorSpace::DeviceRgb));
            c.set_components(&[0.0, 0.0, 0.0]);
            c
        };
        let path = resolve_argb(&value, 1.0, None, None, &opts, ObjectKind::Path, false);
        assert_eq!((path.r, path.g, path.b), (1, 2, 3));
        let other = resolve_argb(&value, 1.0, None, None, &opts, ObjectKind::Other, false);
        assert_eq!((other.r, other.g, other.b), (0, 0, 0));
    }

    #[test]
    fn rgb_to_gray_is_the_oracle_formula() {
        assert_eq!(rgb_to_gray(255, 255, 255), 255);
        assert_eq!(rgb_to_gray(0, 0, 0), 0);
        assert_eq!(rgb_to_gray(255, 0, 0), 76);
        assert_eq!(rgb_to_gray(0, 255, 0), 150);
        assert_eq!(rgb_to_gray(0, 0, 255), 28);
    }
}
