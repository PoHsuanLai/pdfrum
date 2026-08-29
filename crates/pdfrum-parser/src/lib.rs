//! The fidelity-critical file parser (ISO 32000 §7.5): zero-copy lexer,
//! object syntax, xref reading (classic tables, xref streams, hybrids,
//! prev-chains) with PDFium's full-file recovery rebuild, object streams,
//! incremental updates, encryption hookup, and the lazy object store behind
//! `Resolve`. Broken-file tolerance is this crate's superpower (SPEC.md §5).
//!
//! # Reading a document
//!
//! ```
//! use std::sync::Arc;
//! use pdfrum_parser::{LoadOptions, load};
//!
//! let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/minimal.pdf")[..]);
//! let doc = load(bytes, &LoadOptions::default())?;
//! assert_eq!(doc.page_count(), 1);
//! # Ok::<(), pdfrum_parser::LoadError>(())
//! ```
//!
//! # Layers
//!
//! Each module below is a stage that can be tested on values alone:
//!
//! - `lexer` turns bytes into tokens and owns the byte classifier every
//!   other stage asks questions of.
//! - `syntax` turns tokens into [`Object`](pdfrum_object::Object)s, which
//!   is where stream `/Length` repair lives.
//! - `xref` finds and reads cross-reference information by every route a
//!   file can offer, and rebuilds it from scratch when none of them work.
//! - `doc` ties those together into a [`Document`] whose object store
//!   fetches lazily, decrypts, and guards against reference cycles.
//!
//! # Damage is normal
//!
//! Nothing here treats a malformed file as an error if PDFium would open it.
//! A repair is recorded in [`Document::diags`] and the read continues; `Err`
//! is reserved for a document that cannot be opened at all.

#![forbid(unsafe_code)]
#![warn(clippy::indexing_slicing)]

mod decode;
mod doc;
mod error;
mod lexer;
mod objstm;
mod store;
mod syntax;
mod xref;

pub use decode::decoded_stream;
pub use doc::{Document, LoadError, LoadOptions, PageDict, load};
pub use error::Error;
pub use lexer::{
    CharClass, Delim, Lexer, Token, atoi64, atoui, class_of, find_word, is_delimiter,
    is_line_ending, is_numeric, is_whitespace, is_whole_word,
};
pub use objstm::{ObjStm, ObjStmEntry};
pub use store::ObjectStore;
pub use syntax::{Strictness, parse_indirect_object, parse_object};
pub use xref::{Entry, Trailer, Xref, read_xref};
