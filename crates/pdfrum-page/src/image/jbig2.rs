//! JBIG2 decoding, behind the thin entry point SPEC.md §12 requires.
//!
//! The decoder crate never appears in a signature: everything crosses this
//! boundary as bytes in and a [`BitImage`] out, so replacing it with a
//! first-party port is a drop-in change.
//!
//! JBIG2 in PDF is always one bit and one component — the dictionary
//! validation forces that before we get here. `/JBIG2Globals` is fetched from
//! the decode parameters and **absent or unfetchable globals are silently
//! tolerated**: the decoder is simply called without them.

use crate::error::Error;
use pdfrum_common::Limits;

/// A one-bit-per-pixel image, packed MSB-first with each row starting on a
/// byte boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row, `width.div_ceil(8)`.
    pub row_bytes: usize,
    /// The packed bits. A set bit is **black**, matching the JBIG2 convention
    /// and PDF's default `/Decode` for a one-bit image.
    pub bits: Vec<u8>,
}

impl BitImage {
    /// Whether the pixel at `(x, y)` is black.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> bool {
        let Ok(row) = usize::try_from(y) else {
            return false;
        };
        let Ok(col) = usize::try_from(x) else {
            return false;
        };
        let Some(byte) = self.bits.get(row * self.row_bytes + col / 8) else {
            return false;
        };
        (byte >> (7 - (col % 8))) & 1 == 1
    }
}

/// A sink collecting the decoder's pixel callbacks into a packed bitmap.
struct BitSink {
    bits: Vec<u8>,
    row_bytes: usize,
    /// Bit offset within the current row.
    x: usize,
    /// The row currently being filled.
    y: usize,
    height: usize,
}

impl BitSink {
    fn new(width: u32, height: u32) -> Option<Self> {
        let row_bytes = usize::try_from(width.div_ceil(8)).ok()?;
        let height = usize::try_from(height).ok()?;
        let len = row_bytes.checked_mul(height)?;
        Some(Self {
            bits: vec![0; len],
            row_bytes,
            x: 0,
            y: 0,
            height,
        })
    }

    fn set(&mut self, black: bool) {
        if black && self.y < self.height {
            let index = self.y * self.row_bytes + self.x / 8;
            if let Some(byte) = self.bits.get_mut(index) {
                *byte |= 1 << (7 - (self.x % 8));
            }
        }
        self.x += 1;
    }
}

impl hayro_jbig2::Decoder for BitSink {
    fn push_pixel(&mut self, black: bool) {
        self.set(black);
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        for _ in 0..chunk_count.saturating_mul(8) {
            self.set(black);
        }
    }

    fn next_line(&mut self) {
        self.x = 0;
        self.y += 1;
    }
}

/// Decode an embedded JBIG2 image.
///
/// `globals` is the `/JBIG2Globals` stream's decoded bytes when there is one;
/// `None` is normal and not an error. `w` and `h` are the dictionary's
/// dimensions, which the codestream may disagree with — the dictionary wins,
/// because the rest of the image path is already sized to it.
///
/// # Errors
///
/// [`Error::CodecRejected`] when the codestream cannot be decoded, and
/// [`Error::ImageTooLarge`] when the requested bitmap exceeds the byte
/// budget.
pub fn decode_jbig2(
    globals: Option<&[u8]>,
    data: &[u8],
    w: u32,
    h: u32,
    limits: &Limits,
) -> Result<BitImage, Error> {
    let Some(mut sink) = BitSink::new(w, h) else {
        return Err(Error::ImageTooLarge);
    };
    if sink.bits.len() > limits.max_decoded_stream_len {
        return Err(Error::ImageTooLarge);
    }
    let image = hayro_jbig2::Image::new_embedded(data, globals)
        .map_err(|_| Error::CodecRejected { codec: "JBIG2" })?;
    image
        .decode(&mut sink)
        .map_err(|_| Error::CodecRejected { codec: "JBIG2" })?;
    Ok(BitImage {
        width: w,
        height: h,
        row_bytes: sink.row_bytes,
        bits: sink.bits,
    })
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

    use super::{BitImage, decode_jbig2};
    use pdfrum_common::Limits;

    #[test]
    fn garbage_is_rejected_rather_than_panicked_on() {
        let limits = Limits::default();
        for data in [&b""[..], b"\x00", b"not jbig2 at all", &[0xFFu8; 64]] {
            let got = decode_jbig2(None, data, 8, 8, &limits);
            assert!(got.is_err(), "{data:?} should be rejected");
        }
    }

    #[test]
    fn an_oversized_request_is_refused_before_decoding() {
        let limits = Limits {
            max_decoded_stream_len: 16,
            ..Limits::default()
        };
        assert!(decode_jbig2(None, b"", 1000, 1000, &limits).is_err());
    }

    #[test]
    fn pixel_access_is_msb_first_and_bounds_checked() {
        let img = BitImage {
            width: 12,
            height: 2,
            row_bytes: 2,
            bits: vec![0b1000_0001, 0b0100_0000, 0, 0],
        };
        assert!(img.pixel(0, 0));
        assert!(!img.pixel(1, 0));
        assert!(img.pixel(7, 0));
        assert!(img.pixel(9, 0));
        assert!(!img.pixel(0, 1));
        // Out of bounds reads as unset rather than panicking.
        assert!(!img.pixel(99, 0));
        assert!(!img.pixel(0, 99));
    }
}
