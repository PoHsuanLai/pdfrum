//! Text state and the two matrices that track position
//! (ISO 32000-1 §9.3, §9.4.2).
//!
//! Two positions are kept, not one: `pos` is where the next glyph goes, and
//! `line_pos` is where the current line began. `Td` moves the *line* and then
//! syncs the position to it, so a sequence of `Td`s accumulates rather than
//! replacing.
//!
//! Three details are easy to get wrong and are all reproduced:
//!
//! - **`Tz` stores a fraction, not a percentage.** `150 Tz` becomes `1.5`.
//! - **`TD` sets the leading to the *negated* y offset**, so `0 -14 TD` gives
//!   a leading of 14.
//! - **The rise is applied inside the text matrix**, not after it, so a
//!   rotated text matrix rotates the rise too.

use crate::ops::TextRenderMode;
use kurbo::{Affine, Point};
use pdfrum_font::Font;
use std::sync::Arc;

/// The text-showing parameters (ISO 32000-1 table 105).
///
/// Equality compares the font by its identity rather than its contents: a
/// [`Font`] is a large loaded object with no meaningful structural equality,
/// and two states holding the same `Arc` are the same state.
#[derive(Debug, Clone)]
pub struct TextState {
    /// The font and its size. `Tf` always sets the size, and sets the font
    /// only when the name resolved — so a bad `/Font` name changes the size
    /// and leaves the font standing.
    pub font: Option<(Arc<Font>, f32)>,
    /// `Tc`, added after each glyph.
    pub char_space: f32,
    /// `Tw`, added after each single-byte space.
    pub word_space: f32,
    /// `Tz` as a fraction: `150 Tz` is `1.5`.
    pub horz_scale: f32,
    /// `TL`, the distance between baselines.
    pub leading: f32,
    /// `Ts`, the baseline offset.
    pub rise: f32,
    /// `Tr`.
    pub render_mode: TextRenderMode,
}

impl PartialEq for TextState {
    fn eq(&self, other: &Self) -> bool {
        let same_font = match (&self.font, &other.font) {
            (Some((a, sa)), Some((b, sb))) => a.id() == b.id() && sa == sb,
            (None, None) => true,
            _ => false,
        };
        same_font
            && self.char_space == other.char_space
            && self.word_space == other.word_space
            && self.horz_scale == other.horz_scale
            && self.leading == other.leading
            && self.rise == other.rise
            && self.render_mode == other.render_mode
    }
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            char_space: 0.0,
            word_space: 0.0,
            horz_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: TextRenderMode::Fill,
        }
    }
}

/// Where the next glyph goes, and where its line began.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextCursor {
    /// The text matrix, set wholesale by `Tm` and reset by `BT`.
    pub matrix: Affine,
    /// Where the next glyph goes, in text space.
    pub pos: Point,
    /// Where the current line began.
    pub line_pos: Point,
}

impl Default for TextCursor {
    fn default() -> Self {
        Self {
            matrix: Affine::IDENTITY,
            pos: Point::ZERO,
            line_pos: Point::ZERO,
        }
    }
}

impl TextCursor {
    /// `BT` and `Tm`: reset both positions to the origin of the new matrix.
    pub fn set_matrix(&mut self, matrix: Affine) {
        self.matrix = matrix;
        self.pos = Point::ZERO;
        self.line_pos = Point::ZERO;
    }

    /// `Td`: move the line start by `(tx, ty)` and put the position there.
    ///
    /// The offset **accumulates** onto the previous line start, which is why
    /// two `Td`s in a row move twice.
    pub fn move_line(&mut self, tx: f64, ty: f64) {
        self.line_pos.x += tx;
        self.line_pos.y += ty;
        self.pos = self.line_pos;
    }

    /// `T*`: down one leading, back to the line start.
    pub fn next_line(&mut self, leading: f64) {
        self.line_pos.y -= leading;
        self.pos = self.line_pos;
    }

    /// The device-space position of the next glyph.
    ///
    /// The rise is applied **inside** the text matrix, before the CTM, so a
    /// rotated text matrix rotates it.
    #[must_use]
    pub fn device_position(&self, rise: f32, ctm: Affine) -> Point {
        let in_text = Point::new(self.pos.x, self.pos.y + f64::from(rise));
        ctm * (self.matrix * in_text)
    }

    /// Advance the position after showing a run.
    ///
    /// Vertical writing moves y and leaves x alone; horizontal writing does
    /// the opposite and scales by the horizontal scale.
    pub fn advance(&mut self, amount: f64, vertical: bool) {
        if vertical {
            self.pos.y += amount;
        } else {
            self.pos.x += amount;
        }
    }
}

/// The displacement a `TJ` adjustment produces, in text space.
///
/// `k * size / 1000`, times the horizontal scale for horizontal writing.
#[must_use]
pub fn kerning_shift(kerning: f32, font_size: f32, horz_scale: f32, vertical: bool) -> f64 {
    let base = f64::from(kerning) * f64::from(font_size) / 1000.0;
    if vertical {
        base
    } else {
        base * f64::from(horz_scale)
    }
}

/// The matrix a glyph is drawn with, without its translation.
///
/// `[horz_scale 0; 0 1; 0 0] × text_matrix × ctm` in PDF order, which in
/// kurbo's operand order is `ctm * text_matrix * scale` (design brief D14).
#[must_use]
pub fn glyph_matrix(horz_scale: f32, text_matrix: Affine, ctm: Affine) -> Affine {
    let scale = Affine::new([f64::from(horz_scale), 0.0, 0.0, 1.0, 0.0, 0.0]);
    ctm * text_matrix * scale
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

    use super::{TextCursor, TextState, glyph_matrix, kerning_shift};
    use crate::ops::TextRenderMode;
    use kurbo::{Affine, Point};

    #[test]
    fn the_default_horizontal_scale_is_one_not_a_hundred() {
        let s = TextState::default();
        assert!((s.horz_scale - 1.0).abs() < 1e-6);
        assert_eq!(s.render_mode, TextRenderMode::Fill);
        assert!(s.font.is_none());
    }

    #[test]
    fn td_accumulates_onto_the_line_start() {
        let mut c = TextCursor::default();
        c.move_line(10.0, -14.0);
        assert_eq!(c.pos, Point::new(10.0, -14.0));
        assert_eq!(c.line_pos, Point::new(10.0, -14.0));
        // A second `Td` moves again rather than replacing.
        c.move_line(5.0, -14.0);
        assert_eq!(c.pos, Point::new(15.0, -28.0));
    }

    #[test]
    fn t_star_returns_to_the_line_start_one_leading_down() {
        let mut c = TextCursor::default();
        c.move_line(10.0, 0.0);
        // Advance within the line, then break.
        c.advance(50.0, false);
        assert_eq!(c.pos.x, 60.0);
        c.next_line(14.0);
        assert_eq!(c.pos, Point::new(10.0, -14.0));
    }

    #[test]
    fn setting_the_matrix_resets_both_positions() {
        let mut c = TextCursor::default();
        c.move_line(10.0, 20.0);
        c.set_matrix(Affine::translate((100.0, 200.0)));
        assert_eq!(c.pos, Point::ZERO);
        assert_eq!(c.line_pos, Point::ZERO);
    }

    #[test]
    fn the_rise_goes_through_the_text_matrix() {
        let c = TextCursor {
            // A quarter turn, so a rise along +y comes out along −x.
            matrix: Affine::rotate(std::f64::consts::FRAC_PI_2),
            ..TextCursor::default()
        };
        let p = c.device_position(10.0, Affine::IDENTITY);
        assert!(p.x.abs() > 9.0, "the rise should have been rotated: {p:?}");
        assert!(p.y.abs() < 1e-6);

        // Applying it after the matrix would have moved y instead.
        let flat = TextCursor::default().device_position(10.0, Affine::IDENTITY);
        assert!((flat.y - 10.0).abs() < 1e-6);
    }

    #[test]
    fn kerning_scales_by_size_and_by_the_horizontal_scale() {
        // 1000 units of kerning at size 12 is one full em.
        assert!((kerning_shift(1000.0, 12.0, 1.0, false) - 12.0).abs() < 1e-6);
        assert!((kerning_shift(1000.0, 12.0, 2.0, false) - 24.0).abs() < 1e-6);
        // Vertical writing ignores the horizontal scale.
        assert!((kerning_shift(1000.0, 12.0, 2.0, true) - 12.0).abs() < 1e-6);
    }

    #[test]
    fn the_glyph_matrix_applies_the_horizontal_scale_first() {
        let m = glyph_matrix(2.0, Affine::IDENTITY, Affine::IDENTITY);
        // A unit x step comes out twice as long.
        let p = m * Point::new(1.0, 1.0);
        assert!((p.x - 2.0).abs() < 1e-6);
        assert!((p.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn vertical_writing_advances_y() {
        let mut c = TextCursor::default();
        c.advance(20.0, true);
        assert_eq!(c.pos, Point::new(0.0, 20.0));
    }
}
