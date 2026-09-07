//! `libpdfrum` — the C ABI over the [`pdfrum`] facade.
//!
//! This crate is a translation layer and nothing else: every exported
//! function is an opaque handle plus one call into the facade. It holds no
//! parsing, no rendering and no policy, and it is the only crate in the
//! workspace where `unsafe` is permitted.
//!
//! # The unsafe rule
//!
//! The workspace sets `unsafe_code = "forbid"`. This crate overrides it to
//! `allow`, under a rule that is narrower than the lint it replaces:
//!
//! - `unsafe` appears **only** inside an `extern "C"` boundary function, or
//!   a private helper in this crate that exists to serve one. No `unsafe`
//!   reaches the facade or anything under it.
//! - Every `extern "C"` function carries a `# Safety` section naming what the
//!   caller must guarantee about each pointer it passes. cbindgen copies that
//!   comment into the header, so the C programmer and the Rust reviewer read
//!   the same text.
//! - Every `unsafe` block carries a `// SAFETY:` comment saying which of
//!   those guarantees it leans on.
//! - Pointer *validity* is the caller's contract — C cannot prove it and
//!   neither can we. Pointer *nullness* is ours: every pointer this library
//!   reads is null-checked, and a null is answered with an error return, never
//!   a crash.
//!
//! # Types
//!
//! Each handle is its own newtype (`pdfrum_document`, `pdfrum_page`, …). There
//! is no `void *` handle. C-visible enums are `#[repr(u32)]` and convert from
//! the facade by a total `match`, never a numeric cast. Options are structs
//! (`pdfrum_limits`, `pdfrum_save_options`); a zeroed struct is the default.
//!
//! # Memory
//!
//! Every pointer the library hands out is freed by exactly one of the
//! library's own functions, named in that function's documentation. A handle
//! is freed by its `*_close` or `*_free`; a `char *` or a byte buffer by
//! [`pdfrum_free`]. A caller never frees library memory with `free(3)` and
//! never passes its own pointer to a library free.
//!
//! # Threads
//!
//! A `pdfrum_document *` may be used from any thread and from several at
//! once. A `pdfrum_page *`, `pdfrum_form *` and every list handle is
//! single-threaded: one thread at a time, though which thread may change. The
//! intended shape is one document shared and one page per worker, which is
//! what `ctest/test.c` exercises with eight threads.
//!
//! # Errors
//!
//! A fallible function takes a trailing `pdfrum_error *`, which may be null.
//! On failure it returns null, `false` or `0` and, when the pointer is not
//! null, fills it with the facade's [`pdfrum::ErrorCode`] number and a
//! heap-allocated message. The message is freed by `pdfrum_error_free`.
//!
//! # Features
//!
//! `markdown` forwards the facade's own feature and adds
//! `pdfrum_page_markdown`. The facade's `javascript` feature is **not**
//! forwarded: document scripts stay a build-time decision and the C library
//! does not run them.

// The one workspace exception. See the crate-level rustdoc.
#![allow(unsafe_code)]

mod cancel;
mod document;
mod error;
mod form;
mod list;
mod page;
mod str;

pub use cancel::{pdfrum_cancel, pdfrum_cancel_free, pdfrum_cancel_new, pdfrum_cancel_stop};
pub use document::{
    pdfrum_close, pdfrum_document, pdfrum_document_attachments, pdfrum_document_bookmarks,
    pdfrum_document_images, pdfrum_document_metadata, pdfrum_document_page, pdfrum_document_save,
    pdfrum_limits, pdfrum_metadata, pdfrum_metadata_free, pdfrum_open, pdfrum_open_file,
    pdfrum_open_with, pdfrum_page_count, pdfrum_save_options,
};
pub use error::{pdfrum_code, pdfrum_error, pdfrum_error_free};
pub use form::{
    pdfrum_field, pdfrum_field_kind, pdfrum_form, pdfrum_form_field, pdfrum_form_field_count,
    pdfrum_form_free, pdfrum_form_open, pdfrum_form_save, pdfrum_form_set, pdfrum_form_set_checked,
};
pub use list::{
    pdfrum_attachment, pdfrum_attachments, pdfrum_attachments_count, pdfrum_attachments_data,
    pdfrum_attachments_free, pdfrum_attachments_get, pdfrum_bookmark, pdfrum_bookmarks,
    pdfrum_bookmarks_count, pdfrum_bookmarks_free, pdfrum_bookmarks_get, pdfrum_hit, pdfrum_hits,
    pdfrum_hits_count, pdfrum_hits_free, pdfrum_hits_get, pdfrum_image, pdfrum_images,
    pdfrum_images_count, pdfrum_images_free, pdfrum_images_get, pdfrum_images_pixels, pdfrum_link,
    pdfrum_links, pdfrum_links_count, pdfrum_links_free, pdfrum_links_get, pdfrum_word,
    pdfrum_words, pdfrum_words_count, pdfrum_words_free, pdfrum_words_get,
};
#[cfg(feature = "markdown")]
pub use page::pdfrum_page_markdown;
pub use page::{
    pdfrum_page, pdfrum_page_close, pdfrum_page_links, pdfrum_page_render, pdfrum_page_render_size,
    pdfrum_page_search, pdfrum_page_size, pdfrum_page_text, pdfrum_page_words,
};
pub use str::{pdfrum_buffer, pdfrum_free, pdfrum_version};
