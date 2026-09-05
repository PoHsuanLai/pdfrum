//! Unpacking packed samples into pixels (ISO 32000-1 §8.9.5.1).
//!
//! Samples are **MSB-first big-endian**, each scanline starts on a byte
//! boundary, and a truncated stream is **zero-padded** rather than rejected —
//! the last partial row simply reads as black.
//!
//! Three encoding details differ between paths and each is load-bearing:
//!
//! - A **one-bit stencil mask with the default decode is inverted
//!   bit-for-bit**; with `/Decode [1 0]` it is copied verbatim. So `/Decode`
//!   flips a mask's sense in the opposite direction from what one expects.
//! - The oracle additionally *palettizes* any image with
//!   `bpc * components <= 8`, packing component `j` into bits
//!   `[j*bpc, (j+1)*bpc)` and precomputing one colour per index.
//!   **pdfrum does not, and the
//!   pixels are the same either way** — the palette precomputes the very
//!   composition the general path evaluates per pixel, which is proved
//!   exhaustively by
//!   `a_packed_palette_lookup_and_the_general_path_agree_on_every_index`. It
//!   is a lookup-table optimisation, and no corpus image would exercise it.
//!
//! # 16-bit samples are scaled, not truncated
//!
//! ISO 32000-1 §8.9.5 defines a 16-bit sample as a value in `[0, 65535]`
//! mapped linearly onto its `/Decode` range, and the byte a device buffer
//! wants is that value **rounded**. Both readers land there, by different
//! routes:
//!
//! - **pdf.js scales.** `DeviceRgbCS.getRgbBuffer` (`src/core/colorspace.js`)
//!   computes `scale = 255 / ((1 << bits) - 1)` and stores `scale * sample`
//!   into a `Uint8ClampedArray`, whose store rounds. That is the linear map
//!   done exactly.
//! - **PDFium truncates the high byte.** Its default-decode RGB fast path
//!   keeps only each sample's high byte — `sample >> 8`, the low byte
//!   dropped. It is within one count of the rounded answer on every sample
//!   (16 256 of 65 536 differ), an approximation of the same map rather than
//!   a different one.
//!
//! So the ecosystem agrees on the map and differs only on how carefully it is
//! rounded. `image::mod`'s general `/Decode` path computes it exactly, which
//! is why there is no 16-bit special case here: the fast path this module once
//! carried for it would have been a *worse* answer than the general one,
//! and both `>> 8` and a truncated float product read a count low.

/// Read `nbits` bits at `bit_pos`, MSB-first, with `nbits` in `{1,2,4,8,16}`.
///
/// Past the end of the data the result is zero, which is what makes a
/// truncated scanline read as black instead of failing.
#[must_use]
pub fn get_bits(data: &[u8], bit_pos: usize, nbits: u32) -> u32 {
    let byte_index = bit_pos / 8;
    let byte = data.get(byte_index).copied().unwrap_or(0);
    match nbits {
        8 => u32::from(byte),
        16 => u32::from(byte) * 256 + u32::from(data.get(byte_index + 1).copied().unwrap_or(0)),
        _ => {
            let shift = 8u32
                .saturating_sub(nbits)
                .saturating_sub(u32::try_from(bit_pos % 8).unwrap_or(0));
            (u32::from(byte) >> shift) & ((1u32 << nbits.min(31)) - 1)
        }
    }
}

/// How much of a requested scanline the stream actually held.
///
/// The distinction is load-bearing. A row that begins *inside* the stream is
/// zero-padded and then decoded normally. A row that begins at or past the
/// end never reaches the decode at all: the *output* buffer is zeroed, so the
/// pixels are literal black rather than whatever `/Decode` maps a zero sample
/// to. On
/// `bug_554151` — a `/Decode [1.0]` that maps sample 0 to full red — the two
/// spellings differ across the whole page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The stream held the whole row.
    Whole,
    /// The row began inside the stream but ran off its end; the tail is zero.
    Partial,
    /// The row began at or past the end of the stream. The decode is skipped
    /// and the *output* pixels are zero.
    Absent,
}

/// One scanline's raw sample bytes, zero-padded when the source ran short,
/// and how much of it the stream actually held.
#[must_use]
pub fn scanline(data: &[u8], line: u32, pitch: usize) -> (Vec<u8>, Availability) {
    let Some(start) = usize::try_from(line)
        .ok()
        .and_then(|l| l.checked_mul(pitch))
    else {
        return (vec![0; pitch], Availability::Absent);
    };
    // `GetSize() > line * pitch` is the C++'s test for "this row starts inside
    // the stream". A pitch of zero makes every row start at zero, so an empty
    // stream is still `Absent`.
    if start >= data.len() {
        return (vec![0; pitch], Availability::Absent);
    }
    let available = data.get(start..).unwrap_or(&[]);
    if available.len() >= pitch {
        return (
            available.get(..pitch).unwrap_or(&[]).to_vec(),
            Availability::Whole,
        );
    }
    let mut out = vec![0u8; pitch];
    if let Some(dest) = out.get_mut(..available.len()) {
        dest.copy_from_slice(available);
    }
    (out, Availability::Partial)
}

/// One scanline's raw sample bytes into a caller-owned buffer.
///
/// [`scanline`] allocates a `Vec` per row, which for a pull pipeline is one
/// allocation per row of every image on the page. This is the same function
/// against a buffer the caller reuses: `out` is filled to its own length, so
/// the caller sizes it to the pitch once.
pub fn scanline_into(data: &[u8], line: usize, pitch: usize, out: &mut [u8]) -> Availability {
    let Some(start) = line.checked_mul(pitch) else {
        out.fill(0);
        return Availability::Absent;
    };
    if start >= data.len() {
        out.fill(0);
        return Availability::Absent;
    }
    let available = data.get(start..).unwrap_or(&[]);
    if available.len() >= pitch {
        let n = out.len().min(pitch);
        if let (Some(dest), Some(src)) = (out.get_mut(..n), available.get(..n)) {
            dest.copy_from_slice(src);
        }
        if let Some(tail) = out.get_mut(n..) {
            tail.fill(0);
        }
        return Availability::Whole;
    }
    let n = out.len().min(available.len());
    if let (Some(dest), Some(src)) = (out.get_mut(..n), available.get(..n)) {
        dest.copy_from_slice(src);
    }
    if let Some(tail) = out.get_mut(n..) {
        tail.fill(0);
    }
    Availability::Partial
}

/// Invert a one-bit scanline in place, which is what a default-decode stencil
/// mask does.
pub fn invert_line(line: &mut [u8]) {
    for b in line.iter_mut() {
        *b = !*b;
    }
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

    use super::{Availability, get_bits, invert_line, scanline};

    /// The oracle's packing rule, kept here because only these tests need it:
    /// component `j` occupies bits `[j*bpc, (j+1)*bpc)`, first in the low
    /// bits, which is the order `LoadPalette` enumerates indices in.
    fn palette_index(line: &[u8], pixel: usize, components: u32, bpc: u32) -> u32 {
        let mut index = 0u32;
        for j in 0..components {
            let bit_pos = (pixel * components as usize + j as usize) * bpc as usize;
            index |= get_bits(line, bit_pos, bpc) << (j * bpc);
        }
        index
    }

    #[test]
    fn bits_are_read_msb_first_and_zero_past_the_end() {
        let data = [0b1011_0010u8, 0x34];
        assert_eq!(get_bits(&data, 0, 1), 1);
        assert_eq!(get_bits(&data, 1, 1), 0);
        assert_eq!(get_bits(&data, 0, 2), 0b10);
        assert_eq!(get_bits(&data, 0, 4), 0b1011);
        assert_eq!(get_bits(&data, 4, 4), 0b0010);
        assert_eq!(get_bits(&data, 0, 8), 0b1011_0010);
        assert_eq!(get_bits(&data, 0, 16), 0xB234);
        // Past the end: zeros, not a panic.
        assert_eq!(get_bits(&data, 800, 8), 0);
        assert_eq!(get_bits(&[], 0, 8), 0);
    }

    #[test]
    fn a_truncated_scanline_is_zero_padded() {
        let data = [1u8, 2, 3];
        let (line, had) = scanline(&data, 0, 2);
        assert_eq!(line, vec![1, 2]);
        assert_eq!(had, Availability::Whole);
        // The second row has only one byte available.
        let (line, had) = scanline(&data, 1, 2);
        assert_eq!(line, vec![3, 0]);
        assert_eq!(had, Availability::Partial);
        // Past the end entirely: all zeros, and `Absent` rather than
        // `Partial` — the caller must skip the decode, not zero-pad into it.
        let (line, had) = scanline(&data, 9, 2);
        assert_eq!(line, vec![0, 0]);
        assert_eq!(had, Availability::Absent);
        // The boundary: row 2 starts at byte 4, one past the last byte.
        assert_eq!(scanline(&data, 2, 2).1, Availability::Absent);
        // An empty stream has no whole rows at all.
        assert_eq!(scanline(&[], 0, 4).1, Availability::Absent);
    }

    #[test]
    fn inversion_is_bitwise() {
        let mut line = [0b1010_1010u8, 0x00];
        invert_line(&mut line);
        assert_eq!(line, [0b0101_0101, 0xFF]);
    }

    #[test]
    fn the_palette_index_puts_the_first_component_in_the_low_bits() {
        // Two two-bit components: 0b01 then 0b10, packed as 0b0110 in one
        // nibble, must produce index `0b10 << 2 | 0b01`.
        let line = [0b0110_0000u8];
        assert_eq!(palette_index(&line, 0, 2, 2), 0b10_01);
    }

    /// The packed-palette path and the general path are the **same pixels**.
    ///
    /// This is the proof that retired the unwired `palette_index` helper. The
    /// oracle palettizes any image with `bpc * components <= 8`, building one
    /// entry per possible packed index and looking each pixel up. pdfrum
    /// palettizes only `Indexed` and widens every other space to a byte per
    /// component.
    ///
    /// Both routes evaluate the same two functions in the same order:
    /// `decode_min + decode_step * raw` per component — `DecodeMap::apply`
    /// here, the same affine map there — and then the space's own conversion.
    /// The palette merely
    /// *precomputes* that composition for all `1 << (bpc * components)`
    /// inputs, and since the composition is a pure function of the packed
    /// index, precomputing it cannot change an answer. Enumerating every
    /// index and comparing is what turns that argument into a check.
    ///
    /// So the oracle's palette is a **lookup-table optimisation, not a
    /// behaviour**, and the corpus has nothing that would exercise it: a scan
    /// of all 1319 corpus PDFs and 551 `.in` templates found 839 images with
    /// `bpc * components <= 8` and **every one single-component**
    /// (`/DeviceGray` or already-`Indexed`). Wiring it would add a second
    /// decode path for zero board rows, so the helper was deleted and this
    /// test keeps the reasoning.
    #[test]
    fn a_packed_palette_lookup_and_the_general_path_agree_on_every_index() {
        use crate::color::ColorSpace;
        use crate::image::decode_array::DecodeMap;

        // 2-bpc RGB (2*3 = 6 <= 8) and 1-bpc CMYK (1*4 = 4 <= 8): the two
        // shapes the oracle palettizes and we do not.
        for (space, components, bpc) in [
            (ColorSpace::DeviceRgb, 3u32, 2u32),
            (ColorSpace::DeviceCmyk, 4, 1),
        ] {
            let n = components as usize;
            let map = DecodeMap::new(Some(&space), n, bpc, None);

            // The oracle's palette: one entry per packed index, components
            // peeled off from the low bits (`LoadPalette`'s `color_data %`
            // / `/=` loop).
            let entries = 1u32 << (bpc * components);
            let palette: Vec<_> = (0..entries)
                .map(|index| {
                    let mut rest = index;
                    let comps: Vec<f32> = (0..n)
                        .map(|j| {
                            let raw = rest % (1 << bpc);
                            rest /= 1 << bpc;
                            map.apply(j, raw as f32)
                        })
                        .collect();
                    space.to_rgb(&comps)
                })
                .collect();

            // Every packed index, as a one-pixel scanline, through both
            // routes.
            for index in 0..entries {
                let mut line = [0u8; 2];
                for j in 0..components {
                    let v = (index >> (j * bpc)) & ((1 << bpc) - 1);
                    let bit_pos = (j * bpc) as usize;
                    let shift = 8 - bpc as usize - (bit_pos % 8);
                    line[bit_pos / 8] |= u8::try_from(v).unwrap() << shift;
                }
                // `palette_index` must recover the index the palette was
                // built against — the packing and the enumeration agree.
                assert_eq!(
                    palette_index(&line, 0, components, bpc),
                    index,
                    "{space:?} bpc {bpc}: packing and enumeration disagree"
                );
                // The general path: decode each component, widen to a byte,
                // convert.
                let comps: Vec<f32> = (0..n)
                    .map(|j| {
                        let raw = get_bits(&line, j * bpc as usize, bpc);
                        map.apply(j, raw as f32)
                    })
                    .collect();
                assert_eq!(
                    space.to_rgb(&comps),
                    palette[index as usize],
                    "{space:?} bpc {bpc}: index {index} differs between paths"
                );
            }
        }
    }
}
