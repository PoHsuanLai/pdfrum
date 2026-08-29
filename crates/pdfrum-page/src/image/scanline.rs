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
//! - At **16 bits only the high byte of each sample survives**, with no
//!   rounding: the low byte is discarded outright.
//! - At **1, 2 and 4 bits the components are scaled by integer arithmetic**
//!   (`v * 255 / max`), and the packed index puts component `j` in bits
//!   `[j*bpc, (j+1)*bpc)` — an order the palette builder must match exactly.

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

/// One scanline's raw sample bytes, zero-padded when the source ran short.
///
/// The padding is the truncated-stream fallback: PDFium copies whatever
/// remains into a zeroed buffer rather than refusing the image, so a file cut
/// off mid-image still renders its complete rows.
#[must_use]
pub fn scanline(data: &[u8], line: u32, pitch: usize) -> (Vec<u8>, bool) {
    let Some(start) = usize::try_from(line)
        .ok()
        .and_then(|l| l.checked_mul(pitch))
    else {
        return (vec![0; pitch], true);
    };
    let available = data.get(start..).unwrap_or(&[]);
    if available.len() >= pitch {
        return (available.get(..pitch).unwrap_or(&[]).to_vec(), false);
    }
    let mut out = vec![0u8; pitch];
    if let Some(dest) = out.get_mut(..available.len()) {
        dest.copy_from_slice(available);
    }
    (out, true)
}

/// Invert a one-bit scanline in place, which is what a default-decode stencil
/// mask does.
pub fn invert_line(line: &mut [u8]) {
    for b in line.iter_mut() {
        *b = !*b;
    }
}

/// Pack `components` samples of `bpc` bits into one palette index.
///
/// Component `j` occupies bits `[j*bpc, (j+1)*bpc)`, so the *first* component
/// is in the low bits. The palette builder enumerates indices the same way.
#[must_use]
pub fn palette_index(line: &[u8], pixel: usize, components: u32, bpc: u32) -> u32 {
    let mut index = 0u32;
    for j in 0..components {
        let bit_pos = (pixel * components as usize + j as usize) * bpc as usize;
        let v = get_bits(line, bit_pos, bpc);
        index |= v << (j * bpc);
    }
    index
}

/// Scale a raw sample of `bpc` bits onto a byte with integer arithmetic.
///
/// `v * 255 / max`, matching the C++ exactly — not a float multiply, whose
/// rounding differs at several values.
#[must_use]
pub fn scale_to_byte(v: u32, max: u32) -> u8 {
    if max == 0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "v is clamped to max, so the quotient is at most 255"
    )]
    let out = (v.min(max) * 255 / max) as u8;
    out
}

/// Write three bytes at pixel `index`.
fn write(dest: &mut [u8], index: usize, bytes: [u8; 3]) {
    if let Some(px) = dest.get_mut(index * 3..index * 3 + 3) {
        px.copy_from_slice(&bytes);
    }
}

/// Convert a default-decode RGB-family scanline straight to **B, G, R**.
///
/// Returns whether it handled the line: a component count other than three
/// leaves the destination untouched, which is the C++'s "handled but wrote
/// nothing" case.
pub fn rgb_line_to_bgr(
    dest: &mut [u8],
    line: &[u8],
    pixels: usize,
    bpc: u32,
    components: u32,
) -> bool {
    if components != 3 {
        // Handled, having written nothing — the line buffer keeps whatever it
        // held.
        return true;
    }
    match bpc {
        8 => {
            for i in 0..pixels {
                let Some(&[r, g, b]) = line
                    .get(i * 3..i * 3 + 3)
                    .and_then(|s| <&[u8; 3]>::try_from(s).ok())
                else {
                    continue;
                };
                write(dest, i, [b, g, r]);
            }
            true
        }
        16 => {
            // Only the high byte of each big-endian sample survives; the low
            // byte is discarded with no rounding.
            for i in 0..pixels {
                let Some(&[r, _, g, _, b, _]) = line
                    .get(i * 6..i * 6 + 6)
                    .and_then(|s| <&[u8; 6]>::try_from(s).ok())
                else {
                    continue;
                };
                write(dest, i, [b, g, r]);
            }
            true
        }
        1 | 2 | 4 => {
            let max = (1u32 << bpc) - 1;
            for i in 0..pixels {
                let at = |c: usize| {
                    let bit_pos = (i * 3 + c) * bpc as usize;
                    scale_to_byte(get_bits(line, bit_pos, bpc), max)
                };
                write(dest, i, [at(2), at(1), at(0)]);
            }
            true
        }
        _ => false,
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

    use super::{get_bits, invert_line, palette_index, rgb_line_to_bgr, scale_to_byte, scanline};

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
        let (line, padded) = scanline(&data, 0, 2);
        assert_eq!(line, vec![1, 2]);
        assert!(!padded);
        // The second row has only one byte available.
        let (line, padded) = scanline(&data, 1, 2);
        assert_eq!(line, vec![3, 0]);
        assert!(padded);
        // Past the end entirely: all zeros.
        let (line, padded) = scanline(&data, 9, 2);
        assert_eq!(line, vec![0, 0]);
        assert!(padded);
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

    #[test]
    fn sample_scaling_is_integer_arithmetic() {
        // Four bits: 8/15 of 255 truncates to 136, not 136.0 rounded.
        assert_eq!(scale_to_byte(8, 15), 136);
        assert_eq!(scale_to_byte(15, 15), 255);
        assert_eq!(scale_to_byte(0, 15), 0);
        assert_eq!(scale_to_byte(1, 1), 255);
        assert_eq!(scale_to_byte(99, 0), 0);
    }

    #[test]
    fn four_bit_rgb_scales_each_nibble() {
        // The oracle's 2x1 vector: `[0x08, 0xff, 0x08]` gives
        // `[0xff, 0x88, 0x00, 0x88, 0x00, 0xff]` in BGR.
        let line = [0x08u8, 0xff, 0x08];
        let mut dest = [0u8; 6];
        assert!(rgb_line_to_bgr(&mut dest, &line, 2, 4, 3));
        assert_eq!(dest, [0xff, 0x88, 0x00, 0x88, 0x00, 0xff]);
    }

    #[test]
    fn eight_bit_rgb_is_a_channel_swap() {
        // `[0x11..0x66]` gives `[0x33,0x22,0x11, 0x66,0x55,0x44]`.
        let line = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
        let mut dest = [0u8; 6];
        assert!(rgb_line_to_bgr(&mut dest, &line, 2, 8, 3));
        assert_eq!(dest, [0x33, 0x22, 0x11, 0x66, 0x55, 0x44]);
    }

    #[test]
    fn sixteen_bit_rgb_keeps_only_the_high_byte() {
        // Twelve bytes give `[0x55,0x33,0x11, 0xbb,0x99,0x77]`.
        let line = [
            0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
        ];
        let mut dest = [0u8; 6];
        assert!(rgb_line_to_bgr(&mut dest, &line, 2, 16, 3));
        assert_eq!(dest, [0x55, 0x33, 0x11, 0xbb, 0x99, 0x77]);
    }

    #[test]
    fn a_wrong_component_count_writes_nothing_but_reports_handled() {
        let line = [1u8, 2, 3, 4];
        let mut dest = [9u8; 6];
        assert!(rgb_line_to_bgr(&mut dest, &line, 2, 8, 4));
        assert_eq!(dest, [9; 6], "the buffer must be left as it was");
    }
}
