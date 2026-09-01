//! Drawing an image `XObject`: the decision tree in `StartRenderDIBBase`
//! (`cpdf_imagerenderer.cpp:86-186`) and the resample selection
//! (`CStretchEngine::UseInterpolateBilinear`, `cstretchengine.cpp:48-59`).
//!
//! The *kernels* belong to whichever rasterizer is in use; the **selection**
//! belongs here, because both must receive the same answer. Three inputs feed
//! it — the image's own `/Interpolate`, the render options' two smoothing
//! flags, and a size heuristic — and their precedence is not obvious:
//! `bNoSmoothing` wins over `/Interpolate`, and an image above 60 million
//! bytes forces bilinear on unless halftoning was asked for.

use kurbo::Affine;
use pdfrum_page::{BlendMode, ColorSpace, ImageData, Pixels};

use crate::color::Argb;
use crate::device::ImageQuality;
use crate::options::RenderOptions;
use crate::pixmap::{Pixmap, mul255};

/// Above this many bytes, bilinear interpolation is forced on unless
/// halftoning was requested (`kHugeImageSize`, `cpdf_dib.h:39`).
///
/// A memory/quality heuristic, and nonetheless pixel-visible.
pub const HUGE_IMAGE_SIZE: u64 = 60_000_000;

/// `IsImageValueTooBig` (`cpdf_imagerenderer.cpp:52-59`): a destination
/// dimension or offset at or beyond this is rejected outright.
pub const MAX_IMAGE_VALUE: i64 = 256 * 1024 * 1024;

/// The shear threshold between a general transform and an axis-aligned
/// stretch (`cpdf_imagerenderer.cpp:468-469`, `:558`).
pub const SHEAR_THRESHOLD: f64 = 0.5;

/// `UseInterpolateBilinear`, transcribed exactly.
///
/// Both divisions are **integer and truncating**, and only the right-hand
/// product widens. Read plainly: bilinear turns itself on unless the image is
/// being enlarged by more than roughly eight times in area.
#[must_use]
pub fn use_interpolate_bilinear(
    already_bilinear: bool,
    no_smoothing: bool,
    src_width: u32,
    src_height: u32,
    dest_width: i64,
    dest_height: i64,
) -> bool {
    if already_bilinear || no_smoothing {
        return false;
    }
    let dw = dest_width.unsigned_abs();
    if dw == 0 {
        return false;
    }
    let dh = dest_height.unsigned_abs();
    let lhs = dh / 8;
    let rhs = (u64::from(src_width) * u64::from(src_height)) / dw;
    lhs < rhs
}

/// The sampling quality one image draw uses.
///
/// Note that each arm **replaces** the whole options struct rather than
/// setting one field, so a halftone request is silently dropped whenever
/// either of the first two arms fires — reproduced here by the ordering.
#[must_use]
pub fn resample_quality(
    image: &ImageData,
    opts: &RenderOptions,
    src_width: u32,
    src_height: u32,
    dest_width: i64,
    dest_height: i64,
) -> ImageQuality {
    // `bNoSmoothing` wins over `/Interpolate`: the C++ writes them as an
    // `else if` chain, not as independent flags.
    if opts.no_image_smooth {
        return ImageQuality::Nearest;
    }
    let mut bilinear = image.interpolate;
    // The huge-image rule, which fires regardless of `/Interpolate`.
    let bytes = u64::from(src_width)
        .saturating_mul(u64::from(src_height))
        .saturating_mul(image.pixels.components() as u64);
    if bytes > HUGE_IMAGE_SIZE && !opts.force_halftone {
        bilinear = true;
    }
    if !bilinear
        && use_interpolate_bilinear(
            bilinear,
            opts.no_image_smooth,
            src_width,
            src_height,
            dest_width,
            dest_height,
        )
    {
        bilinear = true;
    }
    if bilinear {
        ImageQuality::Bilinear
    } else {
        ImageQuality::Nearest
    }
}

/// Whether a device matrix is an integer-only translation.
///
/// `vello_cpu` silently downgrades bilinear sampling to nearest in exactly
/// this case and `tiny-skia` does not, so the engine detects it and passes
/// [`ImageQuality::Nearest`] itself — making the downgrade a no-op and the
/// two backends agree by construction.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "exactness is the predicate: only a bit-exact identity-plus-integer \
              -translation matrix triggers vello_cpu's nearest downgrade, so an \
              epsilon here would claim a downgrade the backend does not make"
)]
#[expect(
    clippy::many_single_char_names,
    reason = "a..f are the six affine matrix coefficients, named as in the PDF `cm` operands"
)]
pub fn is_integer_translation(m: Affine) -> bool {
    let [a, b, c, d, e, f] = m.as_coeffs();
    a == 1.0
        && b == 0.0
        && c == 0.0
        && d == 1.0
        && e.fract() == 0.0
        && f.fract() == 0.0
        && e.is_finite()
        && f.is_finite()
}

/// The quality the device is actually handed, after the integer-translation
/// normalisation.
#[must_use]
pub fn effective_quality(quality: ImageQuality, to_device: Affine) -> ImageQuality {
    if is_integer_translation(to_device) {
        ImageQuality::Nearest
    } else {
        quality
    }
}

/// The blend mode an image composites with.
///
/// The one genuinely surprising branch of the tree: an image in a
/// *subtractive* colour space, drawn with no transparency of any kind and
/// with `/OP true` and `/OPM 0`, composites with **`Darken`** rather than
/// Normal — PDFium's approximation of overprint. Every clause of the gate is
/// load-bearing; relaxing any one of them changes the corpus.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "the `== 1.0f` alpha clauses are the upstream gate verbatim; an \
              epsilon would apply the Darken approximation to files PDFium \
              composites Normal, changing the corpus"
)]
pub fn overprint_blend(
    space: Option<&ColorSpace>,
    state: &pdfrum_page::state::GeneralState,
) -> BlendMode {
    let subtractive = matches!(
        space.map(ColorSpace::family),
        Some(
            pdfrum_page::color::Family::DeviceCmyk
                | pdfrum_page::color::Family::Separation
                | pdfrum_page::color::Family::DeviceN
        )
    );
    let eligible = subtractive
        && state.fill_overprint
        && state.overprint_mode == 0
        && state.fill_alpha == 1.0
        && state.stroke_alpha == 1.0
        && matches!(state.blend, BlendMode::Normal | BlendMode::Compatible);
    if eligible {
        BlendMode::Darken
    } else {
        state.blend
    }
}

/// The destination flip rule (`GetDimensionsFromUnitRect`,
/// `cpdf_imagerenderer.cpp:667-699`).
///
/// `a < 0` flips horizontally and `d > 0` — not `< 0` — flips vertically,
/// which is correct given PDF's y-down device space.
#[must_use]
pub fn destination_flips(m: Affine) -> (bool, bool) {
    let [a, _, _, d, _, _] = m.as_coeffs();
    (a < 0.0, d > 0.0)
}

/// Whether a destination dimension or offset is within `IsImageValueTooBig`.
#[must_use]
pub fn image_value_fits(v: f64) -> bool {
    #[expect(
        clippy::cast_precision_loss,
        reason = "MAX_IMAGE_VALUE is 2^28, exactly representable in f64"
    )]
    let limit = MAX_IMAGE_VALUE as f64;
    v.is_finite() && v.abs() < limit
}

/// Rasterize decoded image samples into a premultiplied RGBA pixmap.
///
/// A stencil (`/ImageMask true`) is painted in `stencil_color`: a set bit is
/// ink, a clear one is transparent. Everything else takes its own colours,
/// with any `/SMask`, `/Mask` or colour-key mask folded into the alpha.
///
/// A `/Matte` image is the one case where the samples are not the colours:
/// they are pre-blended against the matte, so each one is un-premultiplied
/// by [`matte_source`] before the mask becomes its alpha.
///
/// **The mask is only folded in when it shares the image's sample grid.**
/// A mask is never resolution-reduced (design brief §1.19.9), so it routinely
/// has dimensions of its own, and the two are then resampled to the device
/// *independently* — which is [`separate_mask`]'s job, not this one. Indexing
/// a mask of a different size at the base's `(x, y)` reads the wrong sample
/// everywhere and falls off the end into "opaque" for most of the image.
#[must_use]
pub fn to_pixmap(
    image: &ImageData,
    stencil_color: Argb,
    transfer: Option<&crate::transfer::TransferFunc<'_>>,
) -> Pixmap {
    let mut out = Pixmap::new(image.width, image.height);
    let matte = image.matte.map(pdfrum_page::Rgb::to_bytes);
    // `StartRenderDIBBase` wraps the source in a `CPDF_TransferFuncDIB` when
    // the state carries a `/TR` that is not the identity, so *every sample*
    // goes through the three tables — not just the fill colour. Skipping this
    // leaves an image untouched on a page whose paths all inverted, which is
    // what `transfer_function.in`'s second page shows.
    //
    // A **stencil** takes it through its colour instead: it has no samples of
    // its own, and `GetFillArgb` already ran the same function over the fill.
    let transfer = transfer.filter(|t| !t.is_identity());
    // A mask on its own grid is drawn separately; folding it in here would
    // sample it at the base's coordinates.
    let fused = image.mask.as_ref().filter(|m| is_coregistered(m, image));
    // M12: the stencil test is hoisted out of the pixel loop and the
    // destination is written through its row slice, rather than
    // `set_pixel` recomputing `(y * width + x) * 4` per pixel.
    //
    // The `if let Pixels::Stencil` was inside a loop over every sample of the
    // image, re-deciding a fact that is a property of the *image* — the
    // optimizer cannot hoist it on its own because `image.pixels` is behind a
    // reference it must assume the loop body could change. On the corpus's
    // large images this function is the single most expensive thing in a
    // render (`docs/status/M12.md`), so a branch per pixel is worth removing
    // even though it predicts perfectly.
    //
    // The arithmetic per pixel is untouched, and deliberately: `color_at` stays
    // the one place a sample becomes a colour, matte and transfer still apply
    // in the same order, and the products are still `mul255`. What changed is
    // where the loop-invariant work happens.
    let stencil = match &image.pixels {
        Pixels::Stencil(bits) => Some(bits),
        _ => None,
    };
    // M12b P1: the sample becomes bytes without a float trip, and an indexed
    // image's palette is encoded once instead of once per pixel.
    //
    // `color_at(..).to_bytes()` reached `Rgb` through `f32::from(b) / 255.0`
    // and came back through `(v.clamp(0,1) * 255).round()`, which is exactly
    // the identity on all 256 byte values — so for grey and RGB the whole trip
    // was a no-op the optimizer cannot remove (it cannot know `round` is exact
    // here), and for CMYK it wrapped a table lookup that is byte-in byte-out
    // already. `Pixels::sample_bytes` is the same answer, proved exhaustively
    // by `the_byte_path_is_exactly_the_float_path` rather than by measurement.
    //
    // This runs once per *source* pixel — twenty-five million times on
    // `image_bug_718762`, where it was 90% of the render
    // (docs/status/M12b-P1.md §6).
    let palette = image.pixels.byte_palette();
    let indices = match &image.pixels {
        Pixels::Indexed { indices, .. } => Some(&**indices),
        _ => None,
    };
    let width = image.width as usize;
    for y in 0..image.height {
        let Some(row) = out
            .data_mut()
            .get_mut((y as usize).saturating_mul(width).saturating_mul(4)..)
            .and_then(|rest| rest.get_mut(..width.saturating_mul(4)))
        else {
            continue;
        };
        for (x, slot) in (0..image.width).zip(row.chunks_exact_mut(4)) {
            let alpha = fused.map_or(255, |m| m.alpha_at(x, y));
            let px = if let Some(bits) = stencil {
                // A set bit is ink; a clear one paints nothing at all.
                if bits.pixel(x, y) {
                    let a = mul255(stencil_color.a, alpha);
                    [
                        mul255(stencil_color.r, a),
                        mul255(stencil_color.g, a),
                        mul255(stencil_color.b, a),
                        a,
                    ]
                } else {
                    [0, 0, 0, 0]
                }
            } else {
                // The indexed fast path reads the pre-encoded palette; every
                // other kind goes through `sample_bytes`, which has no palette
                // to hoist.
                let mut rgb = match (indices, palette.as_ref()) {
                    (Some(indices), Some(palette)) => {
                        // An absent index reads as 0 and an absent palette
                        // entry as black, which is `color_at`'s own ladder:
                        // `indices.get(i).unwrap_or(0)`, then
                        // `palette.get(..).unwrap_or(Rgb::BLACK)`.
                        let i = (y as usize)
                            .saturating_mul(width)
                            .saturating_add(x as usize);
                        let entry = indices.get(i).copied().unwrap_or(0);
                        palette
                            .get(usize::from(entry))
                            .copied()
                            .unwrap_or([0, 0, 0])
                    }
                    _ => image.pixels.sample_bytes(x, y, image.width),
                };
                if let Some(matte) = matte {
                    rgb = matte_source(rgb, alpha, matte);
                }
                if let Some(transfer) = transfer {
                    let [r, g, b] = rgb;
                    let mapped = transfer.translate(Argb { a: 255, r, g, b });
                    rgb = [mapped.r, mapped.g, mapped.b];
                }
                let [r, g, b] = rgb;
                [mul255(r, alpha), mul255(g, alpha), mul255(b, alpha), alpha]
            };
            slot.copy_from_slice(&px);
        }
    }
    out
}

/// Whether a mask shares its image's sample grid, so it can be folded into
/// the image's own pixels rather than resampled to the device separately.
///
/// A colour-key mask always is: it was resolved from the base's own raw
/// samples at decode time, so it is the base's grid by construction. An
/// `Alpha` mask is only when its dimensions match, which a `/Mask` stream or
/// `/SMask` at its own resolution generally does not.
#[must_use]
pub fn is_coregistered(mask: &pdfrum_page::ImageMask, image: &ImageData) -> bool {
    // Only the `Alpha` arm carries a grid of its own; everything else is the
    // base's own samples by construction, so it is co-registered.
    let pdfrum_page::ImageMask::Alpha { width, height, .. } = mask else {
        return true;
    };
    *width == image.width && *height == image.height
}

/// The mask an image carries on a grid of its own, as a standalone image to
/// be stretched to the device by itself (`DrawMaskedImage` →
/// `CalculateDrawImage`, `cpdf_imagerenderer.cpp:375-424` and `:263-319`).
///
/// PDFium never fuses a mask into its base's samples. It renders the base
/// into a device-sized buffer, renders the mask into a second buffer over the
/// *same device rect* through the *same* matrix, and multiplies. Both are
/// therefore resampled from their own resolution straight to the device, and
/// neither ever passes through the other's grid — which is what lets a 64×64
/// stencil keep its detail over a 3×3 base (`bug_1396266`), and a 100×100
/// `/SMask` cover the whole of a 400×400 base (`bug_1236`).
///
/// The returned plane carries the mask's coverage, one byte per sample, and
/// [`mask_pixmap`] is what turns it into the pixmap a device draws: white
/// premultiplied by that coverage, so drawing it into a transparent buffer and
/// reading that buffer's alpha reproduces `CalculateDrawImage`'s 8-bpp mask
/// bitmap — including the zero it leaves outside the mask's extent, which
/// luminance over a transparent ground could not distinguish from a covered
/// black sample. A stencil's inversion is already applied by
/// [`pdfrum_page::ImageMask::alpha_at`].
///
/// # Why a plane rather than the pixmap
///
/// The mask is grey by construction — every pixel of the pixmap this used to
/// return was `[a, a, a, a]` — and the caller's next act is to box-filter it
/// toward a device footprint that is, on a soft-masked thumbnail, an order of
/// magnitude smaller in each axis. Building the four-byte form first makes
/// both the build and the filter touch four times the bytes they need to, and
/// three of every four are copies. Returning the plane lets
/// [`reduced_mask_pixmap`] reduce in one channel and expand once, at the
/// destination size.
#[must_use]
pub fn separate_mask(mask: &pdfrum_page::ImageMask) -> Option<(ImageData, Vec<u8>)> {
    let pdfrum_page::ImageMask::Alpha { width, height, .. } = mask else {
        return None;
    };
    let (w, h) = (*width, *height);
    if w == 0 || h == 0 {
        return None;
    }
    let len = (w as usize).checked_mul(h as usize)?;
    let mut plane = Vec::with_capacity(len);
    for y in 0..h {
        for x in 0..w {
            plane.push(mask.alpha_at(x, y));
        }
    }
    // The descriptor the resample selection reads: the mask's own size is
    // what it is scaled from, and `/Interpolate` is not inherited — the C++
    // hands `CalculateDrawImage` the *base's* resample options, and the
    // stretch engine's own size heuristic then decides.
    //
    // `Pixels::Gray8` names the plane's shape for `resample_quality`'s
    // component count and nothing reads its samples through this descriptor,
    // so it borrows the plane's length rather than a second copy of it: the
    // buffer this used to clone was the mask's whole sample array, allocated
    // and memcpy'd on every draw so that `components()` could answer 1.
    let dict = ImageData {
        width: w,
        height: h,
        pixels: Pixels::Gray8(Box::default()),
        mask: None,
        matte: None,
        interpolate: false,
    };
    Some((dict, plane))
}

/// A coverage plane as the pixmap a device draws: white, premultiplied by the
/// coverage, which is `[a, a, a, a]` per sample.
///
/// A plane of the wrong length yields a transparent pixmap rather than reading
/// out of range — the same degradation the rest of this module's bounds checks
/// produce on a malformed file.
#[must_use]
pub fn mask_pixmap(plane: &[u8], width: u32, height: u32) -> Pixmap {
    let mut out = Pixmap::new(width, height);
    if plane.len() != (width as usize).saturating_mul(height as usize) {
        return out;
    }
    for (slot, &a) in out.data_mut().chunks_exact_mut(4).zip(plane.iter()) {
        slot.copy_from_slice(&[a, a, a, a]);
    }
    out
}

/// A coverage plane reduced toward a device footprint and expanded into the
/// pixmap a device draws, with the placement transform that now maps it.
///
/// This is [`crate::stretch::prescale`] for the soft-mask path, and it differs
/// from it in exactly one way: the box filter runs over **one** channel rather
/// than four, and the expansion to four happens afterwards, at the reduced
/// size. Every output byte is identical to prescaling the expanded pixmap —
/// the filter is per-channel and the four channels are equal — and
/// `the_gray_reduction_is_the_rgba_reduction_on_a_gray_image` in
/// [`crate::stretch`] pins that.
///
/// On a soft-masked thumbnail the saving is the whole point rather than a
/// margin: `image_en_fqa` reduces 29.8 million mask samples per render at
/// roughly 8.3x in each axis, so the four-channel form built and filtered
/// 119 MB where one channel needs 30 MB, and the expanded buffer it hands the
/// device is 1/69th the size of the one it used to build.
#[must_use]
pub fn reduced_mask_pixmap(
    plane: &[u8],
    width: u32,
    height: u32,
    to_device: kurbo::Affine,
    dest_width: f64,
    dest_height: f64,
) -> (Pixmap, kurbo::Affine) {
    match crate::stretch::reduction_for(width, height, dest_width, dest_height) {
        Some((new_w, new_h)) => {
            let reduced = crate::stretch::reduce_gray_to(plane, width, height, new_w, new_h);
            (
                mask_pixmap(&reduced, new_w, new_h),
                crate::stretch::reduction_transform(to_device, width, height, new_w, new_h),
            )
        }
        None => (mask_pixmap(plane, width, height), to_device),
    }
}

/// One pixel of the `/Matte` un-premultiplication (`CalculateDrawImage`,
/// `cpdf_imagerenderer.cpp:263-319`), in the integer arithmetic the C++ uses.
///
/// A `/Matte` entry says the image's samples were **already composited**
/// against that colour at the mask's own coverage, so the sample is not the
/// colour to draw — the colour to draw is what the sample would have been
/// before that blend. Recovering it is the inverse blend, and the C++ spells
/// it with a truncating integer divide by the mask byte and a clamp, not with
/// floats: `(dest - matte) * 255 / mask + matte`.
///
/// **A zero mask is skipped rather than guarded.** The C++ never enters the
/// loop body for such a pixel, which is what avoids the divide by zero; the
/// sample is left exactly as it was, and its alpha is zero anyway.
#[must_use]
pub fn matte_source(sample: [u8; 3], mask: u8, matte: [u8; 3]) -> [u8; 3] {
    if mask == 0 {
        return sample;
    }
    let m = i32::from(mask);
    let mut out = sample;
    for i in 0..3 {
        let (Some(&dest), Some(&mt)) = (sample.get(i), matte.get(i)) else {
            continue;
        };
        let orig = (i32::from(dest) - i32::from(mt)) * 255 / m + i32::from(mt);
        if let Some(slot) = out.get_mut(i) {
            #[expect(
                clippy::cast_sign_loss,
                reason = "the clamp lower bound is 0, so the value fits u8 exactly"
            )]
            let byte = orig.clamp(0, 255) as u8;
            *slot = byte;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use pdfrum_page::state::GeneralState;
    use pdfrum_page::{BitImage, ImageMask, Rgb};

    use super::*;

    fn gray_image(w: u32, h: u32, interpolate: bool) -> ImageData {
        ImageData {
            width: w,
            height: h,
            pixels: Pixels::Gray8(vec![128; (w * h) as usize].into_boxed_slice()),
            mask: None,
            matte: None,
            interpolate,
        }
    }

    #[test]
    fn use_interpolate_bilinear_heuristic() {
        // Enlarging by more than ~8x in area keeps nearest; shrinking or a
        // modest enlargement turns bilinear on.
        assert!(
            use_interpolate_bilinear(false, false, 100, 100, 50, 50),
            "downsampling"
        );
        assert!(
            !use_interpolate_bilinear(false, false, 4, 4, 100, 100),
            "big enlargement"
        );
        // The already-set and no-smoothing arms short-circuit.
        assert!(!use_interpolate_bilinear(true, false, 100, 100, 50, 50));
        assert!(!use_interpolate_bilinear(false, true, 100, 100, 50, 50));
        // A zero destination width is rejected before the division.
        assert!(!use_interpolate_bilinear(false, false, 100, 100, 0, 50));
    }

    #[test]
    fn the_heuristic_divisions_truncate() {
        // dest_height/8 and (src area)/dest_width are both integer: at
        // dest_height 7 the left side is 0, so any positive right side wins.
        assert!(use_interpolate_bilinear(false, false, 1, 1, 1, 7));
        // And a right side that truncates to 0 loses.
        assert!(!use_interpolate_bilinear(false, false, 1, 1, 2, 800));
    }

    #[test]
    fn no_smoothing_wins_over_interpolate() {
        let img = gray_image(10, 10, true);
        let opts = RenderOptions {
            no_image_smooth: true,
            ..RenderOptions::default()
        };
        assert_eq!(
            resample_quality(&img, &opts, 10, 10, 5, 5),
            ImageQuality::Nearest
        );
    }

    #[test]
    fn huge_image_forces_bilinear_unless_halftoning() {
        // A 5000x5000 RGB image is 75 million bytes, past the threshold.
        let mut img = gray_image(5000, 5000, false);
        img.pixels = Pixels::Rgb8(vec![0; 3].into_boxed_slice());
        let opts = RenderOptions::default();
        assert_eq!(
            resample_quality(&img, &opts, 5000, 5000, 100_000, 100_000),
            ImageQuality::Bilinear
        );
        let halftone = RenderOptions {
            force_halftone: true,
            ..RenderOptions::default()
        };
        assert_eq!(
            resample_quality(&img, &halftone, 5000, 5000, 100_000, 100_000),
            ImageQuality::Nearest,
            "halftoning suppresses the forcing rule"
        );
    }

    #[test]
    fn cmyk_overprint_becomes_darken() {
        let state = GeneralState {
            fill_overprint: true,
            overprint_mode: 0,
            ..Default::default()
        };
        let cmyk = ColorSpace::DeviceCmyk;
        assert_eq!(overprint_blend(Some(&cmyk), &state), BlendMode::Darken);

        // Every clause of the gate, flipped one at a time.
        let mut alpha = state.clone();
        alpha.fill_alpha = 0.5;
        assert_eq!(overprint_blend(Some(&cmyk), &alpha), BlendMode::Normal);

        let mut stroke = state.clone();
        stroke.stroke_alpha = 0.5;
        assert_eq!(overprint_blend(Some(&cmyk), &stroke), BlendMode::Normal);

        let mut mode = state.clone();
        mode.overprint_mode = 1;
        assert_eq!(overprint_blend(Some(&cmyk), &mode), BlendMode::Normal);

        let mut off = state.clone();
        off.fill_overprint = false;
        assert_eq!(overprint_blend(Some(&cmyk), &off), BlendMode::Normal);

        let mut blend = state.clone();
        blend.blend = BlendMode::Multiply;
        assert_eq!(overprint_blend(Some(&cmyk), &blend), BlendMode::Multiply);

        // An additive space never qualifies.
        assert_eq!(
            overprint_blend(Some(&ColorSpace::DeviceRgb), &state),
            BlendMode::Normal
        );
        assert_eq!(overprint_blend(None, &state), BlendMode::Normal);
    }

    #[test]
    fn dimensions_flip_rule_uses_a_less_than_and_d_greater_than() {
        // `d > 0` is correct, not `d < 0`, because device y points down.
        assert_eq!(destination_flips(Affine::IDENTITY), (false, true));
        assert_eq!(
            destination_flips(Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 0.0])),
            (false, false)
        );
        assert_eq!(
            destination_flips(Affine::new([-1.0, 0.0, 0.0, -1.0, 0.0, 0.0])),
            (true, false)
        );
    }

    #[test]
    fn image_values_are_bounded() {
        assert!(image_value_fits(1000.0));
        assert!(!image_value_fits(300_000_000.0));
        assert!(!image_value_fits(f64::NAN));
        assert!(!image_value_fits(f64::INFINITY));
    }

    #[test]
    fn integer_translations_are_detected() {
        assert!(is_integer_translation(Affine::translate((3.0, -7.0))));
        assert!(!is_integer_translation(Affine::translate((3.5, 0.0))));
        assert!(!is_integer_translation(Affine::scale(2.0)));
        // The normalisation makes vello's silent downgrade a no-op.
        assert_eq!(
            effective_quality(ImageQuality::Bilinear, Affine::translate((3.0, 4.0))),
            ImageQuality::Nearest
        );
        assert_eq!(
            effective_quality(ImageQuality::Bilinear, Affine::scale(2.0)),
            ImageQuality::Bilinear
        );
    }

    #[test]
    fn a_stencil_paints_its_set_bits_in_the_fill_colour() {
        let bits = BitImage {
            width: 2,
            height: 1,
            row_bytes: 1,
            bits: vec![0b1000_0000],
        };
        let img = ImageData {
            width: 2,
            height: 1,
            pixels: Pixels::Stencil(bits),
            mask: None,
            matte: None,
            interpolate: false,
        };
        let p = to_pixmap(&img, Argb::opaque(255, 0, 0), None);
        assert_eq!(p.pixel(0, 0), Some([255, 0, 0, 255]), "the set bit is ink");
        assert_eq!(
            p.pixel(1, 0),
            Some([0, 0, 0, 0]),
            "the clear bit paints nothing"
        );
    }

    #[test]
    fn a_mask_folds_into_the_alpha() {
        let img = ImageData {
            width: 1,
            height: 1,
            pixels: Pixels::Rgb8(vec![255, 255, 255].into_boxed_slice()),
            mask: Some(ImageMask::Alpha {
                width: 1,
                height: 1,
                alpha: vec![128].into_boxed_slice(),
                stencil: false,
            }),
            matte: None,
            interpolate: false,
        };
        let p = to_pixmap(&img, Argb::BLACK, None);
        assert_eq!(
            p.pixel(0, 0),
            Some([128, 128, 128, 128]),
            "premultiplied by the mask"
        );
    }

    /// A mask on a grid of its own is **not** folded in: `to_pixmap` leaves
    /// the image opaque and [`separate_mask`] carries the coverage instead.
    ///
    /// Folding it would index the mask at the base's coordinates. A base
    /// larger than its mask then reads out of range — which `alpha_at`
    /// reports as opaque — so most of the image escapes masking entirely
    /// (`bug_1236`: a 100×100 `/SMask` over a 400×400 base masked only the
    /// top-left quarter). A base *smaller* than its mask reads only the mask's
    /// top-left corner and throws the rest away (`bug_1396266`: a 64×64
    /// stencil over a 3×3 base kept 9 of its 4096 samples).
    #[test]
    fn a_mask_on_its_own_grid_is_not_folded_into_the_samples() {
        let mask = ImageMask::Alpha {
            width: 2,
            height: 2,
            alpha: vec![0, 0, 0, 0].into_boxed_slice(),
            stencil: false,
        };
        let img = ImageData {
            width: 1,
            height: 1,
            pixels: Pixels::Rgb8(vec![255, 255, 255].into_boxed_slice()),
            mask: Some(mask.clone()),
            matte: None,
            interpolate: false,
        };
        assert!(!is_coregistered(&mask, &img), "2x2 mask over a 1x1 base");
        let p = to_pixmap(&img, Argb::BLACK, None);
        assert_eq!(
            p.pixel(0, 0),
            Some([255, 255, 255, 255]),
            "the sample stays opaque; the mask is applied at device resolution"
        );

        // Same dimensions: folded in as before.
        let same = ImageMask::Alpha {
            width: 1,
            height: 1,
            alpha: vec![64].into_boxed_slice(),
            stencil: false,
        };
        let coreg = ImageData {
            mask: Some(same.clone()),
            ..img
        };
        assert!(is_coregistered(&same, &coreg));
        assert_eq!(
            to_pixmap(&coreg, Argb::BLACK, None).pixel(0, 0),
            Some([64; 4])
        );
    }

    /// The standalone mask carries its coverage in the **alpha** channel, so
    /// reading the drawn buffer's alpha yields zero outside the mask's extent
    /// rather than confusing "uncovered" with "covered black".
    #[test]
    fn a_separate_mask_carries_its_coverage_in_the_alpha() {
        let mask = ImageMask::Alpha {
            width: 2,
            height: 1,
            alpha: vec![0, 200].into_boxed_slice(),
            stencil: false,
        };
        let (dict, plane) = separate_mask(&mask).expect("an alpha mask yields a plane");
        assert_eq!((dict.width, dict.height), (2, 1));
        assert_eq!(plane, vec![0, 200]);
        let px = mask_pixmap(&plane, dict.width, dict.height);
        assert_eq!(px.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(px.pixel(1, 0), Some([200, 200, 200, 200]));

        // A stencil's inversion is already applied.
        let stencil = ImageMask::Alpha {
            width: 1,
            height: 1,
            alpha: vec![0].into_boxed_slice(),
            stencil: true,
        };
        let (d, plane) = separate_mask(&stencil).expect("a stencil yields a plane");
        assert_eq!(
            mask_pixmap(&plane, d.width, d.height).pixel(0, 0),
            Some([255; 4]),
            "a clear stencil bit is opaque"
        );
    }

    /// The descriptor `separate_mask` returns is read for its *shape* — the
    /// resample selection's component count and the two dimensions — and never
    /// for its samples, which is why it no longer clones the mask's whole
    /// buffer to carry them.
    ///
    /// If a future edit starts reading `dict.pixels`, this test is the one that
    /// says the samples were never there.
    #[test]
    fn the_separate_masks_descriptor_carries_shape_and_not_samples() {
        let mask = ImageMask::Alpha {
            width: 4,
            height: 3,
            alpha: vec![7; 12].into_boxed_slice(),
            stencil: false,
        };
        let (dict, plane) = separate_mask(&mask).expect("an alpha mask yields a plane");
        assert_eq!(
            dict.pixels.components(),
            1,
            "the component count is the use"
        );
        assert_eq!((dict.width, dict.height), (4, 3));
        assert_eq!(plane.len(), 12, "the coverage is in the plane");
    }

    /// A plane whose length disagrees with the dimensions yields a transparent
    /// pixmap rather than a panic or an out-of-range read.
    #[test]
    fn a_mask_plane_of_the_wrong_length_is_transparent() {
        let px = mask_pixmap(&[1, 2, 3], 4, 4);
        assert_eq!((px.width(), px.height()), (4, 4));
        assert!(px.data().iter().all(|b| *b == 0));
    }

    /// [`reduced_mask_pixmap`] must produce exactly what prescaling the
    /// expanded pixmap produced, transform included — that equality is the
    /// whole licence for reducing in one channel, and the conformance gate
    /// cannot see a mask the corpus happens not to exercise.
    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the placement transform must be bit-identical to the one \
                  `prescale` produced, not merely close: a caller compares it \
                  for equality nowhere, but a difference in the last bit would \
                  land the mask on a different device pixel"
    )]
    fn the_reduced_mask_pixmap_is_the_prescaled_expanded_one() {
        for (w, h, dw, dh) in [
            (64_u32, 40_u32, 8.0_f64, 5.0_f64),
            (137, 85, 16.4, 10.2),
            (1339, 81, 391.5, 10.2),
            (9, 9, 9.0, 9.0),
            (9, 9, 20.0, 20.0),
            (13, 7, 1.0, 1.0),
        ] {
            let plane: Vec<u8> = (0..w * h).map(|i| (i * 37 % 251) as u8).collect();
            let at = kurbo::Affine::new([1.5, 0.0, 0.0, 2.5, 3.0, 4.0]);

            let (got, got_at) = reduced_mask_pixmap(&plane, w, h, at, dw, dh);
            let expanded = mask_pixmap(&plane, w, h);
            let (want, want_at) = match crate::stretch::prescale(&expanded, at, dw, dh) {
                Some(pair) => pair,
                None => (expanded, at),
            };

            assert_eq!(
                (got.width(), got.height()),
                (want.width(), want.height()),
                "{w}x{h} -> {dw}x{dh}"
            );
            assert_eq!(got.data(), want.data(), "{w}x{h} -> {dw}x{dh}");
            assert_eq!(
                got_at.as_coeffs(),
                want_at.as_coeffs(),
                "{w}x{h} -> {dw}x{dh}"
            );
        }
    }

    #[test]
    fn a_transfer_function_reaches_the_images_own_samples() {
        // `StartRenderDIBBase` wraps the source in a `CPDF_TransferFuncDIB`,
        // so a `/TR` runs over every sample rather than only over the fill
        // colour a stencil takes. An inverting function must turn a white
        // image black.
        let invert: [u8; 256] = std::array::from_fn(|i| 255 - u8::try_from(i).unwrap_or(255));
        let inverting = pdfrum_page::TransferFunc {
            identity: false,
            samples: Box::new([invert, invert, invert]),
        };
        let func = crate::transfer::TransferFunc::new(&inverting);
        let img = ImageData {
            width: 1,
            height: 1,
            pixels: Pixels::Rgb8(vec![255, 255, 255].into_boxed_slice()),
            mask: None,
            matte: None,
            interpolate: false,
        };
        assert_eq!(
            to_pixmap(&img, Argb::BLACK, Some(&func)).pixel(0, 0),
            Some([0, 0, 0, 255])
        );
        // Without it the samples travel through untouched.
        assert_eq!(
            to_pixmap(&img, Argb::BLACK, None).pixel(0, 0),
            Some([255, 255, 255, 255])
        );
    }

    #[test]
    fn matte_unpremultiply_skips_a_zero_mask() {
        // The zero-mask pixel is returned untouched — which is what avoids
        // the divide by zero, rather than a guard on the division itself.
        assert_eq!(
            matte_source([100, 100, 100], 0, [50, 50, 50]),
            [100, 100, 100]
        );
    }

    #[test]
    fn matte_unpremultiply_is_the_integer_inverse_blend() {
        // (100 - 50) * 255 / 128 + 50, truncating.
        let expected = u8::try_from((100 - 50) * 255 / 128 + 50).expect("in range");
        assert_eq!(
            matte_source([100, 100, 100], 128, [50, 50, 50])[0],
            expected
        );
        // A fully opaque mask is the identity: nothing was blended away.
        assert_eq!(
            matte_source([37, 90, 200], 255, [50, 50, 50]),
            [37, 90, 200]
        );
    }

    #[test]
    fn matte_unpremultiply_clamps_rather_than_wrapping() {
        // A sample far above its matte at low coverage overshoots 255.
        assert_eq!(matte_source([255, 255, 255], 1, [0, 0, 0]), [255, 255, 255]);
        // And one far below undershoots 0.
        assert_eq!(matte_source([0, 0, 0], 1, [255, 255, 255]), [0, 0, 0]);
    }

    #[test]
    fn a_matte_image_un_premultiplies_before_the_mask_becomes_alpha() {
        // Half-covered mid grey pre-blended against black: the source colour
        // was twice as bright as the sample, and the mask is still the alpha.
        let img = ImageData {
            width: 1,
            height: 1,
            pixels: Pixels::Gray8(vec![64].into_boxed_slice()),
            mask: Some(ImageMask::Alpha {
                width: 1,
                height: 1,
                alpha: vec![128].into_boxed_slice(),
                stencil: false,
            }),
            matte: Some(Rgb::BLACK),
            interpolate: false,
        };
        let p = to_pixmap(&img, Argb::BLACK, None);
        // (64 - 0) * 255 / 128 + 0 = 127, premultiplied by 128/255 = 63.
        let source = u8::try_from(64 * 255 / 128).expect("in range");
        assert_eq!(
            p.pixel(0, 0),
            Some([
                mul255(source, 128),
                mul255(source, 128),
                mul255(source, 128),
                128
            ])
        );
    }

    #[test]
    fn an_image_without_a_matte_takes_its_samples_verbatim() {
        // The same image without `/Matte` keeps 64, not 127: the
        // un-premultiplication must never run on a plain soft-masked image.
        let img = ImageData {
            width: 1,
            height: 1,
            pixels: Pixels::Gray8(vec![64].into_boxed_slice()),
            mask: Some(ImageMask::Alpha {
                width: 1,
                height: 1,
                alpha: vec![128].into_boxed_slice(),
                stencil: false,
            }),
            matte: None,
            interpolate: false,
        };
        let p = to_pixmap(&img, Argb::BLACK, None);
        assert_eq!(
            p.pixel(0, 0),
            Some([mul255(64, 128), mul255(64, 128), mul255(64, 128), 128])
        );
    }

    #[test]
    fn indexed_pixels_read_their_palette() {
        let img = ImageData {
            width: 2,
            height: 1,
            pixels: Pixels::Indexed {
                indices: vec![0, 1].into_boxed_slice(),
                palette: vec![
                    Rgb {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                    },
                    Rgb {
                        r: 0.0,
                        g: 0.0,
                        b: 1.0,
                    },
                ]
                .into_boxed_slice(),
            },
            mask: None,
            matte: None,
            interpolate: false,
        };
        let p = to_pixmap(&img, Argb::BLACK, None);
        assert_eq!(p.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(p.pixel(1, 0), Some([0, 0, 255, 255]));
    }
}
