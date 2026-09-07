#![doc = include_str!("../README.md")]
// Broken-file tolerance is this crate's superpower, and the layering is what
// makes it testable. Each module is a stage that can be exercised on values
// alone:
//
// - `lexer` turns bytes into tokens and owns the byte classifier every other
//   stage asks questions of.
// - `syntax` turns tokens into `Object`s, which is where stream `/Length`
//   repair lives.
// - `xref` finds and reads cross-reference information by every route a file
//   can offer, and rebuilds it from scratch when none of them work.
// - `doc` ties those together into a `Document` whose object store fetches
//   lazily, decrypts, and guards against reference cycles.
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
    CharClass, Delim, Lexer, Token, WordBoundary, atoi64, atoui, class_of, find_word, is_delimiter,
    is_line_ending, is_numeric, is_whitespace, is_whole_word,
};
pub use objstm::{ObjStm, ObjStmEntry};
pub use store::ObjectStore;
pub use syntax::{Indirect, Strictness, parse_indirect_object, parse_object};
pub use xref::{Entry, Section, Trailer, Xref, read_xref};
