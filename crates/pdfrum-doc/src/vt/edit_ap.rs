//! Turning a layout into `Td` / `Tf` / `Tj` operators.
//!
//! The emitter tries to write the smallest text-positioning stream it can:
//! consecutive characters on one line become a single `Tj`, and a `Td` is
//! written only when the pen actually has to move.
//!
//! # The two buffers
//!
//! There are two staging buffers, `line` and `words`, and *which one gets
//! flushed where* is observable in the output ordering — so the buffering is
//! reproduced rather than simplified away.
//!
//! In the grouped path the position and the font operator go into `line`
//! while the characters accumulate in `words`; `words` is flushed into `line`
//! at a font change, and `line` is flushed into the stream only at a line
//! boundary. In the per-character path both go straight into the stream. A
//! right-to-left character always takes the per-character path, whatever the
//! caller asked for, because its position does not follow from the previous
//! one.

use crate::ap::emit::Float;
use crate::ap::fmt;
use crate::vt::{Config, Layout, Metrics, Word, word_width};

/// How the emitter groups characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    /// One `Tj` per run of same-line, same-font characters.
    Continuous,
    /// One `Td` and one `Tj` per character — what a comb field needs, since
    /// every character sits in its own cell.
    PerCharacter,
}

/// Writes a laid-out text's operators.
///
/// `offset` shifts every position, in PDF space. `font_alias` is the resource
/// name the `Tf` operator will carry; an empty one, or a size at or below
/// zero, suppresses the `Tf` entirely and the text inherits whatever font the
/// enclosing stream had set.
#[must_use]
pub fn generate<F>(
    layout: &Layout,
    config: &Config,
    metrics: &Metrics<'_>,
    offset: (f32, f32),
    grouping: Grouping,
    font_alias: &[u8],
    encode: F,
) -> String
where
    F: Fn(u32) -> Vec<u8>,
{
    let mut out = String::new();
    let mut line = String::new();
    let mut words: Vec<u8> = Vec::new();
    let (mut old_x, mut old_y) = (0.0_f32, 0.0_f32);
    let mut current_font: i32 = -1;
    // `(-1, -1)` so the first character always reads as a new line.
    let mut previous_place = (-1_i32, -1_i32);

    for (section, line_index, word) in layout.words() {
        let place = (
            i32::try_from(section).unwrap_or(0),
            i32::try_from(line_index).unwrap_or(0),
        );
        let (x, y) = position(layout, config, word, offset);

        if grouping == Grouping::Continuous && !word.is_rtl {
            if place != previous_place {
                if !words.is_empty() {
                    line.push_str(&render(&words));
                    out.push_str(&line);
                    line.clear();
                    words.clear();
                }
                if let Some(step) = step(x, y, old_x, old_y) {
                    line.push_str(&step);
                    old_x = x;
                    old_y = y;
                }
            } else if words.is_empty()
                && let Some(step) = step(x, y, old_x, old_y)
            {
                line.push_str(&step);
                old_x = x;
                old_y = y;
            }
            if word.font_index != current_font {
                // The pending characters flush into `line`, but `line` does
                // **not** flush into the stream — which is what makes a line
                // ending right after a font change order the way it does.
                if !words.is_empty() {
                    line.push_str(&render(&words));
                    words.clear();
                }
                line.push_str(&font_op(font_alias, layout.font_size));
                current_font = word.font_index;
            }
            words.extend(encode(shown(word, config)));
        } else {
            if !words.is_empty() {
                line.push_str(&render(&words));
                out.push_str(&line);
                line.clear();
                words.clear();
            }
            if let Some(step) = step(x, y, old_x, old_y) {
                out.push_str(&step);
                old_x = x;
                old_y = y;
            }
            if word.font_index != current_font {
                out.push_str(&font_op(font_alias, layout.font_size));
                current_font = word.font_index;
            }
            out.push_str(&render(&encode(shown(word, config))));
        }
        previous_place = place;
        let _ = word_width(word, config, metrics, layout.font_size);
    }

    // Whatever is still pending flushes into `line`, and `line` into the
    // stream — in that order, so a trailing run lands after the position and
    // font operators that precede it.
    line.push_str(&render(&words));
    out.push_str(&line);
    out
}

/// The character that is actually written, which a password field replaces.
fn shown(word: &Word, config: &Config) -> u32 {
    config.sub_word.map_or(word.ch, |sub| sub as u32)
}

/// A character's position in PDF space, offset.
fn position(layout: &Layout, config: &Config, word: &Word, offset: (f32, f32)) -> (f32, f32) {
    let _ = layout;
    let (x, y) = crate::vt::Layout::to_pdf(config.plate, word.x, word.y);
    (x + offset.0, y + offset.1)
}

/// The `Td` for a move, or nothing when the pen is already there.
///
/// `Td` is **relative**, and the comparison is exact float equality on both
/// components — not an epsilon. A position that differs in the last bit
/// writes a `Td` of very nearly zero rather than none, which is what the
/// bytes being matched contain.
#[allow(clippy::float_cmp)]
fn step(x: f32, y: f32, old_x: f32, old_y: f32) -> Option<String> {
    if x == old_x && y == old_y {
        return None;
    }
    Some(format!(
        "{} {} Td\n",
        Float::Shortest.write(x - old_x),
        Float::Shortest.write(y - old_y)
    ))
}

/// The `Tf`, or nothing when there is no usable name or size.
///
/// A size of zero — which automatic sizing produces for a plate with no width
/// — suppresses it, and the text then inherits the enclosing font.
fn font_op(alias: &[u8], size: f32) -> String {
    if alias.is_empty() || size <= 0.0 {
        return String::new();
    }
    format!(
        "/{} {} Tf\n",
        String::from_utf8_lossy(alias),
        fmt::shortest(size)
    )
}

/// The `Tj` for a run of encoded bytes.
fn render(words: &[u8]) -> String {
    if words.is_empty() {
        return String::new();
    }
    let literal = pdfrum_object::encode_string_literal(words);
    format!("{} Tj\n", String::from_utf8_lossy(&literal))
}

#[cfg(test)]
mod tests {
    use super::{Grouping, generate};
    use crate::geom;
    use crate::vt::{Config, Layout, layout, stub};

    /// One byte per character, which is what a simple font does.
    fn one_byte(code: u32) -> Vec<u8> {
        vec![u8::try_from(code).unwrap_or(b'?')]
    }

    fn laid(text: &str, config: &Config) -> Layout {
        layout(text, config, &stub::metrics())
    }

    fn plate() -> Config {
        Config {
            plate: geom::rect(0.0, 0.0, 100.0, 100.0),
            font_size: 10.0,
            ..Config::default()
        }
    }

    #[test]
    fn a_run_on_one_line_becomes_a_single_show_operator() {
        let config = plate();
        let got = generate(
            &laid("hi", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        assert_eq!(got.matches(" Tj\n").count(), 1);
        assert!(got.contains("(hi) Tj\n"), "{got}");
        assert!(got.contains("/Helv 10 Tf\n"), "{got}");
    }

    #[test]
    fn per_character_grouping_writes_one_show_operator_each() {
        let config = plate();
        let got = generate(
            &laid("hi", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::PerCharacter,
            b"Helv",
            one_byte,
        );
        assert_eq!(got.matches(" Tj\n").count(), 2);
    }

    #[test]
    fn no_font_name_or_a_zero_size_suppresses_the_font_operator() {
        let config = plate();
        let nameless = generate(
            &laid("hi", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"",
            one_byte,
        );
        assert!(!nameless.contains(" Tf\n"), "{nameless}");

        let mut zero = plate();
        zero.plate = geom::rect(0.0, 0.0, 0.0, 100.0);
        zero.font_size = 0.0;
        let sized = generate(
            &laid("hi", &zero),
            &zero,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        assert!(!sized.contains(" Tf\n"), "{sized}");
    }

    #[test]
    fn a_password_field_writes_its_substitute_rather_than_the_text() {
        let config = Config {
            sub_word: Some('*'),
            ..plate()
        };
        let got = generate(
            &laid("hi", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        assert!(got.contains("(**) Tj\n"), "{got}");
        assert!(!got.contains('h'), "{got}");
    }

    #[test]
    fn the_position_operator_is_relative_and_is_skipped_when_nothing_moves() {
        let config = plate();
        let got = generate(
            &laid("hi", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        // One move to the start of the only line, and no more.
        assert_eq!(got.matches(" Td\n").count(), 1);
    }

    #[test]
    fn each_line_gets_its_own_relative_move() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.25, 100.0),
            auto_return: true,
            ..plate()
        };
        let got = generate(
            &laid("hello", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        assert_eq!(got.matches(" Td\n").count(), 3);
        assert_eq!(got.matches(" Tj\n").count(), 3);
    }

    #[test]
    fn the_offset_shifts_the_first_move_and_nothing_else() {
        let config = plate();
        let text = laid("hi", &config);
        let plain = generate(
            &text,
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        let shifted = generate(
            &text,
            &config,
            &stub::metrics(),
            (3.0, -3.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        assert_ne!(plain, shifted);
        assert_eq!(
            plain.matches(" Td\n").count(),
            shifted.matches(" Td\n").count()
        );
    }

    #[test]
    fn empty_text_writes_nothing() {
        let config = plate();
        let got = generate(
            &laid("", &config),
            &config,
            &stub::metrics(),
            (0.0, 0.0),
            Grouping::Continuous,
            b"Helv",
            one_byte,
        );
        assert_eq!(got, "");
    }
}
