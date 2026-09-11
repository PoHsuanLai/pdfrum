//! The page handle: size, render, text, words, links, search.

use core::ffi::c_char;
use std::sync::Arc;

use crate::error::{Failure, Result, pdfrum_error, with_handle};
use crate::list::{pdfrum_hits, pdfrum_links, pdfrum_words};
use crate::str::{CString, borrow_str};

/// One page of an open document.
///
/// Holds a reference to its document, so the document may be closed while the
/// page is still open.
///
/// **Threads.** A page is single-threaded: use one from one thread at a time.
/// The supported way to render in parallel is a page per worker, all taken
/// from one shared `pdfrum_document`.
#[derive(Debug)]
pub struct pdfrum_page(pdfrum::OwnedPage);

impl pdfrum_page {
    /// Opens one page of a document as a handle for C.
    pub(crate) fn open(document: &Arc<pdfrum::Document>, index: u32) -> Result<*mut pdfrum_page> {
        let page = document.page_owned(index)?;
        Ok(Box::into_raw(Box::new(pdfrum_page(page))))
    }

    /// The facade page inside.
    pub(crate) fn inner(&self) -> &pdfrum::OwnedPage {
        &self.0
    }

    /// The pixel size this page renders to at `scale`.
    ///
    /// One function, called by both `pdfrum_page_render_size` and the render
    /// itself, so the size a caller allocates for cannot disagree with the
    /// size the render produces.
    fn render_size(&self, scale: f64) -> Result<(u32, u32)> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Failure::Argument("scale must be finite and positive"));
        }
        let width = (self.0.width() * scale).ceil();
        let height = (self.0.height() * scale).ceil();
        if !(width.is_finite() && height.is_finite())
            || width < 1.0
            || height < 1.0
            || width > f64::from(u32::MAX)
            || height > f64::from(u32::MAX)
        {
            return Err(Failure::Argument(
                "the scaled page does not fit in a pixmap",
            ));
        }
        // The bounds above put both values in `u32`, and `as` on a checked
        // finite positive `f64` truncates toward zero, which after `ceil` is
        // the integer itself.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok((width as u32, height as u32))
    }
}

/// The page's size in PDF points, as the page's own boxes and rotation give it.
///
/// `width` and `height` may each be null when the caller wants only the other.
/// A null page is a no-op that leaves both untouched, which a caller who
/// zeroed them first reads as zero.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. `width` and `height` are
/// each null or point to a writable `double`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_size(
    page: *const pdfrum_page,
    width: *mut f64,
    height: *mut f64,
) {
    // SAFETY: the caller's contract says `page` is null or live.
    let Some(page) = (unsafe { page.as_ref() }) else {
        return;
    };
    // SAFETY: the caller's contract says each out-pointer is null or writable.
    unsafe {
        if !width.is_null() {
            width.write(page.0.width());
        }
        if !height.is_null() {
            height.write(page.0.height());
        }
    }
}

/// The pixel size and buffer stride a render at `scale` needs.
///
/// Call this first, allocate `stride * height` bytes, then call
/// [`pdfrum_page_render`] with the same `scale`. The stride reported is the
/// tightest one — `width * 4` — and a caller may pass a larger one to the
/// render if its own buffer is padded.
///
/// Any of `width`, `height` and `stride` may be null. Returns `false` on
/// failure with `error` filled.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. Each out-pointer is null or
/// writable. `error` is null or points to a writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_render_size(
    page: *const pdfrum_page,
    scale: f64,
    width: *mut u32,
    height: *mut u32,
    stride: *mut usize,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(page, error, |page| {
            let (w, h) = page.render_size(scale)?;
            // SAFETY: the caller's contract says each is null or writable.
            if !width.is_null() {
                width.write(w);
            }
            if !height.is_null() {
                height.write(h);
            }
            if !stride.is_null() {
                stride.write(w as usize * 4);
            }
            Ok(())
        })
    }
    .is_some()
}

/// Renders the page into a buffer the caller owns.
///
/// The caller queries [`pdfrum_page_render_size`] first and allocates at least
/// `stride * height` bytes. The library writes **RGBA8, straight (not
/// premultiplied) alpha, row-major, top-down**: pixel *(x, y)* begins at
/// `rgba[y * stride + x * 4]` and is red, green, blue, alpha in that order.
///
/// `stride` must be at least `width * 4`; a larger one lets a caller render
/// into a padded or sub-rectangle-of-a-larger-image buffer. `width` and
/// `height` may each be null; when given, they receive the size actually
/// rendered, which is the size `pdfrum_page_render_size` reported.
///
/// The buffer is the caller's throughout: the library never frees it and never
/// keeps a pointer to it past the call. Returns `false` on failure with
/// `error` filled, in which case the buffer's contents are unspecified.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. `rgba` points to at least
/// `stride * height` writable bytes, where `height` is what
/// [`pdfrum_page_render_size`] reported for this `scale`. `width`, `height`
/// and `error` are each null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_render(
    page: *const pdfrum_page,
    scale: f64,
    rgba: *mut u8,
    stride: usize,
    width: *mut u32,
    height: *mut u32,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(page, error, |page| {
            if rgba.is_null() {
                return Err(Failure::Argument("null rgba buffer"));
            }
            let (w, h) = page.render_size(scale)?;
            let row = w as usize * 4;
            if stride < row {
                return Err(Failure::Argument("stride is narrower than the row"));
            }
            let pixmap = page
                .0
                .render_with(backend(), &pdfrum::RenderOptions::scaled(scale))?;
            // The facade's pixmap is premultiplied; the header promises
            // straight alpha, which is what a caller compositing with its own
            // rules expects and what PDFium's `FPDFBitmap_BGRA` is not.
            let straight = pixmap.to_straight_rgba();
            let rendered = pixmap.width() as usize * 4;
            for y in 0..pixmap.height() as usize {
                let Some(source) = straight.get(y * rendered..y * rendered + rendered) else {
                    return Err(Failure::Argument("the render came back short"));
                };
                // SAFETY: the caller's contract says `rgba` has
                // `stride * height` writable bytes, `y < height`, and
                // `rendered <= row <= stride`, so this row is in bounds and
                // does not overlap `straight`, which the library owns.
                core::ptr::copy_nonoverlapping(
                    source.as_ptr(),
                    rgba.add(y * stride),
                    rendered.min(row),
                );
            }
            // SAFETY: the caller's contract says each is null or writable.
            if !width.is_null() {
                width.write(w);
            }
            if !height.is_null() {
                height.write(h);
            }
            Ok(())
        })
    }
    .is_some()
}

/// The rasterizer every render in this library uses.
///
/// `vello-cpu`, the facade's own default: the C library takes no backend
/// argument, because choosing a rasterizer is a Rust-level decision about
/// which crate to compile and a C caller links whatever this library was built
/// with.
fn backend() -> pdfrum::VelloCpuBackend {
    pdfrum::VelloCpuBackend::new()
}

/// The page's text, in reading order, as a UTF-8 string.
///
/// The caller owns the returned string and frees it with `pdfrum_free`.
/// Returns null on failure with `error` filled; a page with no text yields an
/// empty string, not null.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. `error` is null or points
/// to a writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_text(
    page: *const pdfrum_page,
    error: *mut pdfrum_error,
) -> *mut c_char {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(page, error, |page| {
            Ok(CString::new(&page.0.text().to_string()).into_raw())
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// The page's words, as a list handle.
///
/// A handle rather than a caller-filled array on purpose: each word carries
/// two strings, and a handle owns them all so there is **no per-word free**.
/// Read the count with `pdfrum_words_count`, each word with
/// `pdfrum_words_get`, and free the lot with `pdfrum_words_free`. Every
/// `const char *` a word carries points into the handle and is invalid once
/// the handle is freed.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. `error` is null or points
/// to a writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_words(
    page: *const pdfrum_page,
    error: *mut pdfrum_error,
) -> *mut pdfrum_words {
    // SAFETY: forwarded from this function's own contract.
    unsafe { with_handle(page, error, |page| Ok(pdfrum_words::collect(page.inner()))) }
        .unwrap_or(core::ptr::null_mut())
}

/// The page's links, as a list handle.
///
/// The same no-per-item-free shape as [`pdfrum_page_words`]: free the lot with
/// `pdfrum_links_free`, and every `const char *` a link carries dies with the
/// handle.
///
/// # Safety
///
/// As [`pdfrum_page_words`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_links(
    page: *const pdfrum_page,
    error: *mut pdfrum_error,
) -> *mut pdfrum_links {
    // SAFETY: forwarded from this function's own contract.
    unsafe { with_handle(page, error, |page| Ok(pdfrum_links::collect(page.inner()))) }
        .unwrap_or(core::ptr::null_mut())
}

/// Every occurrence of `needle` in the page's text, as a list handle.
///
/// `ignore_case` true is a case-insensitive search, which is the facade's own
/// default. Each hit is a half-open character range into the same text
/// [`pdfrum_page_text`] returns. Free the list with `pdfrum_hits_free`.
///
/// An empty needle yields an empty list rather than a hit at every position.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. `needle` is a non-null
/// NUL-terminated UTF-8 string. `error` is null or points to a writable
/// `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_search(
    page: *const pdfrum_page,
    needle: *const c_char,
    ignore_case: bool,
    error: *mut pdfrum_error,
) -> *mut pdfrum_hits {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(page, error, |page| {
            let needle = borrow_str(needle, "null needle")?;
            Ok(pdfrum_hits::collect(page.inner(), needle, ignore_case))
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// The page as Markdown.
///
/// Only present when the library was built with the `markdown` feature; a
/// build without it does not export this symbol and the header says so.
///
/// The caller owns the returned string and frees it with `pdfrum_free`.
///
/// # Safety
///
/// `page` is null or a live, unclosed page handle. `error` is null or points
/// to a writable `pdfrum_error`.
#[cfg(feature = "markdown")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_markdown(
    page: *const pdfrum_page,
    error: *mut pdfrum_error,
) -> *mut c_char {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(page, error, |page| {
            Ok(CString::new(&page.0.markdown()).into_raw())
        })
    }
    .unwrap_or(core::ptr::null_mut())
}

/// Closes a page.
///
/// The document it came from stays open; a page holds its own reference, so
/// the order in which a caller closes a document and its pages does not
/// matter. A null pointer is a no-op.
///
/// # Safety
///
/// `page` is null or a live page handle that has not already been closed, and
/// no other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_page_close(page: *mut pdfrum_page) {
    if page.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `page` is a live, unclosed handle
    // that `pdfrum_page::open` made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(page) });
}
