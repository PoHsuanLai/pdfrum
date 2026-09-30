//! Which glyphs a page keeps, and a count of what happened to them.

use peniko::kurbo::{Point, Rect};

/// Where a page's glyphs may land, in its scene's coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GlyphArea {
    /// Every glyph is drawn.
    #[default]
    Anywhere,
    /// A glyph is drawn only if the centre of its box (its advance across, ascent to descent
    /// down) lies inside: a page's clip hides what falls outside it, but a PDF's text layer
    /// does not, so a glyph under the clip would still be found, selected and copied from the
    /// wrong page. The centre decides so each glyph belongs to exactly one of two pages that
    /// share an edge.
    Within(Rect),
}

/// Whether a glyph is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keep {
    Yes,
    No,
}

impl GlyphArea {
    pub(crate) fn holds(self, point: Point) -> Keep {
        match self {
            GlyphArea::Anywhere => Keep::Yes,
            GlyphArea::Within(rect) => {
                let inside = point.x >= rect.x0
                    && point.x < rect.x1
                    && point.y >= rect.y0
                    && point.y < rect.y1;
                if inside { Keep::Yes } else { Keep::No }
            }
        }
    }
}

/// What the pages did with the glyphs they were given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GlyphTally {
    /// Glyphs written.
    pub drawn: usize,
    /// Glyphs outside their page's [`GlyphArea`] (or past the 16-bit glyph ids PDF can
    /// name), not written.
    pub dropped: usize,
    /// Written glyphs whose text came from the caller's [`RunTexts`](crate::RunTexts).
    pub layout_text: usize,
    /// Written glyphs whose text was read back from the font's cmap.
    pub cmap_text: usize,
}

impl std::ops::Add for GlyphTally {
    type Output = GlyphTally;

    fn add(self, other: GlyphTally) -> GlyphTally {
        GlyphTally {
            drawn: self.drawn + other.drawn,
            dropped: self.dropped + other.dropped,
            layout_text: self.layout_text + other.layout_text,
            cmap_text: self.cmap_text + other.cmap_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GlyphArea, Keep};
    use peniko::kurbo::{Point, Rect};

    #[test]
    fn the_area_keeps_its_top_edge_and_not_its_bottom() {
        const CASES: &[((f64, f64), Keep)] = &[
            ((10.0, 0.0), Keep::Yes),
            ((10.0, 49.9), Keep::Yes),
            ((10.0, 50.0), Keep::No),
            ((-0.1, 10.0), Keep::No),
            ((100.0, 10.0), Keep::No),
        ];
        let area = GlyphArea::Within(Rect::new(0.0, 0.0, 100.0, 50.0));
        for &((x, y), want) in CASES {
            assert_eq!(area.holds(Point::new(x, y)), want, "({x}, {y})");
        }
        assert_eq!(GlyphArea::Anywhere.holds(Point::new(1e9, -1e9)), Keep::Yes);
    }
}
