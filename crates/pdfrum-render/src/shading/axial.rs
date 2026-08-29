//! Type 2, axial shadings (`DrawAxialShading`,
//! `cpdf_rendershading.cpp:109-175`).
//!
//! One projection onto the axis per pixel, then a ramp lookup. Two details
//! are pixel-visible: the sample point is the pixel's **corner**, not its
//! centre, and a degenerate axis is not guarded — coincident endpoints give a
//! division by zero whose infinities and NaNs the extend rules then absorb.

use kurbo::{Affine, Point};
use pdfrum_page::shading::Axial;

use crate::pixmap::Pixmap;
use crate::shading::steps::ColorSteps;

/// Rasterize an axial shading into `dest`.
///
/// `to_bitmap` maps shading space to the destination's pixel grid; its
/// inverse is computed once and applied per pixel. `dest` is *not* cleared:
/// pixels the shading declines to paint keep whatever `/Background` left.
pub fn draw(dest: &mut Pixmap, axial: &Axial, steps: &ColorSteps, to_bitmap: Affine) {
    let Some(inverse) = invert(to_bitmap) else {
        return;
    };
    let x_span = axial.end.x - axial.start.x;
    let y_span = axial.end.y - axial.start.y;
    let axis_len_square = x_span * x_span + y_span * y_span;

    for row in 0..dest.height() {
        for col in 0..dest.width() {
            // The pixel *corner*, not its centre.
            let pos = inverse * Point::new(f64::from(col), f64::from(row));
            let scale = ((pos.x - axial.start.x) * x_span + (pos.y - axial.start.y) * y_span)
                / axis_len_square;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the ramp lookup ports the truncating cast"
            )]
            let Some(color) = steps.lookup(scale as f32, axial.extend_start, axial.extend_end)
            else {
                continue;
            };
            dest.set_pixel(col, row, crate::pixmap::premultiply(color.to_peniko()));
        }
    }
}

fn invert(m: Affine) -> Option<Affine> {
    let det = m.determinant();
    (det != 0.0 && det.is_finite()).then(|| m.inverse())
}

#[cfg(test)]
mod tests {
    use pdfrum_page::Rgb;

    use super::*;
    use crate::shading::steps::STEPS;

    fn black_to_white() -> ColorSteps {
        let mut colors = [Rgb::BLACK; STEPS];
        for (i, c) in colors.iter_mut().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "i < 256 is exact")]
            let v = i as f32 / 255.0;
            *c = Rgb { r: v, g: v, b: v };
        }
        ColorSteps::from_colors(colors, 255)
    }

    fn axis(extend: (bool, bool)) -> Axial {
        Axial {
            start: Point::new(0.0, 0.0),
            end: Point::new(8.0, 0.0),
            t_min: 0.0,
            t_max: 1.0,
            extend_start: extend.0,
            extend_end: extend.1,
        }
    }

    #[test]
    fn ramps_along_the_axis_by_pixel_corner() {
        let mut p = Pixmap::new(8, 1);
        draw(
            &mut p,
            &axis((true, true)),
            &black_to_white(),
            Affine::IDENTITY,
        );
        // Column 0 samples the corner at x = 0, so scale = 0 exactly.
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(0));
        // Column 4 samples x = 4, scale 0.5, index trunc(0.5*255) = 127.
        assert_eq!(p.pixel(4, 0).map(|px| px[0]), Some(127));
        // Column 7 is 7/8 along: trunc(0.875 * 255) = 223. Sampling the
        // corner rather than the centre is what puts it here.
        assert_eq!(p.pixel(7, 0).map(|px| px[0]), Some(223));
    }

    #[test]
    fn unextended_ends_leave_pixels_untouched() {
        let mut p = Pixmap::filled(12, 1, peniko::Color::from_rgba8(9, 9, 9, 255));
        // The matrix maps *shading* space to the bitmap, so translating by
        // -4 puts the axis's 0..8 at device columns 4..12 and leaves the
        // first four columns before its start.
        let shifted = Affine::translate((4.0, 0.0));
        draw(&mut p, &axis((false, false)), &black_to_white(), shifted);
        // Before the start: the background survives, it is not clamped.
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(9));
        // Inside, it is painted: column 8 is shading x = 4, scale 0.5.
        assert_eq!(p.pixel(8, 0).map(|px| px[0]), Some(127));
    }

    #[test]
    fn extended_ends_clamp_to_the_ramp() {
        let mut p = Pixmap::filled(12, 1, peniko::Color::from_rgba8(9, 9, 9, 255));
        draw(
            &mut p,
            &axis((true, true)),
            &black_to_white(),
            Affine::translate((4.0, 0.0)),
        );
        assert_eq!(
            p.pixel(0, 0).map(|px| px[0]),
            Some(0),
            "clamped to the ramp's start"
        );
        // Column 11 is shading x = 7, still inside the axis, so it ramps.
        assert_eq!(p.pixel(11, 0).map(|px| px[0]), Some(223));
        // A column genuinely past the end clamps to the ramp's last entry.
        let mut p = Pixmap::filled(20, 1, peniko::Color::from_rgba8(9, 9, 9, 255));
        draw(
            &mut p,
            &axis((true, true)),
            &black_to_white(),
            Affine::IDENTITY,
        );
        assert_eq!(
            p.pixel(19, 0).map(|px| px[0]),
            Some(255),
            "clamped to its end"
        );
    }

    #[test]
    fn a_degenerate_axis_paints_nothing_or_clamps() {
        // Coincident endpoints: axis_len_square is zero, so every scale is
        // an infinity or a NaN. Unextended, that paints nothing at all.
        let degenerate = Axial {
            end: Point::new(0.0, 0.0),
            extend_start: false,
            extend_end: false,
            ..axis((false, false))
        };
        let mut p = Pixmap::filled(4, 1, peniko::Color::from_rgba8(9, 9, 9, 255));
        draw(&mut p, &degenerate, &black_to_white(), Affine::IDENTITY);
        for col in 0..4 {
            assert_eq!(p.pixel(col, 0).map(|px| px[0]), Some(9));
        }
    }

    #[test]
    fn a_singular_matrix_is_declined() {
        let mut p = Pixmap::filled(4, 1, peniko::Color::from_rgba8(9, 9, 9, 255));
        draw(
            &mut p,
            &axis((true, true)),
            &black_to_white(),
            Affine::new([0.0; 6]),
        );
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(9));
    }
}
