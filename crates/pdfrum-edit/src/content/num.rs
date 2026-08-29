//! Number spelling inside content streams (ISO 32000 §7.3.3).
//!
//! Four writers, each producing exactly the digits and nothing else: **no
//! leading space, no trailing space, no `+` on positives**. Every caller
//! supplies its own separator, which is why the operator emitters below read
//! `write_float(out, w); out.push_str(" w ")` rather than composing padded
//! fragments.
//!
//! The digits themselves come from [`pdfrum_object::fmt_number`], the
//! shortest-round-trip spelling the whole workspace shares; these functions
//! add only the layout.

use pdfrum_common::kurbo::{Affine, Point, Rect};
use pdfrum_object::fmt_number;

/// Append one number: `10.5`, `.29`, `-7`, `0`.
///
/// ```
/// let mut out = String::new();
/// pdfrum_edit::write_float(&mut out, 0.5);
/// assert_eq!(out, ".5");
/// ```
pub fn write_float(out: &mut String, value: f32) {
    out.push_str(&fmt_number(value));
}

/// Append a matrix as `a b c d e f` — single spaces, no brackets, no trailing
/// space. This is the operand list of `cm` and `Tm`.
///
/// ```
/// use pdfrum_common::kurbo::Affine;
///
/// let mut out = String::new();
/// pdfrum_edit::write_matrix(&mut out, Affine::new([1.0, 0.0, 0.0, 1.0, 10.5, 20.25]));
/// assert_eq!(out, "1 0 0 1 10.5 20.25");
/// ```
pub fn write_matrix(out: &mut String, m: Affine) {
    let c = m.as_coeffs();
    for (i, v) in c.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "PDF numbers are f32 (SPEC §2); the geometry vocabulary is f64"
        )]
        write_float(out, *v as f32);
    }
}

/// Append a point as `x y`.
///
/// ```
/// use pdfrum_common::kurbo::Point;
///
/// let mut out = String::new();
/// pdfrum_edit::write_point(&mut out, Point::new(1.0, 2.5));
/// assert_eq!(out, "1 2.5");
/// ```
pub fn write_point(out: &mut String, p: Point) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "PDF numbers are f32 (SPEC §2); the geometry vocabulary is f64"
    )]
    {
        write_float(out, p.x as f32);
        out.push(' ');
        write_float(out, p.y as f32);
    }
}

/// Append a rectangle as `left bottom width height` — **not** the
/// `x0 y0 x1 y1` an array-valued `/MediaBox` uses. This is the operand list of
/// `re`, so the last two numbers are extents and may be negative.
///
/// ```
/// use pdfrum_common::kurbo::Rect;
///
/// let mut out = String::new();
/// pdfrum_edit::write_rect(&mut out, Rect::new(1.0, 2.0, 10.0, 20.0));
/// assert_eq!(out, "1 2 9 18");
/// ```
pub fn write_rect(out: &mut String, r: Rect) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "PDF numbers are f32 (SPEC §2); the geometry vocabulary is f64"
    )]
    {
        write_float(out, r.x0 as f32);
        out.push(' ');
        write_float(out, r.y0 as f32);
        out.push(' ');
        write_float(out, (r.x1 - r.x0) as f32);
        out.push(' ');
        write_float(out, (r.y1 - r.y0) as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::{write_float, write_matrix, write_point, write_rect};
    use pdfrum_common::kurbo::{Affine, Point, Rect};

    fn s(f: impl FnOnce(&mut String)) -> String {
        let mut out = String::new();
        f(&mut out);
        out
    }

    // From cpdf_contentstream_write_utils_unittest.cpp:26-69, restated for the
    // no-space contract: these writers add layout, never padding.
    #[test]
    fn floats_carry_no_space_and_no_sign() {
        assert_eq!(s(|o| write_float(o, 0.0)), "0");
        assert_eq!(s(|o| write_float(o, 1.0)), "1");
        assert_eq!(s(|o| write_float(o, -1.0)), "-1");
        assert_eq!(s(|o| write_float(o, 0.5)), ".5");
        assert_eq!(s(|o| write_float(o, -0.5)), "-.5");
        assert_eq!(s(|o| write_float(o, 10.5)), "10.5");
    }

    #[test]
    fn matrix_is_six_space_separated_numbers() {
        assert_eq!(
            s(|o| write_matrix(o, Affine::new([1.0, 0.0, 0.0, 1.0, 10.5, 20.25]))),
            "1 0 0 1 10.5 20.25"
        );
    }

    #[test]
    fn point_is_two_numbers() {
        assert_eq!(s(|o| write_point(o, Point::new(1.0, 2.5))), "1 2.5");
    }

    // The one that surprises: WriteRect emits extents, not corners.
    #[test]
    fn rect_is_left_bottom_width_height() {
        assert_eq!(
            s(|o| write_rect(o, Rect::new(1.0, 2.0, 10.0, 20.0))),
            "1 2 9 18"
        );
    }

    // The N-up golden (cpdf_npagetooneexporter_unittest.cpp:10-19) is as much
    // a number test as a layout one: leading zeros go, tiny and huge values
    // stay positional.
    #[test]
    fn extreme_magnitudes_stay_positional() {
        assert_eq!(s(|o| write_float(o, 0.000_001)), ".000001");
        assert_eq!(s(|o| write_float(o, 1_000_000_000_000.0)), "1000000000000");
        assert_eq!(s(|o| write_float(o, 0.5)), ".5");
    }
}
