#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
// Everything here is written *from* untrusted input: index with `get()`.
#![warn(clippy::indexing_slicing)]

// Every module is private and the `pub use` block below is the whole surface,
// so an item is reachable exactly one way and reading that block is reading
// the API. Siblings still reach
// across — `write` names `encrypt`, `content::marks` names `write::object` —
// which is what the `pub(crate)` on a few *sub*modules is for; at this level
// `mod` already means crate-visible.
mod content;
mod doc;
mod encrypt;
mod error;
mod font;
mod image;
mod import;
mod info;
mod names;
mod pages;
mod write;

pub use content::{
    ContentsShape, PageRewrite, Regenerated, ResourceTable, ShareCounts, apply_rewrite, regenerate,
    shared_objects, write_float, write_matrix, write_point, write_rect,
};
pub use doc::EditDoc;
pub use encrypt::{Encryptor, IvSource};
pub use error::Error;
pub use font::embed::{EmbeddedFont, FontEncoding, MissingGlyph};
pub use font::{GidMap, Subsetted, subset};
pub use image::{EmbeddedImage, PixelFormat};
pub use import::{ImportOptions, NUpOptions, PageRange, import_pages, n_page_to_one};
pub use info::{pdf_date, set_info_entry};
pub use pages::{PageBox, add_blank_page, delete_pages, set_page_box, set_page_rotation};
pub use pdfrum_font::StandardFont;
pub use write::id::{FileId, IdSource};
pub use write::{Encryption, SaveMode, SaveOptions, save};
