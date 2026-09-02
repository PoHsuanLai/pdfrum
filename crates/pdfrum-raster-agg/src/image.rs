//! Sampling an image through a transform.
//!
//! `draw_image` maps the image's own pixel grid through an affine and asks for
//! the source pixel behind each device pixel. That is an *inverse* mapping:
//! invert the transform once, then per device pixel take its centre back into
//! image space and reconstruct a sample there.
//!
//! Two reconstructions, matching the two the trait offers. Which one runs is
//! the engine's decision — it ports the oracle's `/Interpolate` handling, the
//! `kHugeImageSize` rule and the `UseInterpolateBilinear` heuristic — so this
//! module only has to implement both faithfully.

use kurbo::Affine;
use pdfrum_render::{ImageQuality, Pixmap, pixmap};

/// An image bound to an inverse transform, ready to sample per device pixel.
#[derive(Debug, Clone, Copy)]
pub struct Sampler<'a> {
    image: &'a Pixmap,
    /// Device space to image space.
    inverse: Affine,
    quality: ImageQuality,
}

impl<'a> Sampler<'a> {
    /// Bind `image` to the transform mapping its **pixel grid** onto the
    /// device, or `None` when that transform is not invertible.
    ///
    /// A singular transform collapses the image to a line or a point, which
    /// covers no area and so paints nothing — the same outcome the oracle
    /// reaches by refusing the draw.
    #[must_use]
    pub fn new(image: &'a Pixmap, transform: Affine, quality: ImageQuality) -> Option<Self> {
        if image.width() == 0 || image.height() == 0 {
            return None;
        }
        let det = transform.determinant();
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let inverse = transform.inverse();
        inverse
            .as_coeffs()
            .iter()
            .all(|c| c.is_finite())
            .then_some(Self {
                image,
                inverse,
                quality,
            })
    }

    /// The premultiplied source pixel behind device pixel `(x, y)`, or `None`
    /// where the image does not reach.
    #[must_use]
    pub fn sample(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let p = self.inverse * kurbo::Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
        if !p.x.is_finite() || !p.y.is_finite() {
            return None;
        }
        match self.quality {
            ImageQuality::Nearest => self.nearest(p),
            ImageQuality::Bilinear => self.bilinear(p),
        }
    }

    /// The texel containing a point.
    fn nearest(&self, at: kurbo::Point) -> Option<[u8; 4]> {
        let (width, height) = (self.image.width(), self.image.height());
        let texel_x = floor_to_i64(at.x)?;
        let texel_y = floor_to_i64(at.y)?;
        if texel_x < 0 || texel_y < 0 || texel_x >= i64::from(width) || texel_y >= i64::from(height)
        {
            return None;
        }
        self.image
            .pixel(u32::try_from(texel_x).ok()?, u32::try_from(texel_y).ok()?)
    }

    /// A weighted average of the four texels around a point.
    ///
    /// The weights are computed on the **premultiplied** samples, which is the
    /// only correct place to average them: averaging straight colours would
    /// let a fully transparent texel's arbitrary colour bleed into its
    /// neighbours. Edge texels are clamped rather than wrapped, so the image's
    /// own border extends outward by half a texel instead of showing the
    /// opposite edge.
    fn bilinear(&self, at: kurbo::Point) -> Option<[u8; 4]> {
        let (width, height) = (self.image.width(), self.image.height());
        // Texel centres sit at half-integers, so shift before flooring.
        let shifted_x = at.x - 0.5;
        let shifted_y = at.y - 0.5;
        let left = floor_to_i64(shifted_x)?;
        let top_row = floor_to_i64(shifted_y)?;
        // Entirely outside, with the half-texel border the clamp implies.
        if left < -1 || top_row < -1 || left >= i64::from(width) || top_row >= i64::from(height) {
            return None;
        }
        // Fixed-point weights on 0..=256, so the two-axis product shifts off
        // by exactly 16 bits and the whole interpolation stays integer.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a fraction of a texel is 0.0..1.0; scaling it by 256 and \
                      truncating is the fixed-point weight"
        )]
        let weight_x = ((shifted_x - shifted_x.floor()) * 256.0) as i32;
        #[expect(clippy::cast_possible_truncation, reason = "same bound as weight_x")]
        let weight_y = ((shifted_y - shifted_y.floor()) * 256.0) as i32;

        let texel = |x: i64, y: i64| -> [u8; 4] {
            let clamped_x = x.clamp(0, i64::from(width) - 1);
            let clamped_y = y.clamp(0, i64::from(height) - 1);
            match (u32::try_from(clamped_x), u32::try_from(clamped_y)) {
                (Ok(tx), Ok(ty)) => self.image.pixel(tx, ty).unwrap_or([0; 4]),
                _ => [0; 4],
            }
        };
        let top_left = texel(left, top_row);
        let top_right = texel(left + 1, top_row);
        let bottom_left = texel(left, top_row + 1);
        let bottom_right = texel(left + 1, top_row + 1);

        let mut out = [0u8; 4];
        for channel in 0..4 {
            let (Some(&tl), Some(&tr), Some(&bl), Some(&br)) = (
                top_left.get(channel),
                top_right.get(channel),
                bottom_left.get(channel),
                bottom_right.get(channel),
            ) else {
                continue;
            };
            let top = i32::from(tl) * (256 - weight_x) + i32::from(tr) * weight_x;
            let bottom = i32::from(bl) * (256 - weight_x) + i32::from(br) * weight_x;
            let value = (top * (256 - weight_y) + bottom * weight_y) >> 16;
            if let Some(slot) = out.get_mut(channel) {
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "the clamp's lower bound is 0, so the value is non-negative"
                )]
                let byte = value.clamp(0, 255) as u8;
                *slot = byte;
            }
        }
        // Premultiplication must survive the average: a rounded channel can
        // exceed a rounded alpha by a count, which would make the pixel
        // un-representable. Clamping the colours to the alpha restores the
        // invariant without moving a visible value.
        let alpha = out.get(3).copied().unwrap_or(0);
        for slot in out.iter_mut().take(3) {
            *slot = (*slot).min(alpha);
        }
        Some(out)
    }
}

/// `floor` as an `i64`, or `None` for a value no integer can hold.
fn floor_to_i64(v: f64) -> Option<i64> {
    let f = v.floor();
    // Well inside i64's range; anything beyond is a degenerate transform and
    // has no texel behind it anyway.
    (f.is_finite() && f.abs() < 1e15).then(|| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the guard bounds the value to +/-1e15, inside i64"
        )]
        let out = f as i64;
        out
    })
}

/// Scale a premultiplied pixel by a constant alpha, truncating.
///
/// `draw_image`'s `alpha` argument, applied where the samples are read rather
/// than through a layer: this backend has no reason to allocate one, and the
/// per-sample product is the same arithmetic the oracle's own constant-alpha
/// image blit performs.
#[must_use]
pub fn scale_alpha(px: [u8; 4], alpha: u8) -> [u8; 4] {
    if alpha == 255 {
        return px;
    }
    [
        pixmap::mul255(px[0], alpha),
        pixmap::mul255(px[1], alpha),
        pixmap::mul255(px[2], alpha),
        pixmap::mul255(px[3], alpha),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                p.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        p
    }

    #[test]
    fn an_identity_transform_samples_texel_for_pixel() {
        let img = checker(4, 4);
        let s = Sampler::new(&img, Affine::IDENTITY, ImageQuality::Nearest).expect("invertible");
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(s.sample(x, y), img.pixel(x, y), "at ({x}, {y})");
            }
        }
    }

    #[test]
    fn outside_the_image_there_is_no_sample() {
        let img = checker(2, 2);
        let s = Sampler::new(&img, Affine::IDENTITY, ImageQuality::Nearest).expect("invertible");
        assert!(s.sample(2, 0).is_none());
        assert!(s.sample(0, 2).is_none());
    }

    #[test]
    fn a_singular_transform_has_no_sampler() {
        let img = checker(2, 2);
        assert!(Sampler::new(&img, Affine::scale(0.0), ImageQuality::Nearest).is_none());
    }

    #[test]
    fn an_empty_image_has_no_sampler() {
        let img = Pixmap::new(0, 0);
        assert!(Sampler::new(&img, Affine::IDENTITY, ImageQuality::Nearest).is_none());
    }

    #[test]
    fn bilinear_between_two_texels_is_their_average() {
        // Two texels, black and white, sampled exactly on the boundary
        // between their centres.
        let mut img = Pixmap::new(2, 1);
        img.set_pixel(0, 0, [0, 0, 0, 255]);
        img.set_pixel(1, 0, [255, 255, 255, 255]);
        // Scale x by 2 so device pixel 1's centre (1.5) lands at image x 0.75,
        // a quarter of the way from texel 0's centre to texel 1's.
        let s = Sampler::new(
            &img,
            Affine::scale_non_uniform(2.0, 1.0),
            ImageQuality::Bilinear,
        )
        .expect("invertible");
        let px = s.sample(1, 0).expect("inside");
        assert!(
            (60..=70).contains(&px[0]),
            "a quarter of the way is about 64, got {}",
            px[0]
        );
    }

    #[test]
    fn bilinear_clamps_the_border_rather_than_wrapping() {
        // At the far left edge the missing neighbour is the edge texel
        // itself, so the sample is the edge colour rather than the opposite
        // side's.
        let mut img = Pixmap::new(2, 1);
        img.set_pixel(0, 0, [255, 0, 0, 255]);
        img.set_pixel(1, 0, [0, 0, 255, 255]);
        let s = Sampler::new(&img, Affine::IDENTITY, ImageQuality::Bilinear).expect("invertible");
        let px = s.sample(0, 0).expect("inside");
        assert_eq!(px, [255, 0, 0, 255], "the left edge stays its own colour");
    }

    #[test]
    fn bilinear_keeps_colours_within_alpha() {
        // Averaging a transparent texel with an opaque one must not produce a
        // channel above the resulting alpha, which would not be a valid
        // premultiplied pixel.
        let mut img = Pixmap::new(2, 1);
        img.set_pixel(0, 0, [255, 255, 255, 255]);
        img.set_pixel(1, 0, [0, 0, 0, 0]);
        let s = Sampler::new(
            &img,
            Affine::scale_non_uniform(4.0, 1.0),
            ImageQuality::Bilinear,
        )
        .expect("invertible");
        for x in 0..8 {
            if let Some(px) = s.sample(x, 0) {
                let alpha = px.get(3).copied().unwrap_or(0);
                for c in 0..3 {
                    let channel = px.get(c).copied().unwrap_or(0);
                    assert!(channel <= alpha, "channel {c} above alpha at x={x}: {px:?}");
                }
            }
        }
    }

    #[test]
    fn scaling_alpha_truncates_like_the_oracle() {
        assert_eq!(scale_alpha([255, 255, 255, 255], 128), [128, 128, 128, 128]);
        assert_eq!(scale_alpha([10, 20, 30, 40], 255), [10, 20, 30, 40]);
    }
}
