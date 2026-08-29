//! Choosing a size when the field asks for one.
//!
//! A ladder of twenty-five sizes, searched for the largest that fits — except
//! it does not return that one.
//!
//! The search partitions the ladder into "too big" then "fits" and finds the
//! **first fitting** size, then returns the step **before** it, which by
//! construction is one that overflows. It looks like an off-by-one, it is
//! visible in rendered output, and it is the behavior; only the two ends are
//! special-cased, so a field where nothing fits gets the largest candidate
//! and one where everything fits gets the smallest.

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

    let first_fitting = steps
        .iter()
        .position(|size| !is_bigger(sections, config, metrics, f32::from(*size)));

    let chosen = match first_fitting {
        // Nothing fits: the largest candidate.
        None => steps.last().copied(),
        // Even the smallest fits: the smallest.
        Some(0) => steps.first().copied(),
        // Otherwise the step before the first that fits — deliberately one
        // that does not.
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
        // Every step fits, so the first fitting is index zero and the
        // smallest is returned.
        assert!(
            (auto_font_size(&sections("hi"), &config, &stub::metrics()) - 4.0).abs() < f32::EPSILON
        );
    }

    #[test]
    fn a_plate_too_small_for_anything_gets_the_largest_candidate() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.001, 0.001),
            ..Config::default()
        };
        assert!(
            (auto_font_size(&sections("hi"), &config, &stub::metrics()) - 144.0).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn a_multi_line_field_never_sizes_above_twelve() {
        let config = Config {
            plate: geom::rect(0.0, 0.0, 0.001, 0.001),
            multi_line: true,
            ..Config::default()
        };
        // Nothing fits, so the largest *considered* step is returned — and
        // for a multi-line field the ladder stops at twelve.
        assert!(
            (auto_font_size(&sections("hi"), &config, &stub::metrics()) - 12.0).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn the_search_scans_upwards_so_the_smallest_fitting_step_wins() {
        // This is the shape of the off-by-one. The ladder ascends and
        // "fits" *shrinks* with size, so the partition the search assumes —
        // everything too big, then everything fitting — is upside down: the
        // first step it finds that fits is the smallest one that does, and
        // the answer is the step before that.
        //
        // With a plate that comfortably admits size 4, the first fitting
        // step is index zero, the one guard that special-cases the ends
        // fires, and 4 comes back. Every plate wide and tall enough for the
        // smallest step behaves this way, which is why automatic sizing so
        // often lands at 4 rather than at the largest size that would fit.
        for plate in [
            geom::rect(0.0, 0.0, 1.5, 100.0),
            geom::rect(0.0, 0.0, 10_000.0, 1.0),
            geom::rect(0.0, 0.0, 50.0, 50.0),
        ] {
            let config = Config {
                plate,
                ..Config::default()
            };
            let chosen = auto_font_size(&sections("hi"), &config, &stub::metrics());
            assert!((chosen - 4.0).abs() < f32::EPSILON, "{plate:?}: {chosen}");
        }
    }
}
