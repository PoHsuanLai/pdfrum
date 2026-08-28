//! Decoding PNGs into the RGBA form Tier B compares.
//!
//! The oracle writes 8-bit RGB or RGBA PNGs; our own tool will too. Everything
//! is normalized to 8-bit RGBA so `ssim::compare` sees one shape.

use crate::ssim::Image;

/// Why a PNG could not be turned into an `Image`.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("PNG decode failed: {0}")]
    Png(String),
    #[error("unsupported PNG: {bit_depth}-bit {color_type}")]
    Unsupported {
        bit_depth: String,
        color_type: String,
    },
}

/// Decodes PNG bytes into 8-bit RGBA.
pub fn decode(bytes: &[u8]) -> Result<Image, DecodeError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Normalize away palettes, tRNS chunks, 16-bit samples and sub-byte
    // grayscale so the frame always arrives as 8-bit RGB or RGBA.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|err| DecodeError::Png(err.to_string()))?;
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| DecodeError::Png(err.to_string()))?;
    buffer.truncate(info.buffer_size());

    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        other @ png::ColorType::Indexed => {
            return Err(DecodeError::Unsupported {
                bit_depth: format!("{:?}", info.bit_depth),
                color_type: format!("{other:?}"),
            });
        }
    };

    Ok(Image {
        width: info.width,
        height: info.height,
        rgba,
    })
}

#[cfg(test)]
// Exact float equality is deliberate here: these assertions pin values that
// are exact by construction (SSIM of identical input is 1.0; a threshold
// parsed from text round-trips bit-for-bit). An epsilon would weaken them.
#[allow(clippy::float_cmp, reason = "asserting exactly-representable values")]
mod tests {
    use super::*;

    fn encode(width: u32, height: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        out
    }

    #[test]
    fn decodes_rgb_to_opaque_rgba() {
        let png_bytes = encode(2, 1, png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6]);
        let image = decode(&png_bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        image.validate().unwrap();
    }

    #[test]
    fn decodes_rgba_unchanged() {
        let png_bytes = encode(1, 2, png::ColorType::Rgba, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let image = decode(&png_bytes).unwrap();
        assert_eq!(image.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn decodes_grayscale_by_replicating_the_channel() {
        let png_bytes = encode(2, 1, png::ColorType::Grayscale, &[10, 200]);
        let image = decode(&png_bytes).unwrap();
        assert_eq!(image.rgba, [10, 10, 10, 255, 200, 200, 200, 255]);
    }

    #[test]
    fn decodes_grayscale_alpha() {
        let png_bytes = encode(1, 1, png::ColorType::GrayscaleAlpha, &[64, 128]);
        let image = decode(&png_bytes).unwrap();
        assert_eq!(image.rgba, [64, 64, 64, 128]);
    }

    #[test]
    fn a_decoded_pair_compares_as_identical() {
        let png_bytes = encode(8, 8, png::ColorType::Rgb, &[77; 8 * 8 * 3]);
        let image = decode(&png_bytes).unwrap();
        let diff = crate::ssim::compare(&image, &image).unwrap();
        assert_eq!(diff.ssim, 1.0);
        assert!(diff.exact);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode(b"not a png").is_err());
        assert!(decode(&[]).is_err());
        // A valid signature with a truncated body.
        assert!(decode(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).is_err());
    }
}
