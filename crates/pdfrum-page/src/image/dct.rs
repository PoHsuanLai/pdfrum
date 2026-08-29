//! DCT (baseline JPEG) decoding (ISO 32000-1 §7.4.8).
//!
//! # The CMYK inversion quirk is not in the decoder
//!
//! A four-component JPEG in a PDF is Adobe-inverted CMYK, and PDFium does not
//! un-invert it in the codec. It expresses the inversion **through
//! `/Decode`** — writing `[1 0 1 0 1 0 1 0]`, which makes each component's
//! base 1 and its step −1/255 — and, separately, through a "transfer mask"
//! flag that swaps in a naive un-inversion formula. Both are reproduced where
//! they live rather than folded into the decode step, because a file that
//! states its own `/Decode` must override the first and not the second.
//!
//! # The component mismatch gate
//!
//! When the JPEG's component count disagrees with the colour space's, the
//! image is not simply rejected: a per-family gate decides. Device families
//! need only enough components, `Lab` needs exactly three, `ICCBased` needs
//! both counts valid and the space's at least the JPEG's, and every other
//! family — `Indexed`, `Separation`, `DeviceN`, `Pattern`, `CalGray`,
//! `CalRGB` — needs them **exactly equal**.

use crate::color::{ColorSpace, Family};
use crate::error::Error;
use crate::image::dict::is_allowed_bits_per_component;
use zune_jpeg::zune_core::bytestream::ZCursor;

/// Component counts a JPEG may declare in a PDF.
const VALID_COMPONENTS: [u8; 3] = [1, 3, 4];

/// A decoded JPEG.
#[derive(Debug, Clone, PartialEq)]
pub struct DctImage {
    /// Width, from the codestream — which **overrides** the dictionary's.
    pub width: u32,
    /// Height, likewise.
    pub height: u32,
    /// Components per pixel.
    pub components: u8,
    /// Bits per component, always eight from this decoder.
    pub bpc: u32,
    /// Interleaved samples.
    pub data: Vec<u8>,
}

/// The `/Decode` a four-component JPEG implies when the image states none.
///
/// `[1 0 1 0 1 0 1 0]` — every channel reversed, which is the Adobe CMYK
/// inversion expressed as a decode array rather than as a decoder step.
pub const ADOBE_CMYK_DECODE: [f32; 8] = [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0];

/// How much a reduced-resolution decode may shrink the image.
///
/// `1 << min(levels, 3)`, so at most one eighth — libjpeg's limit, which
/// PDFium inherits.
#[must_use]
pub fn scale_denominator(levels: u8) -> u32 {
    1u32 << levels.min(3)
}

/// The size a reduced decode produces: **ceiling** division, unlike the JPX
/// path's plain right shift.
#[must_use]
pub fn scaled_size(dimension: u32, denominator: u32) -> u32 {
    if denominator == 0 {
        return dimension;
    }
    dimension.div_ceil(denominator)
}

/// Whether a reduced-resolution decode is allowed for these MCU dimensions.
///
/// An image whose width or height is not a whole number of MCUs refuses to
/// decode at reduced resolution — `scale_denom` is forced back to 1
/// (crbug 890745), so a 408×408 4:2:0 image asked for at 50×50 still comes
/// back at 408×408.
#[must_use]
pub fn allows_reduced_resolution(width: u32, height: u32, max_h: u32, max_v: u32) -> bool {
    let h_mcu = max_h.saturating_mul(8);
    let v_mcu = max_v.saturating_mul(8);
    h_mcu != 0 && v_mcu != 0 && width.is_multiple_of(h_mcu) && height.is_multiple_of(v_mcu)
}

/// Whether a colour space accepts a JPEG with `components` components when it
/// declares `cs_components` itself.
#[must_use]
pub fn component_mismatch_allowed(space: Option<&ColorSpace>, components: u8) -> bool {
    let Some(space) = space else {
        // With no colour space only `Lab` would have been a problem, and
        // there is no space to be `Lab`.
        return true;
    };
    let cs_components = u8::try_from(space.n_components()).unwrap_or(u8::MAX);
    match space.family() {
        // Device families need merely *enough* components on both sides.
        Family::DeviceGray | Family::DeviceRgb | Family::DeviceCmyk => {
            let min = match space.family() {
                Family::DeviceGray => 1,
                Family::DeviceRgb => 3,
                _ => 4,
            };
            cs_components >= min && components >= min
        }
        Family::Lab => components == 3 && cs_components >= 3,
        Family::IccBased => {
            crate::color::is_valid_icc_components(i64::from(components))
                && crate::color::is_valid_icc_components(i64::from(cs_components))
                && cs_components >= components
        }
        // Everything else needs an exact match.
        _ => cs_components == components,
    }
}

/// Decode a baseline JPEG.
///
/// # Errors
///
/// [`Error::CodecRejected`] when the codestream will not decode or declares a
/// component count or bit depth PDF does not allow.
pub fn decode_dct(data: &[u8]) -> Result<DctImage, Error> {
    let mut decoder = zune_jpeg::JpegDecoder::new(ZCursor::new(data));
    let pixels = decoder
        .decode()
        .map_err(|_| Error::CodecRejected { codec: "DCT" })?;
    let info = decoder
        .info()
        .ok_or(Error::CodecRejected { codec: "DCT" })?;
    let components = u8::try_from(
        pixels.len() / usize::from(info.width).max(1) / usize::from(info.height).max(1),
    )
    .unwrap_or(0);
    if !VALID_COMPONENTS.contains(&components) || !is_allowed_bits_per_component(8) {
        return Err(Error::CodecRejected { codec: "DCT" });
    }
    Ok(DctImage {
        width: u32::from(info.width),
        height: u32::from(info.height),
        components,
        bpc: 8,
        data: pixels,
    })
}

/// The dimensions and component count a JPEG declares, without decoding it.
///
/// This is the probe PDFium runs when the full decode fails: the codestream's
/// own dimensions then **overwrite** the dictionary's, which is why a JPEG
/// disagreeing with its wrapper still renders at the codestream's size.
#[must_use]
pub fn probe(data: &[u8]) -> Option<(u32, u32, u8)> {
    let mut decoder = zune_jpeg::JpegDecoder::new(ZCursor::new(data));
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    Some((
        u32::from(info.width),
        u32::from(info.height),
        info.components,
    ))
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{
        ADOBE_CMYK_DECODE, allows_reduced_resolution, component_mismatch_allowed, decode_dct,
        probe, scale_denominator, scaled_size,
    };
    use crate::color::{ColorSpace, Indexed};

    #[test]
    fn the_scale_denominator_caps_at_one_eighth() {
        assert_eq!(scale_denominator(0), 1);
        assert_eq!(scale_denominator(1), 2);
        assert_eq!(scale_denominator(3), 8);
        // libjpeg refuses to go finer.
        assert_eq!(scale_denominator(4), 8);
        assert_eq!(scale_denominator(255), 8);
    }

    #[test]
    fn scaling_rounds_up_unlike_the_jpx_path() {
        assert_eq!(scaled_size(400, 8), 50);
        // 401/8 is 50.125, and the ceiling keeps the last partial block.
        assert_eq!(scaled_size(401, 8), 51);
        assert_eq!(scaled_size(400, 1), 400);
        assert_eq!(scaled_size(400, 0), 400);
    }

    #[test]
    fn non_mcu_aligned_images_refuse_reduced_resolution() {
        // A 4:2:0 image has 16-pixel MCUs.
        assert!(allows_reduced_resolution(400, 400, 2, 2));
        // 408 % 16 == 8, so this one cannot be reduced (crbug 890745).
        assert!(!allows_reduced_resolution(408, 408, 2, 2));
        // At 1:1 sampling the MCU is eight pixels.
        assert!(allows_reduced_resolution(408, 408, 1, 1));
        assert!(!allows_reduced_resolution(0, 0, 0, 0));
    }

    #[test]
    fn the_adobe_cmyk_decode_reverses_every_channel() {
        assert_eq!(ADOBE_CMYK_DECODE.len(), 8);
        for pair in ADOBE_CMYK_DECODE.chunks(2) {
            assert!((pair[0] - 1.0).abs() < 1e-6);
            assert!(pair[1].abs() < 1e-6);
        }
    }

    #[test]
    fn device_families_need_only_enough_components() {
        // A three-component JPEG under DeviceRGB is fine…
        assert!(component_mismatch_allowed(Some(&ColorSpace::DeviceRgb), 3));
        // …and so is a four-component one, since RGB needs only three.
        assert!(component_mismatch_allowed(Some(&ColorSpace::DeviceRgb), 4));
        // A one-component one is not.
        assert!(!component_mismatch_allowed(Some(&ColorSpace::DeviceRgb), 1));
        assert!(component_mismatch_allowed(Some(&ColorSpace::DeviceGray), 1));
    }

    #[test]
    fn other_families_need_an_exact_match() {
        let indexed = ColorSpace::Indexed(Box::new(Indexed {
            base: Box::new(ColorSpace::DeviceRgb),
            max_index: 3,
            lookup: Box::from(&[0u8; 12][..]),
            component_ranges: Box::from(&[(0.0f32, 1.0f32); 3][..]),
        }));
        // `Indexed` reports one component, so only a one-component JPEG fits.
        assert!(component_mismatch_allowed(Some(&indexed), 1));
        assert!(!component_mismatch_allowed(Some(&indexed), 3));
    }

    #[test]
    fn lab_needs_exactly_three_components() {
        let lab = ColorSpace::Lab(Box::new(crate::color::Lab {
            white_point: [0.9505, 1.0, 1.089],
            black_point: [0.0; 3],
            ranges: [-100.0, 100.0, -100.0, 100.0],
        }));
        assert!(component_mismatch_allowed(Some(&lab), 3));
        assert!(!component_mismatch_allowed(Some(&lab), 1));
        assert!(!component_mismatch_allowed(Some(&lab), 4));
    }

    #[test]
    fn with_no_colour_space_any_component_count_is_accepted() {
        assert!(component_mismatch_allowed(None, 1));
        assert!(component_mismatch_allowed(None, 4));
    }

    #[test]
    fn garbage_is_rejected_rather_than_panicked_on() {
        for data in [&b""[..], b"\xFF\xD8", b"not a jpeg", &[0u8; 64]] {
            assert!(decode_dct(data).is_err(), "{data:?} should be rejected");
            assert!(probe(data).is_none());
        }
    }
}
