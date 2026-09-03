//! Raw interleaved samples as an image `XObject`, with the alpha channel
//! split off into an `/SMask`.

use pdfrum_object::{Array, ByteSpan, Dict, Object, Stream};

use super::EmbeddedImage;
use super::jpeg::{device_space, image_dict};
use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

/// How the bytes handed to [`crate::EditDoc::embed_image`] are laid out.
///
/// The oracle takes a bitmap object and reads the format back off it; here
/// the caller states it, because what arrives is loose bytes rather than a
/// bitmap that knows its own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// One eight-bit grey sample per pixel, `/DeviceGray`.
    Gray8,
    /// Three eight-bit samples per pixel, `/DeviceRGB`.
    Rgb8,
    /// Four eight-bit samples per pixel, `/DeviceCMYK`.
    Cmyk8,
    /// Four eight-bit samples per pixel; the fourth becomes a `/DeviceGray`
    /// `/SMask` and the first three the image's own `/DeviceRGB` samples.
    Rgba8,
    /// One bit per pixel, rows padded to a byte, written as an `/ImageMask`.
    ///
    /// A set bit paints, which is the sense `/Decode [1 0]` gives a stencil
    /// mask; see [`crate::EditDoc::embed_image`].
    Mask1,
}

impl PixelFormat {
    /// Bytes one row of `width` pixels occupies.
    fn row_bytes(self, width: u32) -> Option<usize> {
        let width = usize::try_from(width).ok()?;
        match self {
            Self::Mask1 => width.checked_add(7).map(|w| w / 8),
            Self::Gray8 => Some(width),
            Self::Rgb8 => width.checked_mul(3),
            Self::Cmyk8 | Self::Rgba8 => width.checked_mul(4),
        }
    }

    /// The `/ColorSpace` components the *image* stream carries, which is one
    /// fewer than [`Self::Rgba8`] is handed.
    fn stored_components(self) -> u8 {
        match self {
            Self::Mask1 | Self::Gray8 => 1,
            Self::Rgb8 | Self::Rgba8 => 3,
            Self::Cmyk8 => 4,
        }
    }
}

pub(super) fn embed(
    doc: &mut EditDoc<'_>,
    pixels: &[u8],
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<EmbeddedImage, Error> {
    // `CPDF_Image::SetImage` returns without touching the stream when either
    // dimension is below one (`cpdf_image.cpp:186-189`); with no stream to
    // hand back, an error is the same answer a caller can act on.
    if width == 0 || height == 0 {
        return Err(Error::EmptyImage);
    }
    let row = format.row_bytes(width).ok_or(Error::EmptyImage)?;
    let expected = usize::try_from(height)
        .ok()
        .and_then(|h| row.checked_mul(h))
        .ok_or(Error::EmptyImage)?;
    if pixels.len() != expected {
        return Err(Error::ImageDataLength {
            expected,
            found: pixels.len(),
        });
    }

    let mut dict = image_dict(width, height);
    let data = match format {
        // `:196-220`: a one-bit bitmap whose palette has a transparent entry
        // becomes `/ImageMask true`, and the `reset` colour being the
        // transparent one is what inverts `/Decode`. A caller handing us a
        // mask has said which sense they mean by choosing `Mask1`: a set bit
        // paints, so the sample value 1 must map to 0 — the "paint" end of an
        // `/ImageMask`'s range (ISO 32000-1 §8.9.6.2) — which is `[1 0]`.
        PixelFormat::Mask1 => {
            dict.push(names::IMAGE_MASK.clone(), Object::Bool(true));
            dict.push(
                names::DECODE.clone(),
                Object::Array(Array::of([Object::Int(1), Object::Int(0)])),
            );
            dict.push(names::BITS_PER_COMPONENT.clone(), Object::Int(1));
            pixels.to_vec()
        }
        PixelFormat::Rgba8 => {
            let (colour, alpha) = split_alpha(pixels);
            let smask = doc.add(Object::Stream(Stream::new(
                smask_dict(width, height),
                ByteSpan::from(alpha),
            )));
            push_space(&mut dict, format);
            dict.push(names::SMASK.clone(), Object::Ref(smask));
            colour
        }
        PixelFormat::Gray8 | PixelFormat::Rgb8 | PixelFormat::Cmyk8 => {
            push_space(&mut dict, format);
            pixels.to_vec()
        }
    };

    // No `/Filter`: the stream writer flate-encodes any stream that declares
    // none, which is the same place the subsetter leaves its font programs.
    Ok(EmbeddedImage {
        image: doc.add(Object::Stream(Stream::new(dict, ByteSpan::from(data)))),
        width,
        height,
    })
}

fn push_space(dict: &mut Dict, format: PixelFormat) {
    if let Some(space) = device_space(format.stored_components()) {
        dict.push(names::COLOR_SPACE.clone(), Object::Name(space));
    }
    dict.push(names::BITS_PER_COMPONENT.clone(), Object::Int(8));
}

/// The alpha image's dictionary: eight-bit `/DeviceGray` at the colour
/// image's size.
fn smask_dict(width: u32, height: u32) -> Dict {
    let mut dict = image_dict(width, height);
    dict.push(
        names::COLOR_SPACE.clone(),
        Object::Name(names::DEVICE_GRAY.clone()),
    );
    dict.push(names::BITS_PER_COMPONENT.clone(), Object::Int(8));
    dict
}

/// Interleaved RGBA into an RGB image and its alpha plane.
fn split_alpha(pixels: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let count = pixels.len() / 4;
    let mut colour = Vec::with_capacity(count * 3);
    let mut alpha = Vec::with_capacity(count);
    for px in pixels.chunks_exact(4) {
        colour.extend_from_slice(px.get(..3).unwrap_or_default());
        alpha.push(px.get(3).copied().unwrap_or(0xFF));
    }
    (colour, alpha)
}

#[cfg(test)]
mod tests {
    use super::{PixelFormat, split_alpha};

    #[test]
    fn a_row_is_padded_to_a_byte_only_for_a_mask() {
        assert_eq!(PixelFormat::Mask1.row_bytes(9), Some(2));
        assert_eq!(PixelFormat::Gray8.row_bytes(9), Some(9));
        assert_eq!(PixelFormat::Rgb8.row_bytes(9), Some(27));
        assert_eq!(PixelFormat::Rgba8.row_bytes(9), Some(36));
        assert_eq!(PixelFormat::Cmyk8.row_bytes(9), Some(36));
    }

    // The stored image is RGB even though the caller handed four channels.
    #[test]
    fn rgba_stores_three_components() {
        assert_eq!(PixelFormat::Rgba8.stored_components(), 3);
    }

    #[test]
    fn the_alpha_channel_leaves_the_colour_channels_in_order() {
        let (colour, alpha) = split_alpha(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(colour, vec![1, 2, 3, 5, 6, 7]);
        assert_eq!(alpha, vec![4, 8]);
    }
}
