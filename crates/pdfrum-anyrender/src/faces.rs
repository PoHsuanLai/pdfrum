//! What the painter reads from a face itself: its reverse cmap for runs without text, its
//! vertical metrics and advances for placing each glyph's box.

use anyrender::{Glyph, NormalizedCoord};
use peniko::FontData;
use skrifa::instance::{LocationRef, NormalizedCoord as Coord, Size};
use skrifa::{FontRef, GlyphId, MetadataProvider as _};
use std::collections::HashMap;

/// A face at one instance: the blob, the index in it, the normalized coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FaceKey {
    blob: u64,
    index: u32,
    coords: Vec<NormalizedCoord>,
}

impl FaceKey {
    pub(crate) fn of(font: &FontData, coords: &[NormalizedCoord]) -> Self {
        FaceKey {
            blob: font.data.id(),
            index: font.index,
            coords: coords.to_vec(),
        }
    }
}

/// A face's metrics at one instance.
pub(crate) struct Metrics {
    /// Each glyph's preferred character.
    pub(crate) chars: HashMap<u32, char>,
    /// Ascent and descent (negative below the baseline) per unit of font size.
    ascent: f32,
    descent: f32,
    data: FontData,
    coords: Vec<NormalizedCoord>,
}

impl Metrics {
    /// The metrics of `font` at `coords`; `None` for bytes skrifa cannot read.
    pub(crate) fn load(font: &FontData, coords: &[NormalizedCoord]) -> Option<Self> {
        let parsed = FontRef::from_index(font.data.data(), font.index).ok()?;
        let metrics = parsed.metrics(Size::unscaled(), LocationRef::default());
        let per_em = f32::from(metrics.units_per_em.max(1));
        Some(Metrics {
            chars: crate::cmap::reverse(&parsed),
            ascent: metrics.ascent / per_em,
            descent: metrics.descent / per_em,
            data: font.clone(),
            coords: coords.to_vec(),
        })
    }

    /// Each of `glyphs`' advance at `size`, at this instance; half an em for a glyph the face
    /// does not know.
    pub(crate) fn advances(&self, glyphs: &[Glyph], size: f32) -> Vec<f32> {
        let fallback = size / 2.0;
        let Ok(font) = FontRef::from_index(self.data.data.data(), self.data.index) else {
            return vec![fallback; glyphs.len()];
        };
        let location: Vec<Coord> = self.coords.iter().map(|&c| Coord::from_bits(c)).collect();
        let metrics = font.glyph_metrics(Size::new(size), LocationRef::new(&location));
        glyphs
            .iter()
            .map(|glyph| {
                metrics
                    .advance_width(GlyphId::new(glyph.id))
                    .unwrap_or(fallback)
            })
            .collect()
    }

    /// The centre of `glyph`'s box, in run space (y down): half its advance across, midway
    /// between ascent and descent from the baseline.
    pub(crate) fn centre(&self, glyph: Glyph, advance: f32, size: f32) -> peniko::kurbo::Point {
        peniko::kurbo::Point::new(
            f64::from(glyph.x + advance / 2.0),
            f64::from(glyph.y - f32::midpoint(self.ascent, self.descent) * size),
        )
    }
}
