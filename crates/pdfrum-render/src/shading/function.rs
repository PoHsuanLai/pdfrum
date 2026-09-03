//! Type 1, function-based shadings.
//!
//! The only rasterizer with no ramp: the functions are evaluated per pixel,
//! at the pixel's corner, and the result is converted to bytes by
//! **truncation** where [`ColorSteps`](super::steps::ColorSteps) rounds — the
//! same conversion written two ways upstream, and the difference shows on
//! `2_shading_type1`.
//!
//! Note also the `/Domain` order: `[xmin, xmax, ymin, ymax]`, paired per axis
//! rather than per corner, and the bounds test is inclusive at both ends.

use kurbo::{Affine, Point};
use pdfrum_page::FunctionBased;
use pdfrum_page::Shading;

use crate::color::Argb;
use crate::pixmap::Pixmap;

/// Rasterize a function-based shading into `dest`.
pub fn draw(
    dest: &mut Pixmap,
    shading: &Shading,
    geometry: &FunctionBased,
    alpha: u8,
    to_bitmap: Affine,
) {
    // `object_to_bitmap.inverse() * dict_matrix.inverse()`: the composed
    // inverse takes a destination pixel back into the function's domain.
    let combined = to_bitmap * geometry.matrix;
    let determinant = combined.determinant();
    if determinant == 0.0 || !determinant.is_finite() {
        return;
    }
    let inverse = combined.inverse();

    let inputs = shading
        .functions
        .iter()
        .map(|f| f.output_count())
        .sum::<usize>();
    if inputs == 0 && shading.space.n_components() == 0 {
        return;
    }
    let mut outputs = vec![0.0f32; inputs.max(shading.space.n_components()).max(1)];

    for row in 0..dest.height() {
        for col in 0..dest.width() {
            let pos = inverse * Point::new(f64::from(col), f64::from(row));
            if !geometry.contains(pos) {
                continue;
            }
            // Multiple functions write into successive slices of one buffer,
            // which is *not* cleared between pixels: a function that declines
            // to evaluate leaves the previous pixel's outputs in place.
            let mut written = 0usize;
            for f in &shading.functions {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "shading space is f32 throughout, matching the oracle"
                )]
                let input = [pos.x as f32, pos.y as f32];
                let Some(slice) = outputs.get_mut(written..) else {
                    break;
                };
                // One or two inputs: a function declaring more than the two
                // this shading type supplies is truncated, as upstream does.
                let Some(used) = input.get(..f.input_count().clamp(1, 2)) else {
                    break;
                };
                let n = f.eval_into(used, slice);
                written = written.saturating_add(n);
            }
            let rgb = shading.space.to_rgb(&outputs);
            // Truncating, where the ramp rounds.
            let [r, g, b] = rgb.to_bytes_truncating();
            let color = Argb { a: alpha, r, g, b };
            dest.set_pixel(col, row, crate::pixmap::premultiply(color.to_peniko()));
        }
    }
}

#[cfg(test)]
mod tests {
    use pdfrum_page::Rgb;

    use super::*;

    #[test]
    fn domain_is_paired_per_axis_and_inclusive() {
        let g = FunctionBased {
            domain: [0.0, 10.0, 2.0, 4.0],
            matrix: Affine::IDENTITY,
        };
        assert!(g.contains(Point::new(0.0, 2.0)), "inclusive at the low end");
        assert!(
            g.contains(Point::new(10.0, 4.0)),
            "inclusive at the high end"
        );
        assert!(!g.contains(Point::new(10.1, 3.0)));
        assert!(
            !g.contains(Point::new(5.0, 1.9)),
            "domain[2..4] is the y range"
        );
    }

    #[test]
    fn truncating_conversion_differs_from_the_ramp_rounding() {
        // The one arithmetic difference between this rasterizer and the ramp.
        let c = Rgb {
            r: 0.5,
            g: 0.5,
            b: 0.5,
        };
        assert_eq!(c.to_bytes(), [128, 128, 128]);
        assert_eq!(c.to_bytes_truncating(), [127, 127, 127]);
    }

    #[test]
    fn a_singular_matrix_is_declined() {
        let mut p = Pixmap::filled(2, 2, peniko::Color::from_rgba8(9, 9, 9, 255));
        let shading = Shading {
            geometry: pdfrum_page::Geometry::FunctionBased(FunctionBased {
                domain: [0.0, 1.0, 0.0, 1.0],
                matrix: Affine::IDENTITY,
            }),
            space: std::sync::Arc::new(pdfrum_page::ColorSpace::DeviceRgb),
            functions: Box::new([]),
            background: None,
            bbox: None,
        };
        let g = FunctionBased {
            domain: [0.0, 1.0, 0.0, 1.0],
            matrix: Affine::new([0.0; 6]),
        };
        draw(&mut p, &shading, &g, 255, Affine::IDENTITY);
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(9));
    }
}
