//! Area-average box downscaling of a decoded image before it reaches a
//! rasterizer.
//!
//! Both backends resample an image with a **two-tap** filter: one tap per axis
//! for nearest, two for bilinear. That is the right kernel for an *enlargement*
//! — where the destination samples land between source pixels and two taps
//! bracket each one — and it is the wrong kernel for a reduction, where a
//! destination pixel covers many source pixels and two taps see at most two of
//! them. A 455x455 image drawn 2.86x smaller loses roughly six of every seven
//! source pixels, which reads as aliasing and, on any page with a photograph,
//! as a texture the oracle does not have.
//!
//! PDFium spells the distinction inside its weight table: bilinear is a
//! *branch* taken only when the axis is being enlarged (`|scale| < 1`), and a
//! reduction falls through to a box filter that integrates **every** source
//! pixel the destination pixel's footprint overlaps, weighted by the overlap
//! area. This module is that box filter, and only that: it is a pre-pass that
//! reduces the source to (approximately) its destination size, after which the
//! backend's two-tap kernel is operating near 1:1 and the choice between them
//! stops mattering.
//!
//! # Why a pre-pass rather than a backend kernel
//!
//! The alternative — teaching each backend to box-filter — would put the
//! answer in two places and make it a rasterizer's decision, which
//! [`crate::image`] already argues it must not be: both backends must resample
//! identically or Tier C's interior rule fails. Reducing here leaves one
//! implementation, shared by construction.
//!
//! # What is deliberately not reproduced
//!
//! PDFium's engine is a *complete* resampler: it stretches to the exact
//! destination rectangle, in one axis at a time, writing straight into the
//! device. Ours reduces to whole-pixel dimensions and hands the remainder —
//! the fractional scale, the rotation, the shear, the subpixel placement — to
//! the backend, which is the only part of the pipeline that knows where the
//! image lands. So the two agree on the *low-pass* (the part a two-tap filter
//! cannot do at all) and differ by at most the last two-tap interpolation.
//!
//! Consequently this runs only for an **axis being reduced**, and never for an
//! enlargement, where PDFium's own branch is the bilinear one the backends
//! already implement.

use crate::pixmap::Pixmap;

/// A box-filter tap weight, in fixed point.
///
/// The whole weight path is integers: a weight is an exact rational area
/// scaled by `2 ^ SHIFT` and rounded, and nothing on the way to it is an
/// `f64`. That is not only speed. The areas a box filter integrates are exact
/// rationals in the two axis lengths, so an `f64` intermediate can only lose
/// them, and the scheme below rounds a *running sum* — where one `f64` ulp of
/// drift moves a whole unit of weight from one tap to its neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Weight(u32);

impl Weight {
    /// The bit position of the fixed point (`kFixedPointBits` upstream).
    const SHIFT: u32 = 16;

    /// The value the weights of one destination pixel sum to.
    ///
    /// An accumulation over the taps is `sample * ONE` at most, so shifting it
    /// right by [`Weight::SHIFT`] is the normalisation — no division, and no
    /// per-pixel normalising factor to get wrong.
    const ONE: Self = Self(1 << Self::SHIFT);

    /// The weight as the `u32` an accumulation multiplies by.
    const fn get(self) -> u32 {
        self.0
    }
}

/// One destination pixel's taps: the first source index it reads, and one
/// weight per consecutive source pixel from there.
///
/// The weights sum to [`Weight::ONE`] whenever the range is non-empty, so an
/// accumulation over them needs no normalisation — only a shift.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Taps {
    first: u32,
    weights: Box<[Weight]>,
}

impl Taps {
    /// The first source index, as the `usize` an index computation wants.
    const fn start(&self) -> usize {
        self.first as usize
    }
}

/// The taps for every destination pixel along one axis being reduced.
///
/// Each destination pixel maps back to the half-open source interval
/// `[d * src / dest, (d + 1) * src / dest)`, and each source pixel in it
/// contributes the share of the *destination* pixel that it covers. Computing
/// the overlap in destination space rather than source space is what keeps the
/// weights summing to one without a division per pixel.
///
/// # Why a cumulative rounding rather than a carried residue
///
/// The area of source pixel `s` inside destination pixel `d`, in destination
/// units, is `A(s) - A(s - 1)` where `A` is the cumulative coverage
/// `clamp((s + 1) * dest / src, d, d + 1) - d`. Rounding each *difference* and
/// carrying the residue into the next — which is what the `f64` version did —
/// and rounding each *cumulative* value and differencing are the same sequence
/// of weights, algebraically: the carried scheme's running total after `k`
/// taps is exactly `round(A(k) * ONE)`, because the residue it carries is by
/// construction `A(k) - total / ONE`. So this computes `round(A(k) * ONE)`
/// directly, in exact integer arithmetic, and the sum telescopes to
/// [`Weight::ONE`] with no residue to chase.
///
/// `A(k) * ONE` is `(k + 1) * dest_len * ONE / src_len` clamped, so the whole
/// table is one `u64` multiply and one divide per tap, rounded half away from
/// zero by adding half the divisor before the division.
fn axis_taps(src_len: u32, dest_len: u32) -> Vec<Taps> {
    if src_len == 0 || dest_len == 0 {
        return Vec::new();
    }
    let (src, dest_n) = (u64::from(src_len), u64::from(dest_len));
    let one = u64::from(Weight::ONE.get());
    let mut out = Vec::with_capacity(dest_len as usize);
    for d in 0..dest_n {
        // `first` and `last` are the `floor`s the `f64` path reached through
        // `floor(d * src / dest)`; as an integer division they cannot land on
        // the wrong side of a boundary. `last` is inclusive because a
        // footprint ending exactly on a pixel edge still nominally taps the
        // pixel beyond it — at weight zero, which the area computation then
        // assigns, matching upstream's inclusive `end_i`.
        let first = d.saturating_mul(src) / dest_n;
        let last = (d.saturating_add(1).saturating_mul(src) / dest_n).min(src - 1);
        let first_u32 = u32::try_from(first.min(src - 1)).unwrap_or(0);
        if first > last {
            // The footprint fell outside the source — a clamped edge — and
            // the only defensible answer is the nearest source pixel entire.
            out.push(Taps {
                first: first_u32,
                weights: Box::new([Weight::ONE]),
            });
            continue;
        }
        let mut weights = Vec::with_capacity(usize::try_from(last - first + 1).unwrap_or(0));
        let mut remaining = Weight::ONE.get();
        let mut previous = 0_u64;
        for s in first..last {
            // The cumulative coverage of source pixels `first ..= s`, in fixed
            // point: `(s + 1) * dest / src`, clamped into this destination
            // pixel and taken relative to its start. Rounded half away from
            // zero by adding `src / 2` before the divide.
            let numerator = s
                .saturating_add(1)
                .saturating_mul(dest_n)
                .saturating_mul(one);
            let scaled = numerator.saturating_add(src / 2) / src;
            let low = d.saturating_mul(one);
            let cumulative = scaled.clamp(low, low.saturating_add(one)) - low;
            let step = u32::try_from(cumulative.saturating_sub(previous)).unwrap_or(0);
            previous = cumulative;
            let capped = step.min(remaining);
            remaining -= capped;
            weights.push(Weight(capped));
        }
        // Whatever the fractional areas did not spend belongs to the last tap;
        // the alternative — dropping it — biases every reduced image dark by
        // up to one part in `ONE` per tap, which accumulates across the axis.
        weights.push(Weight(remaining));
        out.push(Taps {
            first: first_u32,
            weights: weights.into_boxed_slice(),
        });
    }
    out
}

/// The destination size an axis reduces to, or `None` when it is not being
/// reduced.
///
/// Only a genuine reduction is box-filtered: at or above 1:1 the two-tap
/// kernel the backends already run *is* the upstream branch, and pre-scaling
/// would replace it with a worse one.
///
/// The size is rounded **up**, not to nearest: a fractional footprint covers
/// one more device pixel than its width names, and the outermost are partial.
/// Reducing to the nearest instead leaves the backend a sample short of them,
/// so an edge row arrives empty rather than faint.
fn reduced_len(src_len: u32, dest_len: f64) -> Option<u32> {
    if !dest_len.is_finite() || src_len == 0 {
        return None;
    }
    let target = dest_len.abs().ceil();
    // A destination smaller than one pixel still reduces — to one pixel, which
    // is the average of the whole axis and is what the box filter converges to.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded below by 1.0 and above by src_len, itself a u32"
    )]
    let target = target.clamp(1.0, f64::from(src_len)) as u32;
    (target < src_len).then_some(target)
}

/// Box-filter a **single-channel** plane down to `dest_width` x `dest_height`.
///
/// Byte for byte the same filter [`reduce_to`] runs — the same two tap tables,
/// the same fixed-point accumulation, the same `>> 16` — over one channel
/// instead of four. It exists for the soft-mask path, where the plane being
/// reduced is a coverage map and the other three channels of a `Pixmap` would
/// carry copies of it.
///
/// `src` is `src_width * src_height` bytes in row-major order; a buffer of any
/// other length reduces to an all-zero plane rather than reading out of range,
/// which is the same degradation [`reduce_to`]'s bounds checks produce.
///
/// # Why this is not `reduce_to` with a channel count
///
/// The four-channel loop's inner body is `acc[c] += weight * px[c]` over a
/// four-byte slice, which is the shape the optimizer unrolls. Making the
/// channel count dynamic would put a loop bound it cannot see into the hottest
/// loop in the module to save a function whose body is thirty lines. The two
/// are kept in step by
/// `the_gray_reduction_is_the_rgba_reduction_on_a_gray_image`, which asserts
/// the equality over a lattice of sizes rather than trusting the reader.
#[must_use]
pub fn reduce_gray_to(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dest_width: u32,
    dest_height: u32,
) -> Vec<u8> {
    let dest_len = (dest_width as usize).saturating_mul(dest_height as usize);
    let x_taps = axis_taps(src_width, dest_width);
    let y_taps = axis_taps(src_height, dest_height);
    let src_w = src_width as usize;
    let dest_w = dest_width as usize;
    if x_taps.is_empty()
        || y_taps.is_empty()
        || src.len() != src_w.saturating_mul(src_height as usize)
    {
        return vec![0; dest_len];
    }

    // Horizontal pass into an intermediate of full source height at
    // destination width, exactly as `reduce_to` does.
    let mut inter = vec![0_u8; dest_w.saturating_mul(src_height as usize)];
    for y in 0..src_height as usize {
        let Some(src_row) = src
            .get(y.saturating_mul(src_w)..)
            .and_then(|rest| rest.get(..src_w))
        else {
            continue;
        };
        let Some(inter_row) = inter
            .get_mut(y.saturating_mul(dest_w)..)
            .and_then(|rest| rest.get_mut(..dest_w))
        else {
            continue;
        };
        for (taps, out) in x_taps.iter().zip(inter_row.iter_mut()) {
            let mut acc = 0_u32;
            for (i, weight) in taps.weights.iter().enumerate() {
                let weight = weight.get();
                let Some(&sample) = taps.start().checked_add(i).and_then(|sx| src_row.get(sx))
                else {
                    continue;
                };
                acc += weight * u32::from(sample);
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the weights sum to Weight::ONE and the sample is a byte, \
                          so the accumulator is at most 255 << 16"
            )]
            let byte = (acc >> 16) as u8;
            *out = byte;
        }
    }

    // Vertical pass. The row slices are resolved once per destination row and
    // paired with their weights, for the reason `reduce_to` spells out: a row
    // the bounds check rejects must drop its weight with it.
    let mut dest = vec![0_u8; dest_len];
    for (y, taps) in y_taps.iter().enumerate() {
        let Some(dest_row) = dest
            .get_mut(y.saturating_mul(dest_w)..)
            .and_then(|rest| rest.get_mut(..dest_w))
        else {
            continue;
        };
        let rows: Vec<(u32, &[u8])> = taps
            .weights
            .iter()
            .enumerate()
            .filter_map(|(i, &weight)| {
                let weight = weight.get();
                let sy = taps.start().checked_add(i)?;
                let at = sy.checked_mul(dest_w)?;
                let row = inter.get(at..at.checked_add(dest_w)?)?;
                Some((weight, row))
            })
            .collect();
        for (x, out) in dest_row.iter_mut().enumerate() {
            let mut acc = 0_u32;
            for &(weight, row) in &rows {
                let Some(&sample) = row.get(x) else { continue };
                acc += weight * u32::from(sample);
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the weights sum to Weight::ONE and the sample is a byte, \
                          so the accumulator is at most 255 << 16"
            )]
            let byte = (acc >> 16) as u8;
            *out = byte;
        }
    }
    dest
}

/// A source row narrowed to destination width, in the accumulator's own
/// width.
///
/// Sixteen bits because a horizontal tap is `sample * weight >> SHIFT`, whose
/// range is a byte — but keeping the *unshifted* sum would need 24, and the
/// two-pass reduction is only equal to a one-pass accumulation of the product
/// weights if each pass rounds. So it rounds here, exactly as [`reduce_to`]'s
/// intermediate pixmap did, and this type is a byte's worth of value in a
/// `u16` slot for the vertical pass to multiply without widening again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
struct Rgba16([u16; 4]);

/// The vertical accumulator for one destination row.
///
/// Thirty-two bits because a destination pixel sums `taps` products of a byte
/// by a weight below [`Weight::ONE`], and a tall reduction has many taps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
struct Rgba32([u32; 4]);

/// One horizontal box pass, pulling rows from whatever yields them.
///
/// The third stage of the pipeline (`docs/design/image-rows.md`): it takes
/// source-width RGBA rows and gives destination-width ones. It owns exactly
/// one row of storage, which is the whole point — the intermediate this
/// replaces was `dest_width * src_height` pixels, materialized in full before
/// the vertical pass began.
struct Narrowed<'a> {
    src: pdfrum_page::Converted<'a>,
    taps: Vec<Taps>,
    buf: Vec<Rgba16>,
}

impl<'a> Narrowed<'a> {
    /// Narrow `src`'s rows to `dest_width` using the taps for `src_width`.
    fn new(src: pdfrum_page::Converted<'a>, src_width: u32, dest_width: u32) -> Self {
        Self {
            src,
            taps: axis_taps(src_width, dest_width),
            buf: vec![Rgba16::default(); dest_width as usize],
        }
    }

    /// The next narrowed row, or `None` when the source is exhausted.
    fn next_row(&mut self, finish: impl FnOnce(&mut [pdfrum_page::Rgba8])) -> Option<&[Rgba16]> {
        let row = pdfrum_page::Converted::next_row_with(&mut self.src, finish)?;
        let src = row.pixels();
        for (taps, out) in self.taps.iter().zip(self.buf.iter_mut()) {
            let mut acc = [0_u32; 4];
            for (i, weight) in taps.weights.iter().enumerate() {
                let weight = weight.get();
                let Some(px) = taps.start().checked_add(i).and_then(|sx| src.get(sx)) else {
                    continue;
                };
                for (slot, &channel) in acc.iter_mut().zip(px.0.iter()) {
                    *slot += weight * u32::from(channel);
                }
            }
            for (slot, a) in out.0.iter_mut().zip(acc) {
                // The weights sum to `Weight::ONE` and each channel is a byte,
                // so the shifted accumulator is at most 255 and the fallback
                // is unreachable — spelled as a `try_from` rather than a cast
                // because the tree forbids an unchecked narrowing here and a
                // saturating one costs nothing on a value that never saturates.
                *slot = u16::try_from(a >> Weight::SHIFT).unwrap_or(u16::MAX);
            }
        }
        Some(&self.buf)
    }
}

/// The vertical accumulation, closing a destination row when its bin closes.
///
/// The last stage. It holds one destination row of accumulators and one
/// destination row of output — never the source's height — because the
/// vertical taps of consecutive destination rows are consecutive runs of
/// source rows, so a source row can be added to its destination row and
/// dropped.
///
/// This is only true because the taps are a *box* filter: every destination
/// row's taps are a contiguous run and the runs advance monotonically. A
/// kernel with negative lobes would need the rows kept.
struct Shortened<'a> {
    src: Narrowed<'a>,
    taps: Vec<Taps>,
    acc: Vec<Rgba32>,
    /// The next source row `src` will yield.
    source_y: usize,
    /// The last source row pulled, kept because consecutive destination rows'
    /// tap runs **overlap**.
    ///
    /// A destination row's run is `[first, first + taps)`, and the next one
    /// starts at `floor((d + 1) * src / dest)`, which is at or before that
    /// end — the boundary source pixel straddles two destination pixels and
    /// is tapped by both. Being a pull pipeline the row cannot be asked for
    /// twice, so exactly one is held back. One is enough: the runs advance
    /// monotonically and each is at least one row long, so no row is ever
    /// wanted by three destination rows.
    held: Option<(usize, Vec<Rgba16>)>,
}

impl<'a> Shortened<'a> {
    /// Accumulate `src`'s rows into `dest_height` rows using the taps for
    /// `src_height`.
    fn new(src: Narrowed<'a>, dest_width: u32, src_height: u32, dest_height: u32) -> Self {
        Self {
            src,
            taps: axis_taps(src_height, dest_height),
            acc: vec![Rgba32::default(); dest_width as usize],
            source_y: 0,
            held: None,
        }
    }

    /// Write every destination row into `out`, a `dest_width` x `dest_height`
    /// pixmap.
    ///
    /// Driven as a loop over destinations rather than as another `Rows` stage
    /// because the pixmap is the end of the line: nothing pulls from here, and
    /// a stage that yields into a buffer its only caller immediately copies
    /// out of would be a copy for nothing.
    fn drain_into(mut self, out: &mut Pixmap, finish: &impl Fn(&mut [pdfrum_page::Rgba8], u32)) {
        let width = out.width() as usize;
        for dest_y in 0..self.taps.len() {
            let Some(taps) = self.taps.get(dest_y) else {
                break;
            };
            for slot in &mut self.acc {
                *slot = Rgba32::default();
            }
            let first = taps.start();
            let last = first.saturating_add(taps.weights.len());
            // One weighted source row into the accumulator.
            let add = |acc: &mut [Rgba32], y: usize, row: &[Rgba16]| {
                let Some(weight) = y
                    .checked_sub(first)
                    .and_then(|o| taps.weights.get(o))
                    .map(|w| w.get())
                else {
                    return;
                };
                for (slot, px) in acc.iter_mut().zip(row) {
                    for (a, &channel) in slot.0.iter_mut().zip(px.0.iter()) {
                        *a += weight * u32::from(channel);
                    }
                }
            };
            // The row held back from the previous destination row, if this one
            // still wants it. If it does not, it is dropped here rather than
            // carried further: the runs advance, so it can never be wanted
            // again.
            if let Some((y, row)) = self.held.take()
                && y >= first
                && y < last
            {
                add(&mut self.acc, y, &row);
            }
            while self.source_y < last {
                let y = self.source_y;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the source height is a u32 and `source_y` counts its rows"
                )]
                let row_y = y as u32;
                let Some(row) = self.src.next_row(|r| finish(r, row_y)) else {
                    break;
                };
                self.source_y += 1;
                add(&mut self.acc, y, row);
                // The last row of this run may also be the first of the next:
                // hold it rather than let the pull pipeline forget it.
                if self.source_y == last {
                    self.held = Some((y, row.to_vec()));
                }
            }
            let Some(dest_row) = out
                .data_mut()
                .get_mut(dest_y.saturating_mul(width).saturating_mul(4)..)
                .and_then(|rest| rest.get_mut(..width.saturating_mul(4)))
            else {
                continue;
            };
            for (slot, acc) in dest_row.chunks_exact_mut(4).zip(self.acc.iter()) {
                for (byte, &a) in slot.iter_mut().zip(acc.0.iter()) {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "the weights sum to Weight::ONE and each channel is a \
                                  byte, so every accumulator is at most 255 << SHIFT"
                    )]
                    let rounded = (a >> Weight::SHIFT) as u8;
                    *byte = rounded;
                }
            }
        }
    }
}

/// Convert and box-filter an image straight to `dest_width` x `dest_height`.
///
/// The fused path: [`crate::image::to_pixmap`]'s conversion and this module's
/// reduction, run as one pull pipeline, so the full-size RGBA pixmap and both
/// full-height intermediates never exist. What is held at once is four rows —
/// the source's, the converted one, the narrowed one and the destination's —
/// against the `src_w * src_h + dest_w * src_h` pixels the two-call path
/// materialized.
///
/// The arithmetic is unchanged, deliberately: the same two tap tables, the
/// same rounding after each pass, the same order (horizontal, then vertical).
/// `the_fused_reduction_is_the_two_call_one` asserts it over a lattice of
/// sizes rather than trusting this paragraph.
#[must_use]
pub fn convert_and_reduce(
    image: &pdfrum_page::ImageData,
    stencil_color: crate::color::Argb,
    transfer: Option<&crate::transfer::TransferFunc<'_>>,
    dest_width: u32,
    dest_height: u32,
) -> Pixmap {
    let mut out = Pixmap::new(dest_width, dest_height);
    if dest_width == 0 || dest_height == 0 || image.width == 0 || image.height == 0 {
        return out;
    }
    let finish = crate::image::RowFinish::new(image, stencil_color, transfer);
    let narrowed = Narrowed::new(crate::image::converted_rows(image), image.width, dest_width);
    let shortened = Shortened::new(narrowed, dest_width, image.height, dest_height);
    shortened.drain_into(&mut out, &|row, y| finish.apply(row, y));
    out
}

/// Box-filter `src` down to `dest_width` x `dest_height`, one axis at a time.
///
/// Horizontal first into an intermediate, then vertical — the same order and
/// the same two weight tables PDFium uses, and the reason a two-pass reduction
/// costs `O(w * h * (taps_x + taps_y))` rather than their product.
///
/// This is the *pixmap* entry point, for a caller that already has one:
/// [`prescale`] and the soft-mask path. The image path takes
/// [`convert_and_reduce`] instead, which never builds the source pixmap at
/// all.
#[must_use]
pub fn reduce_to(src: &Pixmap, dest_width: u32, dest_height: u32) -> Pixmap {
    let x_taps = axis_taps(src.width(), dest_width);
    let y_taps = axis_taps(src.height(), dest_height);
    if x_taps.is_empty() || y_taps.is_empty() {
        return Pixmap::new(dest_width, dest_height);
    }
    // The intermediate is full source height at destination width, held as
    // fixed-point-shifted bytes exactly like the source: one shift per axis
    // keeps the arithmetic identical to a single-pass accumulation of the
    // product weights, up to the two roundings the C++ also performs.
    // M12: both passes walk row slices rather than calling `pixel`/`set_pixel`
    // per texel. The arithmetic is unchanged — same taps, same fixed-point
    // accumulation, same `>> 16` — but `pixel()` returned an `Option<[u8; 4]>`
    // built from four separate bounds-checked `get`s and `set_pixel` recomputed
    // the same index to write it back, on every one of the source image's
    // pixels. On the reduction path that is the whole cost: this function is
    // `O(source pixels x taps)` and the taps are two or three, so the per-pixel
    // overhead was comparable to the multiply-accumulate it wrapped.
    //
    // The slices are taken with `get`, not indexed, so a malformed dimension
    // still cannot panic (`clippy::indexing_slicing` is on in this crate); the
    // difference is that the check now happens once per row instead of four
    // times per pixel, and the inner loop over a `&[u8]` of known length is
    // something the optimizer can unroll.
    let src_width = src.width() as usize;
    let mut inter = Pixmap::new(dest_width, src.height());
    let inter_width = dest_width as usize;
    for y in 0..src.height() as usize {
        let Some(src_row) = src
            .data()
            .get(y.saturating_mul(src_width).saturating_mul(4)..)
            .and_then(|rest| rest.get(..src_width.saturating_mul(4)))
        else {
            continue;
        };
        let Some(inter_row) = inter
            .data_mut()
            .get_mut(y.saturating_mul(inter_width).saturating_mul(4)..)
            .and_then(|rest| rest.get_mut(..inter_width.saturating_mul(4)))
        else {
            continue;
        };
        for (taps, out) in x_taps.iter().zip(inter_row.chunks_exact_mut(4)) {
            let mut acc = [0_u32; 4];
            for (i, weight) in taps.weights.iter().enumerate() {
                let weight = weight.get();
                let Some(px) = taps
                    .start()
                    .checked_add(i)
                    .and_then(|sx| sx.checked_mul(4))
                    .and_then(|at| src_row.get(at..at.checked_add(4)?))
                else {
                    continue;
                };
                for (slot, &channel) in acc.iter_mut().zip(px.iter()) {
                    *slot += weight * u32::from(channel);
                }
            }
            for (slot, a) in out.iter_mut().zip(acc) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the weights sum to Weight::ONE and each channel is \
                              a byte, so every accumulator is at most 255 << 16"
                )]
                let byte = (a >> 16) as u8;
                *slot = byte;
            }
        }
    }

    let mut dest = Pixmap::new(dest_width, dest_height);
    let inter_data = inter.data();
    for (y, taps) in y_taps.iter().enumerate() {
        let Some(dest_row) = dest
            .data_mut()
            .get_mut(y.saturating_mul(inter_width).saturating_mul(4)..)
            .and_then(|rest| rest.get_mut(..inter_width.saturating_mul(4)))
        else {
            continue;
        };
        // The vertical pass taps whole *rows*, so the row slices are resolved
        // once per destination row rather than once per pixel — the inner loop
        // is then a walk down a handful of equal-length `&[u8]`s, which is the
        // shape that autovectorizes. Collected rather than re-derived inside
        // the column loop because there are two or three of them and
        // `dest_width` columns.
        // Paired with its weight rather than collected alongside it: a row the
        // bounds check rejects must drop its weight with it, and a `filter_map`
        // into a bare `Vec` zipped against `weights` afterwards would silently
        // shift every later weight onto the wrong row. That is the one way this
        // rewrite could have changed a pixel, so the pairing is structural.
        let rows: Vec<(u32, &[u8])> = taps
            .weights
            .iter()
            .enumerate()
            .filter_map(|(i, &weight)| {
                let weight = weight.get();
                let sy = taps.start().checked_add(i)?;
                let at = sy.checked_mul(inter_width)?.checked_mul(4)?;
                let row = inter_data.get(at..at.checked_add(inter_width.checked_mul(4)?)?)?;
                Some((weight, row))
            })
            .collect();
        for (x, out) in dest_row.chunks_exact_mut(4).enumerate() {
            let at = x.saturating_mul(4);
            let mut acc = [0_u32; 4];
            for &(weight, row) in &rows {
                let Some(px) = row.get(at..at.saturating_add(4)) else {
                    continue;
                };
                for (slot, &channel) in acc.iter_mut().zip(px.iter()) {
                    *slot += weight * u32::from(channel);
                }
            }
            for (slot, a) in out.iter_mut().zip(acc) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the weights sum to Weight::ONE and each channel is \
                              a byte, so every accumulator is at most 255 << 16"
                )]
                let byte = (a >> 16) as u8;
                *slot = byte;
            }
        }
    }
    dest
}

/// How a reduced pixmap reaches the device.
///
/// The reduction lands on whole pixels, so for a great many images the
/// reduced pixmap **is** the device pixels and the backend has nothing left
/// to resample. Saying so in the type is what stops it being resampled a
/// second time: [`Placement::Exact`] carries only where to put the pixmap,
/// and the filtered path needs the `remaining` transform that variant does
/// not have, so an exact placement cannot be filtered by construction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// The reduced pixmap is the device pixels: blit it at this whole-pixel
    /// offset with the nearest sampler.
    Exact {
        /// Device x of the pixmap's top-left corner, a whole number.
        x: f64,
        /// Device y of the pixmap's top-left corner, a whole number.
        y: f64,
    },
    /// The reduced pixmap still needs the backend's sampler, through this
    /// transform.
    Filtered(kurbo::Affine),
}

impl Placement {
    /// The transform a backend draws with, exact or filtered.
    #[must_use]
    pub fn transform(self) -> kurbo::Affine {
        match self {
            Self::Exact { x, y } => kurbo::Affine::translate((x, y)),
            Self::Filtered(t) => t,
        }
    }

    /// The sampler quality this placement admits.
    ///
    /// An exact placement is nearest whatever the caller wanted: there is
    /// nothing between the texels and the pixels for a filter to do, and
    /// filtering anyway is the second resample this exists to remove.
    #[must_use]
    pub fn quality(self, wanted: crate::device::ImageQuality) -> crate::device::ImageQuality {
        match self {
            Self::Exact { .. } => crate::device::ImageQuality::Nearest,
            Self::Filtered(_) => wanted,
        }
    }
}

/// How close to a whole number a coefficient must be to count as one.
///
/// The transform reaches here through the page matrix, the image's unit
/// square and the reduction ratio, so an axis-aligned placement that is
/// mathematically integral arrives a few ulps off. This is well below half a
/// pixel — the point at which the nearest sampler would pick a different
/// texel — and well above the accumulated error of those three products.
const EXACTNESS: f64 = 1e-9;

/// Classify a reduced image's device transform.
///
/// `Exact` needs three things at once: no rotation or shear, a scale of
/// exactly one in each axis (the reduction already absorbed the rest), and a
/// translation on whole pixels. Anything else keeps the backend's sampler.
///
/// The scale test is against **one** rather than against the reduction ratio
/// because `to_device` here is already the post-reduction transform: the
/// reduced pixmap covers the device rectangle one texel per pixel, or it does
/// not and is filtered.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "a..f are the six affine matrix coefficients, named as in the PDF `cm` operands"
)]
pub fn placement_for(to_device: kurbo::Affine) -> Placement {
    let [a, b, c, d, e, f] = to_device.as_coeffs();
    let integral = |v: f64| v.is_finite() && (v - v.round()).abs() <= EXACTNESS;
    let unit = |v: f64| (v - 1.0).abs() <= EXACTNESS;
    let zero = |v: f64| v.abs() <= EXACTNESS;
    if unit(a) && zero(b) && zero(c) && unit(d) && integral(e) && integral(f) {
        Placement::Exact {
            x: e.round(),
            y: f.round(),
        }
    } else {
        Placement::Filtered(to_device)
    }
}

/// The largest source axis this pre-pass will process.
///
/// A reduction is `O(source pixels)`, which is the same order as decoding the
/// image was, so the guard is against a pathological dimension rather than a
/// large image. Above it the backend's own kernel is used unchanged — a
/// quality loss on a file that has other problems, never a failure.
const MAX_SOURCE_AXIS: u32 = 1 << 16;

/// The pixel dimensions [`prescale`] would reduce a `src_width` x `src_height`
/// image to for the device footprint `dest_width` x `dest_height`, or `None`
/// when it would not reduce at all.
///
/// The same decision `prescale` makes, named so a caller can make it *before*
/// producing any pixels — which is what
/// [`crate::imagecache::PixmapRequest`] needs, because the cache key must
/// distinguish two reductions of one image and must be computable on a hit,
/// where no pixmap exists to measure.
///
/// It answers in *integers*, and that is the point rather than a convenience.
/// The footprint is an `f64` that went through `ceil` and a clamp, so many
/// distinct footprints land on one output size; a cache keyed on the float
/// would miss on two draws of one image whose placements differ in the seventh
/// decimal and whose reduced pixels are byte-identical — which is what a page
/// stepping one image across a row produces. Keeping the two decisions in one
/// function is what stops the key and the reduction disagreeing.
#[must_use]
pub fn reduction_for(
    src_width: u32,
    src_height: u32,
    dest_width: f64,
    dest_height: f64,
) -> Option<(u32, u32)> {
    if src_width > MAX_SOURCE_AXIS || src_height > MAX_SOURCE_AXIS {
        return None;
    }
    let new_w = reduced_len(src_width, dest_width);
    let new_h = reduced_len(src_height, dest_height);
    let (new_w, new_h) = match (new_w, new_h) {
        (None, None) => return None,
        (w, h) => (w.unwrap_or(src_width), h.unwrap_or(src_height)),
    };
    (new_w != 0 && new_h != 0).then_some((new_w, new_h))
}

/// The transform [`prescale`] returns for a reduction to `new_w` x `new_h`.
///
/// The caller's matrix maps the *source* grid to the device; the reduced image
/// covers that same grid with fewer pixels, so each of its pixels is
/// `src / new` of a source pixel wide. Split out from [`prescale`] because a
/// cache hit has the reduced pixmap already and still needs its transform.
#[must_use]
pub fn reduction_transform(
    to_device: kurbo::Affine,
    src_width: u32,
    src_height: u32,
    new_w: u32,
    new_h: u32,
) -> kurbo::Affine {
    let sx = f64::from(src_width) / f64::from(new_w);
    let sy = f64::from(src_height) / f64::from(new_h);
    to_device * kurbo::Affine::scale_non_uniform(sx, sy)
}

/// Pre-reduce `src` toward the device footprint `dest_width` x `dest_height`,
/// returning the reduced pixmap and the placement transform that now maps it.
///
/// Returns `None` when neither axis is being reduced, which is the common case
/// for an enlargement or a 1:1 blit and leaves the caller's own pixmap and
/// transform untouched.
///
/// The returned transform is the caller's, pre-scaled by the reduction ratio,
/// so the image still lands on exactly the same device rectangle: the caller's
/// matrix maps the *source* grid to the device, and the reduced image has
/// fewer pixels covering that same grid.
#[must_use]
pub fn prescale(
    src: &Pixmap,
    to_device: kurbo::Affine,
    dest_width: f64,
    dest_height: f64,
) -> Option<(Pixmap, kurbo::Affine)> {
    // An axis that is not being reduced keeps its own size, so a reduction in
    // one axis alone — a wide image squeezed horizontally — is still filtered
    // in that axis and left alone in the other. `reduction_for` owns that rule
    // so the cache key computed from it cannot disagree with the pixels
    // produced here.
    let (new_w, new_h) = reduction_for(src.width(), src.height(), dest_width, dest_height)?;
    let reduced = reduce_to(src, new_w, new_h);
    Some((
        reduced,
        reduction_transform(to_device, src.width(), src.height(), new_w, new_h),
    ))
}

#[cfg(test)]
mod tests {
    use kurbo::Affine;

    use super::*;

    /// An opaque grey ramp, so every channel carries the same known value.
    fn ramp(width: u32, height: u32) -> Pixmap {
        let mut px = Pixmap::new(width, height);
        for y in 0..height {
            for x in 0..width {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "test fixture dimensions are small"
                )]
                let v = ((x + y * width) % 256) as u8;
                px.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        px
    }

    #[test]
    fn weights_sum_to_one_per_destination_pixel() {
        for (src, dest) in [(455_u32, 159_u32), (100, 7), (9, 4), (1000, 999), (5, 1)] {
            for taps in axis_taps(src, dest) {
                let total: u32 = taps.weights.iter().map(|w| w.get()).sum();
                assert_eq!(total, Weight::ONE.get(), "src {src} dest {dest}");
            }
        }
    }

    #[test]
    fn every_source_pixel_is_tapped_at_an_exact_ratio() {
        // A 4:1 reduction: each destination pixel reads its four source pixels
        // at a quarter each. This is the property a two-tap filter cannot have
        // and the whole reason the module exists.
        //
        // The footprint's closing boundary lands exactly on a pixel edge, and
        // that pixel is tapped too — at weight zero, because it contributes no
        // area. The upstream table does the same (`end_i` is
        // `floor(src_end)`, inclusive), so the trailing zero is the shared
        // shape rather than an artefact of ours.
        let taps = axis_taps(8, 2);
        let [first, second] = taps.as_slice() else {
            panic!("two destination pixels");
        };
        assert_eq!(first.first, 0);
        let quarter = Weight(Weight::ONE.get() / 4);
        assert_eq!(
            &*first.weights,
            [quarter, quarter, quarter, quarter, Weight(0)]
        );
        assert_eq!(second.first, 4);
        // The second pixel's footprint ends at the image's edge, where the
        // clamp to `src_len - 1` stops the range: four taps, no trailing zero.
        assert_eq!(&*second.weights, [quarter; 4]);
    }

    /// The three ratios where the exact table and the old `f64` one part
    /// company, pinned by value so the difference can never be reintroduced
    /// silently.
    ///
    /// These are the cases that moved twenty-eight board rows when the weight
    /// path became integer arithmetic, and in every one of them the `f64`
    /// table was the drifting one:
    ///
    /// - `7 -> 1`: the exact area is `65536 / 7 = 9362.28…` per tap. The
    ///   carried-residue `f64` loop produced
    ///   `[9362, 9363, 9362, 9363, 9362, 9363, 9361]` — the tail is a count
    ///   *low*, which is the accumulated residue drift landing on the last
    ///   tap. The exact table is symmetric about the middle, as the ratio is.
    /// - `10 -> 2`, first destination pixel: the footprint closes exactly on
    ///   a pixel boundary, so the trailing tap covers no area at all. The
    ///   `f64` table gave it a weight of `1` — a whole unit of ink taken from
    ///   inside the footprint and given to a pixel outside it, because
    ///   `floor` of a float that should have been exactly `5.0` was not.
    /// - `5 -> 1`: both tables sum to `ONE`, and they differ only in *which*
    ///   tap carries the `+1` residue — the exact one puts it where the
    ///   cumulative coverage genuinely crosses a half, the third tap, rather
    ///   than on the tail by default.
    #[test]
    fn the_exact_table_is_pinned_where_the_float_one_drifted() {
        let weights = |src, dest| -> Vec<Vec<u32>> {
            axis_taps(src, dest)
                .iter()
                .map(|t| t.weights.iter().map(|w| w.get()).collect())
                .collect()
        };

        // Symmetric, and every tap within one of the exact 9362.28…
        assert_eq!(
            weights(7, 1),
            vec![vec![9362, 9363, 9362, 9362, 9362, 9363, 9362]]
        );

        // The trailing tap of the first pixel covers nothing, and weighs
        // nothing; the second pixel is clamped at the image edge so it has no
        // trailing tap at all.
        assert_eq!(
            weights(10, 2),
            vec![
                vec![13107, 13107, 13108, 13107, 13107, 0],
                vec![13107, 13107, 13108, 13107, 13107],
            ]
        );

        // The residue lands on the third tap, where the cumulative coverage
        // crosses a half — not on the tail.
        assert_eq!(weights(5, 1), vec![vec![13107, 13107, 13108, 13107, 13107]]);

        // And all three still sum to exactly one, which is the property the
        // accumulation's bare `>> SHIFT` depends on.
        for (src, dest) in [(7_u32, 1_u32), (10, 2), (5, 1)] {
            for row in weights(src, dest) {
                assert_eq!(
                    row.iter().sum::<u32>(),
                    Weight::ONE.get(),
                    "{src} -> {dest}"
                );
            }
        }
    }

    #[test]
    fn the_weight_sum_holds_over_the_upstream_unit_tests_grid() {
        // `ExecuteStretchTests`'s own grid of source and destination widths,
        // restated: whatever the ratio, one destination pixel's weights sum to
        // exactly one. The C++ asserts precisely this and nothing else about
        // the table's contents.
        for src in [1_u32, 2, 187, 256, 809, 1110] {
            for dest in [1_u32, 2, 337, 512, 808, 2550] {
                for taps in axis_taps(src, dest) {
                    let total: u32 = taps.weights.iter().map(|w| w.get()).sum();
                    assert_eq!(total, Weight::ONE.get(), "src {src} dest {dest}");
                    assert!(
                        taps.start() + taps.weights.len() <= src as usize + 1,
                        "src {src} dest {dest}: taps run past the source"
                    );
                }
            }
        }
    }

    #[test]
    fn an_exact_halving_is_the_mean_of_each_two_by_two_block() {
        let mut src = Pixmap::new(2, 2);
        src.set_pixel(0, 0, [0, 0, 0, 255]);
        src.set_pixel(1, 0, [100, 100, 100, 255]);
        src.set_pixel(0, 1, [200, 200, 200, 255]);
        src.set_pixel(1, 1, [255, 255, 255, 255]);
        let out = reduce_to(&src, 1, 1);
        assert_eq!(out.width(), 1);
        assert_eq!(out.height(), 1);
        // (0 + 100 + 200 + 255) / 4 = 138.75, truncated by the fixed-point
        // shift to 138.
        let px = out.pixel(0, 0).expect("one pixel");
        assert_eq!(px, [138, 138, 138, 255]);
    }

    #[test]
    fn a_flat_field_survives_any_reduction_exactly() {
        // The weights summing to one is what makes this true; a filter that
        // dropped its rounding residue would darken a flat grey.
        for (dw, dh) in [(1_u32, 1_u32), (3, 7), (13, 5), (64, 64)] {
            let mut src = Pixmap::new(100, 100);
            for y in 0..100 {
                for x in 0..100 {
                    src.set_pixel(x, y, [77, 77, 77, 255]);
                }
            }
            let out = reduce_to(&src, dw, dh);
            for y in 0..dh {
                for x in 0..dw {
                    assert_eq!(
                        out.pixel(x, y),
                        Some([77, 77, 77, 255]),
                        "flat field at {dw}x{dh} pixel ({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn an_enlargement_is_declined() {
        let src = ramp(4, 4);
        assert!(prescale(&src, Affine::IDENTITY, 40.0, 40.0).is_none());
        // And so is an exact 1:1.
        assert!(prescale(&src, Affine::IDENTITY, 4.0, 4.0).is_none());
    }

    #[test]
    fn one_axis_reducing_leaves_the_other_alone() {
        let src = ramp(64, 8);
        let (out, _) = prescale(&src, Affine::IDENTITY, 16.0, 8.0).expect("x reduces");
        assert_eq!(out.width(), 16);
        assert_eq!(out.height(), 8);
    }

    #[test]
    fn the_returned_transform_covers_the_same_device_rect() {
        // The reduced image must land where the source would have: the
        // transform is pre-scaled by exactly the reduction ratio.
        let src = ramp(100, 50);
        let placement = Affine::scale_non_uniform(0.25, 0.25);
        let (out, t) = prescale(&src, placement, 25.0, 12.0).expect("reduces");
        let src_rect = placement.transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, 100.0, 50.0));
        let out_rect = t.transform_rect_bbox(kurbo::Rect::new(
            0.0,
            0.0,
            f64::from(out.width()),
            f64::from(out.height()),
        ));
        assert!((src_rect.width() - out_rect.width()).abs() < 1e-9);
        assert!((src_rect.height() - out_rect.height()).abs() < 1e-9);
        assert!((src_rect.x0 - out_rect.x0).abs() < 1e-9);
        assert!((src_rect.y0 - out_rect.y0).abs() < 1e-9);
    }

    #[test]
    fn a_sub_pixel_destination_reduces_to_one_pixel() {
        let src = ramp(32, 32);
        let (out, _) = prescale(&src, Affine::IDENTITY, 0.4, 0.4).expect("reduces");
        assert_eq!((out.width(), out.height()), (1, 1));
    }

    #[test]
    fn transparency_is_averaged_in_premultiplied_space() {
        // Half the pixels opaque white, half fully transparent: the average is
        // half-covered white, which in premultiplied form is 127 everywhere —
        // not white at half alpha, and not grey.
        let mut src = Pixmap::new(2, 1);
        src.set_pixel(0, 0, [255, 255, 255, 255]);
        src.set_pixel(1, 0, [0, 0, 0, 0]);
        let out = reduce_to(&src, 1, 1);
        assert_eq!(out.pixel(0, 0), Some([127, 127, 127, 127]));
    }

    #[test]
    fn a_degenerate_axis_yields_an_empty_result_rather_than_panicking() {
        let src = Pixmap::new(0, 0);
        assert!(prescale(&src, Affine::IDENTITY, 10.0, 10.0).is_none());
        let src = ramp(4, 4);
        assert!(prescale(&src, Affine::IDENTITY, f64::NAN, f64::NAN).is_none());
    }

    /// The fused convert-and-reduce is `to_pixmap` followed by `reduce_to`,
    /// byte for byte.
    ///
    /// This is the licence [`convert_and_reduce`] spends. The two-call path
    /// materialized the whole source as RGBA and then two full-height
    /// intermediates; the fused one holds four rows. They must nonetheless
    /// agree exactly, because the corpus was gated on the two-call answer —
    /// so the same tap tables, the same rounding after each pass, the same
    /// order. Asserted over a lattice of sizes and every `Pixels` kind,
    /// because the tap tables differ per size and an equality that held only
    /// at 4:1 on grey would be an accident.
    #[test]
    fn the_fused_reduction_is_the_two_call_one() {
        use pdfrum_page::{ImageData, Pixels};

        let kinds = |w: u32, h: u32| -> Vec<(&'static str, Pixels)> {
            let n = (w * h) as usize;
            vec![
                (
                    "gray",
                    Pixels::Gray8(
                        (0..n)
                            .map(|i| u8::try_from(i * 37 % 251).unwrap_or(0))
                            .collect(),
                    ),
                ),
                (
                    "rgb",
                    Pixels::Rgb8(
                        (0..n * 3)
                            .map(|i| u8::try_from(i * 53 % 251).unwrap_or(0))
                            .collect(),
                    ),
                ),
                (
                    "cmyk",
                    Pixels::Cmyk8(
                        (0..n * 4)
                            .map(|i| u8::try_from(i * 29 % 251).unwrap_or(0))
                            .collect(),
                    ),
                ),
                (
                    "indexed",
                    Pixels::Indexed {
                        indices: (0..n)
                            .map(|i| u8::try_from(i * 17 % 256).unwrap_or(0))
                            .collect(),
                        palette: (0..=255u8)
                            .map(|v| pdfrum_page::Rgb {
                                r: f32::from(v) / 255.0,
                                g: f32::from(255 - v) / 255.0,
                                b: 0.25,
                            })
                            .collect(),
                    },
                ),
            ]
        };

        for (w, h, dw, dh) in [
            (64_u32, 40_u32, 8_u32, 5_u32),
            (137, 85, 17, 11),
            (455, 455, 159, 159),
            (9, 9, 1, 1),
            (100, 7, 7, 1),
            (5, 3, 4, 2),
        ] {
            for (name, pixels) in kinds(w, h) {
                let image = ImageData {
                    width: w,
                    height: h,
                    pixels,
                    mask: None,
                    matte: None,
                    interpolate: false,
                };
                let fill = crate::color::Argb::opaque(255, 255, 255);
                let two_call = reduce_to(&crate::image::to_pixmap(&image, fill, None), dw, dh);
                let fused = convert_and_reduce(&image, fill, None, dw, dh);
                assert_eq!(fused.data(), two_call.data(), "{name} {w}x{h} -> {dw}x{dh}");
            }
        }
    }

    /// A stencil carries its colour and its transparency through the fused
    /// path too — the one kind whose pixels are not its samples.
    #[test]
    fn the_fused_reduction_carries_a_stencils_colour() {
        use pdfrum_page::{BitImage, ImageData, Pixels};

        let (w, h) = (16_u32, 16_u32);
        let row_bytes = (w as usize).div_ceil(8);
        let image = ImageData {
            width: w,
            height: h,
            pixels: Pixels::Stencil(BitImage {
                width: w,
                height: h,
                row_bytes,
                bits: (0..row_bytes * h as usize)
                    .map(|i| u8::try_from(i * 73 % 256).unwrap_or(0))
                    .collect(),
            }),
            mask: None,
            matte: None,
            interpolate: false,
        };
        let fill = crate::color::Argb::opaque(200, 100, 50);
        for (dw, dh) in [(4_u32, 4_u32), (8, 3), (1, 1)] {
            let two_call = reduce_to(&crate::image::to_pixmap(&image, fill, None), dw, dh);
            let fused = convert_and_reduce(&image, fill, None, dw, dh);
            assert_eq!(fused.data(), two_call.data(), "stencil -> {dw}x{dh}");
        }
    }

    /// Only a whole-pixel, unit-scale, unrotated placement is exact.
    ///
    /// Every clause is load-bearing: a residual scale means the backend still
    /// has to resample, a rotation or shear likewise, and a fractional
    /// translation means the texels do not line up with the pixels. Getting
    /// any of them wrong draws a filtered image with the nearest sampler,
    /// which is visible.
    #[test]
    fn only_a_whole_pixel_unit_placement_is_exact() {
        use crate::device::ImageQuality;

        let exact = |t: Affine| matches!(placement_for(t), Placement::Exact { .. });

        assert!(exact(Affine::IDENTITY));
        assert!(exact(Affine::translate((13.0, -7.0))));
        // A translation that is integral to within the tolerance, which is
        // what an axis-aligned placement actually arrives as.
        assert!(exact(Affine::translate((13.0 + 1e-12, 4.0 - 1e-12))));

        // A residual scale in either axis, however small, is still a resample.
        assert!(!exact(Affine::scale_non_uniform(1.001, 1.0)));
        assert!(!exact(Affine::scale_non_uniform(1.0, 0.999)));
        assert!(!exact(Affine::scale(2.0)));
        // A flip is a unit scale in magnitude and is *not* exact: the sampler
        // would read the rows in the other order.
        assert!(!exact(Affine::scale_non_uniform(1.0, -1.0)));
        // Rotation and shear.
        assert!(!exact(Affine::rotate(0.5)));
        assert!(!exact(Affine::new([1.0, 0.0, 0.3, 1.0, 0.0, 0.0])));
        // Half a pixel is exactly where the nearest sampler would change its
        // mind, so it must not be called exact.
        assert!(!exact(Affine::translate((0.5, 0.0))));
        assert!(!exact(Affine::translate((0.0, 0.25))));
        // A non-finite coefficient is never exact.
        assert!(!exact(Affine::translate((f64::NAN, 0.0))));

        // An exact placement forces nearest; a filtered one keeps what the
        // caller asked for and its own transform.
        let t = Affine::translate((3.0, 4.0));
        assert_eq!(
            placement_for(t).quality(ImageQuality::Bilinear),
            ImageQuality::Nearest
        );
        assert_eq!(placement_for(t).transform(), t);
        let f = Affine::scale(0.5);
        assert_eq!(
            placement_for(f).quality(ImageQuality::Bilinear),
            ImageQuality::Bilinear
        );
        assert_eq!(placement_for(f).transform(), f);
    }

    /// The single-channel reducer is the four-channel one, byte for byte, on
    /// an image whose four channels are equal.
    ///
    /// This is the licence [`crate::image::reduced_mask_pixmap`] spends: the
    /// soft-mask path reduces one channel where it used to reduce four copies
    /// of it, and the two must not be allowed to drift. Asserted over a
    /// lattice of ratios rather than one, because the tap tables differ per
    /// size and an equality that held only at 8:1 would be an accident.
    #[test]
    fn the_gray_reduction_is_the_rgba_reduction_on_a_gray_image() {
        for (w, h, dw, dh) in [
            (64_u32, 40_u32, 8_u32, 5_u32),
            (137, 85, 17, 11),
            (1339, 81, 392, 11),
            (455, 455, 159, 159),
            (9, 9, 1, 1),
            (100, 7, 7, 1),
            (1000, 999, 999, 998),
            (5, 3, 4, 2),
        ] {
            let plane: Vec<u8> = (0..w * h)
                .map(|i| u8::try_from(i * 37 % 251).unwrap_or(0))
                .collect();
            let mut rgba = Pixmap::new(w, h);
            for (slot, &v) in rgba.data_mut().chunks_exact_mut(4).zip(plane.iter()) {
                slot.copy_from_slice(&[v, v, v, v]);
            }

            let gray = reduce_gray_to(&plane, w, h, dw, dh);
            let four = reduce_to(&rgba, dw, dh);

            assert_eq!(gray.len(), (dw as usize) * (dh as usize), "{w}x{h}");
            for (i, &g) in gray.iter().enumerate() {
                assert_eq!(
                    four.data().get(i * 4..i * 4 + 4),
                    Some(&[g, g, g, g][..]),
                    "{w}x{h} -> {dw}x{dh}, sample {i}"
                );
            }
        }
    }

    /// A plane whose length disagrees with the dimensions reduces to zeroes
    /// rather than reading out of range or panicking.
    #[test]
    fn a_gray_plane_of_the_wrong_length_reduces_to_zeroes() {
        assert_eq!(reduce_gray_to(&[1, 2, 3], 4, 4, 2, 2), vec![0; 4]);
        assert_eq!(reduce_gray_to(&[], 0, 0, 2, 2), vec![0; 4]);
        assert!(reduce_gray_to(&[1; 16], 4, 4, 0, 2).is_empty());
    }
}
