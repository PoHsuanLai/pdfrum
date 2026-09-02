//! Translating `pdfrum-render`'s vocabulary into GPU `vello`'s.
//!
//! `vello` re-exports the same `peniko` and `kurbo` the engine speaks, so most
//! of this is the identity function with a name. The two that are not — the
//! image conversion and the alpha-mask expansion — carry this backend's
//! sharpest edges and are documented where they sit.

use pdfrum_page::BlendMode;
use pdfrum_render::{AlphaMask, FillRule, ImageQuality, Pixmap};
use vello::peniko;

/// The fill rule.
#[must_use]
pub fn to_fill(rule: FillRule) -> peniko::Fill {
    match rule {
        FillRule::Winding => peniko::Fill::NonZero,
        FillRule::EvenOdd => peniko::Fill::EvenOdd,
    }
}

/// The image sampling quality.
///
/// `Nearest` and `Bilinear` are the engine's whole vocabulary, so peniko's
/// `High` (bicubic) is unreachable from here — the engine ports PDFium's
/// `/Interpolate`, `bNoSmoothing`/`bHalftone` and `UseInterpolateBilinear`
/// rules and those never ask for a third kernel.
#[must_use]
pub fn to_image_quality(q: ImageQuality) -> peniko::ImageQuality {
    match q {
        ImageQuality::Nearest => peniko::ImageQuality::Low,
        ImageQuality::Bilinear => peniko::ImageQuality::Medium,
    }
}

/// A PDF blend mode as peniko's mix mode.
#[must_use]
pub fn to_mix(mode: BlendMode) -> peniko::Mix {
    match mode {
        BlendMode::Normal | BlendMode::Compatible => peniko::Mix::Normal,
        BlendMode::Multiply => peniko::Mix::Multiply,
        BlendMode::Screen => peniko::Mix::Screen,
        BlendMode::Overlay => peniko::Mix::Overlay,
        BlendMode::Darken => peniko::Mix::Darken,
        BlendMode::Lighten => peniko::Mix::Lighten,
        BlendMode::ColorDodge => peniko::Mix::ColorDodge,
        BlendMode::ColorBurn => peniko::Mix::ColorBurn,
        BlendMode::HardLight => peniko::Mix::HardLight,
        BlendMode::SoftLight => peniko::Mix::SoftLight,
        BlendMode::Difference => peniko::Mix::Difference,
        BlendMode::Exclusion => peniko::Mix::Exclusion,
        BlendMode::Hue => peniko::Mix::Hue,
        BlendMode::Saturation => peniko::Mix::Saturation,
        BlendMode::Color => peniko::Mix::Color,
        BlendMode::Luminosity => peniko::Mix::Luminosity,
    }
}

/// A blend mode as the full `peniko::BlendMode` a layer takes.
#[must_use]
pub fn to_blend_mode(mode: BlendMode) -> peniko::BlendMode {
    peniko::BlendMode::new(to_mix(mode), peniko::Compose::SrcOver)
}

/// An engine [`Pixmap`] as image data vello can sample.
///
/// The bytes pass through untouched and are declared
/// [`peniko::ImageAlphaType::AlphaPremultiplied`], because that is what a
/// `Pixmap` holds — the engine's whole compositing pipeline works
/// premultiplied. Declaring straight alpha instead would make vello
/// un-premultiply once on the way in, which double-darkens every translucent
/// pixel: a silent, uniform error that no edge-tolerance budget would catch,
/// so it is stated here rather than left to the field's default.
///
/// `None` for a zero-sized pixmap, which vello has no meaningful image for.
#[must_use]
pub fn to_image_data(p: &Pixmap) -> Option<peniko::ImageData> {
    if p.width() == 0 || p.height() == 0 {
        return None;
    }
    Some(peniko::ImageData {
        data: peniko::Blob::from(p.data().to_vec()),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
        width: p.width(),
        height: p.height(),
    })
}

/// An [`AlphaMask`] as a neutral-grey opaque image, for a luminance-mask
/// layer.
///
/// `vello` has no layer that takes a supplied coverage plane: its only masking
/// layer computes the *luminance of content drawn into it*. So a mask byte `m`
/// becomes the opaque pixel `(m, m, m, 255)`, whose luminance is `m` under any
/// coefficient set whose weights sum to one — which is every one of them.
/// That sidesteps the trap `pdfrum-raster-vello-cpu` documents about
/// `Mask::new_luminance`: the BT.709-versus-NTSC question only has an answer
/// for *coloured* content, and this is neutral by construction.
///
/// Opaque, not `(255, 255, 255, m)`. A luminance mask reads the colour
/// channels; encoding the coverage in alpha instead would hand the mask layer
/// a uniformly white luminance and mask nothing.
#[must_use]
pub fn mask_to_image_data(m: &AlphaMask) -> Option<peniko::ImageData> {
    if m.width() == 0 || m.height() == 0 {
        return None;
    }
    let src = m.data();
    let mut rgba = Vec::with_capacity(src.len().saturating_mul(4));
    for &v in src {
        rgba.extend_from_slice(&[v, v, v, 255]);
    }
    Some(peniko::ImageData {
        data: peniko::Blob::from(rgba),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
        width: m.width(),
        height: m.height(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_blend_mode_maps() {
        assert_eq!(to_mix(BlendMode::Compatible), peniko::Mix::Normal);
        assert_eq!(to_mix(BlendMode::Luminosity), peniko::Mix::Luminosity);
        assert_eq!(to_mix(BlendMode::SoftLight), peniko::Mix::SoftLight);
    }

    #[test]
    fn a_pixmap_is_declared_premultiplied() {
        // The engine composites premultiplied throughout, and declaring
        // straight alpha here would double-darken every translucent pixel
        // uniformly — invisible to an edge-tolerance budget.
        let p = Pixmap::filled(2, 2, peniko::Color::from_rgba8(10, 20, 30, 128));
        let image = to_image_data(&p).expect("non-empty");
        assert_eq!(image.alpha_type, peniko::ImageAlphaType::AlphaPremultiplied);
        assert_eq!(image.format, peniko::ImageFormat::Rgba8);
        assert_eq!((image.width, image.height), (2, 2));
    }

    #[test]
    fn an_empty_pixmap_has_no_image() {
        assert!(to_image_data(&Pixmap::new(0, 4)).is_none());
        assert!(to_image_data(&Pixmap::new(4, 0)).is_none());
    }

    #[test]
    fn a_mask_byte_becomes_an_opaque_neutral_grey() {
        // The luminance of (m, m, m) is m for any weights summing to one, so
        // the mask's own bytes survive the round trip exactly. Encoding the
        // coverage in alpha instead would mask nothing at all.
        let m = AlphaMask::filled(2, 1, 77);
        let image = mask_to_image_data(&m).expect("non-empty");
        assert_eq!(image.data.as_ref(), &[77, 77, 77, 255, 77, 77, 77, 255]);
    }

    #[test]
    fn an_empty_mask_has_no_image() {
        assert!(mask_to_image_data(&AlphaMask::new(0, 0)).is_none());
    }
}
