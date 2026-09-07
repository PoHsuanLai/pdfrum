//! One extracted character, and the loose box derived from it.

use kurbo::{Affine, Point, Rect};

/// Where a character came from, which decides how the rest of the pipeline
/// treats it.
///
/// The distinction that matters most: a space **written in the content
/// stream** is [`Normal`](Self::Normal), not [`Generated`](Self::Generated).
/// "Generated" means the extractor invented the character because the
/// geometry implied one — an inter-word gap, an inter-object gap, or a line
/// break's `\r\n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharType {
    /// A character the font decoded from the content stream.
    Normal,
    /// A space or line break the extractor invented from the geometry.
    Generated,
    /// A character code the font could not map to Unicode; its `unicode` is
    /// the raw character code.
    NotUnicode,
    /// The soft hyphen at a line break.
    ///
    /// Its `unicode` is forced to `0x0002`, while
    /// [`TextPage::search_text`](crate::TextPage::search_text) carries
    /// `U+00AD` at the same position — the one place the character stream and
    /// the search-facing text provably disagree.
    Hyphen,
    /// One piece of a character that normalization split into several.
    Piece,
    /// A character synthesized from a marked-content `/ActualText` string
    /// rather than from any glyph.
    ActualText,
}

/// One extracted character, with its metrics and page geometry.
///
/// `unicode` is a `u32`, not a `char`: the character-code passthrough
/// ([`CharType::NotUnicode`]) emits raw codes that need not be Unicode scalar
/// values, and `0` is a legal and observed value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharBox {
    /// Where this character came from.
    pub char_type: CharType,
    /// The `--txt` code unit.
    pub unicode: u32,
    /// The PDF character code, or `None` where the C++ uses its invalid-code
    /// sentinel — which is every generated and `/ActualText` character.
    pub code: Option<pdfrum_font::CharCode>,
    /// The baseline origin, already transformed into page space.
    pub origin: Point,
    /// The tight glyph box in page space. **Zero-area for a generated
    /// character**, by construction.
    pub char_box: Rect,
    /// The font-uniform box: the glyph's advance width by the *font's*
    /// ascent and descent, so two characters of one font share a box height
    /// whatever their glyphs do. Always contains [`char_box`](Self::char_box).
    pub loose_char_box: Rect,
    /// The composed text × form matrix for a real character; the bare form
    /// matrix — usually the identity — for a generated one.
    pub matrix: Affine,
    /// Which of [`Page::objects`](pdfrum_page::Page::objects) produced it, in
    /// content order counting into form `XObject`s. `None` for a character no
    /// text object produced.
    pub object: Option<ObjectIndex>,
    /// The font size in force, or `1.0` for a character with no text object —
    /// which is every generated one.
    pub font_size: f32,
    /// `atan2(matrix.c, matrix.a)` normalized to `[0, 2π)`.
    pub angle: f32,
}

/// A text object's position in the page's flattened object walk.
///
/// Two characters share a text object exactly when their indices are equal.
/// Objects inside form `XObject`s are numbered in the order the walk reaches
/// them, so the index is unique across the whole page.
// An index rather than a pointer, the cross-reference from
// a character back to the object that drew it is data, not a back-edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectIndex(pub u32);

impl CharBox {
    /// Whether this character is one
    /// [`search_text`](crate::TextPage::search_text) keeps.
    ///
    /// A character with a unicode is normal unless it is one of the eight
    /// control code points; a character *without* one is normal exactly when
    /// its character code is non-zero. So the charcode-0 passthrough — which
    /// carries `unicode == 0` — is **not** normal: it lands in
    /// [`chars`](crate::TextPage::chars) as a NUL and never reaches the text.
    #[must_use]
    pub fn is_normal(&self) -> bool {
        if self.unicode == 0 {
            return self.code.is_some_and(|code| code.0 != 0);
        }
        !self.is_control()
    }

    /// Whether this is one of the control code points
    /// [`search_text`](crate::TextPage::search_text) drops.
    ///
    /// A [`CharType::Hyphen`] is **exempt**, which is why its `0x0002`
    /// survives into [`chars`](crate::TextPage::chars).
    // The `0x93..=0x98` band is the Windows-1252 smart-quote and dash range
    // read as raw code points — a historical artifact, kept verbatim.
    #[must_use]
    pub fn is_control(&self) -> bool {
        matches!(
            self.unicode,
            0x02 | 0x03 | 0x93 | 0x94 | 0x96 | 0x97 | 0x98 | 0xFFFE
        ) && self.char_type != CharType::Hyphen
    }

    /// Whether the extractor invented this character.
    #[must_use]
    pub fn is_generated(&self) -> bool {
        self.char_type == CharType::Generated
    }
}

/// The angle a matrix draws text at, in `[0, 2π)`.
#[must_use]
pub fn matrix_angle(matrix: Affine) -> f32 {
    let [a, _, c, ..] = matrix.as_coeffs();
    // `atan2(c, a)`, where `c` is the coefficient carrying y into x. The two
    // matrix conventions agree on `a` and disagree on which of the two
    // off-diagonal terms is called `b`: the C++'s row-major `|a b; c d|` puts
    // its `c` where kurbo's column-major `[a, b, c, d]` puts *its* `c`. Using
    // the other one reflects every angle about the x axis, which shows up as
    // a first quadrant reading as the fourth.
    let angle = c.atan2(a);
    let angle = if angle < 0.0 {
        angle + std::f64::consts::TAU
    } else {
        angle
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an angle in [0, 2pi) is exactly representable enough in f32"
    )]
    let narrowed = angle as f32;
    narrowed
}

/// Everything [`loose_bounds`] needs about one character.
#[derive(Debug)]
pub struct LooseBoundsInput<'a> {
    /// The character's tight box in page space.
    pub char_box: Rect,
    /// Its baseline origin in page space.
    pub origin: Point,
    /// Its composed matrix.
    pub matrix: Affine,
    /// Its character code, or `None` for a generated character.
    pub code: Option<pdfrum_font::CharCode>,
    /// The font that drew it, or `None` for a generated character.
    pub font: Option<&'a pdfrum_font::Font>,
    /// The font size in force.
    pub font_size: f32,
    /// The advance width already scaled by `font_size / 1000`.
    pub scaled_width: f32,
}

/// Whether a float is inside the C++'s float-zero band, `(-1e-4, 1e-4)`.
#[must_use]
pub(crate) fn is_float_zero(value: f32) -> bool {
    value > -0.0001 && value < 0.0001
}

/// A rectangle is empty when it has no positive area in either direction,
/// tested on the corner pair as given rather than on a normalized rect.
fn is_empty(rect: Rect) -> bool {
    rect.x1 <= rect.x0 || rect.y1 <= rect.y0
}

/// The union of two rectangles, taken as unnormalized corner pairs.
fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// The inverse of a matrix, or the **zero matrix** when it is singular —
/// which maps every point to the origin. Degenerate, but never a panic.
#[must_use]
pub fn inverse_or_zero(matrix: Affine) -> Affine {
    let [a, b, c, d, ..] = matrix.as_coeffs();
    if a * d - b * c == 0.0 {
        Affine::new([0.0; 6])
    } else {
        matrix.inverse()
    }
}

/// The font-uniform box around a character (`GetLooseBounds`).
///
/// Four shapes come out of this, in the order they are tried:
///
/// 1. A character whose tight box is empty — every generated one — keeps that
///    empty box, so a generated character's loose box is also zero-area.
/// 2. A vertical CID character gets a box a full font size wide, positioned
///    from the font's vertical origin and as tall as its vertical advance.
/// 3. Anything else with a usable ascent and descent gets the glyph's advance
///    width by the *font's* ascent and descent, computed in the character's
///    own space and transformed back — which is what makes the box
///    font-uniform, and what transposes it under a quarter turn.
/// 4. Failing all of that, the tight box again.
///
/// Both computed shapes finish by unioning the tight box in, so the loose box
/// always contains it.
#[must_use]
pub fn loose_bounds(input: &LooseBoundsInput<'_>) -> Rect {
    let tight = input.char_box;
    if is_empty(tight) {
        return tight;
    }
    let (Some(font), Some(code)) = (input.font, input.code) else {
        return tight;
    };
    let font_size = f64::from(input.font_size);
    if is_float_zero(input.font_size) {
        return tight;
    }

    let vertical = font.is_vertical();
    if vertical && let (Some((vx, vy)), Some(vw)) = (font.vert_origin(code), font.vert_width(code))
    {
        // A vertical CID glyph hangs from its own origin: the box is one font
        // size wide and as tall as the (negative) vertical advance.
        let offset_x = (f64::from(vx) - 500.0) * font_size / 1000.0;
        let offset_y = f64::from(vy) * font_size / 1000.0;
        let height = f64::from(vw) * font_size / 1000.0;
        let left = input.origin.x + offset_x;
        let top = input.origin.y + offset_y;
        let box_rect = Rect::new(left, top + height, left + font_size, top);
        return union(box_rect, tight);
    }

    let mut ascent = font.type_ascent();
    let mut descent = font.type_descent();
    let bbox = font.font_bbox();
    // `FX_RECT` is y-down but holds y-up glyph values here, so "top greater
    // than bottom" is the *ordinary* case and the clamp only fires then.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "font bbox metrics are integral 1000/em values"
    )]
    if bbox.y1 > bbox.y0 {
        ascent = ascent.min(bbox.y1 as i32);
        descent = descent.max(bbox.y0 as i32);
    }
    if ascent == descent {
        return tight;
    }

    let width = f64::from(input.scaled_width);
    let inverse = inverse_or_zero(input.matrix);
    let origin = inverse * input.origin;
    let right = origin.x + if vertical { -width } else { width };
    let bottom = origin.y + f64::from(descent) * font_size / 1000.0;
    let top = origin.y + f64::from(ascent) * font_size / 1000.0;
    // The transform is an axis-aligned bound of the rotated rectangle, which
    // is why a quarter turn swaps the loose box's width and height.
    let box_rect = transform_rect(input.matrix, Rect::new(origin.x, bottom, right, top));
    union(box_rect, tight)
}

/// The axis-aligned bound of a transformed rectangle, taking the corners as
/// an unnormalized pair.
#[must_use]
pub fn transform_rect(matrix: Affine, rect: Rect) -> Rect {
    let corners = [
        matrix * Point::new(rect.x0, rect.y0),
        matrix * Point::new(rect.x1, rect.y0),
        matrix * Point::new(rect.x0, rect.y1),
        matrix * Point::new(rect.x1, rect.y1),
    ];
    let xs = corners.map(|p| p.x);
    let ys = corners.map(|p| p.y);
    Rect::new(
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// The distance a matrix scales a length by: the mean of its two axis
/// scales.
#[must_use]
pub fn transform_distance(matrix: Affine, distance: f64) -> f64 {
    let [a, b, c, d, ..] = matrix.as_coeffs();
    let x_unit = (a * a + b * b).sqrt();
    let y_unit = (c * c + d * d).sqrt();
    distance * (x_unit + y_unit) / 2.0
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;
    use pdfrum_font::CharCode;

    fn char_box(char_type: CharType, unicode: u32, code: Option<u32>) -> CharBox {
        CharBox {
            char_type,
            unicode,
            code: code.map(CharCode),
            origin: Point::ZERO,
            char_box: Rect::ZERO,
            loose_char_box: Rect::ZERO,
            matrix: Affine::IDENTITY,
            object: None,
            font_size: 1.0,
            angle: 0.0,
        }
    }

    #[test]
    fn control_characters_are_the_eight_the_cpp_lists() {
        for unicode in [0x02, 0x03, 0x93, 0x94, 0x96, 0x97, 0x98, 0xFFFE] {
            assert!(char_box(CharType::Normal, unicode, Some(1)).is_control());
        }
        for unicode in [0x01, 0x04, 0x92, 0x95, 0x99, 0x20, 0x41, 0xFFFD] {
            assert!(!char_box(CharType::Normal, unicode, Some(1)).is_control());
        }
    }

    #[test]
    fn a_hyphen_is_exempt_from_the_control_test() {
        // Its unicode is 0x2, which would otherwise make it a control char --
        // and that exemption is what carries 0x2 into the character list.
        let hyphen = char_box(CharType::Hyphen, 0x02, None);
        assert!(!hyphen.is_control());
        assert!(hyphen.is_normal());
    }

    #[test]
    fn normality_falls_back_to_the_char_code_when_there_is_no_unicode() {
        // The charcode-0 passthrough: unicode 0 and code 0, so not normal.
        assert!(!char_box(CharType::Normal, 0, Some(0)).is_normal());
        // Unicode 0 with a real code is normal.
        assert!(char_box(CharType::Normal, 0, Some(7)).is_normal());
        // No code at all is not normal either.
        assert!(!char_box(CharType::Generated, 0, None).is_normal());
        // A control character is not normal.
        assert!(!char_box(CharType::Normal, 0x03, Some(3)).is_normal());
    }

    #[test]
    fn a_singular_matrix_inverts_to_the_zero_matrix() {
        let singular = Affine::new([1.0, 2.0, 2.0, 4.0, 5.0, 6.0]);
        let inverse = inverse_or_zero(singular);
        assert_eq!(inverse.as_coeffs(), [0.0; 6]);
        // Which maps every point to the origin rather than panicking.
        assert_eq!(inverse * Point::new(100.0, 200.0), Point::ZERO);
        // A well-formed matrix inverts normally.
        let scale = Affine::scale(2.0);
        assert_eq!(
            inverse_or_zero(scale) * Point::new(4.0, 6.0),
            Point::new(2.0, 3.0)
        );
    }

    #[test]
    fn the_angle_is_read_from_the_y_into_x_coefficient() {
        use std::f64::consts::{FRAC_PI_2, PI, TAU};
        assert!((matrix_angle(Affine::IDENTITY) - 0.0).abs() < 1e-6);
        // The coefficient the angle comes from is `c`, the one carrying y
        // into x. A matrix with `c` positive and `a` zero is a quarter turn.
        let quarter = matrix_angle(Affine::new([0.0, 0.0, 1.0, 0.0, 0.0, 0.0]));
        assert!((f64::from(quarter) - FRAC_PI_2).abs() < 1e-5, "{quarter}");
        // Negative `a` alone is a half turn.
        let half = matrix_angle(Affine::new([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
        assert!((f64::from(half) - PI).abs() < 1e-5, "{half}");
        // And a negative result wraps forward into the last quadrant rather
        // than staying negative.
        let three_quarter = matrix_angle(Affine::new([0.0, 0.0, -1.0, 0.0, 0.0, 0.0]));
        assert!(
            (f64::from(three_quarter) - 3.0 * FRAC_PI_2).abs() < 1e-5,
            "{three_quarter}"
        );
        assert!(f64::from(three_quarter) < TAU);
    }

    #[test]
    fn a_generated_characters_loose_box_is_its_empty_tight_box() {
        // The first line of GetLooseBounds: an empty tight box comes back
        // untouched, so a generated character is 0x0 both ways.
        let input = LooseBoundsInput {
            char_box: Rect::new(50.0, 100.0, 50.0, 100.0),
            origin: Point::new(50.0, 100.0),
            matrix: Affine::IDENTITY,
            code: None,
            font: None,
            font_size: 1.0,
            scaled_width: 0.0,
        };
        let loose = loose_bounds(&input);
        assert_eq!(loose, Rect::new(50.0, 100.0, 50.0, 100.0));
        assert_eq!(loose.width(), 0.0);
        assert_eq!(loose.height(), 0.0);
    }

    #[test]
    fn transform_distance_averages_the_two_axis_scales() {
        // A pure scale by 2 scales a distance by 2.
        assert_eq!(transform_distance(Affine::scale(2.0), 10.0), 20.0);
        // Anisotropic scaling averages: (3 + 1) / 2 = 2.
        let skewed = Affine::new([3.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(transform_distance(skewed, 10.0), 20.0);
    }

    #[test]
    fn transforming_a_rect_bounds_the_rotated_corners() {
        let rect = Rect::new(0.0, 0.0, 2.0, 1.0);
        let rotated = transform_rect(Affine::rotate(std::f64::consts::FRAC_PI_2), rect);
        // A quarter turn swaps the extents.
        assert!((rotated.width() - 1.0).abs() < 1e-9);
        assert!((rotated.height() - 2.0).abs() < 1e-9);
    }
}
