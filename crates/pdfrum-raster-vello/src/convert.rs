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
pub fn to_fill(rule: FillRule) -> vpeniko::Fill {
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
/// pixel-centre scan converter. That difference is why abutting Coons cells
/// are drawn opaque into a scratch buffer: vello claims a half-covered shared
/// pixel for *both* cells, which is only harmless when both write the same
/// opaque colour.
#[must_use]
pub fn to_aliasing_threshold(aa: AntiAlias) -> Option<u8> {
    match aa {
        AntiAlias::On => None,
        AntiAlias::Off => Some(128),
    }
}

/// The image sampling quality.
///
/// `vello_cpu` internally downgrades `Medium` to `Low` for an integer-only
/// translation; the engine detects that case and passes
/// [`ImageQuality::Nearest`] itself, so the downgrade becomes a no-op and
/// both backends receive the same value.
#[must_use]
pub fn to_image_quality(q: ImageQuality) -> vpeniko::ImageQuality {
    match q {
        ImageQuality::Nearest => vpeniko::ImageQuality::Low,
        ImageQuality::Bilinear => vpeniko::ImageQuality::Medium,
    }
}

/// A PDF blend mode as peniko's mix mode.
#[must_use]
pub fn to_mix(mode: BlendMode) -> vpeniko::Mix {
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
pub fn to_blend_mode(mode: BlendMode) -> vpeniko::BlendMode {
    vpeniko::BlendMode::new(to_mix(mode), vpeniko::Compose::SrcOver)
}

/// An engine `Pixmap` as a vello one, keeping the premultiplied bytes.
#[must_use]
pub fn to_vello_pixmap(p: &Pixmap) -> Option<vello_cpu::Pixmap> {
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

/// An engine [`Pixmap`] built from a vello one.
#[must_use]
pub fn from_vello_pixmap(p: &vello_cpu::Pixmap) -> Pixmap {
    Pixmap::from_vec(
        u32::from(p.width()),
        u32::from(p.height()),
        p.data_as_u8_slice().to_vec(),
    )
    .unwrap_or_else(|| Pixmap::new(u32::from(p.width()), u32::from(p.height())))
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
    fn pixmap_round_trips() {
        let p = Pixmap::filled(3, 2, peniko::Color::from_rgba8(10, 20, 30, 255));
        let v = to_vello_pixmap(&p).expect("converts");
        let back = from_vello_pixmap(&v);
        assert_eq!(back.pixel(1, 1), Some([10, 20, 30, 255]));
    }
}
