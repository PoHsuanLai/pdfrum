//! Final placement: giving each character an x and a y, in visual order.
//!
//! The alignment offset is computed **twice with different widths** — once
//! from the whole section's width, to fix the section's own left edge, and
//! once per line from that line's width. Each character's x is then the
//! difference between the two, so under centred or right alignment the two
//! partially cancel and a short line shifts relative to the section box
//! rather than sitting flush against it. That is the behavior, not a bug to
//! straighten.

use kurbo::Rect;

use crate::geom;
use crate::vt::bidi::{Resolver, Run};
use crate::vt::{Config, Metrics, Section, word_width};

/// Places a section's characters and returns its extent, y-down.
///
/// `measured` is what line breaking reported: its width and height set the
/// section's box, and the per-line offsets are taken from the lines
/// themselves.
#[must_use]
pub fn output_lines(
    section: &mut Section,
    config: &Config,
    metrics: &Metrics<'_>,
    measured: Rect,
) -> Rect {
    let typeset_width = geom::width(config.plate).max(0.0);
    let offset_for = |line_width: f32| match config.alignment {
        crate::vt::Alignment::Left => 0.0,
        crate::vt::Alignment::Center => (typeset_width - line_width) * 0.5,
        crate::vt::Alignment::Right => typeset_width - line_width,
    };

    let min_x = offset_for(geom::width(measured));
    let max_x = min_x + geom::width(measured);
    let max_y = geom::height(measured);

    let resolver = (!section.words.is_empty()).then(|| {
        let chars: Vec<u32> = section.words.iter().map(|w| w.ch).collect();
        Resolver::new(&chars, config.direction)
    });

    let mut pos_y = 0.0_f32;
    for index in 0..section.lines.len() {
        let Some(line) = section.lines.get(index).cloned() else {
            continue;
        };
        let mut pos_x = offset_for(line.width);
        pos_y += line.ascent;
        if let Some(slot) = section.lines.get_mut(index) {
            slot.x = pos_x - min_x;
            slot.y = pos_y;
        }
        let Some(words) = line.words.clone() else {
            // The single line of an empty section: nothing to place, but it
            // still advances the pen.
            pos_y -= line.descent;
            continue;
        };
        let begin = usize::try_from(words.start).unwrap_or(0);
        let length = usize::try_from(words.end.saturating_sub(words.start)).unwrap_or(0);

        let mut runs = resolver
            .as_ref()
            .map(|resolver| resolver.visual_runs(begin, length))
            .unwrap_or_default();
        if runs.is_empty() {
            // A line the resolver reports nothing for — a lone paragraph
            // separator, say — is set as one left-to-right run rather than
            // dropped.
            runs.push(Run {
                start: begin,
                length,
                is_rtl: false,
            });
        }

        for run in runs {
            let span = word_range(section.words.len(), run.start, run.length);
            let indices: Vec<usize> = if run.is_rtl {
                span.rev().collect()
            } else {
                span.collect()
            };
            for word_index in indices {
                let width = section.words.get(word_index).map_or(0.0, |word| {
                    word_width(word, config, metrics, config.font_size)
                });
                if let Some(word) = section.words.get_mut(word_index) {
                    word.is_rtl = run.is_rtl;
                    word.x = pos_x - min_x;
                    word.y = pos_y;
                }
                pos_x += width;
            }
        }
        pos_y -= line.descent;
    }
    geom::rect(min_x, 0.0, max_x, max_y)
}

/// The word indices a run covers, clamped rather than allowed to run off.
///
/// An out-of-range run yields an **empty** range, which is what keeps a
/// resolver that disagrees with the line table from reaching past the words
/// that exist.
fn word_range(total: usize, start: usize, length: usize) -> std::ops::Range<usize> {
    let end = start.saturating_add(length).min(total);
    let begin = start.min(end);
    begin..end
}

#[cfg(test)]
mod tests {
    use super::{output_lines, word_range};
    use crate::geom;
    use crate::vt::{Alignment, Config, Section, Word, split, stub};

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

    /// Lays out one section and returns it.
    fn laid_out(text: &str, config: &Config) -> Section {
        let mut section = section(text);
        let metrics = stub::metrics();
        let measured = split::split_at_size(&mut section, config, &metrics, true, config.font_size);
        let _ = output_lines(&mut section, config, &metrics, measured);
        section
    }

    fn plate_config() -> Config {
        Config {
            plate: geom::rect(0.0, 0.0, 100.0, 100.0),
            font_size: 10.0,
            ..Config::default()
        }
    }

    #[test]
    fn latin_text_advances_left_to_right() {
        let laid = laid_out("hello", &plate_config());
        let xs: Vec<f32> = laid.words.iter().map(|w| w.x).collect();
        assert!(
            xs.windows(2).all(|pair| pair.get(1) > pair.first()),
            "{xs:?}"
        );
        // A tenth of a unit per character at size 10.
        assert!(xs.first().is_some_and(|x| x.abs() < f32::EPSILON));
        assert!(xs.get(1).is_some_and(|x| (x - 0.1).abs() < 1e-6));
    }

    #[test]
    fn hebrew_text_advances_right_to_left() {
        let laid = laid_out("\u{5E9}\u{5DC}\u{5D5}\u{5DD}", &plate_config());
        let xs: Vec<f32> = laid.words.iter().map(|w| w.x).collect();
        assert!(
            xs.windows(2).all(|pair| pair.get(1) < pair.first()),
            "{xs:?}"
        );
        assert!(laid.words.iter().all(|w| w.is_rtl));
    }

    #[test]
    fn an_empty_section_places_nothing_but_still_has_a_line() {
        let mut empty = Section::default();
        let config = plate_config();
        let metrics = stub::metrics();
        let measured = split::split_at_size(&mut empty, &config, &metrics, true, 10.0);
        let box_ = output_lines(&mut empty, &config, &metrics, measured);
        assert_eq!(empty.lines.len(), 1);
        assert!(empty.words.is_empty());
        assert!(geom::height(box_) > 0.0);
    }

    #[test]
    fn centring_shifts_the_section_box_as_well_as_the_line() {
        let centred = Config {
            alignment: Alignment::Center,
            ..plate_config()
        };
        let laid = laid_out("hello", &centred);
        // The section's own offset and the line's are computed from
        // different widths and partially cancel, so a single line — where
        // both widths agree — starts at zero.
        assert!(laid.words.first().is_some_and(|w| w.x.abs() < 1e-6));
    }

    #[test]
    fn a_run_reaching_past_the_words_yields_nothing_rather_than_panicking() {
        assert_eq!(word_range(5, 0, 5), 0..5);
        assert_eq!(word_range(5, 3, 9), 3..5);
        assert_eq!(word_range(5, 9, 2), 5..5);
        assert_eq!(word_range(0, 0, 1), 0..0);
    }

    #[test]
    fn a_line_the_resolver_cannot_order_falls_back_to_one_run() {
        // A lone paragraph separator: the resolver reports no runs, and the
        // fallback lays it out left to right instead of dropping it.
        let laid = laid_out("\u{1D}\u{1D1D}", &plate_config());
        assert_eq!(laid.words.len(), 2);
        let xs: Vec<f32> = laid.words.iter().map(|w| w.x).collect();
        assert!(
            xs.windows(2).all(|pair| pair.get(1) >= pair.first()),
            "{xs:?}"
        );
    }
}
