//! A recorded glyph run as a pdfrum glyph run: which glyphs the page keeps, the text each
//! carries, and the transform that stands them upright in PDF's y-up text space.

use crate::area::{GlyphArea, GlyphTally, Keep};
use crate::faces::Metrics;
use crate::paint;
use crate::run_text::{GlyphSource, RunKey, RunText};
use crate::sources::Sources;
use anyrender::recording::GlyphRunCommand;
use pdfrum_edit::{Canvas, GlyphFont, GlyphRun, Paint, RunGlyph, Stroke};
use peniko::kurbo::Affine;

/// Where a run's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Layout,
    Cmap,
}

/// Draw the glyphs of `run` that `area` keeps, in `font`, onto `canvas`; count them.
pub(crate) fn draw(
    canvas: &mut Canvas<'_, '_>,
    run: &GlyphRunCommand,
    face: (&GlyphFont, &Metrics),
    sources: &Sources,
    area: GlyphArea,
) -> GlyphTally {
    let (font, metrics) = face;
    let Some(colour) = paint::colour(&run.brush) else {
        return GlyphTally::default();
    };
    if run.glyphs.is_empty() || run.font_size <= 0.0 {
        return GlyphTally::default();
    }
    let (text, origin) = text_of(metrics, sources, run);
    let advances = metrics.advances(&run.glyphs, run.font_size);
    let kept: Vec<usize> = (0..run.glyphs.len())
        .filter(|&index| {
            let centre = metrics.centre(run.glyphs[index], advances[index], run.font_size);
            u16::try_from(run.glyphs[index].id).is_ok()
                && area.holds(run.transform * centre) == Keep::Yes
        })
        .collect();
    let mut tally = GlyphTally {
        dropped: run.glyphs.len() - kept.len(),
        ..GlyphTally::default()
    };
    let Some(&first) = kept.first() else {
        return tally;
    };
    tally.drawn = kept.len();
    match origin {
        Origin::Layout => tally.layout_text = kept.len(),
        Origin::Cmap => tally.cmap_text = kept.len(),
    }
    let text = text.select(&kept);
    let glyphs: Vec<RunGlyph> = kept
        .iter()
        .zip(text.clusters())
        .filter_map(|(&index, cluster)| {
            let glyph = run.glyphs[index];
            Some(RunGlyph {
                id: u16::try_from(glyph.id).ok()?,
                x: f64::from(glyph.x),
                // Run space is y-down; text space is y-up.
                y: -f64::from(glyph.y),
                text: cluster.clone(),
            })
        })
        .collect();
    let colour = colour.multiply_alpha(run.brush_alpha);
    let emboldened = (run.embolden.x + run.embolden.y).max(0.0);
    canvas.glyphs(&GlyphRun {
        font,
        size: f64::from(run.font_size),
        transform: run.transform * slant(run.glyph_transform, run.glyphs[first].y) * Affine::FLIP_Y,
        glyphs: &glyphs,
        text: text.text(),
        paint: if emboldened > 0.0 {
            Paint::FillStroke(colour, Stroke::new(colour, emboldened))
        } else {
            Paint::Fill(colour)
        },
    });
    tally
}

/// The run's text from the caller if it has it for exactly these glyphs, else from the cmap.
fn text_of(metrics: &Metrics, sources: &Sources, run: &GlyphRunCommand) -> (RunText, Origin) {
    let key = RunKey::new(&run.font_data, run.font_size, run.glyphs.iter().copied());
    match sources.texts.get(&key) {
        Some(text) if text.len() == run.glyphs.len() => (text.clone(), Origin::Layout),
        _ => {
            let chars: Vec<String> = run
                .glyphs
                .iter()
                .map(|glyph| {
                    metrics
                        .chars
                        .get(&glyph.id)
                        .map(char::to_string)
                        .unwrap_or_default()
                })
                .collect();
            let text =
                RunText::from_glyphs(chars.iter().enumerate().map(|(index, text)| GlyphSource {
                    cluster: index,
                    text,
                }));
            (text, Origin::Cmap)
        }
    }
}

/// A synthetic oblique (the glyph transform a renderer applies to each outline, in y-down run
/// space) as one transform about the run's baseline, which leaves every glyph origin in place.
fn slant(glyph_transform: Option<Affine>, baseline: f32) -> Affine {
    glyph_transform.map_or(Affine::IDENTITY, |slant| {
        let baseline = f64::from(baseline);
        Affine::translate((0.0, baseline)) * slant * Affine::translate((0.0, -baseline))
    })
}

#[cfg(test)]
mod tests {
    use super::slant;
    use peniko::kurbo::{Affine, Point};

    #[test]
    fn a_slant_leaves_the_baseline_in_place() {
        let skew = slant(Some(Affine::skew(0.25, 0.0)), 20.0);
        assert_eq!(skew * Point::new(7.0, 20.0), Point::new(7.0, 20.0));
        assert_ne!(skew * Point::new(7.0, 10.0), Point::new(7.0, 10.0));
    }
}
