//! The document outline — what a reader shows as its bookmarks panel.

use pdfrum_common::{Diagnostics, PageIndex};

use crate::Document;

/// A document's outline, flattened to a pre-order walk (ISO 32000-1 §12.3.3).
///
/// The tree shape survives as each entry's [`Bookmark::depth`], which is what
/// a list-shaped UI wants and what a tree-shaped one can rebuild. Cycles —
/// which damaged files do contain — are cut during the walk, so the sequence
/// is always finite.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/bookmarks.pdf")?;
/// let outline = doc.outline();
///
/// // Six entries over two levels, in pre-order: each top-level item is
/// // immediately followed by its own children.
/// let shape: Vec<(usize, String)> =
///     outline.iter().map(|b| (b.depth(), b.title())).collect();
/// assert_eq!(shape[0], (0, "A Good Beginning".into()));
/// assert_eq!(shape[1], (0, "Open Middle".into()));
/// assert_eq!(shape[2], (1, "Open Middle Descendant".into()));
/// assert_eq!(outline.len(), 6);
///
/// // "Open Middle" is drawn expanded; the closed one is not.
/// assert!(outline.iter().any(|b| b.title() == "Open Middle" && b.is_open()));
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct Outline<'a> {
    doc: &'a Document,
    entries: Vec<pdfrum_doc::Bookmark>,
}

impl<'a> Outline<'a> {
    pub(crate) fn load(doc: &'a Document) -> Outline<'a> {
        let mut diags = Diagnostics::default();
        let entries =
            pdfrum_doc::nav::outline_bookmarks(&doc.catalog(), &doc.inner, &doc.limits, &mut diags);
        doc.note(&diags);
        Outline { doc, entries }
    }

    /// How many entries the outline has, at every level.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the document has no outline, or an empty one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, in pre-order: each item, then its subtree, then its next
    /// sibling.
    pub fn iter(&self) -> OutlineIter<'_, 'a> {
        OutlineIter {
            doc: self.doc,
            inner: self.entries.iter(),
        }
    }
}

/// A pre-order walk of an [`Outline`].
///
/// Produced by [`Outline::iter`] and by iterating `&Outline`. Does not
/// allocate; each item clones the underlying bookmark record. The item's
/// lifetime is the document's, so a [`Bookmark`] can outlive this iterator.
#[derive(Debug, Clone)]
pub struct OutlineIter<'outline, 'doc: 'outline> {
    doc: &'doc Document,
    inner: std::slice::Iter<'outline, pdfrum_doc::Bookmark>,
}

impl<'outline, 'doc: 'outline> Iterator for OutlineIter<'outline, 'doc> {
    type Item = Bookmark<'doc>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|inner| Bookmark {
            doc: self.doc,
            inner: inner.clone(),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for OutlineIter<'_, '_> {}

impl<'outline, 'doc: 'outline> IntoIterator for &'outline Outline<'doc> {
    type Item = Bookmark<'doc>;
    type IntoIter = OutlineIter<'outline, 'doc>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One outline entry.
#[derive(Debug, Clone)]
pub struct Bookmark<'a> {
    doc: &'a Document,
    inner: pdfrum_doc::Bookmark,
}

impl Bookmark<'_> {
    /// The entry's title.
    ///
    /// Control characters are raised to spaces, which is what a reader shows
    /// and what makes titles comparable.
    #[must_use]
    pub fn title(&self) -> String {
        self.inner.title(&self.doc.inner)
    }

    /// How deep the entry sits; the top level is zero.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.inner.depth
    }

    /// Whether the entry is drawn expanded, showing its children.
    ///
    /// `/Count` is positive for an open item and negative for a closed one; a
    /// leaf has neither and reads as closed.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.inner.count(&self.doc.inner) > 0
    }

    /// The page the entry jumps to, when it names one.
    ///
    /// `None` when the entry has no destination, when its destination names
    /// no page, or when the page it names is not in this document — an
    /// outline entry may point into another file.
    #[must_use]
    pub fn page_index(&self) -> Option<PageIndex> {
        let mut diags = Diagnostics::default();
        let catalog = self.doc.catalog();
        let dest = self
            .inner
            .dest(&catalog, &self.doc.inner, &self.doc.limits, &mut diags);
        let index = dest.page_index(
            &self.doc.inner,
            |num| self.doc.page_index_of(num),
            &mut diags,
        );
        self.doc.note(&diags);
        index
    }

    /// The action the entry fires, when it has one rather than a plain
    /// destination — a URI to open, a file to launch, a script to run.
    #[must_use]
    pub fn action(&self) -> Option<pdfrum_doc::Action> {
        self.inner.action(&self.doc.inner)
    }

    /// The entry's colour, when it sets one (`/C`), as RGB in `0.0..=1.0`.
    #[must_use]
    pub fn color(&self) -> Option<(f32, f32, f32)> {
        self.inner.color(&self.doc.inner)
    }
}

impl Document {
    /// The page index an object number names, for destination resolution.
    ///
    /// Linear rather than indexed: an outline is walked once and destinations
    /// are resolved on demand, so the map would cost more to build than the
    /// scans it saves on every document with a short outline.
    pub(crate) fn page_index_of(&self, obj_num: u32) -> Option<PageIndex> {
        (0..self.page_count()).map(PageIndex::from).find(|index| {
            self.inner
                .page(*index)
                .ok()
                .and_then(|page| page.reference)
                .is_some_and(|reference| reference.num == obj_num)
        })
    }
}
