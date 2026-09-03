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
///
/// The appearance builder reads the same key, so this is *its* type rather
/// than a second one that could fold `/MK /R` differently: routing and the
/// generator must agree about which box a click lands in.
pub use pdfrum_doc::geom::WidgetRotation as Rotation;

/// The mapping between a widget's page-space rectangle and its plate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Plate {
    /// The widget's rectangle in page space, normalized.
    pub(crate) rect: Rect,
    /// How the widget is rotated.
    pub(crate) rotation: Rotation,
}

impl Plate {
    /// The mapping for a widget, normalizing an inverted rectangle.
    ///
    /// A file may write a rectangle with its edges the wrong way round; the
    /// specification says to normalize, and a widget whose box is inverted
    /// still draws.
    pub(crate) fn new(rect: Rect, rotation: Rotation) -> Plate {
        Plate {
            rect: normalize(rect),
            rotation,
        }
    }

    /// Converts a page-space point into the widget's own upright box:
    /// **y-up**, origin at the plate's bottom-left.
    ///
    /// This is the space every appearance-stream query in `pdfrum-doc`
    /// consumes — `client_rect`, `place_at_point`, a list box's rows — and
    /// it is the inverse of the widget's own placement matrix:
    ///
    /// | rotation | page → PWL |
    /// |---|---|
    /// | 0° | `(x - left, y - bottom)` |
    /// | 90° | `(y - bottom, W - (x - left))` |
    /// | 180° | `(W - (x - left), H - (y - bottom))` |
    /// | 270° | `(H - (y - bottom), x - left)` |
    ///
    /// where `W`/`H` are the *widget's* width and height, before the quadrant
    /// swap — which is why they are not [`Plate::width`] and
    /// [`Plate::height`].
    ///
    /// Prefer this over [`Plate::to_plate`] for anything handed to a layout
    /// query: those flip y themselves, so passing them a y-down point flips
    /// it twice and lands every click on the wrong line.
    pub(crate) fn to_widget(self, at: Point) -> Point {
        // Offset into the widget's own box first, y-up.
        let dx = at.x - self.rect.left;
        let dy = at.y - self.rect.bottom;
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );

        // Undo the widget's rotation, landing in an upright box, still y-up.
        match self.rotation {
            Rotation::None => Point::new(dx, dy),
            Rotation::Quarter => Point::new(dy, w - dx),
            Rotation::Half => Point::new(w - dx, h - dy),
            Rotation::ThreeQuarter => Point::new(h - dy, dx),
        }
    }
}

/// The inverse half of the plate mapping, and the dimensions it needs.
///
/// [`Plate::to_widget`] is what routing calls; these are what its round-trip
/// and rotation-table tests check it against.
#[cfg(test)]
impl Plate {
    /// The plate's width — the widget's, with the axes exchanged for an odd
    /// quadrant.
    pub(crate) fn width(self) -> f32 {
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );
        if self.rotation.swaps_axes() { h } else { w }
    }

    /// The plate's height, with the same exchange.
    pub(crate) fn height(self) -> f32 {
        let (w, h) = (
            self.rect.right - self.rect.left,
            self.rect.top - self.rect.bottom,
        );
        if self.rotation.swaps_axes() { w } else { h }
    }

    /// Converts a page-space point into plate space: y-down from the plate's
    /// top-left.
    ///
    /// [`Plate::to_widget`] plus the y flip. Callers wanting to hand the
    /// result to a `vt` query want that one instead.
    pub(crate) fn to_plate(self, at: Point) -> Point {
        let upright = self.to_widget(at);
        Point::new(upright.x, self.height() - upright.y)
    }

    /// Converts a plate-space point back into page space.
    pub(crate) fn to_page(self, at: Point) -> Point {
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
pub(crate) fn normalize(rect: Rect) -> Rect {
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

    /// `to_widget` on all four quadrants, corner for corner.
    ///
    /// The expectations are not derived from the table in the doc comment:
    /// they were produced by compiling the oracle's placement matrix verbatim,
    /// inverting it, and transforming this widget's four corners. Writing the
    /// inverse by hand is exactly where a sign or an axis goes missing, and
    /// the resulting click lands on the wrong line with nothing to say so.
    #[test]
    fn to_widget_reproduces_the_four_inverse_matrices() {
        // The widget's four page-space corners, anticlockwise from its own
        // bottom-left.
        let corners = [
            Point::new(20.0, 30.0),
            Point::new(120.0, 30.0),
            Point::new(120.0, 80.0),
            Point::new(20.0, 80.0),
        ];
        let expected = [
            (
                Rotation::None,
                [(0.0, 0.0), (100.0, 0.0), (100.0, 50.0), (0.0, 50.0)],
            ),
            (
                Rotation::Quarter,
                [(0.0, 100.0), (0.0, 0.0), (50.0, 0.0), (50.0, 100.0)],
            ),
            (
                Rotation::Half,
                [(100.0, 50.0), (0.0, 50.0), (0.0, 0.0), (100.0, 0.0)],
            ),
            (
                Rotation::ThreeQuarter,
                [(50.0, 0.0), (50.0, 100.0), (0.0, 100.0), (0.0, 0.0)],
            ),
        ];

        for (rotation, wanted) in expected {
            let p = plate(rotation);
            for (corner, (x, y)) in corners.iter().zip(wanted) {
                assert!(
                    close(p.to_widget(*corner), Point::new(x, y)),
                    "{rotation:?} sends {corner:?} to {:?}, wanted ({x}, {y})",
                    p.to_widget(*corner)
                );
            }
        }
    }

    /// `to_plate` is `to_widget` plus the y flip, and nothing else — the
    /// property that lets a caller pick the one its consumer wants.
    #[test]
    fn a_plate_point_is_a_widget_point_flipped() {
        for rotation in [
            Rotation::None,
            Rotation::Quarter,
            Rotation::Half,
            Rotation::ThreeQuarter,
        ] {
            let p = plate(rotation);
            for at in [Point::new(20.0, 30.0), Point::new(70.0, 55.0)] {
                let upright = p.to_widget(at);
                assert!(close(
                    p.to_plate(at),
                    Point::new(upright.x, p.height() - upright.y)
                ));
            }
        }
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
