//! The two owner types for memory that crosses the boundary as a raw pointer,
//! plus the one free that answers for both.
//!
//! Nothing in this crate calls `Box::from_raw` on a guessed type. Every byte
//! the library hands to C — a `char *` or a `pdfrum_buffer::data` — is
//! allocated by [`Bytes`], the single owner underneath both [`CString`] and
//! [`Buffer`], and is reclaimed by [`pdfrum_free`].
//!
//! # Why the allocation carries its own length
//!
//! Rust's allocator needs the exact layout a block was allocated with in order
//! to free it. A `char *` could recover its length by scanning to the NUL, but
//! a `pdfrum_buffer` of arbitrary bytes cannot, and a header that offers *one*
//! `pdfrum_free` for both must not depend on the caller remembering which kind
//! it holds. So [`Bytes`] allocates a length word ahead of the payload and
//! hands out a pointer to the payload; [`pdfrum_free`] steps back one word to
//! find the layout it must free with. The cost is `size_of::<usize>()` bytes
//! per allocation and the gain is that the header has one free function whose
//! contract a reader can hold in their head.

use core::ffi::{c_char, c_void};
use std::alloc::Layout;

use crate::error::{Failure, Result};

/// How far the payload sits past the start of the allocation.
///
/// The length word is `usize`-aligned and the payload is bytes, so the whole
/// block is `usize`-aligned and the payload begins one word in.
const HEADER: usize = size_of::<usize>();

/// The layout a payload of `len` bytes is allocated with.
///
/// One function, used by both the allocate and the free path, so the two
/// cannot disagree about the layout — which is the one way this scheme could
/// go wrong.
fn layout_for(len: usize) -> Option<Layout> {
    Layout::from_size_align(HEADER.checked_add(len)?, align_of::<usize>()).ok()
}

/// Bytes the library allocated for C to hold and later free.
///
/// The sole allocator of caller-owned memory in this crate. [`CString`] and
/// [`Buffer`] are the two typed faces of it; neither allocates on its own.
#[derive(Debug)]
pub(crate) struct Bytes;

impl Bytes {
    /// Copies `bytes` into an allocation C owns, returning a pointer to the
    /// payload.
    ///
    /// Returns null when the length cannot be laid out or the allocator
    /// refuses — the callers treat null as the failure it is rather than
    /// aborting, because a boundary function's job is to report.
    // `cast_ptr_alignment` fires on every `*mut u8` -> `*mut usize` in this
    // impl and is wrong about each: `layout_for` asks the allocator for
    // `align_of::<usize>()`, so the block *is* `usize`-aligned and the payload
    // sits exactly one `usize` in. The lint reads the pointer's static type
    // and cannot read the layout the allocation was made with. Scoped to the
    // two functions that own that invariant rather than to the crate.
    #[allow(clippy::cast_ptr_alignment)]
    fn give(bytes: &[u8]) -> *mut u8 {
        let Some(layout) = layout_for(bytes.len()) else {
            return core::ptr::null_mut();
        };
        // SAFETY: `layout` has a non-zero size, since it includes `HEADER`.
        // The allocation is typed `*mut usize` from the start, which is what
        // `layout` asks for: the length word wants that alignment and the
        // payload, being bytes, is content with it.
        let block = unsafe { std::alloc::alloc(layout).cast::<usize>() };
        if block.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: `block` is a fresh allocation of `HEADER + bytes.len()`
        // bytes aligned for `usize`, so writing the length word at its start
        // and the payload immediately after are both in bounds, and the two
        // regions do not overlap `bytes`, which the caller owns elsewhere.
        unsafe {
            block.write(bytes.len());
            let payload = block.add(1).cast::<u8>();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), payload, bytes.len());
            payload
        }
    }

    /// Frees a payload pointer [`Bytes::give`] returned.
    ///
    /// # Safety
    ///
    /// `payload` is non-null, came from [`Bytes::give`], and has not been
    /// freed since.
    #[allow(clippy::cast_ptr_alignment)]
    unsafe fn reclaim(payload: *mut u8) {
        // SAFETY: the caller's contract says `payload` is one `usize` past
        // the start of a live `Bytes::give` allocation, so stepping back that
        // far lands on the block and reads the length word `give` wrote. The
        // block was allocated as `*mut usize`, so recovering it as one is the
        // original type rather than a cast to a stricter alignment.
        unsafe {
            let block = payload.cast::<usize>().sub(1);
            let len = block.read();
            if let Some(layout) = layout_for(len) {
                std::alloc::dealloc(block.cast::<u8>(), layout);
            }
        }
    }
}

/// A NUL-terminated UTF-8 string owned by the library until C frees it.
///
/// Not [`std::ffi::CString`]: that type allocates its own way, and this crate
/// needs every caller-owned block to come from [`Bytes`] so that one
/// [`pdfrum_free`] answers for strings and buffers alike.
#[derive(Debug)]
pub(crate) struct CString<'a>(&'a str);

impl<'a> CString<'a> {
    /// A string to hand out.
    pub(crate) fn new(text: &'a str) -> CString<'a> {
        CString(text)
    }

    /// Gives the string to C. The pointer is freed by [`pdfrum_free`].
    ///
    /// An interior NUL becomes U+FFFD. A PDF string can contain one and C
    /// cannot represent one inside a `char *`; truncating there would silently
    /// shorten a title or a field value, while substituting keeps the text's
    /// length and makes the byte visible.
    pub(crate) fn into_raw(self) -> *mut c_char {
        let mut bytes = if self.0.as_bytes().contains(&0) {
            self.0
                .chars()
                .map(|ch| if ch == '\0' { '\u{fffd}' } else { ch })
                .collect::<String>()
                .into_bytes()
        } else {
            self.0.as_bytes().to_vec()
        };
        bytes.push(0);
        Bytes::give(&bytes).cast::<c_char>()
    }
}

/// Bytes owned by the library until C frees them.
#[derive(Debug)]
pub(crate) struct Buffer;

impl Buffer {
    /// Fills a caller's `pdfrum_buffer` with these bytes.
    ///
    /// # Safety
    ///
    /// `out` is non-null and points to a writable `pdfrum_buffer`.
    pub(crate) unsafe fn give(bytes: &[u8], out: *mut pdfrum_buffer) -> Result<()> {
        let data = Bytes::give(bytes);
        if data.is_null() {
            return Err(Failure::Argument("out of memory"));
        }
        // SAFETY: the caller's contract says `out` is a writable
        // `pdfrum_buffer`.
        unsafe {
            (*out).data = data;
            (*out).len = bytes.len();
        }
        Ok(())
    }
}

/// A block of bytes the library allocated and the caller owns.
///
/// Filled by every function that hands out bytes — `pdfrum_document_save`,
/// `pdfrum_form_save`, `pdfrum_images_pixels`. Free `data` with
/// [`pdfrum_free`] when done; `len` is the number of bytes and `data` is not
/// NUL-terminated.
#[repr(C)]
#[derive(Debug)]
pub struct pdfrum_buffer {
    /// The bytes, or null if the call that would have filled this failed.
    pub data: *mut u8,
    /// How many bytes `data` points to.
    pub len: usize,
}

/// Reads a caller's C string as a Rust `&str`.
///
/// # Safety
///
/// `ptr` is null or points to a NUL-terminated string that stays valid and
/// unmodified for the length of the call.
pub(crate) unsafe fn borrow_str<'a>(ptr: *const c_char, what: &'static str) -> Result<&'a str> {
    if ptr.is_null() {
        return Err(Failure::Argument(what));
    }
    // SAFETY: the caller's contract says `ptr` is a live NUL-terminated string.
    unsafe { core::ffi::CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| Failure::Argument("string is not UTF-8"))
}

/// Reads an optional caller C string; a null pointer means "not given".
///
/// # Safety
///
/// As [`borrow_str`].
pub(crate) unsafe fn borrow_opt_str<'a>(ptr: *const c_char) -> Result<Option<&'a str>> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: forwarded from this function's own contract.
    unsafe { borrow_str(ptr, "string") }.map(Some)
}

/// The library's version, as a static NUL-terminated string.
///
/// The pointer is to static storage: it is valid for the life of the process,
/// it is never freed, and it must **not** be passed to [`pdfrum_free`].
#[unsafe(no_mangle)]
pub extern "C" fn pdfrum_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0")
        .as_ptr()
        .cast::<c_char>()
}

/// Frees a string or a byte buffer the library handed out.
///
/// The one free for every `char *` and every `pdfrum_buffer::data` in this
/// header. It is **not** for handles — a `pdfrum_document *` is closed by
/// `pdfrum_close`, a `pdfrum_page *` by `pdfrum_page_close`, and each list
/// handle by its own `*_free` — and it is not for the static
/// [`pdfrum_version`] string.
///
/// A null pointer is a no-op, so a caller need not guard a pointer that a
/// failed call left null.
///
/// # Safety
///
/// `ptr` is null, or is a `char *` or a `pdfrum_buffer::data` this library
/// returned and that has not already been freed. Passing anything else —
/// a handle, a pointer from `malloc`, an interior pointer — is undefined.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `ptr` came from `CString::into_raw`
    // or `Buffer::give`, both of which return a `Bytes::give` payload pointer.
    unsafe { Bytes::reclaim(ptr.cast::<u8>()) };
}

#[cfg(test)]
mod tests {
    use super::{Buffer, Bytes, CString, pdfrum_buffer, pdfrum_free};

    /// A round trip through the allocator, for every length the header word
    /// has to survive — including the empty string, whose payload is one NUL.
    #[test]
    fn strings_round_trip() {
        for text in ["", "a", "Hello, world!", &"x".repeat(1000)] {
            let ptr = CString::new(text).into_raw();
            assert!(!ptr.is_null());
            // SAFETY: `ptr` is a live `CString::into_raw` pointer.
            let seen = unsafe { core::ffi::CStr::from_ptr(ptr) };
            assert_eq!(seen.to_str(), Ok(text));
            // SAFETY: same pointer, not yet freed.
            unsafe { pdfrum_free(ptr.cast()) };
        }
    }

    /// An interior NUL is substituted, not truncated: the C string a caller
    /// reads still covers the whole Rust string.
    #[test]
    fn interior_nul_becomes_replacement() {
        let ptr = CString::new("a\0b").into_raw();
        // SAFETY: `ptr` is a live `CString::into_raw` pointer.
        let seen = unsafe { core::ffi::CStr::from_ptr(ptr) };
        assert_eq!(seen.to_str(), Ok("a\u{fffd}b"));
        // SAFETY: same pointer, not yet freed.
        unsafe { pdfrum_free(ptr.cast()) };
    }

    /// A buffer's length comes back exactly, including zero — the case a
    /// NUL scan could not have recovered.
    #[test]
    fn buffers_round_trip() {
        for bytes in [vec![], vec![0u8], vec![0u8, 1, 2, 0], vec![7u8; 4096]] {
            let mut out = pdfrum_buffer {
                data: core::ptr::null_mut(),
                len: 0,
            };
            // SAFETY: `out` is a writable `pdfrum_buffer` on this stack.
            unsafe { Buffer::give(&bytes, &raw mut out) }.expect("allocation succeeds");
            assert_eq!(out.len, bytes.len());
            if !bytes.is_empty() {
                // SAFETY: `Buffer::give` filled `out.data` with `out.len` bytes.
                let seen = unsafe { core::slice::from_raw_parts(out.data, out.len) };
                assert_eq!(seen, &bytes[..]);
            }
            // SAFETY: `out.data` came from `Buffer::give` and is unfreed.
            unsafe { pdfrum_free(out.data.cast()) };
        }
    }

    /// Freeing null is the no-op the header promises.
    #[test]
    fn free_of_null_is_a_no_op() {
        // SAFETY: a null pointer is explicitly allowed.
        unsafe { pdfrum_free(core::ptr::null_mut()) };
    }

    /// The payload pointer is aligned and the header word is where `reclaim`
    /// looks for it — the invariant the whole scheme rests on.
    #[test]
    #[allow(clippy::cast_ptr_alignment)]
    fn the_length_word_precedes_the_payload() {
        let payload = Bytes::give(&[1, 2, 3]);
        assert!(!payload.is_null());
        // SAFETY: `payload` is a live `Bytes::give` pointer, so the `usize`
        // immediately before it is the length `give` wrote.
        let len = unsafe { payload.cast::<usize>().sub(1).read() };
        assert_eq!(len, 3);
        // SAFETY: same pointer, not yet freed.
        unsafe { pdfrum_free(payload.cast()) };
    }
}
