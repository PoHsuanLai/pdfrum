//! This crate's error type.

/// What can go wrong extracting text.
///
/// Almost nothing: extraction is a pure derivation over an already-built
/// [`Page`](pdfrum_page::Page), and `core/fpdftext/` has no error channel at
/// all — every damaged input is a silent skip or a default value there, and a
/// recorded [`Diagnostic`](pdfrum_common::Diagnostic) here
/// (`docs/design/pdfrum-text.md` §1.17). So [`extract`](crate::extract) is
/// infallible and this enum exists for the query side, where a caller can
/// hand in an index that does not name a character.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A character index ran past the end of the extracted character list.
    #[error("character index {index} is past the end of a {len}-character page")]
    CharIndexOutOfRange {
        /// The index that was asked for.
        index: crate::CharIndex,
        /// How many characters the page has.
        len: usize,
    },
}
