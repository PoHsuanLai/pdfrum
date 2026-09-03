//! DCT and JPX passthrough: the codestream becomes the stream verbatim and
//! its own header fills in the dictionary.

use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, Stream};

use super::EmbeddedImage;
use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

/// What a JPEG's start-of-frame segment says about the image.
///
/// `CPDF_Image::InitJPEG` reads exactly these four out of libjpeg's
/// `jpeg_read_header` (`JpegModule::LoadInfo`,
/// `core/fxcodec/jpeg/libjpeg_scanline_decoder.cpp:360-367`) and writes each
/// straight into the dictionary, so parsing the SOF directly reaches the same
/// values without decoding a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sof {
    width: u32,
    height: u32,
    components: u8,
    bits: u8,
}

/// Component counts a JPEG may declare in a PDF
/// (`CPDF_Image::IsValidJpegComponent`, `cpdf_image.cpp:42-44`).
const VALID_COMPONENTS: [u8; 3] = [1, 3, 4];

/// Sample precisions a JPEG may declare in a PDF
/// (`CPDF_Image::IsValidJpegBitsPerComponent`, `cpdf_image.cpp:47-49`).
const VALID_BITS: [u8; 5] = [1, 2, 4, 8, 16];

/// The twelve-byte JPEG 2000 signature box every JP2 file opens with
/// (ISO/IEC 15444-1 §I.5.1): length 12, type `jP  `, then the magic.
const JP2_SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// A bare JPEG 2000 codestream's start-of-codestream marker followed by the
/// `SIZ` marker that must come next (ISO/IEC 15444-1 §A.4.1).
const J2K_SOC_SIZ: [u8; 4] = [0xFF, 0x4F, 0xFF, 0x51];

pub(super) fn embed(doc: &mut EditDoc<'_>, bytes: &[u8]) -> Result<EmbeddedImage, Error> {
    if is_jpeg2000(bytes) {
        return embed_jpx(doc, bytes);
    }
    let sof = parse_sof(bytes).ok_or(Error::UnrecognisedImageData)?;
    if !VALID_COMPONENTS.contains(&sof.components) || !VALID_BITS.contains(&sof.bits) {
        return Err(Error::UnrecognisedImageData);
    }
    let mut dict = image_dict(sof.width, sof.height);
    let space = match sof.components {
        1 => names::DEVICE_GRAY.clone(),
        3 => names::DEVICE_RGB.clone(),
        // `InitJPEG` (`cpdf_image.cpp:118-127`): a four-component JPEG in a
        // PDF is Adobe-inverted CMYK, and the inversion is stated through
        // `/Decode [1 0 1 0 1 0 1 0]` rather than folded into the samples.
        // pdf.js reads it the same way round: its decoder leaves a PDF-sourced
        // CMYK JPEG's samples alone (`src/core/jpg.js:1061-1068` inverts only
        // when `!this._isSourcePDF`) and `PDFImage` applies the dictionary's
        // `/Decode`, which `DeviceCmykCS.isDefaultDecode` rejects as
        // non-default (`src/core/image.js:277-295`). Writing the array is
        // therefore what makes both readers agree.
        _ => {
            dict.push(names::DECODE.clone(), Object::Array(cmyk_decode()));
            names::DEVICE_CMYK.clone()
        }
    };
    dict.push(names::COLOR_SPACE.clone(), Object::Name(space));
    dict.push(
        names::BITS_PER_COMPONENT.clone(),
        Object::Int(i64::from(sof.bits)),
    );
    dict.push(
        names::FILTER.clone(),
        Object::Name(names::DCT_DECODE.clone()),
    );
    // `:128-132`: a codestream libjpeg did not read as YCbCr or YCCK has had
    // no colour transform applied, and the decoder must be told not to undo
    // one. Our reader honours the same key
    // (`pdfrum_page`'s image dictionary reads `/DecodeParms /ColorTransform`).
    if !has_colour_transform(bytes, sof.components) {
        let parms = Dict::from_pairs([(names::COLOR_TRANSFORM.clone(), Object::Int(0))]);
        dict.push(names::DECODE_PARMS.clone(), Object::Dict(parms));
    }
    Ok(EmbeddedImage {
        image: doc.add(Object::Stream(Stream::new(
            dict,
            ByteSpan::from(bytes.to_vec()),
        ))),
        width: sof.width,
        height: sof.height,
    })
}

/// `/JPXDecode`, whose dictionary carries fewer keys.
///
/// ISO 32000-1 §7.4.9: `/ColorSpace` and `/BitsPerComponent` are supplied by
/// the codestream, and a `/BitsPerComponent` in the dictionary is ignored
/// outright, so writing either would only state something the codestream can
/// contradict.
fn embed_jpx(doc: &mut EditDoc<'_>, bytes: &[u8]) -> Result<EmbeddedImage, Error> {
    let (width, height) = jpx_size(bytes).ok_or(Error::UnrecognisedImageData)?;
    let mut dict = image_dict(width, height);
    dict.push(
        names::FILTER.clone(),
        Object::Name(names::JPX_DECODE.clone()),
    );
    Ok(EmbeddedImage {
        image: doc.add(Object::Stream(Stream::new(
            dict,
            ByteSpan::from(bytes.to_vec()),
        ))),
        width,
        height,
    })
}

/// `/Type /XObject /Subtype /Image /Width /Height`
/// (`CPDF_Image::CreateXObjectImageDict`, `cpdf_image.cpp:401-410`).
pub(super) fn image_dict(width: u32, height: u32) -> Dict {
    Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::XOBJECT.clone())),
        (names::SUBTYPE.clone(), Object::Name(names::IMAGE.clone())),
        (names::WIDTH.clone(), Object::Int(i64::from(width))),
        (names::HEIGHT.clone(), Object::Int(i64::from(height))),
    ])
}

/// `[1 0 1 0 1 0 1 0]`, the Adobe CMYK inversion (`cpdf_image.cpp:121-125`).
fn cmyk_decode() -> Array {
    Array::of(
        [1, 0, 1, 0, 1, 0, 1, 0]
            .into_iter()
            .map(Object::Int)
            .collect::<Vec<_>>(),
    )
}

fn is_jpeg2000(bytes: &[u8]) -> bool {
    bytes.starts_with(&JP2_SIGNATURE) || bytes.starts_with(&J2K_SOC_SIZ)
}

/// Width and height out of the `SIZ` marker segment (ISO/IEC 15444-1 §A.5.1).
///
/// The image is `Xsiz - XOsiz` by `Ysiz - YOsiz`; both offsets are zero in
/// every ordinary codestream but the subtraction is what the standard defines.
fn jpx_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let soc = bytes.windows(4).position(|w| w == J2K_SOC_SIZ)?;
    // `SOC`(2) `SIZ`(2) `Lsiz`(2) `Rsiz`(2), then Xsiz, Ysiz, XOsiz, YOsiz.
    let grid = bytes.get(soc.checked_add(8)?..)?.first_chunk::<16>()?;
    let at = |i: usize| -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(grid.get(i..i + 4).unwrap_or(&[0; 4]));
        u32::from_be_bytes(b)
    };
    let width = at(0).checked_sub(at(8))?;
    let height = at(4).checked_sub(at(12))?;
    (width > 0 && height > 0).then_some((width, height))
}

/// Walk the marker segments to the start-of-frame.
///
/// Every SOF but `SOF4` (define-Huffman-tables), `SOF8` (reserved) and `SOF12`
/// (define-arithmetic-coding) carries the frame header, and libjpeg accepts
/// any of them, so this does not restrict itself to baseline.
fn parse_sof(bytes: &[u8]) -> Option<Sof> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2usize;
    loop {
        let marker = next_marker(bytes, &mut i)?;
        if is_sof(marker) {
            // `Lf`(2) `P`(1) `Y`(2) `X`(2) `Nf`(1), from `i` (ISO/IEC
            // 10918-1 §B.2.2).
            let head = bytes.get(i..)?.first_chunk::<8>()?;
            let bits = *head.get(2)?;
            let height = u32::from(u16::from_be_bytes([*head.get(3)?, *head.get(4)?]));
            let width = u32::from(u16::from_be_bytes([*head.get(5)?, *head.get(6)?]));
            let components = *head.get(7)?;
            return (width > 0 && height > 0).then_some(Sof {
                width,
                height,
                components,
                bits,
            });
        }
        // A start-of-scan means the header is over and no frame was found.
        if marker == 0xDA {
            return None;
        }
        let len = usize::from(u16::from_be_bytes(*bytes.get(i..)?.first_chunk::<2>()?));
        i = i.checked_add(len.max(2))?;
    }
}

/// Advance `i` past the next marker, leaving it on that segment's length.
///
/// Fill bytes (`0xFF` repeated) and the standalone markers are skipped, so a
/// caller sees only the segments that carry a payload.
fn next_marker(bytes: &[u8], i: &mut usize) -> Option<u8> {
    loop {
        while bytes.get(*i) == Some(&0xFF) {
            *i = i.checked_add(1)?;
        }
        let marker = *bytes.get(*i)?;
        *i = i.checked_add(1)?;
        // `RSTn`, `SOI` and `TEM` carry no length word.
        if !matches!(marker, 0xD0..=0xD9 | 0x01 | 0x00) {
            return Some(marker);
        }
        if marker == 0xD9 {
            return None;
        }
        // Resynchronise onto the next `0xFF`.
        while bytes.get(*i).is_some_and(|b| *b != 0xFF) {
            *i = i.checked_add(1)?;
        }
    }
}

fn is_sof(marker: u8) -> bool {
    matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
}

/// Whether libjpeg would report `color_transform`
/// (`libjpeg_scanline_decoder.cpp:364-365`: `jpeg_color_space` is `JCS_YCbCr`
/// or `JCS_YCCK`).
///
/// `jpeg_read_header` derives that space in `default_decompress_parms`
/// (`jdapimin.c:150-210`): with an Adobe APP14 marker the transform byte
/// decides — `0` is RGB or CMYK, `1` is YCbCr, `2` is YCCK — and without one a
/// three-component image is assumed YCbCr unless a JFIF-incompatible component
/// id says otherwise, while a four-component image is assumed plain CMYK.
fn has_colour_transform(bytes: &[u8], components: u8) -> bool {
    match adobe_transform(bytes) {
        Some(transform) => transform == 1 || transform == 2,
        None => components == 3,
    }
}

/// The transform byte of an Adobe APP14 marker, when the file has one.
fn adobe_transform(bytes: &[u8]) -> Option<u8> {
    let mut i = 2usize;
    loop {
        let marker = next_marker(bytes, &mut i)?;
        if marker == 0xDA || is_sof(marker) {
            return None;
        }
        let len = usize::from(u16::from_be_bytes(*bytes.get(i..)?.first_chunk::<2>()?));
        if marker == 0xEE {
            let payload = bytes.get(i.checked_add(2)?..i.checked_add(len)?)?;
            if payload.starts_with(b"Adobe") {
                return payload.last().copied();
            }
        }
        i = i.checked_add(len.max(2))?;
    }
}

/// A colour space name for a component count, for the raw-sample path.
pub(super) fn device_space(components: u8) -> Option<Name> {
    match components {
        1 => Some(names::DEVICE_GRAY.clone()),
        3 => Some(names::DEVICE_RGB.clone()),
        4 => Some(names::DEVICE_CMYK.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Sof, adobe_transform, has_colour_transform, jpx_size, parse_sof};

    const MONA_LISA: &[u8] = include_bytes!("../../../pdfrum/tests/fixtures/mona_lisa.jpg");
    const GRAY_JP2: &[u8] = include_bytes!("../../../pdfrum/tests/fixtures/gray.jp2");

    #[test]
    fn the_sof_of_a_real_jpeg_reads_back() {
        assert_eq!(
            parse_sof(MONA_LISA),
            Some(Sof {
                width: 120,
                height: 120,
                components: 3,
                bits: 8,
            })
        );
    }

    // No APP14: libjpeg assumes YCbCr for three components, so no
    // `/ColorTransform 0` is written.
    #[test]
    fn a_jfif_rgb_jpeg_is_colour_transformed() {
        assert_eq!(adobe_transform(MONA_LISA), None);
        assert!(has_colour_transform(MONA_LISA, 3));
    }

    #[test]
    fn a_jp2_file_yields_its_grid_size() {
        assert_eq!(jpx_size(GRAY_JP2), Some((4, 4)));
    }

    #[test]
    fn junk_is_not_a_jpeg() {
        assert_eq!(parse_sof(b"not a jpeg at all"), None);
        // A start-of-image and nothing else: the walk runs off the end
        // rather than reading past it.
        assert_eq!(parse_sof(&[0xFF, 0xD8]), None);
        assert_eq!(parse_sof(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00]), None);
    }

    // A frame declaring a zero dimension is not an image; PDFium's
    // `CPDF_Image::SetImage` refuses the same shape at `cpdf_image.cpp:186-189`.
    #[test]
    fn a_zero_dimension_frame_is_refused() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
        jpeg.extend_from_slice(&0u16.to_be_bytes());
        jpeg.extend_from_slice(&8u16.to_be_bytes());
        jpeg.push(3);
        assert_eq!(parse_sof(&jpeg), None);
    }
}
