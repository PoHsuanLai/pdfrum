//! The text behind a glyph run, handed over by whoever shaped it. anyrender's `draw_glyphs`
//! carries glyph ids and positions only; a layout engine that still has the run (parley keeps
//! each cluster's byte range in its source text) can record it here under the key the painter
//! will compute from the same call, so the PDF's text is the text that was shaped, not a guess
//! from the font's cmap. That is what makes a ligature (one `fi` glyph) copy as `fi`, and a
//! glyph two code points share (一 and the Kangxi radical ⼀ in CJK fonts) copy as the one the
//! author typed.

use anyrender::Glyph;
use peniko::FontData;
use std::collections::HashMap;
use std::ops::Range;

/// A glyph run as the painter sees it: the font face, the size and every glyph's id and
/// position, bit for bit. Two runs with the same key draw the same thing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunKey {
    face: (u64, u32),
    size: u32,
    glyphs: Vec<(u32, u32, u32)>,
}

impl RunKey {
    /// The key of a run of `glyphs` in `font` at `font_size`, exactly as passed to
    /// `draw_glyphs`.
    pub fn new(font: &FontData, font_size: f32, glyphs: impl IntoIterator<Item = Glyph>) -> Self {
        RunKey {
            face: (font.data.id(), font.index),
            size: font_size.to_bits(),
            glyphs: glyphs
                .into_iter()
                .map(|glyph| (glyph.id, glyph.x.to_bits(), glyph.y.to_bits()))
                .collect(),
        }
    }
}

/// One glyph's share of the source text: the cluster it belongs to (glyphs of one cluster
/// share it) and that cluster's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSource<'a> {
    /// Any value that is the same for every glyph of one cluster and differs between
    /// neighbouring clusters: the cluster's start in the source text is the natural one.
    pub cluster: usize,
    /// The cluster's text: one character usually, several for a ligature, a whole grapheme
    /// for a base with its marks.
    pub text: &'a str,
}

/// The text of one run and, for each glyph in drawing order, the byte range of its cluster in
/// that text. Glyphs of one cluster share a range; pdfrum writes it once, as `/ActualText`
/// when the glyphs cannot carry it in the font's ToUnicode map.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunText {
    text: String,
    clusters: Vec<Range<usize>>,
}

impl RunText {
    /// The run text for glyphs in drawing order.
    pub fn from_glyphs<'a>(glyphs: impl IntoIterator<Item = GlyphSource<'a>>) -> Self {
        let mut run = RunText::default();
        let mut last: Option<usize> = None;
        for glyph in glyphs {
            match (last, run.clusters.last().cloned()) {
                (Some(cluster), Some(range)) if cluster == glyph.cluster => {
                    run.clusters.push(range);
                }
                _ => {
                    let start = run.text.len();
                    run.text.push_str(glyph.text);
                    run.clusters.push(start..run.text.len());
                }
            }
            last = Some(glyph.cluster);
        }
        run
    }

    /// The whole text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Each glyph's cluster range in [`RunText::text`].
    pub fn clusters(&self) -> &[Range<usize>] {
        &self.clusters
    }

    /// How many glyphs the run has.
    pub fn len(&self) -> usize {
        self.clusters.len()
    }

    /// Whether the run has no glyphs.
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// The run cut down to the glyphs at `kept` (ascending indices): the ones a page keeps.
    /// A cluster is kept whole if any of its glyphs is. Glyphs are of one cluster when their
    /// ranges are equal, not merely when they start together: an empty range (a glyph with no
    /// text) and the next glyph's share a start and are still two clusters.
    pub(crate) fn select(&self, kept: &[usize]) -> RunText {
        let mut run = RunText::default();
        let mut last: Option<&Range<usize>> = None;
        for &index in kept {
            let Some(range) = self.clusters.get(index) else {
                continue;
            };
            match (last, run.clusters.last().cloned()) {
                (Some(previous), Some(held)) if previous == range => run.clusters.push(held),
                _ => {
                    let start = run.text.len();
                    run.text
                        .push_str(self.text.get(range.clone()).unwrap_or(""));
                    run.clusters.push(start..run.text.len());
                }
            }
            last = Some(range);
        }
        run
    }
}

/// Run texts by key. When two runs share a key but not a text (only possible when two code
/// points share a glyph and everything else about the runs matches), the first recorded wins:
/// record in document order.
#[derive(Debug, Default)]
pub struct RunTexts {
    runs: HashMap<RunKey, RunText>,
}

impl RunTexts {
    /// Record `text` for runs keyed `key`, unless a run with that key was recorded first.
    pub fn insert(&mut self, key: RunKey, text: RunText) {
        self.runs.entry(key).or_insert(text);
    }

    /// The text recorded for `key`.
    pub fn get(&self, key: &RunKey) -> Option<&RunText> {
        self.runs.get(key)
    }

    /// How many distinct runs have text.
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether no run has text.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{GlyphSource, RunText};

    fn glyph(cluster: usize, text: &str) -> GlyphSource<'_> {
        GlyphSource { cluster, text }
    }

    #[test]
    fn a_ligature_glyph_covers_its_characters() {
        let run = RunText::from_glyphs([glyph(0, "fi"), glyph(2, "n"), glyph(3, "d")]);
        assert_eq!(run.text(), "find");
        assert_eq!(run.clusters(), &[0..2, 2..3, 3..4]);
    }

    #[test]
    fn glyphs_of_one_cluster_share_its_range() {
        let run = RunText::from_glyphs([glyph(0, "é"), glyph(0, "é"), glyph(3, "t")]);
        assert_eq!(run.text(), "ét");
        assert_eq!(run.clusters(), &[0..2, 0..2, 2..3]);
    }

    #[test]
    fn a_glyph_without_text_does_not_swallow_the_next() {
        // A ligature the cmap cannot name (no text), then `c`: two clusters, though both
        // ranges start at the same byte.
        let run = RunText::from_glyphs([glyph(0, ""), glyph(1, "c")]);
        let kept = run.select(&[0, 1]);
        assert_eq!(kept.text(), "c");
        assert_eq!(kept.clusters(), &[0..0, 0..1]);
    }

    #[test]
    fn selecting_keeps_whole_clusters_in_order() {
        let run = RunText::from_glyphs([glyph(0, "a"), glyph(1, "fi"), glyph(3, "b")]);
        let kept = run.select(&[1, 2]);
        assert_eq!(kept.text(), "fib");
        assert_eq!(kept.clusters(), &[0..2, 2..3]);
    }
}
