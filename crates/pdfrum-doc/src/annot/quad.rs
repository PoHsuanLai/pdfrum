//! `/QuadPoints`: eight numbers per quadrilateral, and the rectangle derived
//! from them.
//!
//! A quadrilateral is written `[x1 y1 x2 y2 x3 y3 x4 y4]` — nominally the
//! four corners in the order top-left, top-right, bottom-left, bottom-right.
//! The rectangle taken from it uses **(x3, y3) as left/bottom and (x2, y2) as
//! right/top**, so with the canonical corner ordering the result is a
//! well-formed rectangle, and with any other ordering it is whatever the
//! index arithmetic produces. Ported as arithmetic, not as an interpretation:
//! the corpus contains files that rely on the inverted result.

use kurbo::Rect;
use pdfrum_object::Array;

use crate::geom;

/// How many complete quadrilaterals an array holds; a trailing partial one is
/// ignored.
///
/// ```
/// use pdfrum_doc::annot::quad_point_count;
/// use pdfrum_object::Array;
///
/// let one = Array::of([0, 8, 10, 8, 0, 0, 10, 0]);
/// assert_eq!(quad_point_count(Some(&one)), 1);
/// // A trailing partial quadrilateral does not count.
/// let partial = Array::of([0, 8, 10, 8, 0]);
/// assert_eq!(quad_point_count(Some(&partial)), 0);
/// assert_eq!(quad_point_count(None), 0);
/// ```
#[must_use]
pub fn quad_point_count(array: Option<&Array>) -> usize {
    array.map_or(0, |a| a.len() / 8)
}

/// The rectangle for quadrilateral `index`, straight from the index
/// arithmetic and **not** normalized.
#[must_use]
pub(crate) fn rect_from_quad_points_array(array: &Array, index: usize) -> Rect {
    let base = index * 8;
    let at = |offset: usize| array.number_at_or_zero(base + offset);
    geom::rect(at(4), at(5), at(2), at(3))
}

/// The rectangle for quadrilateral `index`, or the zero rectangle when the
/// index is past the end. Deliberately not clamped to the last quadrilateral.
///
/// ```
/// use pdfrum_doc::annot::rect_from_quad_points;
/// use pdfrum_doc::geom;
/// use pdfrum_object::Array;
///
/// // `[x1 y1 x2 y2 x3 y3 x4 y4]`: (x3, y3) is left/bottom, (x2, y2) right/top.
/// let quad = Array::of([0, 8, 10, 8, 0, 0, 10, 0]);
/// assert_eq!(rect_from_quad_points(Some(&quad), 0), geom::rect(0.0, 0.0, 10.0, 8.0));
/// // Past the end is the zero rectangle, not the last quadrilateral.
/// assert_eq!(rect_from_quad_points(Some(&quad), 1), kurbo::Rect::ZERO);
/// ```
#[must_use]
pub fn rect_from_quad_points(array: Option<&Array>, index: usize) -> Rect {
    let array = array.filter(|a| index < a.len() / 8);
    array.map_or(Rect::ZERO, |a| rect_from_quad_points_array(a, index))
}

/// The bounding box of every quadrilateral.
///
/// With no quadrilaterals this is the zero rectangle. With exactly one it is
/// that quadrilateral's rectangle **unchanged** — including inverted, because
/// no union runs. Two or more normalize, because the union does.
#[must_use]
pub(crate) fn bounding_rect_from_quad_points(array: Option<&Array>) -> Rect {
    let count = quad_point_count(array);
    let Some(array) = array.filter(|_| count > 0) else {
        return Rect::ZERO;
    };
    let mut bounds = rect_from_quad_points_array(array, 0);
    for index in 1..count {
        bounds = geom::union(bounds, rect_from_quad_points_array(array, index));
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::{
        bounding_rect_from_quad_points, quad_point_count, rect_from_quad_points,
        rect_from_quad_points_array,
    };
    use crate::geom;
    use kurbo::Rect;
    use pdfrum_object::{Array, Object};

    /// `count` quadrilaterals whose coordinates run 0, 1, 2, … so the index
    /// arithmetic is readable straight off the assertion.
    fn quads(count: usize) -> Array {
        Array::of((0..count * 8).map(|n| Object::Int(i64::try_from(n).unwrap_or(0))))
    }

    #[test]
    fn the_derived_rectangle_is_index_arithmetic_not_a_normalization() {
        // With the values 0..7 the arithmetic gives left=4, bottom=5,
        // right=2, top=3 — an inverted rectangle that stays inverted.
        let array = quads(2);
        assert_eq!(
            rect_from_quad_points_array(&array, 0),
            geom::rect(4.0, 5.0, 2.0, 3.0)
        );
        assert_eq!(
            rect_from_quad_points_array(&array, 1),
            geom::rect(12.0, 13.0, 10.0, 11.0)
        );
    }

    #[test]
    fn one_quadrilateral_is_never_normalized_but_two_are() {
        let one = quads(1);
        assert_eq!(
            bounding_rect_from_quad_points(Some(&one)),
            geom::rect(4.0, 5.0, 2.0, 3.0)
        );
        let three = quads(3);
        assert_eq!(
            bounding_rect_from_quad_points(Some(&three)),
            geom::rect(2.0, 3.0, 20.0, 21.0)
        );
    }

    #[test]
    fn no_quadrilaterals_gives_the_zero_rectangle() {
        assert_eq!(bounding_rect_from_quad_points(None), Rect::ZERO);
        assert_eq!(
            bounding_rect_from_quad_points(Some(&Array::of([Object::Int(1), Object::Int(2)]))),
            Rect::ZERO
        );
    }

    #[test]
    fn an_out_of_range_index_gives_zero_rather_than_the_last_quadrilateral() {
        let array = quads(2);
        assert_eq!(rect_from_quad_points(Some(&array), 2), Rect::ZERO);
        assert_eq!(
            rect_from_quad_points(Some(&array), 1),
            geom::rect(12.0, 13.0, 10.0, 11.0)
        );
    }

    #[test]
    fn the_count_ignores_a_trailing_partial_quadrilateral() {
        for len in 0..8 {
            let array = Array::of((0..len).map(Object::Int));
            assert_eq!(quad_point_count(Some(&array)), 0, "len {len}");
        }
        for len in 8..16 {
            let array = Array::of((0..len).map(Object::Int));
            assert_eq!(quad_point_count(Some(&array)), 1, "len {len}");
        }
        let array = Array::of((0..65).map(Object::Int));
        assert_eq!(quad_point_count(Some(&array)), 8);
    }
}
