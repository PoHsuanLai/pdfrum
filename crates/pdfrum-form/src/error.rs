//! This crate's error type.
//!
//! Interaction is damage-tolerant in the same way parsing is: an event that
//! cannot be delivered is not an error, it is an event that consumed nothing.
//! `Err` is reserved for a caller mistake that leaves nothing sensible to
//! return.

/// What went wrong.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// An event named a page the context does not carry.
    #[error("no such page in this form context: {page}")]
    NoSuchPage {
        /// The page index asked for.
        page: u32,
    },
    /// An event named a field the form does not have.
    #[error("no such field in this form: {field}")]
    NoSuchField {
        /// The field index asked for.
        field: u32,
    },
}
