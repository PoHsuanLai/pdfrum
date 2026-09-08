//! Comb fields: a fixed number of equal cells, one character each.
//!
//! A comb field's `/MaxLen` is a **cell count**, not a length cap: the plate
//! divides into that many columns and each character is centred in one. The
//! alignment decides which column the first character lands in, and then the
//! first character's own position overwrites the line's — so the alignment's
//! contribution to the line origin is dead whenever there is at least one
//! character.
//!
//! The cell arithmetic runs in **double** precision and narrows once, which
//! matters: computing it in single precision lets the last cell drift off its
//! column by a visible fraction.

use kurbo::Rect;

use crate::geom;
use crate::vt::{Alignment, Config, Line, Metrics, Section, font_ascent, font_descent, word_width};

/// Lays a section out as comb cells and returns its extent, y-down.
#[must_use]
pub fn rearrange_char_array(section: &mut Section, config: &Config, metrics: &Metrics<'_>) -> Rect {
    section.lines.clear();
    let cells = config.char_array.max(1);
    #[allow(clippy::cast_precision_loss)]
    let node = geom::width(config.plate) / cells as f32;
    let ascent = font_ascent(metrics, config.font_size);
    let descent = font_descent(metrics, config.font_size);
    let y = ascent;

    let count = section.words.len();
    #[allow(clippy::cast_precision_loss)]
    let start = match config.alignment {
        Alignment::Left => 0.0_f32,
        Alignment::Center => (cells.saturating_sub(count) / 2) as f32,
        Alignment::Right => cells.saturating_sub(count) as f32,
    };
    let mut line_x = match config.alignment {
        Alignment::Left => node * 0.5,
        Alignment::Center | Alignment::Right => node * start - node * 0.5,
    };

    let mut x = 0.0_f32;
    let mut line_ascent = 0.0_f32;
    let mut line_descent = 0.0_f32;

    for index in 0..count.min(cells) {
        // The next character's width, measured with its tail cleared — the
        // tail is state written back onto the word, so measuring one that
        // still carries a stale tail would compound it.
        let next_width = if index + 1 < count {
            if let Some(next) = section.words.get_mut(index + 1) {
                next.tail = 0.0;
            }
            section.words.get(index + 1).map_or(0.0, |next| {
                word_width(next, config, metrics, config.font_size)
            })
        } else {
            0.0
        };
        if let Some(word) = section.words.get_mut(index) {
            word.tail = 0.0;
        }
        let width = section.words.get(index).map_or(0.0, |word| {
            word_width(word, config, metrics, config.font_size)
        });

        // Computed in double precision and narrowed once, as upstream does.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let cell_x = (f64::from(node) * (index as f64 + f64::from(start) + 0.5)
            - f64::from(width) * 0.5) as f32;

        if let Some(word) = section.words.get_mut(index) {
            word.x = cell_x;
            word.y = y;
            // The first character's position overrides whatever the
            // alignment put on the line.
            // Not a midpoint: the half is the share of the two cells this
            // gap spans, subtracted from the node to leave the space after
            // the word.
            #[expect(clippy::manual_midpoint, reason = "a gap, not a mean")]
            {
                word.tail = if index + 1 == count {
                    0.0
                } else {
                    (node - (width + next_width) * 0.5).max(0.0)
                };
            }
        }
        if index == 0 {
            line_x = cell_x;
        }
        x = cell_x + width;
        line_ascent = line_ascent.max(ascent);
        line_descent = line_descent.min(descent);
    }

    section.lines.push(Line {
        words: Some(0..u32::try_from(count).unwrap_or(u32::MAX)),
        x: line_x,
        y,
        width: x - line_x,
        ascent: line_ascent,
        descent: line_descent,
    });
    geom::rect(0.0, 0.0, x, y - line_descent)
}

#[cfg(test)]
mod tests {
    use super::rearrange_char_array;
    use crate::geom;
    use crate::vt::{Alignment, Config, Section, Word, stub};

    fn section(text: &str) -> Section {
        Section {
            words: text
                .chars()
                .map(|ch| Word {
                    ch: ch as u32,
                    x: 0.0,
                    y: 0.0,
                    tail: 0.0,
                    is_rtl: false,
                })
                .collect(),
            lines: Vec::new(),
            rect: kurbo::Rect::ZERO,
        }
    }

    fn comb(text: &str, cells: usize, alignment: Alignment) -> Section {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 100.0, 20.0),
            font_size: 10.0,
            char_array: cells,
            alignment,
            ..Config::default()
        };
        let mut section = section(text);
        let _ = rearrange_char_array(&mut section, &config, &stub::metrics());
        section
    }

    #[test]
    fn each_character_is_centred_in_its_own_cell() {
        // Ten cells of ten units. A character is a tenth of a unit wide at
        // size 10, so it sits a twentieth left of each cell's middle.
        let laid = comb("abcde", 10, Alignment::Left);
        let xs: Vec<f32> = laid.words.iter().map(|w| w.x).collect();
        for (index, x) in xs.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let expected = 10.0 * (index as f32 + 0.5) - 0.05;
            assert!(
                (x - expected).abs() < 1e-4,
                "cell {index}: {x} vs {expected}"
            );
        }
    }

    #[test]
    fn right_alignment_pushes_the_characters_into_the_last_cells() {
        let left = comb("ab", 10, Alignment::Left);
        let right = comb("ab", 10, Alignment::Right);
        let first_left = left.words.first().map_or(0.0, |w| w.x);
        let first_right = right.words.first().map_or(0.0, |w| w.x);
        // Eight empty cells of ten units each.
        assert!((first_right - first_left - 80.0).abs() < 1e-4);
    }

    #[test]
    fn the_first_characters_position_overrides_the_lines_own() {
        let laid = comb("abc", 10, Alignment::Right);
        assert_eq!(
            laid.lines.first().map(|l| l.x),
            laid.words.first().map(|w| w.x)
        );
    }

    #[test]
    fn more_characters_than_cells_lays_out_only_what_fits() {
        let laid = comb("abcdef", 3, Alignment::Left);
        // Every character is still in the section — the cap is on placement,
        // and the tokenizer applies its own limit separately.
        assert_eq!(laid.words.len(), 6);
        // Only the first three were given positions.
        assert!(laid.words.iter().skip(3).all(|w| w.x.abs() < f32::EPSILON));
    }

    #[test]
    fn the_last_character_carries_no_tail() {
        let laid = comb("ab", 10, Alignment::Left);
        assert!(
            laid.words
                .get(1)
                .is_some_and(|w| w.tail.abs() < f32::EPSILON)
        );
        assert!(laid.words.first().is_some_and(|w| w.tail > 0.0));
    }

    #[test]
    fn an_empty_section_still_produces_one_line() {
        let laid = comb("", 5, Alignment::Left);
        assert_eq!(laid.lines.len(), 1);
    }
}
