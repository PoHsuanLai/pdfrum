//! Failures the object layer can report.

use crate::ObjRef;

/// What can go wrong constructing or resolving PDF objects.
///
/// Deliberately short: almost everything in this crate is total. Typed
/// accessors never error — a wrong type or a broken reference is *absence*
/// (`None` or the C++ fallback value), matching PDFium, and the store records
/// the underlying failure in its `Diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The store has no object under this reference: the cross-reference
    /// entry is free, missing, or the object body failed to parse.
    #[error("no object for reference {}:{}", .0.num, .0.generation)]
    UnresolvedRef(ObjRef),

    /// Fetching this reference re-entered a fetch already in progress
    /// (an object whose body refers to itself, directly or through `/Length`).
    #[error("reference cycle while fetching {}:{}", .0.num, .0.generation)]
    RefLoop(ObjRef),

    /// A byte span was asked for a range outside its backing buffer.
    #[error("span range {start}..{end} is outside a buffer of {len} bytes")]
    SpanOutOfBounds {
        /// Start of the requested range.
        start: usize,
        /// End of the requested range.
        end: usize,
        /// Length of the backing buffer.
        len: usize,
    },
}
