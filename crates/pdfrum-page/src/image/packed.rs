//! Samples that are decoded but **not yet unpacked**, and the stage that
//! unpacks them one row at a time.
//!
//! The image ladder's fourth rung produces bytes: whatever the filter chain
//! left, in the depth the dictionary declares. Widening those to eight bits
//! per component is a row-at-a-time pass: [`Unpacked`] walks the packed
//! samples and yields one widened row per call.
//!
//! [`Packed`] is the same samples in the state the codec left them, plus the
//! geometry needed to walk them: a [`Depth`], a component count, a pitch and
//! the `/Decode` mapping already collapsed into one byte table per component.
//! [`Unpacked`] is the row stage that reads it. Nothing is materialized until
//! a caller asks for whole-image [`Pixels`], which
//! [`crate::image::Samples::to_pixels`] is the one function that does.
//!
//! # Why the decode is a table
//!
//! `DecodeMap::apply` is a float multiply-add, a clamp, a round and a cast,
//! and the mapping's answer depends only on the component index and the raw
//! sample. At every depth the sample space is at most 65 536 wide, so the
//! whole mapping tabulates. This is the same reasoning — and the same
//! rounding — the codec path's own `decode_table` already applies on that
//! path; here it also means the row stage's inner loop is a lookup rather
//! than arithmetic.

use crate::image::decode_array::DecodeMap;
use crate::image::scanline;

/// Bits per component, as the sample stream carries them.
///
/// The five values `/BitsPerComponent` is allowed to take. A type rather than
/// a `u32` because the unpack loop dispatches on it once per image and then
/// runs a loop that cannot be handed a depth the extractor does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// One bit: eight samples to the byte, MSB first.
    One,
    /// Two bits.
    Two,
    /// Four bits: two samples to the byte.
    Four,
    /// Eight bits: one sample to the byte.
    Eight,
    /// Sixteen bits, big-endian.
    Sixteen,
}

impl Depth {
    /// The depth `bpc` names, or `None` when it is not one PDF allows.
    #[must_use]
    pub const fn new(bpc: u32) -> Option<Self> {
        match bpc {
            1 => Some(Self::One),
            2 => Some(Self::Two),
            4 => Some(Self::Four),
            8 => Some(Self::Eight),
            16 => Some(Self::Sixteen),
            _ => None,
        }
    }

    /// Bits per sample.
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Four => 4,
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }

    /// How many distinct raw values this depth can express — the width of one
    /// row of the decode table.
    #[must_use]
    pub const fn levels(self) -> usize {
        1usize << self.bits()
    }
}

/// Decoded-but-not-unpacked samples, with everything needed to walk them.
///
/// This is the state the filter chain leaves an image in for every path that
/// does not go through a codec of its own: raw samples at [`Depth`], in the
/// colour space's own component order, still packed. The `/Decode` array is
/// already folded into a byte table, so unpacking a sample is a lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct Packed {
    /// The sample bytes, exactly as the filter chain left them.
    data: Box<[u8]>,
    /// Bits per component.
    depth: Depth,
    /// Components per pixel.
    components: usize,
    /// Bytes per source row.
    pitch: usize,
    /// Samples across.
    width: usize,
    /// Rows down.
    height: u32,
    /// The `/Decode` mapping, one row of `depth.levels()` bytes per component.
    table: Box<[u8]>,
}

impl Packed {
    /// Hold `data` as `width` x `height` samples of `components` at `depth`,
    /// with the `/Decode` mapping for `space` folded into the table.
    ///
    /// `pitch` is the dictionary's own row stride, which for a packed depth is
    /// wider than `width * components * depth / 8` rounded down: a row is
    /// byte-aligned, so the tail bits of the last byte belong to no pixel.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "packed samples are exactly this geometry, and naming each \
                  piece is what keeps the row walk from re-deriving any of it"
    )]
    pub fn new(
        data: Box<[u8]>,
        depth: Depth,
        components: usize,
        pitch: usize,
        width: u32,
        height: u32,
        space: &crate::color::ColorSpace,
        decode: Option<&pdfrum_object::Array>,
    ) -> Self {
        Self::with_map(
            data,
            depth,
            components,
            pitch,
            width,
            height,
            &DecodeMap::new(Some(space), components, depth.bits(), decode),
        )
    }

    /// The same, from a mapping the image ladder has already built.
    pub(crate) fn with_map(
        data: Box<[u8]>,
        depth: Depth,
        components: usize,
        pitch: usize,
        width: u32,
        height: u32,
        decode: &DecodeMap,
    ) -> Self {
        let levels = depth.levels();
        let mut table = vec![0u8; components.saturating_mul(levels)].into_boxed_slice();
        for component in 0..components {
            for raw in 0..levels {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a table index below 65 536 is exact in f32"
                )]
                let value = decode.apply(component, raw as f32);
                // Rounded, not truncated — the same encode `decode_table`
                // uses, and for the same reason: at 1, 2, 4 and 8 bits the two
                // agree on every raw value, and at 16 they diverge on 32 648
                // of the 65 536 samples with truncation a count low on each.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the clamp bounds the product to 0..=255"
                )]
                let byte = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                if let Some(slot) = table.get_mut(component * levels + raw) {
                    *slot = byte;
                }
            }
        }
        Self {
            data,
            depth,
            components,
            pitch,
            width: width as usize,
            height,
            table,
        }
    }

    /// Components per pixel.
    #[must_use]
    pub const fn components(&self) -> usize {
        self.components
    }

    /// Bytes held: the packed samples plus the decode table.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.data.len() + self.table.len()
    }

    /// Whether any row of the declared height is missing or short.
    ///
    /// The eager pass recorded a truncated-stream diagnostic
    /// while it walked; a lazy one has to answer the same question without
    /// walking, which the stream length does.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.pitch
            .checked_mul(self.height as usize)
            .is_none_or(|want| self.data.len() < want)
    }
}

/// The unpack stage: [`Packed`] samples widened to a byte per component, one
/// row at a time into a buffer it owns and reuses.
///
/// The row this yields is exactly the row the eager pass wrote into its
/// full-size buffer — same table, same rounding, same treatment of a short or
/// absent row — so putting the stage in front of
/// [`crate::image::Converted`] changes when the arithmetic runs and not what
/// it produces.
#[derive(Debug)]
pub struct Unpacked<'a> {
    packed: &'a Packed,
    /// One row of `width * components` bytes, reused.
    buf: Vec<u8>,
    /// The current row's packed bytes, zero-padded to the pitch. Held rather
    /// than allocated per row: the eager pass allocated one `Vec` per
    /// scanline, which on a tall image is as many allocations as rows.
    line: Vec<u8>,
    y: u32,
}

impl<'a> Unpacked<'a> {
    /// Start walking `packed` from its first row.
    #[must_use]
    pub fn new(packed: &'a Packed) -> Self {
        Self {
            buf: vec![0u8; packed.width.saturating_mul(packed.components)],
            line: vec![0u8; packed.pitch],
            packed,
            y: 0,
        }
    }

    /// The next row widened to a byte per component, or `None` past the
    /// bottom.
    ///
    /// A row the stream never reached yields **zeroes**, not the byte
    /// `/Decode` maps a zero sample to: PDFium returns a zeroed *output*
    /// buffer for an absent row without ever running the decode, so the pixels
    /// are literal black -- the distinction the scanline reader's
    /// `Availability` carries.
    pub fn next_row(&mut self) -> Option<&[u8]> {
        if self.y >= self.packed.height {
            return None;
        }
        let y = self.y as usize;
        self.y += 1;
        let p = self.packed;
        let availability = scanline::scanline_into(&p.data, y, p.pitch, &mut self.line);
        if availability == scanline::Availability::Absent {
            self.buf.fill(0);
            return Some(&self.buf);
        }
        let levels = p.depth.levels();
        // The component index cycles `0, 1, .. components-1` across the row,
        // so it is a counter that wraps, not `i % components` on every sample.
        // On a one-bit image the division was the larger half of the loop.
        let table = &*p.table;
        let line = &*self.line;
        match p.depth {
            // The byte-aligned depths are a walk, not a bit extraction: the
            // sample *is* the byte, so the row is a zip through the table.
            Depth::Eight => {
                let mut base = 0usize;
                let mut component = 0usize;
                for (slot, &raw) in self.buf.iter_mut().zip(line) {
                    *slot = table.get(base + usize::from(raw)).copied().unwrap_or(0);
                    component += 1;
                    base += levels;
                    if component == p.components {
                        component = 0;
                        base = 0;
                    }
                }
                // A row the stream could only partly supply keeps what it has
                // and zeroes the rest, which is the eager pass's own tail.
                if let Some(tail) = self.buf.get_mut(line.len()..) {
                    tail.fill(0);
                }
            }
            // Sixteen bits is two bytes per sample, big-endian, and the table
            // is indexed by the whole word.
            Depth::Sixteen => {
                let mut base = 0usize;
                let mut component = 0usize;
                for (i, slot) in self.buf.iter_mut().enumerate() {
                    let hi = line.get(i * 2).copied().unwrap_or(0);
                    let lo = line.get(i * 2 + 1).copied().unwrap_or(0);
                    let raw = usize::from(hi) * 256 + usize::from(lo);
                    *slot = table.get(base + raw).copied().unwrap_or(0);
                    component += 1;
                    base += levels;
                    if component == p.components {
                        component = 0;
                        base = 0;
                    }
                }
            }
            // The sub-byte depths pack several samples into a byte, MSB
            // first. Walking the byte and shifting down through it reads each
            // byte once, where a `get_bits` per sample re-derived the byte
            // index and the shift every time. On the one-bit images that is
            // the whole of this stage's cost.
            Depth::One | Depth::Two | Depth::Four => {
                let bits = p.depth.bits();
                let per_byte = (8 / bits) as usize;
                let mask = u32::from(u8::MAX) >> (8 - bits);
                let mut base = 0usize;
                let mut component = 0usize;
                for (chunk, byte_index) in self.buf.chunks_mut(per_byte).zip(0usize..) {
                    let byte = u32::from(line.get(byte_index).copied().unwrap_or(0));
                    // `k` is a position within one byte, so at most 7.
                    for (slot, k) in chunk.iter_mut().zip(0u32..) {
                        let shift = 8 - bits - k * bits;
                        let raw = ((byte >> shift) & mask) as usize;
                        *slot = table.get(base + raw).copied().unwrap_or(0);
                        component += 1;
                        base += levels;
                        if component == p.components {
                            component = 0;
                            base = 0;
                        }
                    }
                }
            }
        }
        Some(&self.buf)
    }

    /// The whole image unpacked, for the callers that genuinely want it.
    ///
    /// The one place the full-size buffer this stage exists to avoid is built:
    /// [`crate::image::Samples::to_pixels`], reached by the CLI's image
    /// export, the facade's edit path and the stencil check.
    #[must_use]
    pub fn collect_all(mut self) -> Box<[u8]> {
        let p = self.packed;
        let stride = p.width.saturating_mul(p.components);
        let mut out =
            Vec::with_capacity(stride.saturating_mul(usize::try_from(p.height).unwrap_or(0)));
        while let Some(row) = self.next_row() {
            out.extend_from_slice(row);
        }
        out.into_boxed_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorSpace;

    fn map(components: usize, bpc: u32) -> DecodeMap {
        DecodeMap::new(Some(&ColorSpace::DeviceGray), components, bpc, None)
    }

    fn packed(data: &[u8], bpc: u32, components: usize, width: u32, height: u32) -> Packed {
        let depth = Depth::new(bpc).expect("a real depth");
        let pitch = (width as usize * components * bpc as usize).div_ceil(8);
        Packed::with_map(
            data.into(),
            depth,
            components,
            pitch,
            width,
            height,
            &map(components, bpc),
        )
    }

    #[test]
    fn eight_bit_samples_pass_through_unchanged() {
        let p = packed(&[0, 128, 255, 7], 8, 1, 2, 2);
        let mut u = Unpacked::new(&p);
        assert_eq!(u.next_row(), Some(&[0, 128][..]));
        assert_eq!(u.next_row(), Some(&[255, 7][..]));
        assert_eq!(u.next_row(), None);
    }

    /// A one-bit sample widens to the two ends of the range, and the tail bits
    /// of a byte-aligned row belong to no pixel.
    #[test]
    fn one_bit_samples_widen_to_black_and_white() {
        // `0b1010_0000` across four declared pixels: set, clear, set, clear.
        let p = packed(&[0b1010_0000], 1, 1, 4, 1);
        let mut u = Unpacked::new(&p);
        assert_eq!(u.next_row(), Some(&[255, 0, 255, 0][..]));
    }

    #[test]
    fn four_bit_samples_span_the_range() {
        let p = packed(&[0x0F, 0x80], 4, 1, 4, 1);
        let mut u = Unpacked::new(&p);
        // 0 -> 0, 15 -> 255, 8 -> 136, 0 -> 0.
        assert_eq!(u.next_row(), Some(&[0, 255, 136, 0][..]));
    }

    /// Sixteen bits keeps the high byte, which is what the decode table's
    /// rounding over 65 536 levels comes to for the identity mapping.
    #[test]
    fn sixteen_bit_samples_keep_their_high_byte() {
        let p = packed(&[0xFF, 0xFF, 0x00, 0x00], 16, 1, 2, 1);
        let mut u = Unpacked::new(&p);
        assert_eq!(u.next_row(), Some(&[255, 0][..]));
    }

    /// A row that begins past the end of the stream is zeroes, not whatever
    /// the decode maps a zero sample to. A row that begins inside it keeps the
    /// samples it has and zero-pads the rest, which then *does* go through the
    /// decode.
    #[test]
    fn an_absent_row_is_zero_and_a_short_one_is_padded() {
        // Five bytes of a 2x4 image: rows 0 and 1 whole, row 2 half, row 3
        // absent.
        let p = packed(&[1, 2, 3, 4, 5], 8, 1, 2, 4);
        let mut u = Unpacked::new(&p);
        assert_eq!(u.next_row(), Some(&[1, 2][..]));
        assert_eq!(u.next_row(), Some(&[3, 4][..]));
        assert_eq!(u.next_row(), Some(&[5, 0][..]));
        assert_eq!(u.next_row(), Some(&[0, 0][..]));
        assert_eq!(u.next_row(), None);
        assert!(p.truncated());
    }

    /// A `/Decode [1 0]` inversion reaches the row through the table.
    #[test]
    fn a_decode_inversion_is_in_the_table() {
        let decode = pdfrum_object::Array::of([
            pdfrum_object::Object::Real(1.0),
            pdfrum_object::Object::Real(0.0),
        ]);
        let p = Packed::new(
            vec![0, 255].into(),
            Depth::Eight,
            1,
            2,
            2,
            1,
            &ColorSpace::DeviceGray,
            Some(&decode),
        );
        let mut u = Unpacked::new(&p);
        assert_eq!(u.next_row(), Some(&[255, 0][..]));
    }

    #[test]
    fn collecting_every_row_is_the_whole_image() {
        let p = packed(&[1, 2, 3, 4], 8, 1, 2, 2);
        assert_eq!(&*Unpacked::new(&p).collect_all(), &[1, 2, 3, 4]);
    }
}
