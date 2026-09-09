//! One page's object graph, opened for editing.
//!
//! An owned copy of what a page draws, plus a record of what changed. A
//! writer turns that record into replacement objects; the document the graph
//! came from is never mutated.

use crate::page::PageObject;
use crate::{IndexOutOfRange, Page};
use kurbo::Affine;
use pdfrum_common::PageIndex;

/// One page's object graph, opened for editing.
///
/// Obtained from the facade's `Page::edit`. Holds the objects the page draws, in painting
/// order, plus a record of what has changed — which is what tells the save
/// which content streams to write again and which to leave alone.
///
/// # Nothing is mutated until you save
///
/// `Page::edit` hands back an owned graph, you change *that*, and
/// `Document::save_pages` turns the changes
/// into replacement objects on the way out. The document is never touched,
/// which is why editing a page needs no `&mut Document` and why two threads
/// can edit two pages at once.
///
/// # Saving an edited page rewrites it, and rewriting loses things
///
/// A page whose objects you changed is written again **from the object
/// graph**, not patched. Only `rg`/`RG` colours survive, so a CMYK or
/// ICC-based fill comes back black; patterns, shadings and Type 3 text are
/// lost; text keeps only its matrix, font, render mode and strings, so
/// character and word spacing go. The `pdfrum-edit` documentation lists them in full. This applies **only to pages you edited** — every other page
/// is copied through byte-for-byte.
///
/// ```ignore
/// use pdfrum::{Document, SaveOptions};
///
/// let doc = Document::open("in.pdf")?;
/// let mut page = doc.page(0)?.edit();
/// page.remove(0);
/// doc.save_pages("out.pdf", &[page], &SaveOptions::default())?;
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct PageEdit {
    pub(crate) index: PageIndex,
    pub(crate) page: Page,
}

impl PageEdit {
    /// An editable graph for the page at `index`.
    #[must_use]
    pub fn new(index: PageIndex, page: Page) -> Self {
        Self { index, page }
    }

    /// How many objects the page draws, including any switched off with
    /// [`PageEdit::hide`].
    #[must_use]
    pub fn len(&self) -> usize {
        self.page.objects().len()
    }

    /// Whether the page draws nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.page.objects().is_empty()
    }

    /// The zero-based index of the page being edited.
    #[must_use]
    pub fn index(&self) -> PageIndex {
        self.index
    }

    /// The objects, in painting order.
    #[must_use]
    pub fn objects(&self) -> &[PageObject] {
        self.page.objects()
    }

    /// The object at `index`, for a caller who wants to change it in place.
    ///
    /// Taking this reference *is* the edit — the object is marked changed on
    /// the way out, so its content stream is rewritten whether or not you go
    /// on to touch it. Read with [`PageEdit::objects`] when you only want to
    /// look.
    pub fn object_mut(&mut self, index: usize) -> Option<&mut PageObject> {
        self.page.object_mut(index)
    }

    /// Append an object, drawn last and therefore on top.
    pub fn push(&mut self, object: PageObject) {
        self.page.push_object(object);
    }

    /// Insert an object at `index`, pushing the ones there and after later in
    /// the painting order. An index equal to [`PageEdit::len`] appends.
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when `index` is past the end.
    pub fn insert(&mut self, index: usize, object: PageObject) -> Result<(), IndexOutOfRange> {
        self.page.insert_object(index, object)
    }

    /// Remove the object at `index` and hand it back.
    pub fn remove(&mut self, index: usize) -> Option<PageObject> {
        self.page.remove_object(index)
    }

    /// Show the object at `index` without removing it.
    ///
    /// A hidden object keeps its place and its index, so it can be shown again
    /// — but it contributes nothing to the saved page, and reloading the saved
    /// file will not find it.
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when there is no object at `index`.
    pub fn show(&mut self, index: usize) -> Result<(), IndexOutOfRange> {
        self.set_active(index, true)
    }

    /// Hide the object at `index` without removing it.
    ///
    /// See [`PageEdit::show`].
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when there is no object at `index`.
    pub fn hide(&mut self, index: usize) -> Result<(), IndexOutOfRange> {
        self.set_active(index, false)
    }

    fn set_active(&mut self, index: usize, active: bool) -> Result<(), IndexOutOfRange> {
        let len = self.page.objects.len();
        let Some(object) = self.page.objects.get_mut(index) else {
            return Err(IndexOutOfRange { index, len });
        };
        object.set_active(active);
        Ok(())
    }

    /// Whether the object at `index` is drawn.
    #[must_use]
    pub fn is_visible(&self, index: usize) -> Option<bool> {
        self.page.objects().get(index).map(PageObject::is_active)
    }

    /// Move the object at `index` by `transform`.
    ///
    /// The transform is applied *before* the object's existing one, so
    /// `Affine::translate((10.0, 0.0))` moves it ten points right in page
    /// space whatever it was already doing.
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when there is no object at `index`.
    pub fn transform(&mut self, index: usize, transform: Affine) -> Result<(), IndexOutOfRange> {
        let len = self.page.objects.len();
        let Some(object) = self.page.object_mut(index) else {
            return Err(IndexOutOfRange { index, len });
        };
        transform_object(object, transform);
        Ok(())
    }

    /// Whether anything has been changed since the page was opened.
    ///
    /// A `false` here means the save will not rewrite this page's content at
    /// all, and its bytes will come through untouched.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.page.is_dirty()
    }

    /// **Escape hatch — requires `pdfrum-page`.** The object graph
    /// underneath, for a caller reaching past this surface.
    ///
    /// Matches the facade's `Page::objects` and `Document::parser`.
    #[must_use]
    pub fn graph(&self) -> &Page {
        &self.page
    }

    /// **Escape hatch — requires `pdfrum-page`.** The graph, mutably.
    ///
    /// Marks nothing: a caller reaching here is responsible for saying what it
    /// changed.
    pub fn graph_mut(&mut self) -> &mut Page {
        &mut self.page
    }
}

/// Move `object` by `transform`, composed before whatever it already had —
/// the body of [`PageEdit::transform`], for an object not yet on a page.
pub fn transform_object(object: &mut PageObject, transform: Affine) {
    match object {
        PageObject::Path(p) => p.object.matrix = transform * p.object.matrix,
        PageObject::Text(t) => {
            t.object.matrix = transform * t.object.matrix;
            t.object.position = transform * t.object.position;
        }
        PageObject::Image(i) => i.object.matrix = transform * i.object.matrix,
        PageObject::Shading(s) => s.object.matrix = transform * s.object.matrix,
        PageObject::Form(f) => f.object.matrix = transform * f.object.matrix,
    }
}

impl PageEdit {
    /// The `/Font` resource the text object at `index` uses, for a
    /// `TextBuilder` that wants to write in the same font.
    ///
    /// `None` when there is no such object, when it is not text, or when its
    /// font was written inline in the resource dictionary and so has no object
    /// to name.
    #[must_use]
    pub fn font_of(&self, index: usize) -> Option<pdfrum_object::ObjRef> {
        match self.page.objects().get(index)? {
            PageObject::Text(text) => text.object.font_source,
            _ => None,
        }
    }

    /// The image `XObject` the object at `index` draws, for an
    /// `ImageBuilder` that wants to place the same image again.
    ///
    /// `None` for anything that is not an image, and for an inline image,
    /// which has no indirect object to name.
    #[must_use]
    pub fn image_of(&self, index: usize) -> Option<pdfrum_object::ObjRef> {
        match self.page.objects().get(index)? {
            PageObject::Image(image) => image.object.source,
            _ => None,
        }
    }
}
