//! The list handles: words, links, bookmarks, attachments, search hits and
//! images.
//!
//! # Why a handle and not a caller-filled array
//!
//! Every one of these items carries at least one string, and several carry
//! two. A caller-filled array would mean either a per-item free — six more
//! functions and a leak for every caller who forgets one — or a second call to
//! ask how much text to allocate for. A handle owns the whole list including
//! every string, so the shape is the same for all six:
//!
//! ```c
//! pdfrum_words *words = pdfrum_page_words(page, &error);
//! for (size_t i = 0; i < pdfrum_words_count(words); i++) {
//!     pdfrum_word word;
//!     pdfrum_words_get(words, i, &word);   /* word.text points into `words` */
//! }
//! pdfrum_words_free(words);                /* one free for the lot */
//! ```
//!
//! Every `const char *` an item carries points into its handle and is invalid
//! the moment the handle is freed. A caller who needs a string to outlive the
//! handle copies it.

use core::ffi::c_char;
use std::sync::Arc;

use crate::error::{Failure, Result};
use crate::str::{Buffer, CString, pdfrum_buffer};

/// One list handle's storage: the items, and the strings they point into.
///
/// The strings are `CString::into_raw` pointers this handle owns and frees on
/// drop. They are kept in their own vector rather than reached through the
/// items, so freeing does not have to know which of an item's fields are
/// strings — the one place a new item type could get that wrong.
#[derive(Debug)]
struct List<T> {
    items: Vec<T>,
    strings: Vec<*mut c_char>,
}

impl<T> List<T> {
    /// An empty list.
    fn new() -> List<T> {
        List {
            items: Vec::new(),
            strings: Vec::new(),
        }
    }

    /// Interns a string into the handle, returning a pointer the items may
    /// hold. The pointer lives exactly as long as this handle.
    fn intern(&mut self, text: &str) -> *const c_char {
        let ptr = CString::new(text).into_raw();
        self.strings.push(ptr);
        ptr.cast_const()
    }

    /// Interns an optional string; `None` becomes a null pointer.
    fn intern_opt(&mut self, text: Option<&str>) -> *const c_char {
        text.map_or(core::ptr::null(), |text| self.intern(text))
    }

    /// How many items the list holds.
    fn count(&self) -> usize {
        self.items.len()
    }

    /// Copies one item into a caller's out-parameter.
    ///
    /// # Safety
    ///
    /// `out` is null or points to a writable `T`.
    unsafe fn get(&self, index: usize, out: *mut T) -> Result<()>
    where
        T: Copy,
    {
        if out.is_null() {
            return Err(Failure::Argument("null out"));
        }
        let item = *self
            .items
            .get(index)
            .ok_or(Failure::Argument("index out of range"))?;
        // SAFETY: `out` is non-null and the caller's contract says it is a
        // writable `T`.
        unsafe { out.write(item) };
        Ok(())
    }
}

impl<T> Drop for List<T> {
    fn drop(&mut self) {
        for ptr in self.strings.drain(..) {
            // SAFETY: every pointer here came from `List::intern`, which made
            // it with `CString::into_raw`, and nothing else frees them: they
            // are not handed to C as owned pointers, only as borrows into this
            // handle.
            unsafe { crate::str::pdfrum_free(ptr.cast()) };
        }
    }
}

/// One word of a page's text.
///
/// The rectangle is in PDF points in the page's own space, `y` growing upward
/// from the bottom-left as PDF measures it — the same space
/// `pdfrum_page_size` reports and *not* the top-down pixel space a render
/// produces.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_word {
    /// Left edge.
    pub x0: f64,
    /// Bottom edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Top edge.
    pub y1: f64,
    /// The font size this word was set at, in points.
    pub size: f64,
    /// The first character of the word, as an index into `pdfrum_page_text`.
    pub start: usize,
    /// One past the last character, so `end - start` is the length.
    pub end: usize,
    /// The word's text, owned by the handle.
    pub text: *const c_char,
    /// The font the word was set in, owned by the handle, or null when the
    /// page does not say.
    pub font: *const c_char,
}

/// Every word on a page, in reading order.
///
/// From `pdfrum_page_words`. Owns each word's `text` and `font`, which is why
/// there is no per-word free.
#[derive(Debug)]
pub struct pdfrum_words {
    list: List<pdfrum_word>,
}

/// How many words the list holds.
///
/// Returns 0 for a null handle, which is also the answer for an empty list; a
/// caller who must tell them apart checks the handle it was given.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_words` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_words_count(list: *const pdfrum_words) -> usize {
    // SAFETY: the caller's contract says `list` is null or live.
    unsafe { list.as_ref() }.map_or(0, |list| list.list.count())
}

/// Copies one of the words into `out`.
///
/// `index` is zero-based. Returns `false` — leaving `out` untouched — for a
/// null handle, a null `out`, or an index at or past the count.
///
/// Any `const char *` the item carries points **into the handle** and is
/// invalid once the handle is freed. Copy it if it must outlive the list.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_words` that has not been freed. `out` is null
/// or points to a writable `pdfrum_word`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_words_get(
    list: *const pdfrum_words,
    index: usize,
    out: *mut pdfrum_word,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.list.get(index, out).is_ok())
    }
}

/// Frees the list and everything in it.
///
/// One free for the whole list; there is no per-item free. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_words` that has not already been freed, and no
/// other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_words_free(list: *mut pdfrum_words) {
    if list.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `list` is a live, unfreed handle that
    // its constructor made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(list) });
}

impl pdfrum_words {
    /// Reads a page's words into a handle.
    pub(crate) fn collect(page: &pdfrum::OwnedPage) -> *mut pdfrum_words {
        let mut list = List::new();
        for word in page.words() {
            let text = list.intern(&word.text);
            let font = list.intern_opt(word.font.as_deref());
            list.items.push(pdfrum_word {
                x0: word.rect.x0,
                y0: word.rect.y0,
                x1: word.rect.x1,
                y1: word.rect.y1,
                size: word.size,
                start: word.range.start.get(),
                end: word.range.end.get(),
                text,
                font,
            });
        }
        Box::into_raw(Box::new(pdfrum_words { list }))
    }
}

/// One link on a page.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_link {
    /// Left edge of the clickable area, in PDF points.
    pub x0: f64,
    /// Bottom edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Top edge.
    pub y1: f64,
    /// The URI this link opens, owned by the handle, or null when the link
    /// goes to a page of this document instead.
    pub uri: *const c_char,
    /// The zero-based page this link goes to, or `UINT32_MAX` when it does not
    /// go to a page of this document.
    pub page_index: u32,
}

/// What `pdfrum_link::page_index` says when the link is not an internal one.
const NO_PAGE: u32 = u32::MAX;

/// Every link on a page.
///
/// From `pdfrum_page_links`. Owns each link's `uri`.
#[derive(Debug)]
pub struct pdfrum_links {
    list: List<pdfrum_link>,
}

/// How many links the list holds.
///
/// Returns 0 for a null handle, which is also the answer for an empty list; a
/// caller who must tell them apart checks the handle it was given.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_links` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_links_count(list: *const pdfrum_links) -> usize {
    // SAFETY: the caller's contract says `list` is null or live.
    unsafe { list.as_ref() }.map_or(0, |list| list.list.count())
}

/// Copies one of the links into `out`.
///
/// `index` is zero-based. Returns `false` — leaving `out` untouched — for a
/// null handle, a null `out`, or an index at or past the count.
///
/// Any `const char *` the item carries points **into the handle** and is
/// invalid once the handle is freed. Copy it if it must outlive the list.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_links` that has not been freed. `out` is null
/// or points to a writable `pdfrum_link`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_links_get(
    list: *const pdfrum_links,
    index: usize,
    out: *mut pdfrum_link,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.list.get(index, out).is_ok())
    }
}

/// Frees the list and everything in it.
///
/// One free for the whole list; there is no per-item free. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_links` that has not already been freed, and no
/// other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_links_free(list: *mut pdfrum_links) {
    if list.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `list` is a live, unfreed handle that
    // its constructor made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(list) });
}

impl pdfrum_links {
    /// Reads a page's links into a handle.
    pub(crate) fn collect(page: &pdfrum::OwnedPage) -> *mut pdfrum_links {
        let mut list = List::new();
        for link in page.page_links() {
            let (uri, page_index) = match &link.target {
                pdfrum::LinkTarget::Uri(uri) => (list.intern(uri), NO_PAGE),
                pdfrum::LinkTarget::Page(index) => (core::ptr::null(), index.get()),
                _ => (core::ptr::null(), NO_PAGE),
            };
            list.items.push(pdfrum_link {
                x0: link.rect.x0,
                y0: link.rect.y0,
                x1: link.rect.x1,
                y1: link.rect.y1,
                uri,
                page_index,
            });
        }
        Box::into_raw(Box::new(pdfrum_links { list }))
    }
}

/// One entry of a document's outline.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_bookmark {
    /// The entry's text, owned by the handle.
    pub title: *const c_char,
    /// How deep the entry sits: 0 for a top-level entry, 1 for its children,
    /// and so on. The list is depth-first, so a run of deeper entries after
    /// one is that one's subtree.
    pub depth: usize,
    /// The zero-based page the entry goes to, or `UINT32_MAX` when it does not
    /// name a page of this document.
    pub page_index: u32,
}

/// A document's outline, flattened depth-first.
///
/// From `pdfrum_document_bookmarks`. Owns each entry's `title`.
#[derive(Debug)]
pub struct pdfrum_bookmarks {
    list: List<pdfrum_bookmark>,
}

/// How many bookmarks the list holds.
///
/// Returns 0 for a null handle, which is also the answer for an empty list; a
/// caller who must tell them apart checks the handle it was given.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_bookmarks` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_bookmarks_count(list: *const pdfrum_bookmarks) -> usize {
    // SAFETY: the caller's contract says `list` is null or live.
    unsafe { list.as_ref() }.map_or(0, |list| list.list.count())
}

/// Copies one of the bookmarks into `out`.
///
/// `index` is zero-based. Returns `false` — leaving `out` untouched — for a
/// null handle, a null `out`, or an index at or past the count.
///
/// Any `const char *` the item carries points **into the handle** and is
/// invalid once the handle is freed. Copy it if it must outlive the list.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_bookmarks` that has not been freed. `out` is null
/// or points to a writable `pdfrum_bookmark`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_bookmarks_get(
    list: *const pdfrum_bookmarks,
    index: usize,
    out: *mut pdfrum_bookmark,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.list.get(index, out).is_ok())
    }
}

/// Frees the list and everything in it.
///
/// One free for the whole list; there is no per-item free. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_bookmarks` that has not already been freed, and no
/// other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_bookmarks_free(list: *mut pdfrum_bookmarks) {
    if list.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `list` is a live, unfreed handle that
    // its constructor made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(list) });
}

impl pdfrum_bookmarks {
    /// Reads a document's outline into a handle.
    pub(crate) fn collect(document: &Arc<pdfrum::Document>) -> *mut pdfrum_bookmarks {
        let mut list = List::new();
        for bookmark in &document.outline() {
            let title = list.intern(&bookmark.title());
            list.items.push(pdfrum_bookmark {
                title,
                depth: bookmark.depth(),
                page_index: bookmark
                    .page_index()
                    .map_or(NO_PAGE, pdfrum::PageIndex::get),
            });
        }
        Box::into_raw(Box::new(pdfrum_bookmarks { list }))
    }
}

/// One file embedded in a document.
///
/// The bytes are not in this struct: read them with
/// [`pdfrum_attachments_data`], which allocates a buffer the caller frees.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_attachment {
    /// The name the document files this attachment under, owned by the handle.
    pub name: *const c_char,
    /// The attachment's own file name, owned by the handle.
    pub file_name: *const c_char,
    /// Its description, owned by the handle; empty when the document gives
    /// none.
    pub description: *const c_char,
    /// Its MIME type, owned by the handle, or null when the document does not
    /// say.
    pub subtype: *const c_char,
}

/// A document's embedded files.
///
/// From `pdfrum_document_attachments`. Owns every string each entry carries;
/// the *contents* of an attachment come separately, from
/// `pdfrum_attachments_data`.
#[derive(Debug)]
pub struct pdfrum_attachments {
    list: List<pdfrum_attachment>,
    /// Each entry's bytes, read when the list was built, so
    /// `pdfrum_attachments_data` does not have to go back to the document.
    /// `None` where the document names a file but carries no stream for it.
    payload: Vec<Option<Vec<u8>>>,
}

/// How many attachments the list holds.
///
/// Returns 0 for a null handle, which is also the answer for an empty list; a
/// caller who must tell them apart checks the handle it was given.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_attachments` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_attachments_count(list: *const pdfrum_attachments) -> usize {
    // SAFETY: the caller's contract says `list` is null or live.
    unsafe { list.as_ref() }.map_or(0, |list| list.list.count())
}

/// Copies one of the attachments into `out`.
///
/// `index` is zero-based. Returns `false` — leaving `out` untouched — for a
/// null handle, a null `out`, or an index at or past the count.
///
/// Any `const char *` the item carries points **into the handle** and is
/// invalid once the handle is freed. Copy it if it must outlive the list.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_attachments` that has not been freed. `out` is null
/// or points to a writable `pdfrum_attachment`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_attachments_get(
    list: *const pdfrum_attachments,
    index: usize,
    out: *mut pdfrum_attachment,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.list.get(index, out).is_ok())
    }
}

/// Frees the list and everything in it.
///
/// One free for the whole list; there is no per-item free. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_attachments` that has not already been freed, and no
/// other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_attachments_free(list: *mut pdfrum_attachments) {
    if list.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `list` is a live, unfreed handle that
    // its constructor made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(list) });
}

impl pdfrum_attachments {
    /// Reads a document's attachments into a handle, keeping the document so
    /// the bytes can be fetched later.
    pub(crate) fn collect(document: &Arc<pdfrum::Document>) -> *mut pdfrum_attachments {
        let mut list = List::new();
        let mut payload = Vec::new();
        for attachment in document.attachments() {
            let name = list.intern(&attachment.name);
            let file_name = list.intern(&attachment.file_name());
            let description = list.intern(&attachment.description());
            let subtype = list.intern_opt(attachment.subtype().as_deref());
            payload.push(attachment.data());
            list.items.push(pdfrum_attachment {
                name,
                file_name,
                description,
                subtype,
            });
        }
        Box::into_raw(Box::new(pdfrum_attachments { list, payload }))
    }

    /// Hands one attachment's bytes to a caller's buffer.
    ///
    /// # Safety
    ///
    /// `out` is null or points to a writable `pdfrum_buffer`.
    unsafe fn data(&self, index: usize, out: *mut pdfrum_buffer) -> Result<()> {
        if out.is_null() {
            return Err(Failure::Argument("null out"));
        }
        let bytes = self
            .payload
            .get(index)
            .ok_or(Failure::Argument("index out of range"))?
            .as_ref()
            .ok_or(Failure::Argument("the attachment carries no stream"))?;
        // SAFETY: `out` is non-null and the caller's contract says it is a
        // writable `pdfrum_buffer`.
        unsafe { Buffer::give(bytes, out) }
    }
}

/// One occurrence of a search needle in a page's text.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_hit {
    /// The first character of the match, as an index into `pdfrum_page_text`.
    pub start: usize,
    /// One past the last character.
    pub end: usize,
}

/// Every occurrence of a needle in a page's text.
///
/// From `pdfrum_page_search`. Carries no strings; it is a handle anyway so the
/// six lists have one shape a caller learns once.
#[derive(Debug)]
pub struct pdfrum_hits {
    list: List<pdfrum_hit>,
}

/// How many hits the list holds.
///
/// Returns 0 for a null handle, which is also the answer for an empty list; a
/// caller who must tell them apart checks the handle it was given.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_hits` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_hits_count(list: *const pdfrum_hits) -> usize {
    // SAFETY: the caller's contract says `list` is null or live.
    unsafe { list.as_ref() }.map_or(0, |list| list.list.count())
}

/// Copies one of the hits into `out`.
///
/// `index` is zero-based. Returns `false` — leaving `out` untouched — for a
/// null handle, a null `out`, or an index at or past the count.
///
/// The item is plain data: it borrows nothing from the handle.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_hits` that has not been freed. `out` is null
/// or points to a writable `pdfrum_hit`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_hits_get(
    list: *const pdfrum_hits,
    index: usize,
    out: *mut pdfrum_hit,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.list.get(index, out).is_ok())
    }
}

/// Frees the list and everything in it.
///
/// One free for the whole list; there is no per-item free. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_hits` that has not already been freed, and no
/// other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_hits_free(list: *mut pdfrum_hits) {
    if list.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `list` is a live, unfreed handle that
    // its constructor made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(list) });
}

impl pdfrum_hits {
    /// Searches a page's text into a handle.
    pub(crate) fn collect(
        page: &pdfrum::OwnedPage,
        needle: &str,
        ignore_case: bool,
    ) -> *mut pdfrum_hits {
        let mut list = List::new();
        if !needle.is_empty() {
            let options = pdfrum::FindOptions {
                match_case: !ignore_case,
                ..pdfrum::FindOptions::default()
            };
            for range in page.text().find_with(needle, options) {
                list.items.push(pdfrum_hit {
                    start: range.start.get(),
                    end: range.end.get(),
                });
            }
        }
        Box::into_raw(Box::new(pdfrum_hits { list }))
    }
}

/// One image drawn on a page.
///
/// The pixels are not in this struct: read them with [`pdfrum_images_pixels`],
/// which allocates a buffer the caller frees.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_image {
    /// The image's width in its own pixels, not as drawn on the page.
    pub width: u32,
    /// Its height in its own pixels.
    pub height: u32,
    /// Whether the image is a stencil mask rather than a picture: it paints
    /// the current fill colour through a one-bit shape.
    pub is_mask: bool,
}

/// The images drawn on one page.
///
/// From `pdfrum_document_images`. The pixels come separately, from
/// `pdfrum_images_pixels`.
#[derive(Debug)]
pub struct pdfrum_images {
    list: List<pdfrum_image>,
    /// Each image's pixels, decoded when the list was built.
    payload: Vec<Vec<u8>>,
}

/// How many images the list holds.
///
/// Returns 0 for a null handle, which is also the answer for an empty list; a
/// caller who must tell them apart checks the handle it was given.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_images` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_images_count(list: *const pdfrum_images) -> usize {
    // SAFETY: the caller's contract says `list` is null or live.
    unsafe { list.as_ref() }.map_or(0, |list| list.list.count())
}

/// Copies one of the images into `out`.
///
/// `index` is zero-based. Returns `false` — leaving `out` untouched — for a
/// null handle, a null `out`, or an index at or past the count.
///
/// The item is plain data: it borrows nothing from the handle.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_images` that has not been freed. `out` is null
/// or points to a writable `pdfrum_image`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_images_get(
    list: *const pdfrum_images,
    index: usize,
    out: *mut pdfrum_image,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.list.get(index, out).is_ok())
    }
}

/// Frees the list and everything in it.
///
/// One free for the whole list; there is no per-item free. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `list` is null or a live `pdfrum_images` that has not already been freed, and no
/// other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_images_free(list: *mut pdfrum_images) {
    if list.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `list` is a live, unfreed handle that
    // its constructor made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(list) });
}

impl pdfrum_images {
    /// Reads one page's images into a handle, keeping the decoded pixmaps so
    /// `pdfrum_images_pixels` can hand them out without decoding again.
    pub(crate) fn collect(
        document: &Arc<pdfrum::Document>,
        page_index: u32,
    ) -> Result<*mut pdfrum_images> {
        let page = document.page(page_index)?;
        let mut list = List::new();
        let mut payload = Vec::new();
        for image in page.images() {
            list.items.push(pdfrum_image {
                width: image.width,
                height: image.height,
                is_mask: image.is_mask,
            });
            // Decoded once, here, rather than on each `pdfrum_images_pixels`:
            // the caller asked for this page's images and decoding is the
            // work, so doing it twice for a caller who reads the pixels would
            // be the surprise.
            payload.push(image.pixmap().to_straight_rgba());
        }
        Ok(Box::into_raw(Box::new(pdfrum_images { list, payload })))
    }

    /// Hands one image's pixels to a caller's buffer.
    ///
    /// # Safety
    ///
    /// `out` is null or points to a writable `pdfrum_buffer`.
    unsafe fn pixels(&self, index: usize, out: *mut pdfrum_buffer) -> Result<()> {
        if out.is_null() {
            return Err(Failure::Argument("null out"));
        }
        let bytes = self
            .payload
            .get(index)
            .ok_or(Failure::Argument("index out of range"))?;
        // SAFETY: `out` is non-null and the caller's contract says it is a
        // writable `pdfrum_buffer`.
        unsafe { Buffer::give(bytes, out) }
    }
}

/// The bytes of one attachment.
///
/// Fills `out` with the file's contents; the caller frees `out->data` with
/// `pdfrum_free`. Returns `false` — leaving `out` untouched — for a null
/// handle, a null `out`, an index past the end, or an attachment whose stream
/// the document does not actually carry.
///
/// # Safety
///
/// `list` is null or a live, unfreed attachments handle. `out` is null or
/// points to a writable `pdfrum_buffer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_attachments_data(
    list: *const pdfrum_attachments,
    index: usize,
    out: *mut pdfrum_buffer,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.data(index, out).is_ok())
    }
}

/// The pixels of one image, as RGBA8 straight alpha, row-major, top-down.
///
/// The buffer is `width * height * 4` bytes for the `width` and `height`
/// `pdfrum_images_get` reported for the same index. The caller frees
/// `out->data` with `pdfrum_free`. Returns `false` — leaving `out` untouched —
/// for a null handle, a null `out`, or an index past the end.
///
/// # Safety
///
/// `list` is null or a live, unfreed images handle. `out` is null or points to
/// a writable `pdfrum_buffer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_images_pixels(
    list: *const pdfrum_images,
    index: usize,
    out: *mut pdfrum_buffer,
) -> bool {
    // SAFETY: the caller's contract says `list` is null or live and `out` is
    // null or writable.
    unsafe {
        list.as_ref()
            .is_some_and(|list| list.pixels(index, out).is_ok())
    }
}
