#![doc = include_str!("../README.md")]
// The one workspace exception. The crate README states the rule: `unsafe`
// only inside `extern "C"`, or a private helper of one.
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
