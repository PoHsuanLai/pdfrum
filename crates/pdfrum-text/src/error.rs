//! This crate's error type.

/// What can go wrong querying an extracted page.
///
/// [`extract`](crate::extract) itself is infallible — every damaged input is a
/// recorded [`Diagnostic`](pdfrum_common::Diagnostic), not an `Err`. This enum
/// exists for the query side, where a caller can hand in an index that does
/// not name a character.
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
