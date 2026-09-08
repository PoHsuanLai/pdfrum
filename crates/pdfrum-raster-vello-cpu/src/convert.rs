//! Translating `pdfrum-render`'s vocabulary into `vello_cpu`'s.
//!
//! `vello_cpu` speaks kurbo and peniko natively, so most of this is a
//! pass-through. The two conversions that are not — the aliasing threshold
//! and the image sampler — carry the backend's sharpest edges and are
//! documented where they sit.

use pdfrum_page::BlendMode;
use pdfrum_render::{AntiAlias, FillRule, ImageQuality, Pixmap};
use vello_cpu::peniko as vpeniko;

/// The fill rule.
#[must_use]
pub(crate) fn to_fill(rule: FillRule) -> vpeniko::Fill {
    match rule {
        FillRule::Winding => vpeniko::Fill::NonZero,
        FillRule::EvenOdd => vpeniko::Fill::EvenOdd,
    }
}

/// The aliasing threshold for a primitive.
///
/// `None` is ordinary antialiasing. `Some(128)` binarizes coverage at the
/// midpoint, which is what PDFium's `aliased_path` does too
/// (`> 127 -> 255`) — the two agree on the *rule*, unlike tiny-skia's
/// pixel-centre scan converter.
///
/// `Some(1)` is `full_cover`, and the threshold happens to express it exactly:
/// the flag keeps the rasterizer's choice of covered pixels and discards only
/// their coverage *value*, so "opaque wherever coverage is non-zero" is the
/// whole rule. Abutting Coons cells are still drawn opaque into a scratch
/// buffer, for the reason §5.3 gives — vello claims a half-covered shared
/// pixel for *both* cells, which is only harmless when both write the same
/// opaque colour.
#[must_use]
pub(crate) fn to_aliasing_threshold(aa: AntiAlias) -> Option<u8> {
    match aa {
        AntiAlias::On => None,
        AntiAlias::Off => Some(128),
        AntiAlias::FullCover => Some(1),
    }
}

/// The image sampling quality.
///
/// `vello_cpu` internally downgrades `Medium` to `Low` for an integer-only
/// translation; the engine detects that case and passes
/// [`ImageQuality::Nearest`] itself, so the downgrade becomes a no-op and
/// both backends receive the same value.
#[must_use]
pub(crate) fn to_image_quality(q: ImageQuality) -> vpeniko::ImageQuality {
    match q {
        ImageQuality::Nearest => vpeniko::ImageQuality::Low,
        ImageQuality::Bilinear => vpeniko::ImageQuality::Medium,
    }
}

/// A PDF blend mode as peniko's mix mode.
#[must_use]
pub(crate) fn to_mix(mode: BlendMode) -> vpeniko::Mix {
    match mode {
        BlendMode::Normal | BlendMode::Compatible => vpeniko::Mix::Normal,
        BlendMode::Multiply => vpeniko::Mix::Multiply,
        BlendMode::Screen => vpeniko::Mix::Screen,
        BlendMode::Overlay => vpeniko::Mix::Overlay,
        BlendMode::Darken => vpeniko::Mix::Darken,
        BlendMode::Lighten => vpeniko::Mix::Lighten,
        BlendMode::ColorDodge => vpeniko::Mix::ColorDodge,
        BlendMode::ColorBurn => vpeniko::Mix::ColorBurn,
        BlendMode::HardLight => vpeniko::Mix::HardLight,
        BlendMode::SoftLight => vpeniko::Mix::SoftLight,
        BlendMode::Difference => vpeniko::Mix::Difference,
        BlendMode::Exclusion => vpeniko::Mix::Exclusion,
        BlendMode::Hue => vpeniko::Mix::Hue,
        BlendMode::Saturation => vpeniko::Mix::Saturation,
        BlendMode::Color => vpeniko::Mix::Color,
        BlendMode::Luminosity => vpeniko::Mix::Luminosity,
    }
}

/// A blend mode as the full `peniko::BlendMode` a layer takes.
#[must_use]
pub(crate) fn to_blend_mode(mode: BlendMode) -> vpeniko::BlendMode {
    vpeniko::BlendMode::new(to_mix(mode), vpeniko::Compose::SrcOver)
}

/// An engine `Pixmap` as a vello one, keeping the premultiplied bytes.
#[must_use]
pub(crate) fn to_vello_pixmap(p: &Pixmap) -> Option<vello_cpu::Pixmap> {
    let (w, h) = (
        u16::try_from(p.width()).ok()?,
        u16::try_from(p.height()).ok()?,
    );
    let mut out = vello_cpu::Pixmap::new(w, h);
    let dst = out.data_as_u8_slice_mut();
    let src = p.data();
    let n = dst.len().min(src.len());
    dst.get_mut(..n)?.copy_from_slice(src.get(..n)?);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_edges_binarize_at_the_oracle_threshold() {
        // PDFium's aliased_path is `> 127 -> 255`, i.e. a threshold of 128 —
        // the same rule vello applies, unlike tiny-skia's pixel-centre one.
        assert_eq!(to_aliasing_threshold(AntiAlias::Off), Some(128));
        assert_eq!(to_aliasing_threshold(AntiAlias::On), None);
    }

    #[test]
    fn every_blend_mode_maps() {
        assert_eq!(to_mix(BlendMode::Compatible), vpeniko::Mix::Normal);
        assert_eq!(to_mix(BlendMode::Luminosity), vpeniko::Mix::Luminosity);
        assert_eq!(to_mix(BlendMode::SoftLight), vpeniko::Mix::SoftLight);
    }

    #[test]
    fn a_pixmap_converts_byte_for_byte() {
        // The two pixmaps are the same premultiplied RGBA8 bytes in the same
        // order, which is what lets the backend rasterize straight into an
        // engine `Pixmap` through a `PixmapMut` instead of converting back.
        let p = Pixmap::filled(3, 2, peniko::Color::from_rgba8(10, 20, 30, 255));
        let v = to_vello_pixmap(&p).expect("converts");
        assert_eq!(v.width(), 3);
        assert_eq!(v.height(), 2);
        assert_eq!(v.data_as_u8_slice(), p.data());
    }
}
