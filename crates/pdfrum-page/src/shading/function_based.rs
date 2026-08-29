//! Type 1: function-based shadings (ISO 32000-1 §8.7.4.5.2).
//!
//! A two-input function evaluated over a rectangle of the shading's own
//! coordinate space. Two things differ from every other type:
//!
//! - **`/Domain` is `[xmin xmax ymin ymax]`**, not `[xmin ymin xmax ymax]`.
//!   The pairing is per axis, not per corner.
//! - **There is no colour lookup table.** The function is evaluated per pixel
//!   and its result truncated to bytes, where axial and radial round through
//!   their 256-entry ramp. On a smooth gradient the two differ by one level.
//!
//! There is no `/Extend` for this type: outside the domain box the pixel is
//! left untouched.

use crate::names;
use kurbo::{Affine, Point};
use pdfrum_object::{Dict, Resolve};

/// A type 1 shading's geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FunctionBased {
    /// The domain rectangle, `[xmin, xmax, ymin, ymax]` — note the ordering.
    pub domain: [f32; 4],
    /// The shading's own `/Matrix`, mapping the domain into the shading's
    /// space. Distinct from a pattern's `/Matrix`.
    pub matrix: Affine,
}

impl FunctionBased {
    /// Load from a shading dictionary.
    pub(super) fn load(dict: &Dict, r: &impl Resolve) -> Self {
        let domain = match dict.array(names::DOMAIN, r) {
            Some(a) => [
                a.number_at_or_zero(0),
                a.number_at_or_zero(1),
                a.number_at_or_zero(2),
                a.number_at_or_zero(3),
            ],
            None => [0.0, 1.0, 0.0, 1.0],
        };
        Self {
            domain,
            matrix: dict.matrix(names::MATRIX, r),
        }
    }

    /// Whether a point in the domain's coordinate space is inside it.
    ///
    /// The test is **inclusive** at both ends.
    #[must_use]
    pub fn contains(&self, p: Point) -> bool {
        let x = p.x;
        let y = p.y;
        x >= f64::from(self.domain[0])
            && x <= f64::from(self.domain[1])
            && y >= f64::from(self.domain[2])
            && y <= f64::from(self.domain[3])
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

    use super::FunctionBased;
    use kurbo::{Affine, Point};

    #[test]
    fn the_domain_pairs_per_axis_not_per_corner() {
        let s = FunctionBased {
            // x spans 0..10 and y spans 100..200.
            domain: [0.0, 10.0, 100.0, 200.0],
            matrix: Affine::IDENTITY,
        };
        assert!(s.contains(Point::new(5.0, 150.0)));
        // Reading the array as `[xmin ymin xmax ymax]` would put this inside.
        assert!(!s.contains(Point::new(5.0, 50.0)));
        // The bounds are inclusive.
        assert!(s.contains(Point::new(0.0, 100.0)));
        assert!(s.contains(Point::new(10.0, 200.0)));
        assert!(!s.contains(Point::new(10.001, 200.0)));
    }
}
