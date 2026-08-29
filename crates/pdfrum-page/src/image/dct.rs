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
//!
//! # The output component count is the codestream's
//!
//! PDFium never asks libjpeg to reduce a JPEG's channel count. It leaves
//! `out_color_space` at the value `jpeg_read_header` chose — `JCS_GRAYSCALE`
//! for one channel, `JCS_RGB` for three, **`JCS_CMYK` for four** — and only
//! ever *narrows* that, by pinning a three-channel image with no Adobe marker
//! to its own `jpeg_color_space`:
//!
//! ```text
//! if (common_.cinfo.num_components == 3 && !jpeg_transform_) {
//!   common_.cinfo.out_color_space = common_.cinfo.jpeg_color_space;
//! }
//! ```
//!
//! so `comps_ = cinfo.num_components` always equals the codestream's count.
//! `zune-jpeg` instead defaults its *output* to RGB and will happily convert
//! a four-channel CMYK or YCCK image down to three, which then reads as a
//! component mismatch against a `/DeviceCMYK` image dictionary and rejects
//! the whole image — `bug_718762` and `bug_1646`, both 5000×5000 CMYK JPEGs.
//! We therefore choose the output space from the header's channel count
//! rather than taking the default.

use crate::color::{ColorSpace, Family};
use crate::error::Error;
use crate::image::dict::is_allowed_bits_per_component;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace as ZColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

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

/// The output colour space libjpeg would have chosen for `channels`.
///
/// `jpeg_read_header` picks the output space from the codestream's channel
/// count alone, and PDFium leaves that choice standing. Four channels stay
/// four; anything else takes `zune-jpeg`'s default, which already matches
/// (`JCS_GRAYSCALE` for one, `JCS_RGB` for three).
fn output_space(channels: u8) -> Option<ZColorSpace> {
    (channels == 4).then_some(ZColorSpace::CMYK)
}

/// The two byte offsets a known-bad SOF height is patched at.
///
/// `kKnownBadHeaderWithInvalidHeightByteOffsetStarts`,
/// `libjpeg_scanline_decoder.cpp:26`. They are *positions*, not a search: the
/// two encoders that emit this header put their SOF segment at one of exactly
/// these two places, and PDFium declines to guess anywhere else.
const KNOWN_BAD_HEIGHT_OFFSETS: [usize; 2] = [94, 163];

/// How far back from the dimension bytes the SOF marker sits.
///
/// `kSofMarkerByteOffset`, used at `libjpeg_scanline_decoder.cpp:290`: two
/// marker bytes, two length bytes and the one-byte sample precision.
const SOF_MARKER_BACK_OFFSET: usize = 5;

/// Whether a codestream carries the known-bad SOF header with height `0xffff`,
/// at `offset`.
///
/// `HasKnownBadHeaderWithInvalidHeight`,
/// `libjpeg_scanline_decoder.cpp:272-303`. Its own comment calls the checks
/// "lots of possibly redundant" ones, and they are kept in full for the reason
/// it gives: this rewrites image bytes, so a false positive corrupts a picture
/// that would otherwise have decoded. The declared width must match the
/// dictionary's *exactly*, which is what makes a bare height of `0xffff`
/// insufficient on its own.
fn has_known_bad_height(data: &[u8], offset: usize, declared: (u32, u32)) -> bool {
    let (width, height) = declared;
    if width == 0 || width > JPEG_MAX_DIMENSION || height == 0 || height > JPEG_MAX_DIMENSION {
        return false;
    }
    let Some(marker_at) = offset.checked_sub(SOF_MARKER_BACK_OFFSET) else {
        return false;
    };
    // `IsSofSegment`: any of the sixteen start-of-frame markers.
    if !matches!(
        data.get(marker_at..marker_at + 2),
        Some(&[0xff, sof]) if (0xc0..=0xcf).contains(&sof)
    ) {
        return false;
    }
    let expected = big_endian(width);
    matches!(
        data.get(offset..offset + 4),
        Some(&[0xff, 0xff, high, low]) if [high, low] == expected
    )
}

/// libjpeg's own `JPEG_MAX_DIMENSION`.
///
/// The reason this repair path exists at all: `0xffff` is above it, so libjpeg
/// raises `JERR_IMAGE_TOO_BIG` and refuses the header outright rather than
/// decoding a very tall image.
const JPEG_MAX_DIMENSION: u32 = 65500;

/// A dimension as the two big-endian bytes a SOF segment stores it in.
#[allow(clippy::cast_possible_truncation)]
fn big_endian(dimension: u32) -> [u8; 2] {
    [((dimension >> 8) & 0xff) as u8, (dimension & 0xff) as u8]
}

/// Decode a baseline JPEG.
///
/// `declared` is the image dictionary's `/Width` and `/Height`, which the
/// codestream normally overrides — see [`probe`]. It is threaded in for the
/// one case where the codestream is *wrong* and the dictionary is right:
/// PDFium seeds `cinfo` with the dictionary's dimensions before reading the
/// header (`libjpeg_scanline_decoder.cpp:85-86`), and when the header is
/// refused it looks for a specific malformation and rewrites the bytes.
///
/// # Errors
///
/// [`Error::CodecRejected`] when the codestream will not decode or declares a
/// component count or bit depth PDF does not allow.
pub fn decode_dct(data: &[u8], declared: (u32, u32)) -> Result<DctImage, Error> {
    // A header this decoder refuses gets one repair attempt, on exactly the
    // malformation PDFium repairs: a SOF declaring height `0xffff` — above
    // libjpeg's `JPEG_MAX_DIMENSION`, so `JERR_IMAGE_TOO_BIG` — beside the
    // dictionary's own width. `PatchUpKnownBadHeaderWithInvalidHeight`
    // (`libjpeg_scanline_decoder.cpp:309-315`) writes the dictionary's height
    // over those two bytes and reads the header again. Upstream patches the
    // source buffer in place through a `const_cast`; a copy is the same
    // decision without the aliasing.
    let patched: Option<Vec<u8>> = probe(data)
        .is_none()
        .then(|| {
            KNOWN_BAD_HEIGHT_OFFSETS
                .into_iter()
                .find(|&offset| has_known_bad_height(data, offset, declared))
                .map(|offset| {
                    let mut copy = data.to_vec();
                    if let Some(slot) = copy.get_mut(offset..offset + 2) {
                        slot.copy_from_slice(&big_endian(declared.1));
                    }
                    copy
                })
        })
        .flatten();
    let data = patched.as_deref().unwrap_or(data);
    // The header pass first, so the output space can be pinned to the
    // codestream's channel count before any samples are produced.
    let channels = probe(data).map_or(0, |(_, _, c)| c);
    let options = output_space(channels)
        .map(|space| DecoderOptions::default().jpeg_set_out_colorspace(space));
    let mut decoder = match options {
        Some(options) => zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options),
        None => zune_jpeg::JpegDecoder::new(ZCursor::new(data)),
    };
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
        ADOBE_CMYK_DECODE, ZColorSpace, allows_reduced_resolution, component_mismatch_allowed,
        decode_dct, has_known_bad_height, output_space, probe, scale_denominator, scaled_size,
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
            assert!(
                decode_dct(data, (0, 0)).is_err(),
                "{data:?} should be rejected"
            );
            assert!(probe(data).is_none());
        }
    }

    /// A codestream shaped like `bug_86459`'s: a SOF2 at offset 158 declaring
    /// height `0xffff` and width 612, which is the layout that puts the
    /// dimension bytes at the second of the two offsets PDFium recognises.
    fn known_bad_header() -> Vec<u8> {
        let mut data = vec![0u8; 200];
        data[158] = 0xff;
        data[159] = 0xc2;
        // Length and sample precision fill the three bytes to the dimensions.
        data[163] = 0xff;
        data[164] = 0xff;
        data[165] = 0x02;
        data[166] = 0x64;
        data
    }

    #[test]
    fn a_sof_height_of_ffff_beside_the_declared_width_is_the_known_bad_header() {
        let data = known_bad_header();
        assert!(has_known_bad_height(&data, 163, (612, 792)));
        // The other sanctioned offset does not match this layout, and no
        // unlisted offset is even looked at.
        assert!(!has_known_bad_height(&data, 94, (612, 792)));
    }

    #[test]
    fn the_repair_declines_every_way_the_evidence_can_fall_short() {
        let data = known_bad_header();
        // A width that is not the dictionary's. This is the check that keeps a
        // bare `0xffff` from being enough on its own.
        assert!(!has_known_bad_height(&data, 163, (613, 792)));
        // A dictionary height of zero, or one above libjpeg's own maximum.
        assert!(!has_known_bad_height(&data, 163, (612, 0)));
        assert!(!has_known_bad_height(&data, 163, (612, 65501)));
        // A marker that is not a start-of-frame.
        let mut not_sof = data.clone();
        not_sof[159] = 0xd8;
        assert!(!has_known_bad_height(&not_sof, 163, (612, 792)));
        // A height that is not `0xffff` after all.
        let mut sane = data;
        sane[163] = 0x03;
        sane[164] = 0x18;
        assert!(!has_known_bad_height(&sane, 163, (612, 792)));
        // And a codestream too short to hold the bytes the test reads.
        assert!(!has_known_bad_height(&[0xff, 0xc2], 163, (612, 792)));
    }

    #[test]
    fn a_four_channel_jpeg_stays_four_channels() {
        // libjpeg's `jpeg_read_header` picks `JCS_CMYK` for four channels and
        // PDFium never overrides it, so the scanline keeps all four. Only
        // this count needs pinning: one and three already match `zune-jpeg`'s
        // own default, and asking for a conversion there would be the change.
        assert_eq!(output_space(4), Some(ZColorSpace::CMYK));
        assert_eq!(output_space(1), None);
        assert_eq!(output_space(3), None);
        // A count PDF does not allow is left alone too — the component gate
        // above rejects it, and pinning an output space would not save it.
        assert_eq!(output_space(2), None);
        assert_eq!(output_space(0), None);
    }
}
