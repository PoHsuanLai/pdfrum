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
#[must_use]
pub fn to_pixmap(image: &ImageData, stencil_color: Argb) -> Pixmap {
    let mut out = Pixmap::new(image.width, image.height);
    for y in 0..image.height {
        for x in 0..image.width {
            let alpha = image.mask.as_ref().map_or(255, |m| m.alpha_at(x, y));
            let px = if let Pixels::Stencil(bits) = &image.pixels {
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
                let [r, g, b] = image.pixels.color_at(x, y, image.width).to_bytes();
                [mul255(r, alpha), mul255(g, alpha), mul255(b, alpha), alpha]
            };
            out.set_pixel(x, y, px);
        }
    }
    out
}

/// The `/Matte` un-premultiplication (`CalculateDrawImage`,
/// `cpdf_imagerenderer.cpp:263-319`).
///
/// Integer, in place, against the matte colour, applied **before** the mask
/// is — and a pixel whose mask is zero is skipped, which is what avoids the
/// division by zero rather than a guard.
pub fn un_premultiply_matte(pixmap: &mut Pixmap, mask: &crate::pixmap::AlphaMask, matte: [u8; 3]) {
    if mask.width() != pixmap.width() || mask.height() != pixmap.height() {
        return;
    }
    let matte = matte.map(i32::from);
    for (chunk, &m) in pixmap.data_mut().chunks_exact_mut(4).zip(mask.data()) {
        if m == 0 {
            continue;
        }
        let m = i32::from(m);
        for i in 0..3 {
            let (Some(&dest), Some(&mt)) = (chunk.get(i), matte.get(i)) else {
                continue;
            };
            let orig = (i32::from(dest) - mt) * 255 / m + mt;
            if let Some(slot) = chunk.get_mut(i) {
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "the clamp lower bound is 0, so the value fits u8 exactly"
                )]
                let byte = orig.clamp(0, 255) as u8;
                *slot = byte;
            }
        }
    }
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
        let p = to_pixmap(&img, Argb::opaque(255, 0, 0));
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
        let p = to_pixmap(&img, Argb::BLACK);
        assert_eq!(
            p.pixel(0, 0),
            Some([128, 128, 128, 128]),
            "premultiplied by the mask"
        );
    }

    #[test]
    fn matte_unpremultiply_skips_zero_mask_pixels() {
        let mut p = Pixmap::filled(2, 1, peniko::Color::from_rgba8(100, 100, 100, 255));
        let mask = crate::pixmap::AlphaMask::from_vec(2, 1, vec![0, 128]).expect("sized");
        un_premultiply_matte(&mut p, &mask, [50, 50, 50]);
        // The zero-mask pixel is untouched — which is what avoids the divide
        // by zero, rather than a guard on the division itself.
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(100));
        // The other is un-premultiplied against the matte: (100-50)*255/128+50.
        #[expect(
            clippy::cast_sign_loss,
            reason = "the clamp lower bound is 0, so the value fits u8 exactly"
        )]
        let expected = ((100 - 50) * 255 / 128 + 50).clamp(0, 255) as u8;
        assert_eq!(p.pixel(1, 0).map(|px| px[0]), Some(expected));
    }

    #[test]
    fn matte_ignores_a_mismatched_mask() {
        let mut p = Pixmap::filled(2, 1, peniko::Color::from_rgba8(100, 100, 100, 255));
        let before = p.clone();
        let mask = crate::pixmap::AlphaMask::filled(4, 4, 200);
        un_premultiply_matte(&mut p, &mask, [50, 50, 50]);
        assert_eq!(p, before);
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
        let p = to_pixmap(&img, Argb::BLACK);
        assert_eq!(p.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(p.pixel(1, 0), Some([0, 0, 255, 255]));
    }
}
