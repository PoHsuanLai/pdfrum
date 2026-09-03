//! JBIG2 decoding, behind a thin entry point.
//!
//! The decoder crate never appears in a signature: everything crosses this
//! boundary as bytes in and a [`BitImage`] out, so replacing it with a
//! first-party port is a drop-in change.
//!
//! JBIG2 in PDF is always one bit and one component — the dictionary
//! validation forces that before we get here. `/JBIG2Globals` is fetched from
//! the decode parameters and **absent or unfetchable globals are silently
//! tolerated**: the decoder is simply called without them.

use super::BitImage;
use crate::error::Error;
use pdfrum_common::Limits;

/// A sink collecting the decoder's pixel callbacks into a packed bitmap.
///
/// The sink is sized by the *dictionary*, and the codestream is free to
/// disagree with it — see [`decode_jbig2`]'s note on which one wins. Both
/// fields below exist to make that disagreement cheap and correct rather than
/// merely survivable; a corpus document exists where it was neither.
struct BitSink {
    bits: Vec<u8>,
    row_bytes: usize,
    /// Pixels per row, from the dictionary.
    ///
    /// Kept alongside `row_bytes` rather than derived from it, because the two
    /// disagree whenever the width is not a multiple of eight and the
    /// difference is exactly the padding a write must not spill into.
    width: usize,
    /// Bit offset within the current row.
    x: usize,
    /// The row currently being filled.
    y: usize,
    height: usize,
}

impl BitSink {
    fn new(width: u32, height: u32) -> Option<Self> {
        let row_bytes = usize::try_from(width.div_ceil(8)).ok()?;
        let width = usize::try_from(width).ok()?;
        let height = usize::try_from(height).ok()?;
        let len = row_bytes.checked_mul(height)?;
        Some(Self {
            bits: vec![0; len],
            row_bytes,
            width,
            x: 0,
            y: 0,
            height,
        })
    }

    /// Is the cursor outside the bitmap the dictionary asked for?
    ///
    /// Past the last row, or past the last pixel of the current row. The second
    /// half is the one that was missing: the flat index `y * row_bytes + x / 8`
    /// is checked against the buffer's *length*, which a write one row too far
    /// right passes — it simply lands on the next row's leading byte. A
    /// codestream wider than the dictionary therefore bled set pixels into the
    /// row below, silently, on every row.
    fn out_of_range(&self) -> bool {
        self.y >= self.height || self.x >= self.width
    }

    fn set(&mut self, black: bool) {
        if black && !self.out_of_range() {
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
        let pixels = usize::try_from(chunk_count)
            .unwrap_or(usize::MAX)
            .saturating_mul(8);
        // A chunk landing entirely outside the bitmap the dictionary asked for
        // is skipped in O(1) instead of one no-op `set` per pixel. This is not
        // a micro-optimization: M12 found `image_ccitt_transfer` declaring a
        // 400x400 image over a codestream whose page is 3562x851, so nineteen
        // twentieths of three million callback-driven pixel writes were being
        // made and then discarded — 16.3 ms of the document's 26.6 ms, on an
        // image of 160,000 pixels. The decoder cannot be told to stop, so the
        // sink stops instead.
        //
        // Both edges matter and they are hit by different halves of that file's
        // excess: `y >= height` skips the 451 rows past the bottom, and
        // `x >= width` skips the 3162 columns past the right of each of the 400
        // rows that are kept. A chunk never straddles a row boundary — only
        // `next_line` advances `y` — so once the cursor is past the right edge
        // it stays past it for the rest of the row, and the early return is
        // exact rather than approximate.
        if self.out_of_range() {
            self.x = self.x.saturating_add(pixels);
            return;
        }
        for _ in 0..pixels {
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
    fn a_codestream_wider_than_the_dictionary_does_not_bleed_into_the_next_row() {
        use hayro_jbig2::Decoder as _;

        // The dictionary says four pixels wide. `row_bytes` is 1, so the flat
        // index `y * row_bytes + x / 8` stays inside the buffer for x up to 7 —
        // which is how a wider codestream used to write row 0's pixel 5 and
        // find itself still in row 0's only byte, and pixel 8 in *row 1*.
        //
        // This is `image_ccitt_transfer`'s shape in miniature: it declares
        // 400x400 over a codestream whose page is 3562x851.
        let mut sink = super::BitSink::new(4, 2).expect("4x2 fits");
        for _ in 0..4 {
            sink.push_pixel(false);
        }
        // Four more black pixels the dictionary has no room for.
        for _ in 0..4 {
            sink.push_pixel(true);
        }
        sink.next_line();
        for _ in 0..4 {
            sink.push_pixel(false);
        }

        assert_eq!(
            sink.bits,
            vec![0, 0],
            "pixels past the declared width must be dropped, not folded into \
             the row's padding bits or the row below"
        );
    }

    #[test]
    fn a_chunk_straddling_the_right_edge_keeps_its_valid_prefix() {
        use hayro_jbig2::Decoder as _;

        // The early return in `push_pixel_chunk` tests the cursor at the
        // chunk's *start*, so a chunk that begins inside the row and runs off
        // the right edge must still take the per-pixel path and write the part
        // that fits. Getting this wrong would drop real pixels, which is a
        // worse bug than the one the fast path fixes.
        //
        // Twelve pixels declared, so `row_bytes` is 2 and the second byte's
        // top four bits are the row while its bottom four are padding. One
        // 8-pixel chunk of black starting at x=8 covers pixels 8..15: 8..11
        // are the row's last four, 12..15 are padding that must stay clear.
        let mut sink = super::BitSink::new(12, 1).expect("12x1 fits");
        for _ in 0..8 {
            sink.push_pixel(false);
        }
        sink.push_pixel_chunk(true, 1);
        assert_eq!(
            sink.bits,
            vec![0b0000_0000, 0b1111_0000],
            "the four pixels inside the declared width are written and the \
             four padding bits past it are not"
        );
    }

    #[test]
    fn a_chunk_past_the_last_row_is_skipped_rather_than_walked() {
        use hayro_jbig2::Decoder as _;

        // The cost fix, stated as behaviour: a chunk callback aimed at a row the
        // dictionary does not have must leave the bitmap alone and advance the
        // cursor by the right amount, without touching a pixel per bit. The
        // observable half is the bitmap; the cheap half is what the loop does
        // not do, and there is nothing to assert about that except that the
        // answer is still right.
        let mut sink = super::BitSink::new(8, 1).expect("8x1 fits");
        sink.next_line(); // now at row 1, past the single row
        sink.push_pixel_chunk(true, 100_000);
        assert_eq!(sink.bits, vec![0], "nothing outside the bitmap is written");
        assert_eq!(sink.y, 1);
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
