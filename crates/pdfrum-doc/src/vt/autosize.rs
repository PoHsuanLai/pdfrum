//! Choosing a size when the field asks for one.
//!
//! A ladder of twenty-five ascending sizes, searched for the largest that
//! fits.
//!
//! The search is a binary search whose predicate is "this size still fits", so
//! it lands on the **first size that does not**, and the answer is the step
//! before it. Both ends are special-cased in the direction that reads
//! naturally: a field where even the smallest step overflows takes the
//! smallest, and one where every step fits takes the largest.
//!
//! **Corrected 2026-08-29.** This was ported with the predicate inverted —
//! reading the search as landing on the first size that *fits* and returning
//! the step before, which is one that overflows — and the module said so, at
//! length, as deliberate behavior. It is not: `lower_bound`'s comparator is
//! `!IsBigger(size)`, and `lower_bound` returns the first element for which
//! the comparator is **false**, so it finds the first size that is *bigger*
//! than the plate. Every field with no explicit size was therefore set at 4
//! rather than at the size that fills its box. Nothing caught it because the
//! only consumer until now was a free-text annotation, which almost always
//! carries a size in its `/DA`; `calculate.pdf`'s two `/Tx` widgets carry none
//! and their goldens show twelve-point digits where this produced specks.

use crate::vt::{Config, FONT_SIZE_STEPS, Metrics, Section, split};

/// The size a field with no explicit one is set at.
///
/// A **multi-line** field only considers the first quarter of the ladder —
/// six steps — so it can never auto-size above 12. A plate with no width
/// answers zero, which the caller turns into "write no font operator at all".
#[must_use]
pub fn auto_font_size(sections: &[Section], config: &Config, metrics: &Metrics<'_>) -> f32 {
    if crate::geom::width(config.plate) <= 0.0 {
        return 0.0;
    }
    let span = if config.multi_line {
        FONT_SIZE_STEPS.len() / 4
    } else {
        FONT_SIZE_STEPS.len()
    };
    let steps = FONT_SIZE_STEPS.get(..span).unwrap_or(&FONT_SIZE_STEPS);

    let first_too_big = steps
        .iter()
        .position(|size| is_bigger(sections, config, metrics, f32::from(*size)));

    let chosen = match first_too_big {
        // Every step fits: the largest considered.
        None => steps.last().copied(),
        // Even the smallest overflows: the smallest, rather than nothing.
        Some(0) => steps.first().copied(),
        // Otherwise the largest that still fits.
        Some(index) => steps.get(index - 1).copied(),
    };
    chosen.map_or(0.0, f32::from)
}

/// Whether the text overflows the plate at this size, in either dimension.
///
/// Measured section by section: the width is the widest, the height is the
/// sum, and the first section that pushes either past the plate answers true
/// without measuring the rest.
#[must_use]
pub fn is_bigger(
    sections: &[Section],
    config: &Config,
    metrics: &Metrics<'_>,
    font_size: f32,
) -> bool {
    let (mut width, mut height) = (0.0_f32, 0.0_f32);
    for section in sections {
        let mut probe = section.clone();
        let size = split::split_at_size(&mut probe, config, metrics, false, font_size);
        width = crate::geom::width(size).max(width);
        height += crate::geom::height(size);
        if crate::geom::is_float_bigger(width, crate::geom::width(config.plate))
            || crate::geom::is_float_bigger(height, crate::geom::height(config.plate))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::auto_font_size;
    use crate::geom;
    use crate::vt::{Config, Section, Word, stub};

    fn sections(text: &str) -> Vec<Section> {
        vec![Section {
            words: text
                .chars()
                .map(|ch| Word {
                    ch: ch as u32,
                    font_index: 0,
                    x: 0.0,
                    y: 0.0,
                    tail: 0.0,
                    is_rtl: false,
                })
                .collect(),
            lines: Vec::new(),
            rect: kurbo::Rect::ZERO,
        }]
    }

    #[test]
    fn a_plate_with_no_width_answers_zero() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.0, 100.0),
            ..Config::default()
        };
        assert!(auto_font_size(&sections("hi"), &config, &stub::metrics()).abs() < f32::EPSILON);
    }

    #[test]
    fn a_plate_that_fits_even_the_largest_step_gets_the_largest() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 100_000.0, 100_000.0),
            ..Config::default()
        };
        // No step overflows, so the search runs off the end and the ladder's
        // last entry is the answer.
        assert!(
            (auto_font_size(&sections("hi"), &config, &stub::metrics()) - 144.0).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn a_plate_too_small_for_anything_gets_the_smallest_candidate() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.001, 0.001),
            ..Config::default()
        };
        // Even the smallest step overflows, and the end guard returns it
        // rather than nothing.
        assert!(
            (auto_font_size(&sections("hi"), &config, &stub::metrics()) - 4.0).abs() < f32::EPSILON
        );
    }

    #[test]
    fn a_multi_line_field_never_sizes_above_twelve() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 100_000.0, 100_000.0),
            multi_line: true,
            ..Config::default()
        };
        // Everything fits, so the largest *considered* step is returned — and
        // for a multi-line field the ladder stops at twelve.
        assert!(
            (auto_font_size(&sections("hi"), &config, &stub::metrics()) - 12.0).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn the_answer_is_the_largest_step_that_still_fits() {
        // The stub font is ten units wide per character at a thousandth of an
        // em, so "hi" sets `size / 50` wide and `size * 12 / 1000` tall. Each
        // plate below admits exactly one step and not the next.
        for (plate, expected) in [
            // 0.6 wide admits 25 (0.5) and not 30 (0.6 is not *bigger* than
            // 0.6 — the comparison has a tolerance — so 30 fits too, and 35
            // at 0.7 does not).
            (geom::rect(0.0, 0.0, 0.6, 100.0), 30.0),
            // Tall and narrow: width decides, and 0.2 admits 10 exactly.
            (geom::rect(0.0, 0.0, 0.2, 100.0), 10.0),
            // Wide and short: height decides, `size * 0.012 <= 0.1` gives 8.
            (geom::rect(0.0, 0.0, 10_000.0, 0.1), 8.0),
        ] {
            let config = Config {
                plate,
                ..Config::default()
            };
            let chosen = auto_font_size(&sections("hi"), &config, &stub::metrics());
            assert!(
                (chosen - expected).abs() < f32::EPSILON,
                "{plate:?}: got {chosen}, wanted {expected}"
            );
        }
    }
}
