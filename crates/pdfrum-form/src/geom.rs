//! Page space to plate space, and back.
//!
//! # Two spaces, one transform
//!
//! Events arrive in **page space** — PDF user space, y-up, origin at the
//! page's crop box. The text engine works in **plate space** — y-down from
//! the client rectangle's top-left. Everything between them is one affine,
//! computed once when a field is first touched, and its inverse.
//!
//! The C++ has four spaces rather than two, and the extra one exists only
//! because its edit control is a widget-toolkit window that wants its own
//! origin. It also computes the rotation twice, in two places that disagree:
//! one subtracts the rectangle's edges directly, which is right only for an
//! already-normalized rectangle, and the other calls accessors that normalize;
//! and its rotation accessor can return a negative number that one caller
//! takes the absolute value of and the other does not. Here the rotation is
//! normalized into `[0, 360)` once, at the point the plate rectangle is
//! computed, and there is one transform.
//!
//! # The quadrant swap
//!
//! At 90° and 270° the plate's width and height exchange places, because the
//! text is set across what is now the tall axis. The test is on the quadrant
//! number, not on the angle, which is why a rotation of 450° behaves as 90°
//! once normalized.

use crate::event::Point;
use crate::tab::Rect;

/// A widget's rotation, normalized to one of the four quadrants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    /// Upright.
    #[default]
    None,
    /// A quarter turn counter-clockwise.
    Quarter,
    /// A half turn.
    Half,
    /// Three quarters counter-clockwise.
    ThreeQuarter,
}

impl Rotation {
    /// Normalizes a rotation in degrees, from any integer, into a quadrant.
    ///
    /// A negative or out-of-range angle is folded into `[0, 360)` first, and
    /// an angle that is not a multiple of ninety degrees rounds **down** to
    /// the quadrant containing it — matching the oracle's integer division.
    #[must_use]
    pub fn from_degrees(degrees: i32) -> Rotation {
        let normalized = degrees.rem_euclid(360);
        match normalized / 90 {
            1 => Rotation::Quarter,
            2 => Rotation::Half,
            3 => Rotation::ThreeQuarter,
            _ => Rotation::None,
        }
    }

    /// Whether this rotation exchanges the plate's width and height.
    #[must_use]
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Quarter | Rotation::ThreeQuarter)
    }

    /// The rotation in degrees.
    #[must_use]
    pub fn degrees(self) -> i32 {
        match self {
            Rotation::None => 0,
            Rotation::Quarter => 90,
            Rotation::Half => 180,
            Rotation::ThreeQuarter => 270,
        }
    }
}

/// The mapping between a widget's page-space rectangle and its plate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plate {
    /// The widget's rectangle in page space, normalized.
    pub rect: Rect,
    /// How the widget is rotated.
    pub rotation: Rotation,
}

impl Plate {
    /// The mapping for a widget, normalizing an inverted rectangle.
    ///
    /// A file may write a rectangle with its edges the wrong way round; the
    /// specification says to normalize, and a widget whose box is inverted
    /// still draws.
    #[must_use]
    pub fn new(rect: Rect, rotation: Rotation) -> Plate {
        Plate {
            rect: normalize(rect),
            rotation,
        }
    }

    /// The plate's width — the widget's, with the axes exchanged for an odd
    /// quadrant.
    #[must_use]
    pub fn width(self) -> f32 {
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );
        if self.rotation.swaps_axes() { h } else { w }
    }

    /// The plate's height, with the same exchange.
    #[must_use]
    pub fn height(self) -> f32 {
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );
        if self.rotation.swaps_axes() { w } else { h }
    }

    /// Converts a page-space point into plate space: y-down from the plate's
    /// top-left.
    #[must_use]
    pub fn to_plate(self, at: Point) -> Point {
        // Offset into the widget's own box first, y-up.
        let dx = at.x - self.rect.left;
        let dy = at.y - self.rect.bottom;
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );

        // Undo the widget's rotation, landing in an upright box, still y-up.
        let (ux, uy) = match self.rotation {
            Rotation::None => (dx, dy),
            Rotation::Quarter => (dy, w - dx),
            Rotation::Half => (w - dx, h - dy),
            Rotation::ThreeQuarter => (h - dy, dx),
        };

        // Flip to y-down from the top-left of the (possibly swapped) plate.
        Point::new(ux, self.height() - uy)
    }

    /// Converts a plate-space point back into page space.
    #[must_use]
    pub fn to_page(self, at: Point) -> Point {
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );

        // Undo the y-down flip.
        let ux = at.x;
        let uy = self.height() - at.y;

        // Re-apply the widget's rotation.
        let (dx, dy) = match self.rotation {
            Rotation::None => (ux, uy),
            Rotation::Quarter => (w - uy, ux),
            Rotation::Half => (w - ux, h - uy),
            Rotation::ThreeQuarter => (uy, h - ux),
        };

        Point::new(self.rect.left + dx, self.rect.bottom + dy)
    }
}

/// Puts a rectangle's edges the right way round.
#[must_use]
pub fn normalize(rect: Rect) -> Rect {
    Rect::new(
        rect.left.min(rect.right),
        rect.bottom.min(rect.top),
        rect.left.max(rect.right),
        rect.bottom.max(rect.top),
    )
}

#[cfg(test)]
mod tests {
    // The dimensions asserted below are differences of exactly-representable
    // values, so they are exact; the round-trip tests, where rounding is
    // real, compare with a tolerance instead.
    #![allow(clippy::float_cmp)]

    use super::*;

    fn plate(rotation: Rotation) -> Plate {
        // A 100 x 50 widget whose origin is not the page's.
        Plate::new(Rect::new(20.0, 30.0, 120.0, 80.0), rotation)
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    }

    #[test]
    fn a_rotation_normalizes_into_a_quadrant() {
        assert_eq!(Rotation::from_degrees(0), Rotation::None);
        assert_eq!(Rotation::from_degrees(90), Rotation::Quarter);
        assert_eq!(Rotation::from_degrees(180), Rotation::Half);
        assert_eq!(Rotation::from_degrees(270), Rotation::ThreeQuarter);
        assert_eq!(Rotation::from_degrees(360), Rotation::None);
        assert_eq!(Rotation::from_degrees(450), Rotation::Quarter);
    }

    /// A negative angle folds forward rather than truncating toward zero,
    /// which is the difference between a quarter turn and no turn at all.
    #[test]
    fn a_negative_rotation_folds_forward() {
        assert_eq!(Rotation::from_degrees(-90), Rotation::ThreeQuarter);
        assert_eq!(Rotation::from_degrees(-180), Rotation::Half);
        assert_eq!(Rotation::from_degrees(-270), Rotation::Quarter);
        assert_eq!(Rotation::from_degrees(-360), Rotation::None);
    }

    #[test]
    fn only_the_odd_quadrants_exchange_the_axes() {
        assert!(!Rotation::None.swaps_axes());
        assert!(Rotation::Quarter.swaps_axes());
        assert!(!Rotation::Half.swaps_axes());
        assert!(Rotation::ThreeQuarter.swaps_axes());

        assert_eq!(plate(Rotation::None).width(), 100.0);
        assert_eq!(plate(Rotation::None).height(), 50.0);
        assert_eq!(plate(Rotation::Quarter).width(), 50.0);
        assert_eq!(plate(Rotation::Quarter).height(), 100.0);
    }

    /// An upright widget's plate is its box, flipped to y-down and moved to
    /// the origin.
    #[test]
    fn an_upright_plate_is_a_translation_and_a_flip() {
        let p = plate(Rotation::None);
        // The widget's top-left corner is the plate's origin.
        assert!(close(
            p.to_plate(Point::new(20.0, 80.0)),
            Point::new(0.0, 0.0)
        ));
        // Its bottom-right is the far corner, y-down.
        assert!(close(
            p.to_plate(Point::new(120.0, 30.0)),
            Point::new(100.0, 50.0)
        ));
    }

    /// Whatever the rotation, converting out and back is the identity.
    #[test]
    fn the_two_directions_are_inverses() {
        for rotation in [
            Rotation::None,
            Rotation::Quarter,
            Rotation::Half,
            Rotation::ThreeQuarter,
        ] {
            let p = plate(rotation);
            for (x, y) in [
                (20.0, 30.0),
                (120.0, 80.0),
                (70.0, 55.0),
                (25.5, 77.25),
                (119.0, 31.0),
            ] {
                let page = Point::new(x, y);
                let round_tripped = p.to_page(p.to_plate(page));
                assert!(
                    close(page, round_tripped),
                    "{rotation:?}: {page:?} became {round_tripped:?}"
                );
            }
        }
    }

    /// Every corner of the widget maps into the plate's own box, for every
    /// rotation — which is what makes a click anywhere in the widget land on
    /// a character rather than outside the text.
    #[test]
    fn every_corner_lands_inside_the_plate() {
        for rotation in [
            Rotation::None,
            Rotation::Quarter,
            Rotation::Half,
            Rotation::ThreeQuarter,
        ] {
            let p = plate(rotation);
            for (x, y) in [(20.0, 30.0), (120.0, 30.0), (20.0, 80.0), (120.0, 80.0)] {
                let at = p.to_plate(Point::new(x, y));
                assert!(
                    at.x >= -1e-3 && at.x <= p.width() + 1e-3,
                    "{rotation:?}: x {} outside 0..{}",
                    at.x,
                    p.width()
                );
                assert!(
                    at.y >= -1e-3 && at.y <= p.height() + 1e-3,
                    "{rotation:?}: y {} outside 0..{}",
                    at.y,
                    p.height()
                );
            }
        }
    }

    /// A file may write a rectangle inside out; the widget still draws, and
    /// the plate is built from the normalized box.
    #[test]
    fn an_inverted_rectangle_is_normalized() {
        // The shape of a real corpus fixture: top below bottom.
        let inverted = Rect::new(100.0, 100.0, 200.0, -130.0);
        let p = Plate::new(inverted, Rotation::None);

        assert_eq!(p.rect.left, 100.0);
        assert_eq!(p.rect.right, 200.0);
        assert_eq!(p.rect.bottom, -130.0);
        assert_eq!(p.rect.top, 100.0);
        assert_eq!(p.width(), 100.0);
        assert_eq!(p.height(), 230.0);
    }

    /// A degenerate box does not panic and does not produce infinities.
    #[test]
    fn a_zero_sized_plate_is_finite() {
        let p = Plate::new(Rect::new(50.0, 50.0, 50.0, 50.0), Rotation::None);
        assert_eq!(p.width(), 0.0);
        assert_eq!(p.height(), 0.0);

        let at = p.to_plate(Point::new(50.0, 50.0));
        assert!(at.x.is_finite() && at.y.is_finite());
    }

    /// The quarter turn puts the widget's bottom-left corner at the plate's
    /// top-left, which is the corner the text starts from once rotated.
    #[test]
    fn a_quarter_turn_moves_the_origin_corner() {
        let p = plate(Rotation::Quarter);
        assert!(close(
            p.to_plate(Point::new(20.0, 30.0)),
            Point::new(0.0, 0.0)
        ));
    }
}
