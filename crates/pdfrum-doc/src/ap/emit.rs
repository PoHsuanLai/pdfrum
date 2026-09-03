//! Writing content-stream bytes.
//!
//! A thin `String` sink whose only job is to make the **choice of float
//! writer explicit at every call site**. There is no default: a caller says
//! [`Float::Shortest`] or [`Float::G6`] every time, because upstream's choice
//! varies per emitter with no rule behind it — the newer generators write
//! shortest-round-trip, the older per-subtype markup ones stream through a
//! C++ `ostream` — and a silent default would quietly pick the wrong one.

use std::fmt::Write as _;

use kurbo::Rect;

use crate::ap::fmt;
use crate::color::Color;
use crate::geom;

/// Which float writer a value goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    /// Shortest round-trip, fixed notation, no leading zero: `.5`.
    Shortest,
    /// Six significant digits with scientific notation past the ends: `0.5`.
    G6,
}

impl Float {
    /// Formats one value.
    #[must_use]
    pub fn write(self, value: f32) -> String {
        match self {
            Float::Shortest => fmt::shortest(value),
            Float::G6 => fmt::g6(value),
        }
    }
}

/// Whether a colour paints the interior or the outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintOp {
    /// Non-stroking: `g`, `rg`, `k`.
    Fill,
    /// Stroking: `G`, `RG`, `K`.
    Stroke,
}

/// A content-stream sink.
#[derive(Debug, Clone, Default)]
pub struct Content {
    bytes: String,
}

impl Content {
    /// An empty stream.
    #[must_use]
    pub fn new() -> Content {
        Content::default()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The bytes written so far.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.bytes
    }

    /// Consumes the sink for its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes.into_bytes()
    }

    /// Appends a literal.
    pub fn raw(&mut self, text: &str) {
        self.bytes.push_str(text);
    }

    /// Appends one number, followed by a single space.
    pub fn num(&mut self, value: f32, how: Float) {
        self.bytes.push_str(&how.write(value));
        self.bytes.push(' ');
    }

    /// Appends `x y `, both through the same writer.
    pub fn point(&mut self, x: f32, y: f32, how: Float) {
        self.num(x, how);
        self.num(y, how);
    }

    /// Appends a rectangle as `left bottom width height ` — the operand order
    /// a `re` operator wants, **not** the corner order a `/Rect` array holds.
    /// The last two can be negative.
    pub fn rect(&mut self, r: Rect, how: Float) {
        self.num(geom::left(r), how);
        self.num(geom::bottom(r), how);
        self.num(geom::width(r), how);
        self.num(geom::height(r), how);
    }
}

/// A colour-setting operator, or the empty string.
///
/// The empty return for a transparent colour is load-bearing: every caller
/// tests it to decide whether to emit the surrounding path at all, so
/// "no colour" and "black" produce very different streams.
#[must_use]
pub fn color_op(color: Color, paint: PaintOp) -> String {
    color_op_via(color, paint, Float::Shortest)
}

/// The same operator through a named float writer.
///
/// The choice is not cosmetic and it is not per-colour: it is per **producer**.
/// The shape and border generators write their components through the
/// shortest-round-trip writer; the per-subtype markup generators and the
/// form-control chrome write theirs at six significant digits. A colour whose
/// components are integers reads the same either way — which is why every
/// widget-chrome colour in the corpus was indifferent to it
/// — but a byte value divided by 255 is not: `51/255` is `.2` through one and
/// `0.2` through the other, and `113/255` is `.443137254` against `0.443137`.
#[must_use]
pub fn color_op_via(color: Color, paint: PaintOp, how: Float) -> String {
    let mut out = String::new();
    let (gray, rgb, cmyk) = match paint {
        PaintOp::Fill => ("g", "rg", "k"),
        PaintOp::Stroke => ("G", "RG", "K"),
    };
    let mut write = |values: &[f32], op: &str| {
        for value in values {
            let _ = write!(out, "{} ", how.write(*value));
        }
        let _ = writeln!(out, "{op}");
    };
    match color {
        Color::Transparent => {}
        Color::Gray(g) => write(&[g], gray),
        Color::Rgb(r, g, b) => write(&[r, g, b], rgb),
        Color::Cmyk(c, m, y, k) => write(&[c, m, y, k], cmyk),
    }
    out
}

/// A colour read from an array, or the caller's default when the key is
/// absent.
///
/// A **present** array always wins, even one of two or five elements that
/// names no space at all — so `/C []` says "paint nothing" while an absent
/// `/C` takes the subtype's default.
#[must_use]
pub fn color_with_default(
    array: Option<&pdfrum_object::Array>,
    default: Color,
    paint: PaintOp,
) -> String {
    match array {
        Some(array) => color_op(Color::from_array(array), paint),
        None => color_op(default, paint),
    }
}

/// The path-painting operator for a given stroke/fill combination.
#[must_use]
pub fn paint_operator(stroke: bool, fill: bool) -> &'static str {
    match (stroke, fill) {
        (true, true) => "b",
        (true, false) => "s",
        (false, true) => "f",
        (false, false) => "n",
    }
}

#[cfg(test)]
mod tests {
    use super::{Content, Float, PaintOp, color_op, color_with_default, paint_operator};
    use crate::color::Color;
    use crate::geom;
    use pdfrum_object::{Array, Object};

    #[test]
    fn a_rectangle_is_written_as_operator_operands_not_as_corners() {
        let mut out = Content::new();
        out.rect(geom::rect(10.0, 20.0, 30.0, 25.0), Float::Shortest);
        out.raw("re\n");
        // left, bottom, width, height — the last two are extents.
        assert_eq!(out.as_str(), "10 20 20 5 re\n");
    }

    #[test]
    fn a_negative_extent_survives_into_the_operands() {
        let mut out = Content::new();
        out.rect(geom::rect(30.0, 20.0, 10.0, 25.0), Float::Shortest);
        assert_eq!(out.as_str(), "30 20 -20 5 ");
    }

    #[test]
    fn the_two_writers_produce_different_bytes_for_the_same_number() {
        let mut shortest = Content::new();
        shortest.num(0.5, Float::Shortest);
        let mut ostream = Content::new();
        ostream.num(0.5, Float::G6);
        assert_eq!(shortest.as_str(), ".5 ");
        assert_eq!(ostream.as_str(), "0.5 ");
    }

    #[test]
    fn a_transparent_colour_writes_nothing_at_all() {
        assert_eq!(color_op(Color::Transparent, PaintOp::Fill), "");
        assert_eq!(color_op(Color::Transparent, PaintOp::Stroke), "");
    }

    #[test]
    fn each_space_writes_its_own_operator_pair() {
        assert_eq!(color_op(Color::Gray(0.5), PaintOp::Fill), ".5 g\n");
        assert_eq!(color_op(Color::Gray(0.5), PaintOp::Stroke), ".5 G\n");
        assert_eq!(
            color_op(Color::Rgb(1.0, 0.0, 0.0), PaintOp::Fill),
            "1 0 0 rg\n"
        );
        assert_eq!(
            color_op(Color::Cmyk(0.0, 0.25, 0.5, 1.0), PaintOp::Stroke),
            "0 .25 .5 1 K\n"
        );
    }

    #[test]
    fn a_present_but_unusable_array_beats_the_default() {
        let empty = Array::new();
        assert_eq!(
            color_with_default(Some(&empty), Color::Rgb(1.0, 1.0, 0.0), PaintOp::Fill),
            ""
        );
        assert_eq!(
            color_with_default(None, Color::Rgb(1.0, 1.0, 0.0), PaintOp::Fill),
            "1 1 0 rg\n"
        );
        let black = Array::of([0.0_f32, 0.0, 0.0].map(Object::from));
        assert_eq!(
            color_with_default(Some(&black), Color::Rgb(1.0, 1.0, 0.0), PaintOp::Fill),
            "0 0 0 rg\n"
        );
    }

    #[test]
    fn the_paint_operator_covers_all_four_combinations() {
        assert_eq!(paint_operator(true, true), "b");
        assert_eq!(paint_operator(true, false), "s");
        assert_eq!(paint_operator(false, true), "f");
        assert_eq!(paint_operator(false, false), "n");
    }
}
