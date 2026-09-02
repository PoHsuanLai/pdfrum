//! Line breaking: grouping a section's characters into lines.
//!
//! One loop, run in two modes. With `typeset` it records the lines it finds;
//! without it, it only measures, which is how automatic sizing tries a size
//! without committing to it.
//!
//! The loop's shape is worth knowing before reading it. The index is **not**
//! advanced on the wrapping branch — that is what lets the wrap rewind to the
//! last word boundary and re-run the characters it had already consumed. The
//! two exceptions are a single character too wide for the whole line, which
//! must advance or the loop would never end, and the ordinary non-wrapping
//! step.

use kurbo::Rect;

use crate::geom;
use crate::vt::classify::{is_open_style_punctuation, is_space, need_division};
use crate::vt::{Config, Metrics, Section, font_ascent, font_descent, word_width};

/// Breaks a section into lines and returns its extent, y-down.
///
/// With `typeset` set the lines are recorded on the section; otherwise the
/// return value is the whole answer.
#[must_use]
pub fn split_lines(
    section: &mut Section,
    config: &Config,
    metrics: &Metrics<'_>,
    typeset: bool,
) -> Rect {
    split_at_size(section, config, metrics, typeset, config.font_size)
}

/// The same, at an explicit size — which is what measuring for automatic
/// sizing needs, since the config's own size is not yet decided.
///
/// One long loop on purpose. Its state — the running line, the backup copy
/// of it taken at each word boundary, and the rewind — is mutually
/// dependent, and splitting it into helpers would move the interesting part
/// (which variable is restored, and when) out of sight of the branch that
/// restores it.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn split_at_size(
    section: &mut Section,
    config: &Config,
    metrics: &Metrics<'_>,
    typeset: bool,
    font_size: f32,
) -> Rect {
    let ascent = font_ascent(metrics, font_size);
    let descent = font_descent(metrics, font_size);

    if section.words.is_empty() {
        // An empty paragraph is still one line, marked with an out-of-range
        // word range so placement knows to skip it, and it still occupies a
        // line's height.
        if typeset {
            section.lines.push(crate::vt::Line {
                words: None,
                x: 0.0,
                y: 0.0,
                width: 0.0,
                ascent,
                descent,
            });
        }
        return geom::rect(0.0, 0.0, 0.0, ascent - descent);
    }

    let typeset_width = (geom::width(config.plate)).max(0.0);
    let total = section.words.len();

    let mut line_head = 0usize;
    let (mut max_x, mut max_y) = (0.0_f32, 0.0_f32);
    let (mut line_width, mut backup_width) = (0.0_f32, 0.0_f32);
    let (mut line_ascent, mut backup_ascent) = (0.0_f32, 0.0_f32);
    let (mut line_descent, mut backup_descent) = (0.0_f32, 0.0_f32);
    let mut word_start = 0usize;
    let mut full_word = false;
    let mut full_word_index = 0i32;
    let mut char_index = 0i32;
    let mut opened = false;
    let mut index = 0usize;
    let mut lines: Vec<crate::vt::Line> = Vec::new();

    while index < total {
        let Some(word) = section.words.get(index) else {
            break;
        };
        let previous = if index > 0 {
            section.words.get(index - 1).map_or(word.ch, |w| w.ch)
        } else {
            word.ch
        };

        // A line's ascent starts at **zero**, not at the font's, and is a
        // running maximum; its descent starts at zero and is a running
        // minimum. So a font with a negative ascent yields a line ascent of
        // zero rather than a negative one.
        line_ascent = line_ascent.max(ascent);
        line_descent = line_descent.min(descent);
        let word_width = word_width(word, config, metrics, font_size);

        if opened {
            if !is_space(word.ch) && !is_open_style_punctuation(word.ch) {
                opened = false;
            }
        } else if is_open_style_punctuation(word.ch) {
            opened = true;
            full_word = true;
        } else if need_division(previous, word.ch) {
            full_word = true;
        }

        if full_word {
            full_word = false;
            // Evaluated *before* the character counter advances, so the very
            // first character of a section never marks a word boundary.
            if char_index > 0 {
                full_word_index += 1;
            }
            word_start = index;
            backup_width = line_width;
            backup_ascent = line_ascent;
            backup_descent = line_descent;
        }
        char_index += 1;

        if config.auto_return && typeset_width > 0.0 && line_width + word_width > typeset_width {
            if full_word_index > 0 {
                // Rewind to the last word boundary and re-run from there.
                index = word_start;
                line_width = backup_width;
                line_ascent = backup_ascent;
                line_descent = backup_descent;
            }
            if char_index == 1 {
                // One character wider than the whole line: it goes on this
                // line alone, and the index must advance or nothing ever
                // would.
                line_width = word_width;
                index += 1;
            }
            lines.push(line_of(
                line_head,
                index.saturating_sub(1),
                line_width,
                line_ascent,
                line_descent,
            ));
            max_y += line_ascent;
            max_y -= line_descent;
            max_x = line_width.max(max_x);
            line_head = index;
            line_width = 0.0;
            line_ascent = 0.0;
            line_descent = 0.0;
            char_index = 0;
            full_word_index = 0;
            full_word = false;
        } else {
            line_width += word_width;
            index += 1;
        }
    }

    if line_head <= total.saturating_sub(1) {
        lines.push(line_of(
            line_head,
            total - 1,
            line_width,
            line_ascent,
            line_descent,
        ));
        max_y += line_ascent;
        max_y -= line_descent;
        max_x = line_width.max(max_x);
    }
    if typeset {
        section.lines.extend(lines);
    }
    geom::rect(0.0, 0.0, max_x, max_y)
}

/// One line record, with its placement left for the placement pass.
fn line_of(head: usize, tail: usize, width: f32, ascent: f32, descent: f32) -> crate::vt::Line {
    crate::vt::Line {
        // `tail` is the last index the line covers; the range is half-open.
        words: Some(
            u32::try_from(head).unwrap_or(0)..u32::try_from(tail).unwrap_or(0).saturating_add(1),
        ),
        x: 0.0,
        y: 0.0,
        width,
        ascent,
        descent,
    }
}

#[cfg(test)]
mod tests {
    use super::split_at_size;
    use crate::geom;
    use crate::vt::{Config, Section, Word, stub};

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

    #[test]
    fn an_empty_section_is_still_one_line_with_a_height() {
        let mut empty = Section::default();
        let config = Config::default();
        let got = split_at_size(&mut empty, &config, &stub::metrics(), true, 10.0);
        assert_eq!(empty.lines.len(), 1);
        // The single line of an empty section covers no words at all, which
        // used to be spelled `(begin, end) == (-1, -1)`.
        assert_eq!(
            empty.lines.first().map(|l| l.words.clone()),
            Some(None),
            "an empty section's one line covers nothing"
        );
        // Ascent 0.1 less descent -0.02 at size 10.
        assert!((geom::height(got) - 0.12).abs() < 1e-6, "{got:?}");
        assert!(geom::width(got).abs() < f32::EPSILON);
    }

    #[test]
    fn measuring_records_no_lines_while_typesetting_does() {
        let config = Config::default();
        let mut measured = section("hello");
        let _ = split_at_size(&mut measured, &config, &stub::metrics(), false, 10.0);
        assert!(measured.lines.is_empty());

        let mut set = section("hello");
        let _ = split_at_size(&mut set, &config, &stub::metrics(), true, 10.0);
        assert_eq!(set.lines.len(), 1);
    }

    #[test]
    fn without_auto_return_everything_stays_on_one_line() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.2, 10.0),
            ..Config::default()
        };
        let mut one = section("hello");
        let _ = split_at_size(&mut one, &config, &stub::metrics(), true, 10.0);
        assert_eq!(one.lines.len(), 1);
        // Five characters at a tenth each.
        assert!(
            one.lines
                .first()
                .is_some_and(|l| (l.width - 0.5).abs() < 1e-6)
        );
    }

    #[test]
    fn auto_return_wraps_at_the_plate_width() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.25, 10.0),
            auto_return: true,
            ..Config::default()
        };
        let mut wrapped = section("hello");
        let _ = split_at_size(&mut wrapped, &config, &stub::metrics(), true, 10.0);
        // A tenth per character into a quarter: two lines of two, then one.
        assert_eq!(wrapped.lines.len(), 3);
        // Half-open now, so a line that used to read `(0, 1)` inclusive
        // reads `0..2`.
        assert_eq!(
            wrapped
                .lines
                .iter()
                .map(|l| l.words.clone())
                .collect::<Vec<_>>(),
            [Some(0..2), Some(2..4), Some(4..5)]
        );
    }

    #[test]
    fn a_single_character_wider_than_the_line_still_advances() {
        // A plate narrower than one character would otherwise loop forever.
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.05, 10.0),
            auto_return: true,
            ..Config::default()
        };
        let mut narrow = section("abc");
        let _ = split_at_size(&mut narrow, &config, &stub::metrics(), true, 10.0);
        assert_eq!(narrow.lines.len(), 3);
    }

    #[test]
    fn wrapping_rewinds_to_the_last_word_boundary() {
        // Wide enough for "ab " plus one more, so the wrap rewinds past the
        // start of "cd" rather than splitting it.
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.45, 10.0),
            auto_return: true,
            ..Config::default()
        };
        let mut text = section("ab cd");
        let _ = split_at_size(&mut text, &config, &stub::metrics(), true, 10.0);
        assert_eq!(text.lines.len(), 2);
        // The break falls after the space, not inside the second word: the
        // first line's last character is index 2, so its half-open range ends
        // at 3.
        assert_eq!(
            text.lines.first().and_then(crate::vt::Line::last_word),
            Some(2)
        );
    }
}
