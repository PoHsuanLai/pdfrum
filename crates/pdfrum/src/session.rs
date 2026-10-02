//! The caches one run of many pages reuses.

use pdfrum_common::Deadline;
use pdfrum_page::BuildContext;
use pdfrum_render::RenderCaches;

/// Everything a run of many pages can reuse between them: the resources a
/// page is *built* from, and the glyph outlines it is *drawn* with.
///
/// Two caches, both public: [`BuildContext`] holds fonts, colour spaces,
/// functions and decoded images; [`RenderCaches`] holds the flattened glyph
/// outlines the rasterizer draws.
///
/// Reached through [`Page::render_on`](crate::Page::render_on) and
/// [`Page::text_on`](crate::Page::text_on); extraction moves only the `build`
/// half, so one session serves a run that does both. It is used through
/// `&mut`, so under `rayon` each worker keeps its own:
/// `pages.par_iter().map_init(RenderSession::new, |session, page| …)`.
///
/// ```
/// use pdfrum::{Document, RenderOptions, RenderSession, VelloCpuBackend};
///
/// let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
/// let mut session = RenderSession::new();
///
/// // Both pages share one set of caches: the fonts are parsed once, and so
/// // are the glyph outlines drawn from them.
/// for page in doc.pages() {
///     let pixmap = page.render_on(VelloCpuBackend, &RenderOptions::default(), &mut session)?;
///     assert!(pixmap.width() > 0);
/// }
/// # Ok::<(), pdfrum::Error>(())
/// ```
///
/// # When not to use it
///
/// Type-3 glyph snapping is order-dependent by design, so a page drawn with a
/// warm cache can differ by a snapped pixel from the same page drawn cold.
/// For a byte-identical per-page baseline call
/// [`Page::render`](crate::Page::render), which gives every page fresh caches.
#[derive(Debug, Default)]
pub struct RenderSession {
    /// Fonts, colour spaces, functions and decoded images — what a page is
    /// built from.
    pub build: BuildContext,
    /// Flattened glyph outlines — what the rasterizer draws.
    pub caches: RenderCaches,
    /// The stop for the next renders through this session, if any.
    pub(crate) deadline: Option<Deadline>,
}

impl RenderSession {
    /// Empty caches for a new run.
    #[must_use]
    pub fn new() -> RenderSession {
        RenderSession::default()
    }

    /// Gives the renders that follow through this session a stop of their
    /// own, or removes it with `None`.
    ///
    /// Unlike [`Limits::deadline`](crate::Limits::deadline), which belongs to
    /// the document and stays passed once passed, this one belongs to the
    /// session: a render that finds it passed (or raised with
    /// [`Deadline::stop`]) fails with
    /// [`Error::Limit`](crate::Error::Limit), and the document is untouched.
    /// Set a fresh [`Deadline`] for the next render — the passed one stays
    /// passed — or clear it. A viewer scrolling past a tile keeps a clone of
    /// the tile's `Deadline`, calls `stop` on it, and renders the next tile
    /// with a new one.
    ///
    /// It bounds the rasterizing, checked before the target is allocated and
    /// per drawn object, and is also checked before a page is interpreted.
    /// Interpreting a page for [`Page::render_on`](crate::Page::render_on) is
    /// not interrupted partway; to stop mid-interpretation use the
    /// document's deadline, or [`Page::prepare`](crate::Page::prepare) once
    /// and cancel the draws. The document's own deadline still applies as
    /// well; either one passing ends the render.
    ///
    /// ```
    /// use pdfrum::{
    ///     Deadline, Document, Error, LimitExceeded, RenderOptions, RenderSession, VelloCpuBackend,
    /// };
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let page = doc.page(0)?;
    /// let mut session = RenderSession::new();
    ///
    /// let stale = Deadline::manual();
    /// session.set_deadline(Some(stale.clone()));
    /// stale.stop();
    /// let result = page.render_on(VelloCpuBackend, &RenderOptions::default(), &mut session);
    /// assert!(matches!(result, Err(Error::Limit(LimitExceeded::Stopped { .. }))));
    ///
    /// // The document is fine; the next render is simply given no stop.
    /// session.set_deadline(None);
    /// assert!(page.render_on(VelloCpuBackend, &RenderOptions::default(), &mut session).is_ok());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn set_deadline(&mut self, deadline: Option<Deadline>) {
        self.deadline = deadline;
    }

    /// The stop set with [`RenderSession::set_deadline`], if any.
    #[must_use]
    pub fn deadline(&self) -> Option<&Deadline> {
        self.deadline.as_ref()
    }
}
