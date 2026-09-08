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
//! The literal `[1 0 1 0 1 0 1 0]` has no home in this crate: it is
//! materialised only when *authoring* an image `XObject` from a JPEG file,
//! never on a read or render path, and pdfrum has no image-writing API.
//! Reading takes the file's own `/Decode` through
//! `super::decode_array::DecodeMap`.
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
//!
//! # A four-channel JPEG is two different images
//!
//! `Adobe_transform` splits them: `0` is plain CMYK and `2` is **YCCK**, whose
//! CMY channels carry the YCbCr transform. libjpeg reads the byte at
//! `jdapimin.c:185-198` and converts YCCK with `ycck_cmyk_convert`
//! (`jdcolor.c:544-587`); pdf.js honours the same byte at `jpg.js:1292-1296`,
//! and so does `zune-jpeg` at `headers.rs:496-508`. `zune-jpeg`'s *conversion*
//! table is the gap: `worker.rs:63-82` implements `(YCCK, RGB)` and
//! `(YCCK, RGBA)` and has no `(YCCK, CMYK)` arm, so pinning CMYK — which the
//! plain-CMYK case needs — would reject every YCCK image outright. Both are
//! asked for their own space, which takes the identity copy at
//! `worker.rs:41-43`, and [`ycck_to_cmyk`] finishes the YCCK one here. The
//! conversion has to be ours rather than the decoder's so that `/Decode` still
//! sees CMYK — see that function.

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

/// How much a reduced-resolution decode may shrink the image.
///
/// `1 << min(levels, 3)`, so at most one eighth — libjpeg's limit, which
/// PDFium inherits.
#[must_use]
#[allow(
    dead_code,
    reason = "unwired: no decode path asks for reduced resolution yet"
)]
pub fn scale_denominator(levels: u8) -> u32 {
    1u32 << levels.min(3)
}

/// The size a reduced decode produces: **ceiling** division, unlike the JPX
/// path's plain right shift.
#[must_use]
#[allow(
    dead_code,
    reason = "unwired: no decode path asks for reduced resolution yet"
)]
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
#[allow(
    dead_code,
    reason = "unwired: no decode path asks for reduced resolution yet"
)]
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
///
/// `input` is the space the codestream declares, because a four-channel image
/// splits: `Adobe_transform == 0` is plain CMYK and comes back unconverted,
/// while `== 2` is YCCK and `zune-jpeg` has no `(YCCK, CMYK)` arm at all
/// (`worker.rs:63-82` implements only `(YCCK, RGB)` and `(YCCK, RGBA)`).
/// Asking for YCCK *itself* takes the four-component identity copy at
/// `worker.rs:41-43`, which hands back the raw Y/Cb/Cr/K planes for
/// [`ycck_to_cmyk`] to convert — see that function for why the conversion is
/// ours rather than the decoder's.
fn output_space(channels: u8, input: Option<ZColorSpace>) -> Option<ZColorSpace> {
    if channels != 4 {
        return None;
    }
    Some(match input {
        Some(ZColorSpace::YCCK) => ZColorSpace::YCCK,
        _ => ZColorSpace::CMYK,
    })
}

/// libjpeg's `SCALEBITS`: the fixed-point fraction its YCbCr tables carry.
const SCALEBITS: i32 = 16;

/// `ONE_HALF`, the rounding term added before the right shift.
const ONE_HALF: i32 = 1 << (SCALEBITS - 1);

// The four coefficients `build_ycc_rgb_table` scales, as `FIX(x)` —
// `(JLONG)(x * (1 << SCALEBITS) + 0.5)`, `jdcolor.c:82` — evaluated once.

/// `FIX(1.40200)`, the Cr→R coefficient.
const CR_R: i32 = 91_881;
/// `FIX(1.77200)`, the Cb→B coefficient.
const CB_B: i32 = 116_130;
/// `FIX(0.71414)`, the Cr→G coefficient, applied negated.
const CR_G: i32 = 46_802;
/// `FIX(0.34414)`, the Cb→G coefficient, applied negated.
const CB_G: i32 = 22_554;

/// `MAXJSAMPLE`, the largest eight-bit sample.
const MAX_SAMPLE: i32 = 255;

/// `CENTERJSAMPLE`, the value a chroma channel is centred on.
const CENTER_SAMPLE: i32 = 128;

/// Convert YCCK samples to CMYK in place, as libjpeg's `ycck_cmyk_convert`.
///
/// A four-component JPEG whose Adobe marker says `transform == 2` stores
/// **YCCK**: the CMY channels have been run through the same YCbCr transform a
/// colour photograph gets, with K left alone. libjpeg undoes it at
/// `third_party/libjpeg_turbo/src/jdcolor.c:544-587` — YCbCr→RGB per channel,
/// then `MAXJSAMPLE -` each result, with `outptr[3] = inptr3[col]` passing K
/// through untouched. The complement is what turns RGB back into CMY.
///
/// # Why the conversion is ours and not the decoder's
///
/// `zune-jpeg` will decode YCCK straight to RGB (`worker.rs:63-82`), and that
/// is the wrong answer here for two reasons that are the same reason twice:
/// its composite bakes in the Adobe inversion *and* collapses four channels to
/// three, so the image would arrive with neither the components the image
/// dictionary declares nor a value `/Decode` can still act on. §7.4.8 puts the
/// CMYK inversion in `/Decode`, not in the codec — pdf.js agrees
/// (`jpg.js:1275-1279`) — and this crate reproduces that in `image/mod.rs`.
/// Producing raw CMYK here keeps that seam intact.
///
/// The arithmetic is libjpeg's exactly, tables and all: the per-value tables
/// `build_ycc_rgb_table` (`jdcolor.c:215-254`) precomputes are the same four
/// products evaluated inline, so the result is bit-identical rather than
/// merely close.
fn ycck_to_cmyk(samples: &mut [u8]) {
    // A slice pattern rather than four indices: the chunk's length is the
    // pattern's, so the K channel being untouched is visible in the binding.
    for [c, m, y_channel, _k] in samples.as_chunks_mut::<4>().0 {
        let (y, cb, cr) = (
            i32::from(*c),
            i32::from(*m) - CENTER_SAMPLE,
            i32::from(*y_channel) - CENTER_SAMPLE,
        );
        // `Cr_r_tab[cr]` and `Cb_b_tab[cb]`, rounded at the shift.
        let red = y + ((CR_R * cr + ONE_HALF) >> SCALEBITS);
        // `Cb_g_tab[cb] + Cr_g_tab[cr]` carries `ONE_HALF` in the Cb term, so
        // the sum is shifted once — one rounding, not two.
        let green = y + ((-CB_G * cb - CR_G * cr + ONE_HALF) >> SCALEBITS);
        let blue = y + ((CB_B * cb + ONE_HALF) >> SCALEBITS);
        // `range_limit[MAXJSAMPLE - v]`: the complement, clamped. libjpeg's
        // range-limit table wraps rather than saturating for the far
        // out-of-range values DCT noise cannot reach; a clamp is the same
        // answer over every value a real codestream produces.
        *c = clamp_sample(MAX_SAMPLE - red);
        *m = clamp_sample(MAX_SAMPLE - green);
        *y_channel = clamp_sample(MAX_SAMPLE - blue);
        // K passes through unchanged (`jdcolor.c:579-580`).
    }
}

/// `range_limit`: a sample clamped into `0..=255`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_sample(value: i32) -> u8 {
    value.clamp(0, MAX_SAMPLE) as u8
}

/// The two byte offsets a known-bad SOF height is patched at.
///
/// They are *positions*, not a search: the two encoders that emit this header
/// put their SOF segment at one of exactly these two places, and nothing here
/// guesses anywhere else.
// The oracle's `kKnownBadHeaderWithInvalidHeightByteOffsetStarts`,
// `libjpeg_scanline_decoder.cpp:26`.
const KNOWN_BAD_HEIGHT_OFFSETS: [usize; 2] = [94, 163];

/// How far back from the dimension bytes the SOF marker sits: two marker
/// bytes, two length bytes and the one-byte sample precision.
// The oracle's `kSofMarkerByteOffset`, used at
// `libjpeg_scanline_decoder.cpp:290`.
const SOF_MARKER_BACK_OFFSET: usize = 5;

/// Whether a codestream carries the known-bad SOF header with height `0xffff`,
/// at `offset`.
///
/// The checks are redundant several times over, and they are kept in full for
/// the reason the oracle gives for its own: this rewrites image bytes, so a
/// false positive corrupts a picture that would otherwise have decoded. The
/// declared width must match the dictionary's *exactly*, which is what makes a
/// bare height of `0xffff` insufficient on its own.
// `HasKnownBadHeaderWithInvalidHeight`,
// `libjpeg_scanline_decoder.cpp:272-303`; its own comment calls these "lots of
// possibly redundant" checks.
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
/// one case where the codestream is *wrong* and the dictionary is right: when
/// the header is refused, a specific malformation is looked for and the bytes
/// rewritten from the dictionary's dimensions.
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
    // codestream's channel count — and, for four channels, to which of the two
    // four-channel spaces it declares — before any samples are produced.
    let (channels, input) = read_header(data).map_or((0, None), |(_, _, c, space)| (c, space));
    let pinned = output_space(channels, input);
    let options = pinned.map(|space| DecoderOptions::default().jpeg_set_out_colorspace(space));
    let mut decoder = match options {
        Some(options) => zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options),
        None => zune_jpeg::JpegDecoder::new(ZCursor::new(data)),
    };
    let mut pixels = decoder
        .decode()
        .map_err(|_| Error::CodecRejected { codec: "DCT" })?;
    let info = decoder
        .info()
        .ok_or(Error::CodecRejected { codec: "DCT" })?;
    // The YCCK planes come back raw, which is the point: the transform is
    // ours so that `/Decode` still sees CMYK. Nothing else is touched.
    if pinned == Some(ZColorSpace::YCCK) {
        ycck_to_cmyk(&mut pixels);
    }
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

/// The header facts a decode needs: dimensions, channel count, and the space
/// the codestream declares.
///
/// The fourth is what [`probe`] does not carry and [`decode_dct`] cannot do
/// without, since `Adobe_transform` splits the four-channel case in two.
fn read_header(data: &[u8]) -> Option<(u32, u32, u8, Option<ZColorSpace>)> {
    let mut decoder = zune_jpeg::JpegDecoder::new(ZCursor::new(data));
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    Some((
        u32::from(info.width),
        u32::from(info.height),
        info.components,
        decoder.input_colorspace(),
    ))
}

/// The dimensions and component count a JPEG declares, without decoding it.
///
/// This is the probe PDFium runs when the full decode fails: the codestream's
/// own dimensions then **overwrite** the dictionary's, which is why a JPEG
/// disagreeing with its wrapper still renders at the codestream's size.
#[must_use]
pub fn probe(data: &[u8]) -> Option<(u32, u32, u8)> {
    read_header(data).map(|(width, height, components, _)| (width, height, components))
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
        ZColorSpace, allows_reduced_resolution, component_mismatch_allowed, decode_dct,
        has_known_bad_height, output_space, probe, scale_denominator, scaled_size, ycck_to_cmyk,
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
        assert_eq!(
            output_space(4, Some(ZColorSpace::CMYK)),
            Some(ZColorSpace::CMYK)
        );
        assert_eq!(output_space(1, Some(ZColorSpace::Luma)), None);
        assert_eq!(output_space(3, Some(ZColorSpace::YCbCr)), None);
        // A count PDF does not allow is left alone too — the component gate
        // above rejects it, and pinning an output space would not save it.
        assert_eq!(output_space(2, None), None);
        assert_eq!(output_space(0, None), None);
    }

    #[test]
    fn a_ycck_jpeg_is_asked_for_its_own_space_not_cmyk() {
        // `zune-jpeg` has no `(YCCK, CMYK)` arm (`worker.rs:63-82`), so asking
        // for CMYK here is the error that dropped every YCCK image. Asking for
        // YCCK takes the four-component identity copy instead and leaves the
        // conversion to `ycck_to_cmyk`.
        assert_eq!(
            output_space(4, Some(ZColorSpace::YCCK)),
            Some(ZColorSpace::YCCK)
        );
        // A four-channel codestream whose space did not read still asks for
        // CMYK, which is what the plain-CMYK files need.
        assert_eq!(output_space(4, None), Some(ZColorSpace::CMYK));
    }

    #[test]
    fn ycck_leaves_a_neutral_pixel_black_and_passes_k_through() {
        // Chroma at the centre and full luma is white in RGB, so the
        // complement is zero in CMY — and K is copied, not touched.
        let mut samples = [255, 128, 128, 42];
        ycck_to_cmyk(&mut samples);
        assert_eq!(samples, [0, 0, 0, 42]);
        // Zero luma is black in RGB, so the complement saturates all three.
        let mut dark = [0, 128, 128, 7];
        ycck_to_cmyk(&mut dark);
        assert_eq!(dark, [255, 255, 255, 7]);
    }

    #[test]
    fn ycck_matches_libjpegs_fixed_point_arithmetic_exactly() {
        // The reference is `ycck_cmyk_convert` (`jdcolor.c:569-581`) driven by
        // `build_ycc_rgb_table`'s tables (`:236-250`), evaluated here in the
        // same order and at the same widths. A float transform would differ
        // by a unit on many of these; this asserts it does not.
        // Named for libjpeg's own tables: `red_from_cr`, `blue_from_cb`, and
        // the two halves of the green sum.
        let reference = |y: i32, cb: i32, cr: i32| -> [i32; 3] {
            let (x_b, x_r) = (cb - 128, cr - 128);
            let red_from_cr = (91881 * x_r + 32768) >> 16;
            let blue_from_cb = (116130 * x_b + 32768) >> 16;
            let green_chroma_red_term = -46802 * x_r;
            let green_chroma_blue_term = -22554 * x_b + 32768;
            [
                255 - (y + red_from_cr),
                255 - (y + ((green_chroma_blue_term + green_chroma_red_term) >> 16)),
                255 - (y + blue_from_cb),
            ]
        };
        for y in (0u8..=255).step_by(17) {
            for cb in (0u8..=255).step_by(23) {
                for cr in (0u8..=255).step_by(29) {
                    let mut got = [y, cb, cr, 200];
                    ycck_to_cmyk(&mut got);
                    let want = reference(i32::from(y), i32::from(cb), i32::from(cr));
                    for (channel, expected) in want.into_iter().enumerate() {
                        assert_eq!(
                            i32::from(got[channel]),
                            expected.clamp(0, 255),
                            "channel {channel} at y={y} cb={cb} cr={cr}"
                        );
                    }
                    assert_eq!(got[3], 200, "K must pass through");
                }
            }
        }
    }

    #[test]
    fn a_ycck_codestream_decodes_to_four_channels_rather_than_being_dropped() {
        // The image `corpus/fx/other/1.pdf` carries, whose APP14 transform
        // byte is 2. Before this fix `decode_dct` returned `CodecRejected` for
        // it and the image was silently dropped — SSIM 0.99491 with a
        // `max_channel_diff` of 255. The corpus lives outside the repository,
        // so this skips when it is absent, as the corpus tests do.
        let Some(data) = ycck_codestream() else {
            return;
        };
        let image = decode_dct(&data, (429, 542)).expect("a YCCK JPEG must decode");
        assert_eq!((image.width, image.height), (429, 542));
        assert_eq!(image.components, 4);
        assert_eq!(image.data.len(), 429 * 542 * 4);
        // The raw YCCK planes begin `242, 111, 130, 0`. Reaching CMYK means
        // the transform ran: a copy would have left the first three alone.
        let mut expected = [242, 111, 130, 0];
        ycck_to_cmyk(&mut expected);
        assert_eq!(&image.data[..4], &expected);
        assert_ne!(&image.data[..3], &[242, 111, 130]);
    }

    /// The read-only C++ PDFium checkout, resolved as every script and test in
    /// this repository resolves it: `$PDFRUM_ORACLE_CHECKOUT`, else the
    /// sibling `../pdfium-c++` directory the README names.
    fn oracle_checkout() -> std::path::PathBuf {
        std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
            std::path::PathBuf::from,
        )
    }

    /// The `/DCTDecode` codestream out of `corpus/fx/other/1.pdf`, or `None`
    /// when the corpus is not present.
    ///
    /// The file stores it uncompressed and unencrypted, so the bytes between
    /// the last `stream` keyword and its `endstream` are the JPEG itself.
    fn ycck_codestream() -> Option<Vec<u8>> {
        let pdf = std::fs::read(oracle_checkout().join("testing/corpus/fx/other/1.pdf")).ok()?;
        let start = pdf
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| *pair == b"\xff\xd8")
            .map(|(at, _)| at)
            .next_back()?;
        let end = pdf
            .windows(9)
            .enumerate()
            .find(|(at, window)| *at > start && *window == b"endstream")
            .map(|(at, _)| at)?;
        Some(pdf.get(start..end)?.to_vec())
    }
}
