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

/// The fixed-point scale the weights are carried in (`kFixedPointBits`).
///
/// Weights within one destination pixel sum to exactly this, which is what
/// lets the accumulation shift rather than divide.
const FIXED_ONE: u32 = 1 << 16;

/// Round-half-away-from-zero, which is what the oracle does and what
/// `f64::round` also does — spelled out because the weight table's exactness
/// depends on the tie direction and a reader should not have to check.
fn fixed_from(v: f64) -> u32 {
    let scaled = v * f64::from(FIXED_ONE);
    if !scaled.is_finite() || scaled <= 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to [0, FIXED_ONE] just above and below, both exactly \
                  representable in f64 and inside u32"
    )]
    let rounded = scaled.round().min(f64::from(FIXED_ONE)) as u32;
    rounded
}

/// One destination pixel's taps: the first source index it reads, and one
/// weight per consecutive source pixel from there.
///
/// The weights sum to [`FIXED_ONE`] whenever the range is non-empty, so an
/// accumulation over them needs no normalisation — only a shift.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Taps {
    start: usize,
    weights: Vec<u32>,
}

/// The taps for every destination pixel along one axis being reduced.
///
/// Each destination pixel maps back to the half-open source interval
/// `[d * scale, (d + 1) * scale)`, and each source pixel in it contributes the
/// share of the *destination* pixel that it covers. Computing the overlap in
/// destination space rather than source space is what keeps the weights
/// summing to one without a division per pixel.
///
/// The fractional residue of each weight is carried into the next
/// (`rounding_error`), and whatever is still unspent lands on the final tap —
/// so the sum is exactly [`FIXED_ONE`] rather than one part in 65536 short,
/// which over a wide image would otherwise show as a gradient.
fn axis_taps(src_len: u32, dest_len: u32) -> Vec<Taps> {
    let (src_len_i, dest_len_i) = (i64::from(src_len), i64::from(dest_len));
    if src_len_i == 0 || dest_len_i == 0 {
        return Vec::new();
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "both are image dimensions, far below 2^53"
    )]
    let scale = src_len_i as f64 / dest_len_i as f64;
    let mut out = Vec::with_capacity(dest_len as usize);
    for dest in 0..dest_len_i {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a destination index, far below 2^53"
        )]
        let dest_f = dest as f64;
        let span_start = dest_f * scale;
        let span_end = span_start + scale;
        // The source pixels the destination pixel's footprint touches, clamped
        // to the image. `floor(span_end)` is inclusive because a footprint
        // ending exactly on a boundary still nominally taps the pixel beyond
        // it — at weight zero, which the area computation then assigns.
        let first = span_start.floor().max(0.0);
        let last = span_end.floor().min(f64::from(src_len - 1));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to [0, src_len - 1] above"
        )]
        let (first, last) = (first as usize, last as usize);
        if first > last {
            out.push(Taps {
                start: first.min(src_len as usize - 1),
                weights: vec![FIXED_ONE],
            });
            continue;
        }
        let mut weights = Vec::with_capacity(last - first + 1);
        let mut remaining = FIXED_ONE;
        let mut rounding_error = 0.0_f64;
        for src in first..last {
            #[expect(clippy::cast_precision_loss, reason = "a source index, far below 2^53")]
            let src_f = src as f64;
            // This source pixel's extent, expressed in destination pixels.
            let cover_start = (src_f / scale).max(dest_f);
            let cover_end = ((src_f + 1.0) / scale).min(dest_f + 1.0);
            let area = (cover_end - cover_start).max(0.0);
            let weight = fixed_from(area + rounding_error);
            weights.push(weight.min(remaining));
            remaining = remaining.saturating_sub(weight);
            rounding_error = area - f64::from(weight) / f64::from(FIXED_ONE);
        }
        // Whatever the fractional areas did not spend belongs to the last tap;
        // the alternative — dropping it — biases every reduced image dark by
        // up to one part in 65536 per tap, which accumulates across the axis.
        weights.push(remaining);
        out.push(Taps {
            start: first,
            weights,
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
            for (i, &weight) in taps.weights.iter().enumerate() {
                let Some(&sample) = taps.start.checked_add(i).and_then(|sx| src_row.get(sx)) else {
                    continue;
                };
                acc += weight * u32::from(sample);
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the weights sum to FIXED_ONE and the sample is a byte, \
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
                let sy = taps.start.checked_add(i)?;
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
                reason = "the weights sum to FIXED_ONE and the sample is a byte, \
                          so the accumulator is at most 255 << 16"
            )]
            let byte = (acc >> 16) as u8;
            *out = byte;
        }
    }
    dest
}

/// Box-filter `src` down to `dest_width` x `dest_height`, one axis at a time.
///
/// Horizontal first into an intermediate, then vertical — the same order and
/// the same two weight tables PDFium uses, and the reason a two-pass reduction
/// costs `O(w * h * (taps_x + taps_y))` rather than their product.
///
/// Public because [`crate::walk`] reduces *inside* a cache miss, where the
/// size has already been decided by [`reduction_for`] and only the pixels are
/// wanted; [`prescale`] remains the entry point for a caller who wants the
/// decision and the pixels together.
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
            for (i, &weight) in taps.weights.iter().enumerate() {
                let Some(px) = taps
                    .start
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
                    reason = "the weights sum to FIXED_ONE and each channel is \
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
                let sy = taps.start.checked_add(i)?;
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
                    reason = "the weights sum to FIXED_ONE and each channel is \
                              a byte, so every accumulator is at most 255 << 16"
                )]
                let byte = (a >> 16) as u8;
                *slot = byte;
            }
        }
    }
    dest
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
                let total: u32 = taps.weights.iter().sum();
                assert_eq!(total, FIXED_ONE, "src {src} dest {dest}");
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
        assert_eq!(first.start, 0);
        assert_eq!(
            first.weights,
            vec![
                FIXED_ONE / 4,
                FIXED_ONE / 4,
                FIXED_ONE / 4,
                FIXED_ONE / 4,
                0
            ]
        );
        assert_eq!(second.start, 4);
        // The second pixel's footprint ends at the image's edge, where the
        // clamp to `src_len - 1` stops the range: four taps, no trailing zero.
        assert_eq!(second.weights, vec![FIXED_ONE / 4; 4]);
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
                    let total: u32 = taps.weights.iter().sum();
                    assert_eq!(total, FIXED_ONE, "src {src} dest {dest}");
                    assert!(
                        taps.start + taps.weights.len() <= src as usize + 1,
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
            let plane: Vec<u8> = (0..w * h).map(|i| (i * 37 % 251) as u8).collect();
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
