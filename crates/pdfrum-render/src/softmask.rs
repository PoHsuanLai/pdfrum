//! Turning a rendered soft-mask group into an [`AlphaMask`]
//! (`LoadSMask`, `cpdf_renderstatus.cpp:1434-1542`).
//!
//! The parse half — which keys, the `/S` default, the `/TR` acceptance rule,
//! the `/BC` backdrop colour — belongs to `pdfrum-page`; what belongs here is
//! the rendering half, and three of its contracts are pixel-visible:
//!
//! - **The mask is rendered at exactly the clip rect's device resolution**,
//!   on the device's own pixel grid, so applying it never resamples.
//! - **A luminosity buffer is opaque**, cleared to the `/BC` backdrop
//!   (default black), so an area the group never paints contributes the
//!   backdrop's luminosity rather than zero.
//! - **An alpha buffer starts at zero** and the group renders in alpha colour
//!   mode, where every drawing operation writes alpha as gray.
//!
//! And one that is easy to get wrong: the luminosity readback uses PDFium's
//! `FXRGB2GRAY` weights. Both rasterizers ship a luminance-mask helper and
//! both use BT.709; neither may be used here.

use pdfrum_page::{SoftMask, SoftMaskKind};

use crate::pixmap::{AlphaMask, Pixmap};

/// The colour a soft-mask group's buffer is cleared to before it renders.
///
/// A luminosity mask clears to its `/BC` backdrop, opaque; an alpha mask
/// clears to nothing at all.
#[must_use]
pub fn backdrop(mask: &SoftMask) -> peniko::Color {
    match mask.kind {
        SoftMaskKind::Luminosity => {
            let [r, g, b] = mask.backdrop.to_bytes();
            peniko::Color::from_rgba8(r, g, b, 255)
        }
        SoftMaskKind::Alpha => peniko::Color::TRANSPARENT,
    }
}

/// Read a rendered soft-mask group back as a coverage plane.
///
/// The `/TR` lookup, when present, maps every byte through a 256-entry table
/// built from the function's **output component 0 only**, regardless of how
/// many the function declares.
#[must_use]
pub fn readback(mask: &SoftMask, rendered: &Pixmap) -> AlphaMask {
    let mut out = match mask.kind {
        SoftMaskKind::Luminosity => rendered.luminosity_mask(),
        SoftMaskKind::Alpha => rendered.alpha_mask(),
    };
    if let Some(table) = &mask.transfer {
        out.apply_transfer(table);
    }
    out
}

#[cfg(test)]
mod tests {
    use kurbo::Affine;
    use pdfrum_object::{ByteSpan, Dict, Stream};
    use pdfrum_page::Rgb;

    use super::*;

    fn soft_mask(kind: SoftMaskKind, backdrop_rgb: Rgb) -> SoftMask {
        SoftMask {
            group: Stream {
                dict: Dict::default(),
                data: ByteSpan::empty(),
            },
            kind,
            backdrop: backdrop_rgb,
            transfer: None,
            matrix: Affine::IDENTITY,
            objects: Vec::new(),
        }
    }

    #[test]
    fn luminosity_defaults_to_an_opaque_black_backdrop() {
        let m = soft_mask(SoftMaskKind::Luminosity, Rgb::BLACK);
        assert_eq!(backdrop(&m), peniko::Color::from_rgba8(0, 0, 0, 255));
    }

    #[test]
    fn bc_clears_the_backdrop() {
        let m = soft_mask(
            SoftMaskKind::Luminosity,
            Rgb {
                r: 1.0,
                g: 1.0,
                b: 1.0,
            },
        );
        assert_eq!(backdrop(&m), peniko::Color::from_rgba8(255, 255, 255, 255));
    }

    #[test]
    fn an_alpha_mask_starts_from_nothing() {
        let m = soft_mask(
            SoftMaskKind::Alpha,
            Rgb {
                r: 1.0,
                g: 1.0,
                b: 1.0,
            },
        );
        assert_eq!(
            backdrop(&m),
            peniko::Color::TRANSPARENT,
            "/BC is ignored for an alpha mask"
        );
    }

    #[test]
    fn luminosity_readback_uses_ntsc_not_bt709() {
        let m = soft_mask(SoftMaskKind::Luminosity, Rgb::BLACK);
        // Pure green: FXRGB2GRAY gives 255*59/100 = 150; BT.709 gives 182.
        let rendered = Pixmap::filled(1, 1, peniko::Color::from_rgba8(0, 255, 0, 255));
        assert_eq!(readback(&m, &rendered).data(), &[150]);
    }

    #[test]
    fn alpha_readback_is_the_alpha_channel() {
        let m = soft_mask(SoftMaskKind::Alpha, Rgb::BLACK);
        let rendered = Pixmap::filled(1, 1, peniko::Color::from_rgba8(255, 255, 255, 77));
        assert_eq!(readback(&m, &rendered).data(), &[77]);
    }

    #[test]
    fn a_transfer_function_maps_every_byte() {
        let mut m = soft_mask(SoftMaskKind::Alpha, Rgb::BLACK);
        // A table that zeroes everything: a mask that hides its whole group.
        m.transfer = Some(Box::new([0u8; 256]));
        let rendered = Pixmap::filled(2, 1, peniko::Color::from_rgba8(255, 255, 255, 255));
        assert_eq!(readback(&m, &rendered).data(), &[0, 0]);
    }

    #[test]
    fn a_luminosity_mask_over_an_unpainted_area_reads_the_backdrop() {
        // The whole point of the opaque /BC clear: a region the group never
        // touched contributes the backdrop's luminosity, not zero.
        let m = soft_mask(
            SoftMaskKind::Luminosity,
            Rgb {
                r: 1.0,
                g: 1.0,
                b: 1.0,
            },
        );
        let cleared = Pixmap::filled(1, 1, backdrop(&m));
        assert_eq!(readback(&m, &cleared).data(), &[255]);
    }
}
