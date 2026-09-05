//! The document handle: opening, page count, whole-document queries, saving.

use core::ffi::{c_char, c_void};
use std::sync::Arc;

use crate::cancel::pdfrum_cancel;
use crate::error::{Failure, catch, pdfrum_error, with_handle};
use crate::list::{pdfrum_attachments, pdfrum_bookmarks, pdfrum_images};
use crate::page::pdfrum_page;
use crate::str::{Buffer, CString, borrow_opt_str, borrow_str, pdfrum_buffer};

/// An open PDF document.
///
/// The root handle: pages, forms, bookmarks, attachments, metadata and images
/// all come from one of these, and each keeps the document alive for as long
/// as it lives, so closing the document while a page of it is open is defined
/// — the bytes stay until the last handle goes.
///
/// **Threads.** A document may be used from any thread and from several at
/// once: every function that takes a `const pdfrum_document *` is safe to call
/// concurrently. That is what makes the one-document-many-workers shape in
/// `ctest/test.c` possible. [`pdfrum_close`] is the exception, as every free
/// is: it needs the handle to itself.
#[derive(Debug)]
pub struct pdfrum_document(Arc<pdfrum::Document>);

impl pdfrum_document {
    /// The facade document inside.
    pub(crate) fn inner(&self) -> &Arc<pdfrum::Document> {
        &self.0
    }

    /// Boxes a freshly opened document into a handle for C.
    fn give(document: pdfrum::Document) -> *mut pdfrum_document {
        Box::into_raw(Box::new(pdfrum_document(Arc::new(document))))
    }
}

/// The limits and cancellation an open runs under.
///
/// A struct rather than a positional list, so a field added later does not
/// renumber a call. Zero means "no limit" for both numbers and a null
/// `cancel` means "not cancellable", so a zeroed `pdfrum_limits` is exactly
/// the default [`pdfrum_open`] uses.
#[repr(C)]
#[derive(Debug)]
pub struct pdfrum_limits {
    /// The largest render this document will attempt, in pixels. Zero leaves
    /// the facade's own default in place.
    pub max_render_pixels: u64,
    /// A flag from `pdfrum_cancel_new`, or null. Raising it stops work in
    /// progress; the handle must outlive the document unless it is freed,
    /// which is always allowed because the flag is reference-counted inside.
    pub cancel: *mut pdfrum_cancel,
    /// A wall-clock budget in milliseconds, counted from the open. Zero means
    /// no budget.
    pub time_limit_ms: u64,
}

impl pdfrum_limits {
    /// The facade options these limits describe.
    ///
    /// # Safety
    ///
    /// `self.cancel` is null or a live `pdfrum_cancel` handle.
    unsafe fn options(&self, password: Option<&str>) -> pdfrum::OpenOptions {
        let mut options = pdfrum::OpenOptions {
            password: password.map(|p| p.as_bytes().to_vec()),
            ..pdfrum::OpenOptions::default()
        };
        if self.max_render_pixels > 0 {
            options.limits.max_render_pixels = Some(self.max_render_pixels);
        }
        // A flag and a budget are one deadline, not two: the facade holds a
        // single `Option<Deadline>`, and `Deadline::after` is already
        // stoppable. So a caller who gives both gets a deadline that expires
        // on the timer *and* answers `pdfrum_cancel_stop` — but only when the
        // flag it answers is the one C holds, which is why the flag wins the
        // slot and the budget is folded into it.
        options.limits.deadline = match (
            // SAFETY: the caller's contract says `cancel` is null or live.
            unsafe { self.cancel.as_ref() },
            self.time_limit_ms,
        ) {
            (Some(cancel), _) => Some(cancel.deadline()),
            (None, 0) => None,
            (None, ms) => Some(pdfrum_cancel::with_time_limit(ms)),
        };
        options
    }
}

/// How a save is written.
#[repr(C)]
#[derive(Debug)]
pub struct pdfrum_save_options {
    /// Write the same bytes for the same input every time: no timestamps and a
    /// file identifier derived from the content rather than the clock. What a
    /// build system or a test wants.
    pub deterministic: bool,
    /// Drop the document's encryption, writing the result in the clear. The
    /// caller is responsible for whether that is allowed; the library reports
    /// the permissions and does not enforce them.
    pub remove_security: bool,
}

/// The seed `pdfrum_save_options::deterministic` uses.
///
/// Any constant would do; this one is `pdfrum` in ASCII, padded. It is part of
/// the library's output contract — changing it changes the bytes a
/// deterministic save produces — so it lives here as a named constant rather
/// than inline.
const DETERMINISTIC_SEED: [u8; 16] = *b"pdfrum-capi-det\0";

impl pdfrum_save_options {
    /// The facade options these describe.
    pub(crate) fn to_facade(&self) -> pdfrum::SaveOptions {
        pdfrum::SaveOptions {
            // `Fixed` seeds both the file identifier and the font-subset
            // tag from a constant, which is what makes the same input save
            // to the same bytes. The seed is this library's, not the
            // caller's: a C caller who wants determinism wants *a* stable
            // answer, not a choice of which.
            id_source: if self.deterministic {
                pdfrum::IdSource::Fixed(DETERMINISTIC_SEED)
            } else {
                pdfrum::IdSource::default()
            },
            remove_security: self.remove_security,
            ..pdfrum::SaveOptions::default()
        }
    }
}

/// A document's information dictionary, as eight strings.
///
/// Every field is a `char *` that is null when the document does not carry
/// that entry. The whole struct is freed at once by [`pdfrum_metadata_free`];
/// the individual strings must **not** be passed to `pdfrum_free`.
#[repr(C)]
#[derive(Debug)]
pub struct pdfrum_metadata {
    /// `/Title`, or null.
    pub title: *mut c_char,
    /// `/Author`, or null.
    pub author: *mut c_char,
    /// `/Subject`, or null.
    pub subject: *mut c_char,
    /// `/Keywords`, or null.
    pub keywords: *mut c_char,
    /// `/Creator`, or null.
    pub creator: *mut c_char,
    /// `/Producer`, or null.
    pub producer: *mut c_char,
    /// `/CreationDate` as the PDF date string it is, or null.
    pub creation_date: *mut c_char,
    /// `/ModDate` as the PDF date string it is, or null.
    pub modification_date: *mut c_char,
}

/// A `char *` for an optional string: null when there is nothing to say.
fn optional(text: Option<&String>) -> *mut c_char {
    text.map_or(core::ptr::null_mut(), |text| CString::new(text).into_raw())
}

/// Opens a document from bytes in memory.
///
/// The bytes are **copied** into the library, so the caller's buffer may be
/// freed or reused as soon as this returns. `password` may be null for a
/// document that needs none; a wrong one fails with `PDFRUM_CODE_WRONG_PASSWORD`.
///
/// Returns a handle the caller closes with [`pdfrum_close`], or null on
/// failure with `error` filled.
///
/// # Safety
///
/// `bytes` points to at least `len` readable bytes. `password` is null or a
/// NUL-terminated UTF-8 string. `error` is null or points to a writable
/// `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_open(
    bytes: *const u8,
    len: usize,
    password: *const c_char,
    error: *mut pdfrum_error,
) -> *mut pdfrum_document {
    let limits = pdfrum_limits {
        max_render_pixels: 0,
        cancel: core::ptr::null_mut(),
        time_limit_ms: 0,
    };
    // SAFETY: forwarded from this function's own contract; the limits are a
    // local whose `cancel` is null.
    unsafe { pdfrum_open_with(bytes, len, password, &raw const limits, error) }
}

/// [`pdfrum_open`] with limits and a cancellation flag.
///
/// `limits` may be null, which is the same as a zeroed one and therefore the
/// same as [`pdfrum_open`]. The limits govern the document for its whole life,
/// not just the open: a render that exceeds `max_render_pixels`, or runs past
/// `time_limit_ms`, or meets a raised `cancel`, fails with
/// `PDFRUM_CODE_LIMIT`.
///
/// # Safety
///
/// As [`pdfrum_open`], and `limits` is null or points to a readable
/// `pdfrum_limits` whose `cancel` is null or a live `pdfrum_cancel`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_open_with(
    bytes: *const u8,
    len: usize,
    password: *const c_char,
    limits: *const pdfrum_limits,
    error: *mut pdfrum_error,
) -> *mut pdfrum_document {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        catch(error, || {
            if bytes.is_null() && len > 0 {
                return Err(Failure::Argument("null bytes"));
            }
            let password = borrow_opt_str(password)?;
            let copied: Arc<[u8]> = if len == 0 {
                Arc::from(&[][..])
            } else {
                // SAFETY: the caller's contract says `bytes` has `len`
                // readable bytes, and this copies them before returning.
                Arc::from(core::slice::from_raw_parts(bytes, len))
            };
            let options = match limits.as_ref() {
                Some(limits) => limits.options(password),
                None => pdfrum::OpenOptions {
                    password: password.map(|p| p.as_bytes().to_vec()),
                    ..pdfrum::OpenOptions::default()
                },
            };
            Ok(pdfrum_document::give(pdfrum::Document::from_bytes_with(
                copied, &options,
            )?))
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// Opens a document from a file.
///
/// The library reads the file itself; `path` is a host path in the platform's
/// own encoding, which on every platform this library builds for is UTF-8.
/// `password` may be null.
///
/// # Safety
///
/// `path` is a non-null NUL-terminated string. `password` is null or a
/// NUL-terminated UTF-8 string. `error` is null or points to a writable
/// `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_open_file(
    path: *const c_char,
    password: *const c_char,
    error: *mut pdfrum_error,
) -> *mut pdfrum_document {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        catch(error, || {
            let path = borrow_str(path, "null path")?;
            let password = borrow_opt_str(password)?;
            let options = pdfrum::OpenOptions {
                password: password.map(|p| p.as_bytes().to_vec()),
                ..pdfrum::OpenOptions::default()
            };
            Ok(pdfrum_document::give(pdfrum::Document::open_with(
                path, &options,
            )?))
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// Closes a document.
///
/// Every page, form and list handle taken from this document holds its own
/// reference, so closing here while one is still open is defined: the
/// document's bytes stay until the last of them is freed. A null pointer is a
/// no-op.
///
/// # Safety
///
/// `document` is null or a live handle from an open that has not already been
/// closed, and no other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_close(document: *mut pdfrum_document) {
    if document.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `document` is a live, unclosed handle
    // that an open made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(document) });
}

/// How many pages the document has.
///
/// Returns 0 for a null handle, which is also a legitimate answer for a
/// document with no pages; a caller who must tell them apart checks the handle
/// it was given.
///
/// # Safety
///
/// `document` is null or a live, unclosed document handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_count(document: *const pdfrum_document) -> u32 {
    // SAFETY: the caller's contract says `document` is null or live.
    unsafe { document.as_ref() }.map_or(0, |document| document.0.page_count())
}

/// One page of the document, as a handle that keeps the document alive.
///
/// `index` is zero-based; one past the end fails with `PDFRUM_CODE_DOC`.
///
/// **Threads.** The returned page is single-threaded: use it from one thread
/// at a time. Taking a page per worker from one shared document is the
/// supported way to render in parallel, and is what `ctest/test.c` proves.
///
/// The caller closes the page with [`pdfrum_page_close`].
///
/// # Safety
///
/// `document` is null or a live, unclosed document handle. `error` is null or
/// points to a writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page(
    document: *const pdfrum_document,
    index: u32,
    error: *mut pdfrum_error,
) -> *mut pdfrum_page {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(document, error, |document| {
            pdfrum_page::open(document.inner(), index)
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// The document's outline, flattened depth-first into a list handle.
///
/// The caller frees the list with `pdfrum_bookmarks_free`. A document with no
/// outline yields an empty list, not a failure.
///
/// # Safety
///
/// `document` is null or a live, unclosed document handle. `error` is null or
/// points to a writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_document_bookmarks(
    document: *const pdfrum_document,
    error: *mut pdfrum_error,
) -> *mut pdfrum_bookmarks {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(document, error, |document| {
            Ok(pdfrum_bookmarks::collect(document.inner()))
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// The document's embedded files, as a list handle.
///
/// The caller frees the list with `pdfrum_attachments_free`.
///
/// # Safety
///
/// As [`pdfrum_document_bookmarks`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_document_attachments(
    document: *const pdfrum_document,
    error: *mut pdfrum_error,
) -> *mut pdfrum_attachments {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(document, error, |document| {
            Ok(pdfrum_attachments::collect(document.inner()))
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// The images drawn on one page, as a list handle.
///
/// `page_index` is zero-based. The caller frees the list with
/// `pdfrum_images_free`, and reads each image's pixels with
/// `pdfrum_images_pixels`.
///
/// # Safety
///
/// As [`pdfrum_document_bookmarks`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_document_images(
    document: *const pdfrum_document,
    page_index: u32,
    error: *mut pdfrum_error,
) -> *mut pdfrum_images {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(document, error, |document| {
            pdfrum_images::collect(document.inner(), page_index)
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// The document's information dictionary.
///
/// Fills `out` with eight `char *`, each null where the document says nothing.
/// The whole struct is freed by [`pdfrum_metadata_free`] and its strings must
/// not be freed individually.
///
/// Returns `false` on failure with `error` filled.
///
/// # Safety
///
/// `document` is null or a live, unclosed document handle. `out` is non-null
/// and points to a writable `pdfrum_metadata`. `error` is null or points to a
/// writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_document_metadata(
    document: *const pdfrum_document,
    out: *mut pdfrum_metadata,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(document, error, |document| {
            if out.is_null() {
                return Err(Failure::Argument("null out"));
            }
            let meta = document.0.metadata();
            // SAFETY: `out` is non-null and the caller's contract says it is
            // writable.
            out.write(pdfrum_metadata {
                title: optional(meta.title.as_ref()),
                author: optional(meta.author.as_ref()),
                subject: optional(meta.subject.as_ref()),
                keywords: optional(meta.keywords.as_ref()),
                creator: optional(meta.creator.as_ref()),
                producer: optional(meta.producer.as_ref()),
                creation_date: optional(meta.creation_date.as_ref()),
                modification_date: optional(meta.modification_date.as_ref()),
            });
            Ok(())
        })
    }
    .is_some()
}

/// Frees every string in a metadata struct and zeroes it.
///
/// Calling this twice is defined and does nothing the second time. A null
/// pointer is a no-op.
///
/// # Safety
///
/// `metadata` is null or points to a `pdfrum_metadata` that
/// [`pdfrum_document_metadata`] filled and that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_metadata_free(metadata: *mut pdfrum_metadata) {
    if metadata.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `metadata` points to a struct this
    // library filled, so each non-null field is a `CString::into_raw` pointer.
    unsafe {
        let meta = &mut *metadata;
        for field in [
            &raw mut meta.title,
            &raw mut meta.author,
            &raw mut meta.subject,
            &raw mut meta.keywords,
            &raw mut meta.creator,
            &raw mut meta.producer,
            &raw mut meta.creation_date,
            &raw mut meta.modification_date,
        ] {
            crate::str::pdfrum_free((*field).cast::<c_void>());
            *field = core::ptr::null_mut();
        }
    }
}

/// Writes the document out to a byte buffer the caller owns.
///
/// `options` may be null, which is the same as a zeroed `pdfrum_save_options`.
/// On success `out` is filled and the caller frees `out->data` with
/// `pdfrum_free`; on failure `false` comes back and `out` is untouched.
///
/// # Safety
///
/// `document` is null or a live, unclosed document handle. `options` is null
/// or points to a readable `pdfrum_save_options`. `out` is non-null and points
/// to a writable `pdfrum_buffer`. `error` is null or points to a writable
/// `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_document_save(
    document: *const pdfrum_document,
    options: *const pdfrum_save_options,
    out: *mut pdfrum_buffer,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(document, error, |document| {
            if out.is_null() {
                return Err(Failure::Argument("null out"));
            }
            // SAFETY: the caller's contract says `options` is null or readable.
            let facade = options
                .as_ref()
                .map_or_else(pdfrum::SaveOptions::default, pdfrum_save_options::to_facade);
            let mut bytes = Vec::new();
            document.0.write_to(&mut bytes, &facade)?;
            // SAFETY: `out` is non-null and the caller's contract says it is
            // a writable `pdfrum_buffer`.
            Buffer::give(&bytes, out)
        })
    }
    .is_some()
}
