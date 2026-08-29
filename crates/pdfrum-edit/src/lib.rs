//! Editing and saving (ISO 32000 §7.5.8 incremental updates and full
//! rewrite): the deterministic object serializer, full and incremental save,
//! page import and N-up, content-stream generation, and CID font subsetting
//! (SPEC.md §11).
//!
//! This is the one crate that *writes*. Everything below it derives values
//! from bytes; this turns values back into bytes another reader must be able
//! to open.
//!
//! # Saving a document
//!
//! ```
//! use std::sync::Arc;
//! use pdfrum_edit::{EditDoc, IdSource, SaveOptions, save};
//! use pdfrum_parser::{LoadOptions, load};
//!
//! let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
//! let doc = load(bytes, &LoadOptions::default())?;
//! let edit = EditDoc::new(&doc);
//!
//! let mut out = Vec::new();
//! save(&edit, &SaveOptions::default(), &mut out)?;
//! assert!(out.starts_with(b"%PDF-1.7\r\n"));
//!
//! // What we wrote, another reader opens.
//! let reloaded = load(Arc::from(&out[..]), &LoadOptions::default())?;
//! assert_eq!(reloaded.page_count(), doc.page_count());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Three machines, one serializer
//!
//! - [`save`] and the [`mod@write`] module: object enumeration, cross-reference
//!   emission, trailer construction, incremental append.
//! - [`content`]: a page-object graph back into operator bytes.
//! - [`import`]: cross-document deep copy with reference remapping, N-up
//!   imposition, and the page-range grammar.
//!
//! Font subsetting ([`subset`]) hangs off the first as an object-override
//! pass.
//!
//! # Determinism is a parameter
//!
//! The C++ writer draws `/ID[1]` and the six-letter font-subset tag from a
//! process-global random source, which makes its output unreproducible.
//! [`IdSource`] makes that an argument instead: [`IdSource::Random`] by
//! default, matching the observable behavior, and [`IdSource::Fixed`] when
//! the same input must produce the same bytes.
//!
//! # What a regenerated page loses
//!
//! Regenerating a page's content is lossy, and deliberately so — it
//! reproduces the C++'s emitter exactly, because a conformance comparison
//! against the oracle's regenerated page requires matching what the oracle
//! regenerates. The losses are enumerated in [`content`]'s docs; the short
//! version is that only `rg`/`RG` colours survive, patterns and shadings do
//! not, and text keeps only `Tm`, `Tf`, `Tr` and `TJ`. A page whose objects
//! were never touched is not regenerated at all, so none of this applies to
//! an ordinary save.

#![forbid(unsafe_code)]
// Everything here is written *from* untrusted input: index with `get()`.
#![warn(clippy::indexing_slicing)]

pub mod content;
mod doc;
pub mod encrypt;
mod error;
pub mod font;
pub mod import;
mod names;
pub mod write;

pub use content::{
    ContentsShape, PageRewrite, Regenerated, ResourceTable, ShareCounts, apply_rewrite, regenerate,
    shared_objects, write_float, write_matrix, write_point, write_rect,
};
pub use doc::EditDoc;
pub use encrypt::{Encryptor, IvSource};
pub use error::Error;
pub use font::{GidMap, Subsetted, subset};
pub use import::{ImportOptions, NUpOptions, PageRange, import_pages, n_page_to_one};
pub use write::id::{FileId, IdSource};
pub use write::{SaveMode, SaveOptions, save};
