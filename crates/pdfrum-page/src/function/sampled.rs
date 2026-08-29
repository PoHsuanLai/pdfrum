//! Type 0: sampled functions (ISO 32000-1 §7.10.2).
//!
//! Two details of the interpolation are quirks rather than mathematics, and
//! both are pixel-visible through shadings and `Separation` colour, so both
//! are ported exactly:
//!
//! - **A negative encoded input lands on the *top* cell, not the bottom
//!   one.** The C++ casts to `uint32_t` before clamping, so `-0.5` wraps to
//!   about four billion and clamps down to `size - 1`. Rust's `as u32`
//!   saturates negatives to zero, which would be the opposite corner, so the
//!   wrap is written out explicitly (design brief D12).
//! - **The interpolation is a sum of per-axis gradients against one base
//!   sample**, not a tensor-product multilinear blend. It is exact for one
//!   input and deliberately approximate above that. A degenerate axis
//!   (`Size [… 1 …]`) *multiplies* instead of interpolating and **overwrites**
//!   the accumulator, discarding earlier axes.
//!
//! `/Order` is never read: only linear sampling exists, and a cubic-spline
//! request is silently downgraded.

use super::{Common, interpolate};
use crate::names;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_filters::decode_chain;
use pdfrum_object::{Resolve, Stream};

/// Sample widths PDFium accepts, exactly.
const VALID_BITS_PER_SAMPLE: [u32; 8] = [1, 2, 4, 8, 12, 16, 24, 32];

/// A type 0 function.
#[derive(Debug, Clone, PartialEq)]
pub struct Sampled {
    /// `/Domain`.
    pub domain: Box<[f32]>,
    /// `/Range`, which is required for this type.
    pub range: Box<[f32]>,
    /// How many outputs, from `/Range`.
    pub outputs: usize,
    /// `/Size`: samples along each input axis, every entry at least one.
    pub sizes: Box<[u32]>,
    /// `/BitsPerSample`.
    pub bits_per_sample: u32,
    /// `/Encode`, two per input.
    pub encode: Box<[f32]>,
    /// `/Decode`, two per output, defaulting to `/Range`.
    pub decode: Box<[f32]>,
    /// The packed samples.
    pub samples: Box<[u8]>,
    /// `2^bits - 1`, the largest raw sample value.
    pub sample_max: u32,
}

/// A big-endian, MSB-first bit reader that returns **zero past the end**
/// rather than failing — matching `CFX_BitStream`, whose out-of-range
/// behaviour several quirks depend on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: u64,
}

impl<'a> BitReader<'a> {
    /// A reader at the start of `data`.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// Move to an absolute bit offset. Unchecked, like `SkipBits`.
    pub(crate) fn seek_bits(&mut self, bits: u64) {
        self.bit_pos = bits;
    }

    /// Round up to the next byte boundary.
    pub(crate) fn byte_align(&mut self) {
        self.bit_pos = self.bit_pos.div_ceil(8) * 8;
    }

    /// Bits still available.
    pub(crate) fn remaining(&self) -> u64 {
        let total = (self.data.len() as u64).saturating_mul(8);
        total.saturating_sub(self.bit_pos)
    }

    /// Read `n` bits (at most 32), MSB first. Zero past the end.
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        let mut out = 0u32;
        for _ in 0..n.min(32) {
            let byte = self
                .data
                .get(usize::try_from(self.bit_pos / 8).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(0);
            let shift = 7u32 - u32::try_from(self.bit_pos % 8).unwrap_or(0);
            out = (out << 1) | u32::from((byte >> shift) & 1);
            self.bit_pos = self.bit_pos.saturating_add(1);
        }
        out
    }
}

impl Sampled {
    /// Load from a stream. Types 0 and 4 are the two that **must** be
    /// streams; a dictionary alone will not do.
    pub(super) fn load<R: Resolve>(
        stream: &Stream,
        common: &Common,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        // `/Range` is required for this type.
        let outputs = common.outputs();
        if outputs == 0 {
            return None;
        }
        let inputs = common.inputs();

        let bits_per_sample = u32::try_from(stream.dict.int(names::BITS_PER_SAMPLE, r)?).ok()?;
        if !VALID_BITS_PER_SAMPLE.contains(&bits_per_sample) {
            return None;
        }

        let size_array = stream.dict.array(names::SIZE, r)?;
        if size_array.is_empty() {
            return None;
        }
        let encode_array = stream.dict.array(names::ENCODE, r);
        let mut sizes = Vec::with_capacity(inputs);
        let mut encode = Vec::with_capacity(inputs * 2);
        let mut total_samples: u64 = 1;
        for i in 0..inputs {
            let size = size_array.int_at(i).unwrap_or(0);
            // A zero or negative axis is fatal.
            if size <= 0 {
                return None;
            }
            let size = u32::try_from(size).ok()?;
            sizes.push(size);
            total_samples = total_samples.checked_mul(u64::from(size))?;
            if let Some(a) = &encode_array {
                // Read positionally with no length check, so a short array
                // silently yields zeros.
                encode.push(a.number_at_or_zero(i * 2));
                encode.push(a.number_at_or_zero(i * 2 + 1));
            } else {
                // The default avoids a zero-width encode range on a
                // one-sample axis.
                encode.push(0.0);
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "sample counts far below f32's exact integer range"
                )]
                let top = if size == 1 { 1.0 } else { (size - 1) as f32 };
                encode.push(top);
            }
        }

        // The whole-table cap: the product must fit a `u32` of *bits*.
        let total_bits = total_samples
            .checked_mul(u64::from(bits_per_sample))?
            .checked_mul(u64::try_from(outputs).ok()?)?;
        if total_bits == 0 || total_bits > u64::from(u32::MAX) {
            return None;
        }
        let total_bytes = usize::try_from(total_bits.div_ceil(8)).ok()?;

        let samples: Box<[u8]> = decode_chain(stream, total_bytes, r, limits, diags)
            .data
            .into();
        if total_bytes > samples.len() {
            return None;
        }

        // `/Decode` defaults to `/Range`, which type 0 always has.
        let decode_array = stream.dict.array(names::DECODE, r);
        let decode: Box<[f32]> = (0..outputs * 2)
            .map(|i| match &decode_array {
                Some(a) => a.number_at_or_zero(i),
                None => common.range.get(i).copied().unwrap_or(0.0),
            })
            .collect();

        Some(Self {
            domain: common.domain.clone(),
            range: common.range.clone(),
            outputs,
            sizes: sizes.into(),
            bits_per_sample,
            encode: encode.into(),
            decode,
            samples,
            sample_max: if bits_per_sample >= 32 {
                u32::MAX
            } else {
                (1u32 << bits_per_sample) - 1
            },
        })
    }

    /// Evaluate. See the module docs for the two quirks.
    pub(super) fn eval(&self, input: &[f32], out: &mut [f32]) -> bool {
        let inputs = self.sizes.len();
        // `blocksize[0] = 1` and the rest cumulative, so the **first** input
        // varies fastest.
        let mut blocksize = vec![0u64; inputs];
        let mut acc = 1u64;
        for (i, slot) in blocksize.iter_mut().enumerate() {
            *slot = acc;
            acc = acc.saturating_mul(u64::from(self.sizes.get(i).copied().unwrap_or(1)));
        }

        let mut encoded_input = vec![0f32; inputs];
        let mut index = vec![0u32; inputs];
        let mut pos = 0u64;
        for i in 0..inputs {
            let size = self.sizes.get(i).copied().unwrap_or(1);
            let e = interpolate(
                input.get(i).copied().unwrap_or(0.0),
                self.domain.get(i * 2).copied().unwrap_or(0.0),
                self.domain.get(i * 2 + 1).copied().unwrap_or(0.0),
                self.encode.get(i * 2).copied().unwrap_or(0.0),
                self.encode.get(i * 2 + 1).copied().unwrap_or(0.0),
            );
            if let Some(slot) = encoded_input.get_mut(i) {
                *slot = e;
            }
            // The negative-wrap quirk, written out.
            let idx = if e.is_nan() {
                0
            } else if e < 0.0 {
                size.saturating_sub(1)
            } else {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the branch above excludes negatives and NaN"
                )]
                let raw = e as u32;
                raw.min(size.saturating_sub(1))
            };
            if let Some(slot) = index.get_mut(i) {
                *slot = idx;
            }
            pos = pos.saturating_add(
                u64::from(idx).saturating_mul(blocksize.get(i).copied().unwrap_or(1)),
            );
        }

        let bps = u64::from(self.bits_per_sample);
        let outputs = u64::try_from(self.outputs).unwrap_or(0);
        let Some(base_bits) = pos.checked_mul(outputs).and_then(|v| v.checked_mul(bps)) else {
            return false;
        };

        let mut reader = BitReader::new(&self.samples);
        reader.seek_bits(base_bits);
        for i in 0..self.outputs {
            let sample = reader.read(self.bits_per_sample);
            let mut encoded = sample_as_f32(sample);
            for j in 0..inputs {
                let size = self.sizes.get(j).copied().unwrap_or(1);
                let idx = index.get(j).copied().unwrap_or(0);
                if idx == size.saturating_sub(1) {
                    // On the top edge nothing normally happens — except on a
                    // degenerate one-sample axis, where the accumulator is
                    // *multiplied* and *overwritten*.
                    if idx == 0 {
                        encoded =
                            encoded_input.get(j).copied().unwrap_or(0.0) * sample_as_f32(sample);
                    }
                    continue;
                }
                // The neighbour along this axis, read with a fresh stream.
                let Some(neighbour_bits) = blocksize
                    .get(j)
                    .copied()
                    .unwrap_or(1)
                    .checked_add(pos)
                    .and_then(|p| p.checked_mul(outputs))
                    .and_then(|p| p.checked_add(u64::try_from(i).unwrap_or(0)))
                    .and_then(|p| p.checked_mul(bps))
                else {
                    continue;
                };
                let mut neighbour = BitReader::new(&self.samples);
                neighbour.seek_bits(neighbour_bits);
                let sample2 = neighbour.read(self.bits_per_sample);
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "matching the C++'s float widening of a grid index"
                )]
                let frac = encoded_input.get(j).copied().unwrap_or(0.0) - idx as f32;
                encoded += frac * (sample_as_f32(sample2) - sample_as_f32(sample));
            }
            if let Some(slot) = out.get_mut(i) {
                *slot = interpolate(
                    encoded,
                    0.0,
                    sample_as_f32(self.sample_max),
                    self.decode.get(i * 2).copied().unwrap_or(0.0),
                    self.decode.get(i * 2 + 1).copied().unwrap_or(0.0),
                );
            }
        }
        true
    }
}

/// A raw sample as a float, with the widening the C++ does.
#[expect(
    clippy::cast_precision_loss,
    reason = "matching the C++'s uint32-to-float widening, including its loss"
)]
fn sample_as_f32(v: u32) -> f32 {
    v as f32
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

    use super::{BitReader, Sampled};
    use crate::function::Function;

    /// A one-input, one-output ramp over four eight-bit samples.
    fn ramp() -> Sampled {
        Sampled {
            domain: Box::from(&[0.0f32, 1.0][..]),
            range: Box::from(&[0.0f32, 1.0][..]),
            outputs: 1,
            sizes: Box::from(&[4u32][..]),
            bits_per_sample: 8,
            encode: Box::from(&[0.0f32, 3.0][..]),
            decode: Box::from(&[0.0f32, 1.0][..]),
            samples: Box::from(&[0u8, 85, 170, 255][..]),
            sample_max: 255,
        }
    }

    #[test]
    fn bit_reader_is_msb_first_and_reads_zero_past_the_end() {
        let mut r = BitReader::new(&[0b1010_0000, 0xFF]);
        assert_eq!(r.read(1), 1);
        assert_eq!(r.read(1), 0);
        assert_eq!(r.read(2), 0b10);
        let mut r = BitReader::new(&[0x12, 0x34]);
        assert_eq!(r.read(16), 0x1234);
        // Past the end: zeros, not a failure.
        assert_eq!(r.read(8), 0);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn byte_align_rounds_up() {
        let mut r = BitReader::new(&[0xFF; 4]);
        r.read(3);
        r.byte_align();
        assert_eq!(r.remaining(), 24);
        // Already aligned: no movement.
        r.byte_align();
        assert_eq!(r.remaining(), 24);
    }

    #[test]
    fn a_one_input_ramp_interpolates_exactly() {
        let f = Function::Sampled(ramp());
        let mut out = [0.0f32];
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!(out[0].abs() < 1e-6);
        f.eval(&[1.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6);
        // Halfway between samples 1 and 2.
        f.eval(&[1.5 / 3.0], &mut out).expect("evaluates");
        assert!((out[0] - 0.5).abs() < 0.01, "got {}", out[0]);
    }

    #[test]
    fn a_negative_encoded_input_lands_on_the_top_cell() {
        // An `/Encode` that maps the whole domain below zero.
        let mut sampled = ramp();
        sampled.encode = Box::from(&[-5.0f32, -1.0][..]);
        let f = Function::Sampled(sampled);
        let mut out = [0.0f32];
        f.eval(&[0.0], &mut out).expect("evaluates");
        // The top cell is 255/255 = 1.0, not the bottom cell's 0.0.
        assert!((out[0] - 1.0).abs() < 1e-6, "got {}", out[0]);
    }

    #[test]
    fn a_single_sample_axis_multiplies_rather_than_interpolating() {
        let sampled = Sampled {
            domain: Box::from(&[0.0f32, 1.0][..]),
            range: Box::from(&[0.0f32, 1.0][..]),
            outputs: 1,
            sizes: Box::from(&[1u32][..]),
            bits_per_sample: 8,
            // The default encode for a one-sample axis is `[0, 1]`.
            encode: Box::from(&[0.0f32, 1.0][..]),
            decode: Box::from(&[0.0f32, 1.0][..]),
            samples: Box::from(&[255u8][..]),
            sample_max: 255,
        };
        let f = Function::Sampled(sampled);
        let mut out = [0.0f32];
        // At the domain's top the encoded input is 1.0, so 1.0 * 255 = 255,
        // decoding to 1.0.
        f.eval(&[1.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6, "got {}", out[0]);
        // At the bottom the encoded input is 0.0, so the product is zero —
        // the multiply, not an interpolation towards the same sample.
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!(out[0].abs() < 1e-6, "got {}", out[0]);
    }
}
