//! The caches one run of many pages reuses.

use pdfrum_page::BuildContext;
use pdfrum_render::RenderCaches;

/// Everything a run of many pages can reuse between them: the resources a
/// page is *built* from, and the glyph outlines it is *drawn* with.
///
/// A plain record of two caches, both public, so a caller who wants only one
/// of them can reach it (STYLE.md §1: data, not an object). It exists because
/// the two halves belong to different crates and neither knows about the
/// other — [`BuildContext`] caches fonts, colour spaces, functions and
/// decoded images; [`RenderCaches`] caches the flattened glyph outlines the
/// rasterizer draws. Threading only the first still re-flattens every glyph
/// on every page, which is why the two travel together.
///
/// Reached through [`Page::render_on`](crate::Page::render_on) and
/// [`Page::text_on`](crate::Page::text_on) — extraction moves only the
/// `build` half, so one session serves a run that does both.
/// Like [`BuildContext`] it is used through `&mut`, so under `rayon` each
/// worker keeps its own rather than sharing one behind a lock — see the crate
/// docs on rendering in parallel.
///
/// ```
/// use pdfrum::{Document, RenderOptions, RenderSession, VelloCpuBackend};
///
/// let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
/// let backend = VelloCpuBackend::new();
/// let mut session = RenderSession::new();
///
/// // Both pages share one set of caches: the fonts are parsed once, and so
/// // are the glyph outlines drawn from them.
/// for page in doc.pages() {
///     let pixmap = page.render_on(&backend, &RenderOptions::default(), &mut session)?;
///     assert!(pixmap.width() > 0);
/// }
/// # Ok::<(), pdfrum::Error>(())
/// ```
///
/// # When not to use it
///
/// Type-3 glyph snapping is order-dependent by design, so a page drawn with a
/// warm glyph cache can differ by a snapped pixel from the same page drawn
/// with a cold one. Reuse across the pages of one document is the intended
/// use. For a byte-identical per-page baseline, call
/// [`Page::render`](crate::Page::render), which gives every page fresh caches.
#[derive(Debug, Default)]
pub struct RenderSession {
    /// Fonts, colour spaces, functions and decoded images — what a page is
    /// built from.
    pub build: BuildContext,
    /// Flattened glyph outlines — what the rasterizer draws.
    pub caches: RenderCaches,
}

impl RenderSession {
    /// Empty caches for a new run.
    #[must_use]
    pub fn new() -> RenderSession {
        RenderSession::default()
    }
}
