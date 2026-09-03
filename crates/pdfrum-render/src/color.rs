//! Turning a page object's `ColorValue` into the 32-bit colour a device draws
//! with.
//!
//! Four details in that pipeline are pixel-visible and none of them is what a
//! re-derivation would produce: the alpha is truncated from `alpha * 255`, not
//! rounded; the transfer function applies to the colour before any grayscale
//! translation; a missing colour inherits from the enclosing render state
//! rather than defaulting to black; and the invisibility sentinel is the whole
//! 32-bit word `0xFFFFFFFF`, which a resolved colour can never be.
//!
//! That last one is the trap: a resolved colour occupies the low 24 bits, so
//! a genuinely white fill is `0x00FFFFFF`, while `0xFFFFFFFF` is produced only
//! when nothing resolved and by the pattern fallback. Testing the *colour* for
//! white instead of the word for the sentinel makes every white object in the
//! corpus paint nothing.

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
///
/// `Hash` because a stencil's ink is part of a rendered image's cache key
/// (`crate::imagecache::PixmapRequest`): four bytes with a derived `Eq`, so
/// the derived hash agrees with it by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// `gray = (b*11 + g*59 + r*30) / 100`, integer and truncating.
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

/// Identity in normal and alpha modes, a gray collapse otherwise.
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

/// What a colour ref resolved to, in the three states the oracle
/// distinguishes.
///
/// The distinction matters because two of them are white. A resolved colour
/// occupies the **low 24 bits**, so a genuinely white fill is `0x00FFFFFF`;
/// the "nothing resolved" value is `0xFFFFFFFF`, with the top byte set, and
/// the invisibility test compares the whole 32-bit word.
///
/// So **a white fill is white, not invisible.** Collapsing the two costs
/// every white object in the corpus: a transparency group painting a white
/// square through a soft mask paints nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ColorRef {
    /// A colour the space produced.
    Resolved(Rgb),
    /// `0xFFFFFFFF` — nothing resolved, and the object is invisible.
    Invisible,
    /// No colour of its own, so the enclosing state's is inherited.
    Missing,
}

/// Resolve a colour value to one of the three [`ColorRef`] states.
///
/// A **pattern** never resolves through its colour space in the ordinary way:
/// the pattern space's own answer wins when it has one, and otherwise one of
/// two sentinels — mid grey for a **coloured tiling** pattern, which is
/// visible, and the invisibility word for everything else. That path
/// rarely reaches a pixel, because a pattern is drained out of the ordinary
/// draw; the exception is a type-3 text object, whose `GetFillArgbForType3`
/// runs before the pattern check.
#[must_use]
fn color_ref(value: &ColorValue) -> ColorRef {
    if let Some(rgb) = value.to_rgb() {
        return ColorRef::Resolved(rgb);
    }
    let Some(pattern) = value.pattern.as_ref() else {
        return ColorRef::Missing;
    };
    let colored_tiling = matches!(
        pattern.loaded.as_deref(),
        Some(pdfrum_page::Pattern::Tiling(t)) if t.colored
    );
    if colored_tiling {
        // `0x00BFBFBF`: mid grey, chosen precisely so it is *not* the
        // invisibility word.
        ColorRef::Resolved(Rgb {
            r: 191.0 / 255.0,
            g: 191.0 / 255.0,
            b: 191.0 / 255.0,
        })
    } else {
        ColorRef::Invisible
    }
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
    let base = match color_ref(value) {
        ColorRef::Resolved(rgb) => {
            let [r, g, b] = rgb.to_bytes();
            Argb { a: 255, r, g, b }
        }
        ColorRef::Invisible => return Argb::TRANSPARENT,
        // "MissingFillColor": no colour of its own, so inherit. With nothing
        // to inherit the C++ reads a zeroed colour ref, which is black.
        ColorRef::Missing => inherited.unwrap_or(Argb::BLACK),
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
            pdfrum_page::PatternSpace {
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
    fn a_white_fill_is_white_and_not_the_invisibility_sentinel() {
        // `FXSYS_BGR` packs a resolved colour into the low 24 bits, so white
        // is `0x00FFFFFF`; the invisibility test at cpdf_renderstatus.cpp:481
        // compares the whole word against `0xFFFFFFFF`, which only
        // `value_or(0xFFFFFFFF)` and the pattern fallback ever produce.
        //
        // Reading a resolved white as the sentinel makes every white object
        // in the corpus paint nothing — a white square in a transparency
        // group, a white-on-black `/BC` mask, a white page-covering fill.
        let opts = RenderOptions::default();
        let c = resolve_argb(&gray(1.0), 1.0, None, None, &opts, ObjectKind::Path, false);
        assert_eq!(c, Argb::opaque(255, 255, 255));
        assert!(!c.is_invisible());
    }

    #[test]
    fn a_colour_that_will_not_resolve_inherits_rather_than_vanishing() {
        // A `/Separation /None` produces no colour at all. With an enclosing
        // colour it inherits; with none the C++ reads a zeroed colour ref,
        // which is black — not transparent.
        let none = ColorValue {
            space: Some(Arc::new(ColorSpace::Separation(Box::new(
                pdfrum_page::Separation {
                    none: true,
                    alternate: None,
                    tint: None,
                },
            )))),
            components: SmallVec::from_slice(&[1.0]),
            pattern: None,
        };
        let opts = RenderOptions::default();
        assert_eq!(
            resolve_argb(&none, 1.0, None, None, &opts, ObjectKind::Path, false),
            Argb::BLACK
        );
        assert_eq!(
            resolve_argb(
                &none,
                1.0,
                None,
                Some(Argb::opaque(9, 8, 7)),
                &opts,
                ObjectKind::Path,
                false
            ),
            Argb::opaque(9, 8, 7)
        );
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
            let _ = c.set_components(&[0.0, 0.0, 0.0]);
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
