//! Pages of recorded scenes written as one PDF.

use crate::area::{GlyphArea, GlyphTally};
use crate::prepare::Prepared;
use crate::replay::Replay;
use crate::sources::Sources;
use anyrender::Scene;
use pdfrum_common::Limits;
use pdfrum_edit::{EditDoc, Error, Fill, SaveOptions, blank_document, save};
use peniko::kurbo::{Affine, Rect, Size};

/// One page: its sheet, what is painted on it, and where.
#[derive(Debug, Clone)]
pub struct Page {
    /// The sheet, in PDF points.
    pub size: Size,
    /// What the page shows, in the renderer's coordinates.
    pub scene: Scene,
    /// Scene coordinates to the page's (PDF points, y up, origin at the sheet's lower left): a
    /// renderer's y-down pixels on a sheet with margins are a scale, a flip and a move.
    pub placement: Affine,
    /// The part of the scene the page shows, in scene coordinates; anything outside is clipped.
    pub clip: Rect,
    /// Which glyphs the page keeps, in scene coordinates.
    pub area: GlyphArea,
}

/// A written PDF and what became of its glyphs.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The file.
    pub bytes: Vec<u8>,
    /// Over every page.
    pub glyphs: GlyphTally,
}

/// `pages` as one PDF, their text and images looked up in `sources`.
///
/// # Errors
///
/// Whatever pdfrum refuses: a face it cannot read, subset or instance, an image it cannot
/// embed, a page size with no area.
pub fn write(pages: &[Page], sources: &Sources) -> Result<Written, Error> {
    let sizes: Vec<Size> = pages.iter().map(|page| page.size).collect();
    let base = blank_document(&sizes)?;
    let mut edit = EditDoc::new(&base);
    let prepared = Prepared::new(&mut edit, pages, &sources.images)?;
    let mut glyphs = GlyphTally::default();
    edit.draw_pages(&Limits::default(), |canvas| {
        let Some(page) = canvas
            .page()
            .and_then(|index| pages.get(usize::try_from(u32::from(index)).ok()?))
        else {
            return;
        };
        canvas.saved(|canvas| {
            canvas.transform(page.placement);
            canvas.clip(page.clip, Fill::NonZero);
            let mut replay = Replay::new(&prepared, sources, page.area);
            replay.run(canvas, &mut page.scene.commands.iter());
            glyphs = glyphs + replay.tally;
        });
    })?;
    let mut bytes = Vec::new();
    save(&edit, &SaveOptions::default(), &mut bytes)?;
    Ok(Written { bytes, glyphs })
}
