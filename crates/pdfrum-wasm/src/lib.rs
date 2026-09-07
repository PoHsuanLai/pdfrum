#![doc = include_str!("../README.md")]
mod document;
mod error;
mod form;
mod options;
mod page;
mod value;

pub use document::Document;
pub use form::Form;
pub use options::{Cancel, OpenOptions, RenderOptions, SaveOptions};
pub use page::Page;
pub use value::{
    Attachment, Bookmark, Field, FieldKind, Hit, ImageInfo, Link, Metadata, RenderResult, Word,
};

use wasm_bindgen::prelude::wasm_bindgen;

/// The library's version, as `Cargo.toml` declares it.
///
/// The same string `pdfrum_version()` returns from the C library, from the
/// same source.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
