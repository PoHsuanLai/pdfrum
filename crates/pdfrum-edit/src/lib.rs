//! Editing and saving (ISO 32000 §7.5.8 incremental updates and full
//! rewrite): the deterministic object serializer, full and incremental save,
//! page import and N-up, content-stream generation, and CID font subsetting.
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
//! - [`save`]: object enumeration, cross-reference emission, trailer
//!   construction, incremental append.
//! - [`regenerate`] and [`apply_rewrite`]: a page-object graph back into
//!   operator bytes, and that result back into the document.
//! - [`import_pages`] and [`n_page_to_one`]: cross-document deep copy with
//!   reference remapping, N-up imposition, and the [`PageRange`] grammar.
//!
//! Font subsetting ([`subset`]) hangs off the first as an object-override
//! pass. Embedding a caller-supplied program ([`EditDoc::embed_font`]) or
//! one of the standard 14 ([`EditDoc::standard_font`]) is how a new `/Font`
//! reaches that pass; [`EditDoc::embed_jpeg`] and [`EditDoc::embed_image`]
//! are the same door for an image `XObject`.
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
//! Regenerating a page's content is lossy, and a caller of [`regenerate`]
//! should know in what way. The losses, in full:
//!
//! - **Colour.** Only `rg` and `RG` are ever written, but **every colour space
//!   converts to them**. Only a *pattern* emits nothing and inherits the black
//!   the per-stream prologue set, because a pattern paints through a resource
//!   no `rg` can name.
//! - **Shadings.** A shading page object emits nothing at all.
//! - **Text.** Only `Tm`, `Tf`, `Tr` and `TJ`. All positioning collapses into
//!   `Tm`, so character and word spacing are lost, and a Type 3 font drops its
//!   whole text object.
//! - **Graphics state.** Only `ca`, `CA` and `BM` reach an `/ExtGState`; the
//!   miter limit and soft masks do not.
//! - **Clips.** Path clips only: text clips, clip-path soft masks and shading
//!   clips are never written.
//!
//! A page whose objects were never touched is not regenerated at all, so none
//! of this applies to an ordinary save.
//!
//! *Corrected 2026-09-02 (A71).* The losses above are limits of this emitter,
//! **not** a matching requirement: nothing constrains a regenerated page to
//! look like anyone else's. The paragraph that used to claim otherwise was
//! wrong, and the crate's private `content` module holds the three lines that
//! settle it.

#![forbid(unsafe_code)]
// Everything here is written *from* untrusted input: index with `get()`.
#![warn(clippy::indexing_slicing)]

// Every module is private and the `pub use` block below is the whole surface,
// so an item is reachable exactly one way and reading that block is reading
// the API (`docs/design/idiomatic-api.md` §A.11 step 12). Siblings still reach
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
pub use font::embed::{EmbeddedFont, FontEncoding};
pub use font::{GidMap, Subsetted, subset};
pub use image::{EmbeddedImage, PixelFormat};
pub use import::{ImportOptions, NUpOptions, PageRange, import_pages, n_page_to_one};
pub use pages::{PageBox, add_blank_page, delete_pages, set_page_box, set_page_rotation};
pub use pdfrum_font::StandardFont;
pub use write::id::{FileId, IdSource};
pub use write::{Encryption, SaveMode, SaveOptions, save};
