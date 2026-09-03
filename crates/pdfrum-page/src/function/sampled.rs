//! Type 0: sampled functions (ISO 32000-1 §7.10.2).
//!
//! `[oracle-bug]` **Three defects live in the oracle's forty-line sampled
//! evaluator, and none of them is reproduced here.** All three are
//! pixel-visible through shadings and `Separation` colour. The `//` notes on
//! each site below carry the oracle's file and line.
//!
//! - **A2 — a negative encoded input lands on the *top* cell.** `:128` is
//!   `std::clamp(static_cast<uint32_t>(encoded_input[i]), 0U, sizes - 1)`: the
//!   cast to `uint32_t` happens *before* the clamp, so `-0.5` becomes about
//!   four billion and clamps down to `size - 1` — the opposite corner from the
//!   one it should reach. §7.10.2 states `e'ᵢ = min(max(eᵢ, 0), Sizeᵢ − 1)` on
//!   the **real** value, and `function.js:231` is `MathClamp(e, 0, size_i - 1)`,
//!   landing on cell 0. We clamp the real value.
//! - **A3 — the interpolation is a sum of per-axis gradients against one base
//!   sample**, not a multilinear blend. `:163` seeds `float encoded = sample;`
//!   from a single base corner and `:184` adds
//!   `(eⱼ − ⌊eⱼ⌋) · (Sⱼ − S₀)` per axis, so it reads `m + 1` samples and never
//!   the `2^m` corners: every cross term is dropped and the result is the
//!   tangent plane at the base corner. It is exact for one input and wrong for
//!   every function above one. §7.10.2 specifies multilinear interpolation and
//!   `function.js:205-260` builds the real hypercube with **product** weights
//!   in `cubeN`. We interpolate over all `2^m` corners.
//! - **A4 — a degenerate axis (`/Size` 1) multiplies and overwrites.**
//!   `:165-168` fires when `sizes == 1` and executes
//!   `encoded = encoded_input[j] * sample` — an *assignment*, discarding every
//!   axis already folded in, and a multiplication where §7.10.2 makes the
//!   weight on a one-element axis exactly 1, i.e. the identity.
//!   `function.js:231-236` clamps `e` to 0 and takes `n0 = 0, n1 = 1`: no
//!   scaling and no clobber. A one-element axis contributes nothing here.
//!
//! `/Order` is never read: only linear sampling exists, and a cubic-spline
//! request is silently downgraded — that one is **not** a bug, since
//! `function.js:180-185` reads `/Order`, logs, and ignores it too (audit A5).

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
/// rather than failing. Several sampled-function quirks depend on that: a
/// truncated `/Length` reads as zero samples rather than an error.
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

    /// Evaluate. See the module docs for the three `[oracle-bug]` items.
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

        // Per axis: the lower corner, and the fraction toward the upper one.
        // `[oracle-bug]` A2 — the clamp is on the **real** value, per §7.10.2's
        // `e'ᵢ = min(max(eᵢ, 0), Sizeᵢ − 1)`, so a negative input reaches cell
        // 0 rather than wrapping to the top cell.
        let mut lower = vec![0u64; inputs];
        let mut frac = vec![0f32; inputs];
        for i in 0..inputs {
            let size = self.sizes.get(i).copied().unwrap_or(1);
            let top = f32::from(u16::try_from(size.saturating_sub(1)).unwrap_or(u16::MAX));
            let e = interpolate(
                input.get(i).copied().unwrap_or(0.0),
                self.domain.get(i * 2).copied().unwrap_or(0.0),
                self.domain.get(i * 2 + 1).copied().unwrap_or(0.0),
                self.encode.get(i * 2).copied().unwrap_or(0.0),
                self.encode.get(i * 2 + 1).copied().unwrap_or(0.0),
            );
            let clamped = if e.is_nan() { 0.0 } else { e.clamp(0.0, top) };
            let floor = clamped.floor();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the clamp bounds the value to 0.0..=size-1"
            )]
            let low = floor as u64;
            // `[oracle-bug]` A4 — a one-element axis has weight 1 and no upper
            // neighbour, so its fraction is zero and it contributes nothing.
            let f = if size <= 1 { 0.0 } else { clamped - floor };
            if let Some(slot) = lower.get_mut(i) {
                *slot = low.min(u64::from(size.saturating_sub(1)));
            }
            if let Some(slot) = frac.get_mut(i) {
                *slot = f;
            }
        }

        let bps = u64::from(self.bits_per_sample);
        let outputs = u64::try_from(self.outputs).unwrap_or(0);

        // `[oracle-bug]` A3 — a true multilinear blend over the `2^m` corners
        // of the cell, with the weights multiplied rather than summed. Only
        // the axes with a non-zero fraction have two corners, so the loop
        // enumerates the corners of the *effective* hypercube: a function of
        // one input, or one landing exactly on a grid point, costs the same
        // single read the tangent-plane form did.
        let varying: Vec<usize> = (0..inputs)
            .filter(|j| frac.get(*j).copied().unwrap_or(0.0) != 0.0)
            .collect();
        // `1 << 20` corners is already far beyond any real function; a wider
        // hypercube is refused rather than walked.
        if varying.len() > 20 {
            return false;
        }
        let corners = 1u32 << varying.len();

        for i in 0..self.outputs {
            let mut value = 0f32;
            for corner in 0..corners {
                let mut weight = 1f32;
                let mut pos = 0u64;
                for j in 0..inputs {
                    let size = self.sizes.get(j).copied().unwrap_or(1);
                    let low = lower.get(j).copied().unwrap_or(0);
                    let f = frac.get(j).copied().unwrap_or(0.0);
                    // Which side of this axis this corner sits on.
                    let upper = varying
                        .iter()
                        .position(|v| *v == j)
                        .is_some_and(|bit| corner >> bit & 1 == 1);
                    let (step, w) = if upper { (1u64, f) } else { (0u64, 1.0 - f) };
                    weight *= w;
                    let at = (low + step).min(u64::from(size.saturating_sub(1)));
                    pos = pos
                        .saturating_add(at.saturating_mul(blocksize.get(j).copied().unwrap_or(1)));
                }
                if weight == 0.0 {
                    continue;
                }
                let Some(bits) = pos
                    .checked_mul(outputs)
                    .and_then(|p| p.checked_add(u64::try_from(i).unwrap_or(0)))
                    .and_then(|p| p.checked_mul(bps))
                else {
                    return false;
                };
                let mut reader = BitReader::new(&self.samples);
                reader.seek_bits(bits);
                value = weight.mul_add(sample_as_f32(reader.read(self.bits_per_sample)), value);
            }
            if let Some(slot) = out.get_mut(i) {
                *slot = interpolate(
                    value,
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

    /// Audit item **A3**, the largest behavioural change of the three, and
    /// the one no test pinned.
    ///
    /// The oracle seeds one base corner and adds one per-axis gradient term
    /// each, reading `m + 1` of the `2^m` corners: every **cross term** is
    /// dropped, and the result is the tangent plane at the base corner. It is
    /// exact for one input and wrong for every function above one. §7.10.2
    /// specifies multilinear interpolation, which is what we do.
    ///
    /// A 2x2 grid whose corners are `0, 0 / 0, 1` separates the two forms at
    /// a stroke. At the centre `(0.5, 0.5)`:
    ///
    /// * multilinear reads all four corners with product weights `1/4` each:
    ///   `(0 + 0 + 0 + 1) / 4 = 0.25`;
    /// * the tangent plane at corner `(0,0)` sums the two per-axis gradients
    ///   `f(1,0) - f(0,0) = 0` and `f(0,1) - f(0,0) = 0`, both zero here, so
    ///   it answers `0.0` — the whole surface reads flat.
    #[test]
    fn a_two_input_function_blends_every_corner_not_a_tangent_plane() {
        let sampled = Sampled {
            domain: Box::from(&[0.0f32, 1.0, 0.0, 1.0][..]),
            range: Box::from(&[0.0f32, 1.0][..]),
            outputs: 1,
            sizes: Box::from(&[2u32, 2][..]),
            bits_per_sample: 8,
            encode: Box::from(&[0.0f32, 1.0, 0.0, 1.0][..]),
            decode: Box::from(&[0.0f32, 1.0][..]),
            // Row-major over the first axis fastest: (0,0) (1,0) (0,1) (1,1).
            samples: Box::from(&[0u8, 0, 0, 255][..]),
            sample_max: 255,
        };
        let f = Function::Sampled(sampled);
        let mut out = [0.0f32];

        // The corners themselves agree under either form.
        f.eval(&[0.0, 0.0], &mut out).expect("evaluates");
        assert!(out[0].abs() < 1e-6, "got {}", out[0]);
        f.eval(&[1.0, 1.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6, "got {}", out[0]);

        // The centre is where they part: 0.25, not the tangent plane's 0.0.
        f.eval(&[0.5, 0.5], &mut out).expect("evaluates");
        assert!((out[0] - 0.25).abs() < 1e-6, "got {}", out[0]);

        // And the far corner of the cross term, which the tangent plane also
        // flattens to zero.
        f.eval(&[1.0, 0.5], &mut out).expect("evaluates");
        assert!((out[0] - 0.5).abs() < 1e-6, "got {}", out[0]);
    }

    /// Audit item **A2**. This asserted `1.0`, the *top* cell: the oracle
    /// clamps the truncated integer index rather than the real value, and a
    /// negative float cast to an unsigned index wraps to the top of the
    /// range. §7.10.2's `e'ᵢ = min(max(eᵢ, 0), Sizeᵢ − 1)` clamps the **real**
    /// value, so an `/Encode` entirely below zero lands on the bottom cell.
    #[test]
    fn a_negative_encoded_input_clamps_to_the_bottom_cell() {
        // An `/Encode` that maps the whole domain below zero.
        let mut sampled = ramp();
        sampled.encode = Box::from(&[-5.0f32, -1.0][..]);
        let f = Function::Sampled(sampled);
        let mut out = [0.0f32];
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!(out[0].abs() < 1e-6, "got {}", out[0]);
    }

    /// Audit item **A4**. A `/Size 1` axis has one sample and nothing to
    /// interpolate towards, so §7.10.2 gives it weight exactly 1 and the sole
    /// sample is the answer everywhere on that axis. The oracle instead
    /// assigns `encoded = encoded_input[j] * sample`, multiplying by the
    /// encoded input **and discarding every axis already folded in**, so the
    /// function reads as a ramp at the bottom of the domain.
    #[test]
    fn a_single_sample_axis_weighs_one_rather_than_multiplying() {
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
        // The sole sample is 255, decoding to 1.0 — at the top of the domain
        // and, since the axis has weight 1, at the bottom too.
        f.eval(&[1.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6, "got {}", out[0]);
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6, "got {}", out[0]);
    }
}
