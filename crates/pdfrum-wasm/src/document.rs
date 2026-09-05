//! The document handle: opening, page count, whole-document queries, saving.

use std::sync::Arc;

use wasm_bindgen::prelude::wasm_bindgen;

use crate::error::Result;
use crate::options::{OpenOptions, SaveOptions};
use crate::page::Page;
use crate::value::{Attachment, Bookmark, Metadata};

/// An open PDF document.
///
/// The root handle: pages, forms, bookmarks, attachments, metadata and images
/// all come from one of these, and each keeps the document alive for as long
/// as it lives — so `doc.free()` while a page of it is still in use is
/// defined, and the bytes stay until the last handle goes.
///
/// **Freeing is explicit.** JavaScript's garbage collector does not know how
/// many bytes a handle holds inside the WebAssembly module's linear memory,
/// so a document that is only dropped on the JavaScript side keeps its parsed
/// object store, its font cache and its file bytes until the module itself
/// goes. Call `free()` when done. Every handle in this binding has one, and it
/// is the same rule wasm-bindgen applies to every exported struct.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Document(Arc<pdfrum::Document>);

#[wasm_bindgen]
impl Document {
    /// Opens a document from bytes in memory.
    ///
    /// The bytes are **copied** into the module, so the caller's
    /// `Uint8Array` may be reused or detached as soon as this returns.
    /// `password` is optional; a wrong one throws with `.code === 3`.
    ///
    /// ```js
    /// const doc = Document.open(new Uint8Array(await file.arrayBuffer()));
    /// ```
    ///
    /// # Errors
    ///
    /// Throws when the bytes are not a PDF (`.code === 2`), when the document
    /// needs a password this one does not open (`.code === 3`), or when a
    /// limit set by the caller was reached (`.code === 9`).
    pub fn open(bytes: &[u8], password: Option<String>) -> Result<Document> {
        Document::open_with(bytes, password, None)
    }

    /// [`Document::open`] with limits and a cancellation flag.
    ///
    /// The options govern the document for its whole life: every render of
    /// every page of it is bounded by the same `maxRenderPixels` and stopped
    /// by the same `cancel`.
    ///
    /// ```js
    /// const stop = new Cancel();
    /// const opts = new OpenOptions();
    /// opts.maxRenderPixels = 4_000_000;
    /// opts.cancel = stop;
    /// const doc = Document.openWith(bytes, undefined, opts);
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Document::open`].
    #[wasm_bindgen(js_name = openWith)]
    // `Option<String>` rather than `Option<&str>` because that is what
    // wasm-bindgen accepts for an optional JavaScript string: a borrowed one
    // has no lifetime to borrow from across the boundary. The clone the lint
    // sees is the copy out of JavaScript's heap, which happens either way.
    #[allow(clippy::needless_pass_by_value)]
    pub fn open_with(
        bytes: &[u8],
        password: Option<String>,
        options: Option<OpenOptions>,
    ) -> Result<Document> {
        let options = options.unwrap_or_default();
        let facade = options.to_facade(password.as_deref());
        // `Arc<[u8]>` from the caller's slice: one copy, which is the copy the
        // doc comment promises, and the facade holds it for the document's
        // life without a second.
        let bytes: Arc<[u8]> = Arc::from(bytes);
        let document = pdfrum::Document::from_bytes_with(bytes, &facade)?;
        Ok(Document(Arc::new(document)))
    }

    /// How many pages the document has.
    #[wasm_bindgen(getter, js_name = pageCount)]
    #[must_use]
    pub fn page_count(&self) -> u32 {
        self.0.page_count()
    }

    /// One page, by zero-based index.
    ///
    /// The page keeps the document alive, so it stays valid after
    /// `doc.free()`.
    ///
    /// # Errors
    ///
    /// Throws with `.code === 4` for an index past the end, and for a page
    /// whose own objects cannot be read: the facade reports both as a failure
    /// to read the document.
    pub fn page(&self, index: u32) -> Result<Page> {
        Ok(Page::new(self.0.page_owned(index)?))
    }

    /// The document's information dictionary.
    #[must_use]
    pub fn metadata(&self) -> Metadata {
        let metadata = self.0.metadata();
        Metadata {
            title: metadata.title,
            author: metadata.author,
            subject: metadata.subject,
            keywords: metadata.keywords,
            creator: metadata.creator,
            producer: metadata.producer,
            creation_date: metadata.creation_date,
            modification_date: metadata.modification_date,
        }
    }

    /// The document's outline, flattened depth-first.
    ///
    /// An empty array for a document with no outline. A run of entries with a
    /// greater `depth` after one is that one's subtree.
    #[must_use]
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        self.0
            .outline()
            .iter()
            .map(|bookmark| Bookmark {
                title: bookmark.title(),
                depth: bookmark.depth(),
                page_index: bookmark.page_index().map(pdfrum::PageIndex::get),
            })
            .collect()
    }

    /// The files embedded in the document, with their bytes.
    #[must_use]
    pub fn attachments(&self) -> Vec<Attachment> {
        self.0
            .attachments()
            .into_iter()
            .map(|attachment| Attachment {
                name: attachment.name.clone(),
                file_name: attachment.file_name(),
                description: attachment.description(),
                subtype: attachment.subtype(),
                data: attachment.data(),
            })
            .collect()
    }

    /// Writes the document out as a PDF.
    ///
    /// The document itself is unchanged; the bytes are a fresh file. To save a
    /// filled form, use [`Form::save`][crate::form::Form::save] instead —
    /// this one does not see a form's buffered writes.
    ///
    /// # Errors
    ///
    /// Throws with `.code === 7` when the document cannot be written out.
    pub fn save(&self, options: Option<SaveOptions>) -> Result<Vec<u8>> {
        let options = options.unwrap_or_default().to_facade();
        let mut bytes = Vec::new();
        self.0.write_to(&mut bytes, &options)?;
        Ok(bytes)
    }
}

impl Document {
    /// The facade document inside, for the other handles in this crate.
    pub(crate) fn inner(&self) -> &Arc<pdfrum::Document> {
        &self.0
    }
}
