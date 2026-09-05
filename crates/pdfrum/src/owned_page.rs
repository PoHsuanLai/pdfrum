//! A page that owns its document: the handle a caller without a lifetime
//! holds.

use std::sync::Arc;

use pdfrum_common::PageIndex;

use crate::{
    Annotation, Document, Page, PageImage, PageLink, Pixmap, RasterBackend, RenderOptions,
    RenderSession, Result, Rotation, TextPage, Word,
};

/// One page of a [`Document`], holding the document rather than borrowing
/// it.
///
/// [`Page`] borrows its document, which is the right shape for a loop over
/// `doc.pages()`. It is the wrong shape for a handle that outlives the scope
/// it was made in — a page stored in a struct beside its document, sent to
/// another thread, or handed across a language boundary where no lifetime
/// can follow it. An `OwnedPage` is that handle: it keeps an
/// `Arc<Document>`, and the document lives as long as any page of it does.
///
/// The read-only surface of [`Page`], under the same names: geometry,
/// [`render`](OwnedPage::render), [`text`](OwnedPage::text),
/// [`words`](OwnedPage::words), [`links`](OwnedPage::links),
/// [`annotations`](OwnedPage::annotations), [`images`](OwnedPage::images),
/// [`structure`](OwnedPage::structure), and with the `markdown` feature
/// [`markdown`](OwnedPage::markdown). Each is the borrowed method, called
/// through a [`Page`] this handle lends its record to for the length of the
/// call: the dictionary and boxes a `Page` reads at load are read once here
/// too and kept, so nothing is re-derived per call and a render costs what
/// [`Page::render`] costs. `Send + Sync`, like everything else here.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum::{Document, RenderOptions, VelloCpuBackend};
///
/// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
/// let page = doc.page_owned(0)?;
/// drop(doc); // the page keeps the document alive
///
/// let pixmap = page.render(&VelloCpuBackend::new(), &RenderOptions::default())?;
/// assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
/// assert!(page.text().to_string().contains("Hello, world!"));
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct OwnedPage {
    doc: Arc<Document>,
    page: PageRecord,
}

/// What a [`Page`] reads at load and keeps — everything but the document it
/// borrows.
#[derive(Debug, Clone)]
struct PageRecord {
    dict: Arc<pdfrum_parser::PageDict>,
    index: PageIndex,
    media_box: kurbo::Rect,
    crop_box: kurbo::Rect,
    rotation: Rotation,
}

impl OwnedPage {
    /// Reads the page once, as [`Document::page`] does, and keeps what it
    /// read beside the document.
    pub(crate) fn load(doc: &Arc<Document>, index: PageIndex) -> Result<OwnedPage> {
        let page = doc.page(index)?;
        Ok(OwnedPage {
            page: PageRecord {
                dict: page.dict,
                index: page.index,
                media_box: page.media_box,
                crop_box: page.crop_box,
                rotation: page.rotation,
            },
            doc: Arc::clone(doc),
        })
    }

    /// The borrowed page every method delegates to: this handle's record
    /// over a borrow of the document it holds. The one cost above the
    /// borrowed path — one reference count on the shared dictionary.
    fn page(&self) -> Page<'_> {
        Page {
            doc: &self.doc,
            dict: Arc::clone(&self.page.dict),
            index: self.page.index,
            media_box: self.page.media_box,
            crop_box: self.page.crop_box,
            rotation: self.page.rotation,
        }
    }

    /// The document this page belongs to — the one every page from the same
    /// [`Document::page_owned`] call shares.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let page = doc.page_owned(0)?;
    /// assert!(Arc::ptr_eq(page.document(), &doc));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn document(&self) -> &Arc<Document> {
        &self.doc
    }

    /// The page's zero-based index in the document.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, PageIndex};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world_2_pages.pdf")?);
    /// assert_eq!(doc.page_owned(1)?.index(), PageIndex::from(1));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn index(&self) -> PageIndex {
        self.page.index
    }

    /// The page's displayed width in points, after rotation — as
    /// [`Page::width`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert_eq!(doc.page_owned(0)?.width().round(), 200.0);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn width(&self) -> f64 {
        self.page().width()
    }

    /// The page's displayed height in points, after rotation — as
    /// [`Page::height`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert_eq!(doc.page_owned(0)?.height().round(), 200.0);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn height(&self) -> f64 {
        self.page().height()
    }

    /// The page's `/MediaBox` — as [`Page::media_box`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let page = doc.page_owned(0)?;
    /// assert_eq!(page.media_box(), doc.page(0)?.media_box());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn media_box(&self) -> kurbo::Rect {
        self.page.media_box
    }

    /// The page's `/CropBox`, intersected with the media box — as
    /// [`Page::crop_box`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let page = doc.page_owned(0)?;
    /// assert_eq!(page.crop_box(), doc.page(0)?.crop_box());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn crop_box(&self) -> kurbo::Rect {
        self.page.crop_box
    }

    /// The page's `/BleedBox`, only when the page sets one — as
    /// [`Page::bleed_box`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert!(doc.page_owned(0)?.bleed_box().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn bleed_box(&self) -> Option<kurbo::Rect> {
        self.page().bleed_box()
    }

    /// The page's `/TrimBox`, only when the page sets one — as
    /// [`Page::trim_box`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert!(doc.page_owned(0)?.trim_box().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn trim_box(&self) -> Option<kurbo::Rect> {
        self.page().trim_box()
    }

    /// The page's `/ArtBox`, only when the page sets one — as
    /// [`Page::art_box`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert!(doc.page_owned(0)?.art_box().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn art_box(&self) -> Option<kurbo::Rect> {
        self.page().art_box()
    }

    /// The page's `/Rotate`, normalized to a quarter turn — as
    /// [`Page::rotation`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, Rotation};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert_eq!(doc.page_owned(0)?.rotation(), Rotation::None);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn rotation(&self) -> Rotation {
        self.page.rotation
    }

    /// Renders the page on the rasterizer you name, with caches of its own —
    /// as [`Page::render`], byte for byte.
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, RenderOptions, VelloCpuBackend};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let backend = VelloCpuBackend::new();
    /// let owned = doc.page_owned(0)?.render(&backend, &RenderOptions::default())?;
    /// let borrowed = doc.page(0)?.render(&backend, &RenderOptions::default())?;
    /// assert_eq!(owned.data(), borrowed.data());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn render<B: RasterBackend>(&self, backend: &B, options: &RenderOptions) -> Result<Pixmap> {
        self.page().render(backend, options)
    }

    /// [`OwnedPage::render`] reusing a caller-owned [`RenderSession`] — as
    /// [`Page::render_on`].
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, RenderOptions, RenderSession, VelloCpuBackend};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world_2_pages.pdf")?);
    /// let backend = VelloCpuBackend::new();
    /// let mut session = RenderSession::new();
    /// for page in doc.pages_owned() {
    ///     let pixmap = page?.render_on(&backend, &RenderOptions::default(), &mut session)?;
    ///     assert!(pixmap.width() > 0);
    /// }
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        self.page().render_on(backend, options, session)
    }

    /// Extracts the page's text — as [`Page::text`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let text = doc.page_owned(0)?.text();
    /// assert!(text.to_string().contains("Hello, world!"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn text(&self) -> TextPage {
        self.page().text()
    }

    /// [`OwnedPage::text`] reusing a caller-owned [`RenderSession`] — as
    /// [`Page::text_on`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, RenderSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let mut session = RenderSession::new();
    /// let text = doc.page_owned(0)?.text_on(&mut session);
    /// assert!(text.to_string().contains("Hello, world!"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn text_on(&self, session: &mut RenderSession) -> TextPage {
        self.page().text_on(session)
    }

    /// The page's words in reading order, each with its box — as
    /// [`Page::words`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let words = doc.page_owned(0)?.words();
    /// let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    /// assert_eq!(texts, ["Hello,", "world!", "Goodbye,", "world!"]);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn words(&self) -> Vec<Word> {
        self.page().words()
    }

    /// The page's annotations, in `/Annots` order — as
    /// [`Page::annotations`]. Each borrows this handle rather than the
    /// document, so they live as long as the page does.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, Subtype};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let page = doc.page_owned(0)?;
    /// let annots = page.annotations();
    /// assert!(annots.iter().any(|a| a.subtype() == Subtype::Widget));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn annotations(&self) -> Vec<Annotation<'_>> {
        self.page().annotations()
    }

    /// The page's link annotations with the destination or action each one
    /// carries — as [`Page::links`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert!(doc.page_owned(0)?.links().is_empty());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn links(&self) -> Vec<pdfrum_doc::Link> {
        self.page().links()
    }

    /// The page's links with where each one leads — as
    /// [`Page::page_links`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert!(doc.page_owned(0)?.page_links().is_empty());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn page_links(&self) -> Vec<PageLink> {
        self.page().page_links()
    }

    /// The images the page draws, in drawing order — as [`Page::images`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/jpx_two_sizes.pdf")?);
    /// let images = doc.page_owned(0)?.images();
    /// assert_eq!(images.len(), doc.page(0)?.images().len());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn images(&self) -> Vec<PageImage> {
        self.page().images()
    }

    /// The page's view of the document's structure tree, or `None` when
    /// the document is not tagged — as [`Page::structure`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// assert!(doc.page_owned(0)?.structure().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn structure(&self) -> Option<pdfrum_doc::structure::StructTree> {
        self.page().structure()
    }

    /// The page as GitHub-flavoured Markdown — as [`Page::markdown`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let markdown = doc.page_owned(0)?.markdown();
    /// assert_eq!(markdown, doc.page(0)?.markdown());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[cfg(feature = "markdown")]
    #[must_use]
    pub fn markdown(&self) -> String {
        self.page().markdown()
    }

    /// The page's content as Markdown blocks — as [`Page::markdown_blocks`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let blocks = doc.page_owned(0)?.markdown_blocks();
    /// assert_eq!(blocks.len(), doc.page(0)?.markdown_blocks().len());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[cfg(feature = "markdown")]
    #[must_use]
    pub fn markdown_blocks(&self) -> Vec<crate::Block> {
        self.page().markdown_blocks()
    }

    /// The page's text with its layout kept — as [`Page::layout_text`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let text = doc.page_owned(0)?.layout_text();
    /// assert_eq!(text, doc.page(0)?.layout_text());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[cfg(feature = "markdown")]
    #[must_use]
    pub fn layout_text(&self) -> String {
        self.page().layout_text()
    }
}

impl Document {
    /// One page that holds the document rather than borrowing it — see
    /// [`OwnedPage`].
    ///
    /// Takes `&Arc<Document>` because the page keeps a reference count: a
    /// document is made an `Arc` once, by the caller who will hand its pages
    /// around, and every page from it shares that one allocation.
    ///
    /// # Errors
    ///
    /// As [`Document::page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world.pdf")?);
    /// let page = doc.page_owned(0)?;
    /// assert_eq!(page.width().round(), 200.0);
    /// assert!(doc.page_owned(1).is_err(), "there is only one page");
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn page_owned(self: &Arc<Self>, index: impl Into<PageIndex>) -> Result<OwnedPage> {
        OwnedPage::load(self, index.into())
    }

    /// Every page in order as an [`OwnedPage`], read lazily as the iterator
    /// advances.
    ///
    /// Unlike [`Document::pages`], a page that will not load is **yielded as
    /// its error** rather than skipped, so the index of each item is the
    /// page's own and a caller collecting into a `Vec` sees every failure.
    /// The iterator holds its own reference to the document, so it may
    /// outlive the `Arc` it was made from.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::Document;
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/hello_world_2_pages.pdf")?);
    /// let pages: Vec<_> = doc.pages_owned().collect::<Result<_, _>>()?;
    /// assert_eq!(pages.len(), 2);
    /// assert_eq!(pages[1].index(), 1.into());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn pages_owned(self: &Arc<Self>) -> impl Iterator<Item = Result<OwnedPage>> + 'static {
        let doc = Arc::clone(self);
        (0..self.page_count()).map(move |index| doc.page_owned(index))
    }
}
