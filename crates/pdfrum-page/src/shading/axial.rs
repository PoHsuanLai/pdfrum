//! Type 2: axial shadings (ISO 32000-1 §8.7.4.5.3).
//!
//! A linear gradient between two points. The parametric position of a point
//! is its projection onto the axis, normalized by the axis's squared length —
//! which is why a **zero-length axis** divides by zero. The C++ leaves that
//! unguarded and then casts the resulting infinity or NaN to an integer,
//! which is undefined behaviour; we treat it as "every pixel is out of range
//! at the start end", the branch x86 actually lands in, and record a
//! diagnostic.

use super::{read_domain, read_extend};
use crate::names;
use kurbo::Point;
use pdfrum_object::{Dict, Resolve};

/// A type 2 shading's geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axial {
    /// The axis's start point.
    pub start: Point,
    /// The axis's end point.
    pub end: Point,
    /// `/Domain`'s low bound.
    pub t_min: f32,
    /// `/Domain`'s high bound.
    pub t_max: f32,
    /// Whether the gradient continues past the start point.
    pub extend_start: bool,
    /// Whether it continues past the end point.
    pub extend_end: bool,
}

impl Axial {
    /// Load from a shading dictionary.
    ///
    /// `/Coords` is required, but its **length is not checked**: a short
    /// array simply reads zeros for what it does not state.
    pub(super) fn load(dict: &Dict, r: &impl Resolve) -> Option<Self> {
        let coords = dict.array(names::COORDS, r)?;
        let at = |i: usize| f64::from(coords.number_at_or_zero(i));
        let (t_min, t_max) = read_domain(dict, r);
        let (extend_start, extend_end) = read_extend(dict, r);
        Some(Self {
            start: Point::new(at(0), at(1)),
            end: Point::new(at(2), at(3)),
            t_min,
            t_max,
            extend_start,
            extend_end,
        })
    }

    /// The squared axis length, which is the normalizing divisor.
    ///
    /// Zero for a degenerate axis, which is the case the caller must handle.
    #[must_use]
    pub fn axis_len_squared(&self) -> f64 {
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        dx * dx + dy * dy
    }

    /// The parametric position of a point in the shading's own space.
    ///
    /// `None` for a degenerate axis, where the C++ divides by zero. A caller
    /// treating that as "out of range at the start" reproduces what the
    /// undefined cast produces in practice.
    #[must_use]
    pub fn position(&self, p: Point) -> Option<f32> {
        let len_sq = self.axis_len_squared();
        if len_sq == 0.0 {
            return None;
        }
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        let scale = ((p.x - self.start.x) * dx + (p.y - self.start.y) * dy) / len_sq;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the shading LUT indexes with f32 throughout, matching the C++"
        )]
        Some(scale as f32)
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

    use super::Axial;
    use kurbo::Point;

    fn horizontal() -> Axial {
        Axial {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
            t_min: 0.0,
            t_max: 1.0,
            extend_start: false,
            extend_end: false,
        }
    }

    #[test]
    fn the_position_is_the_normalized_projection() {
        let a = horizontal();
        assert!(
            a.position(Point::new(0.0, 0.0))
                .is_some_and(|v| v.abs() < 1e-6)
        );
        assert!(
            a.position(Point::new(5.0, 99.0))
                .is_some_and(|v| (v - 0.5).abs() < 1e-6),
            "the off-axis component does not matter"
        );
        assert!(
            a.position(Point::new(10.0, 0.0))
                .is_some_and(|v| (v - 1.0).abs() < 1e-6)
        );
        // Beyond the ends the position simply keeps going.
        assert!(a.position(Point::new(20.0, 0.0)).is_some_and(|v| v > 1.9));
        assert!(a.position(Point::new(-10.0, 0.0)).is_some_and(|v| v < -0.9));
    }

    #[test]
    fn a_degenerate_axis_has_no_position() {
        let a = Axial {
            start: Point::new(3.0, 4.0),
            end: Point::new(3.0, 4.0),
            ..horizontal()
        };
        assert_eq!(a.axis_len_squared(), 0.0);
        assert!(a.position(Point::new(0.0, 0.0)).is_none());
    }
}
