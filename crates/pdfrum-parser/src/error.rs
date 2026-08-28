//! What can go wrong reading a file.
//!
//! Short by design. Almost everything this crate does about damage is a
//! *recovery* recorded in `Diagnostics`, not a failure, so the error type
//! only names the conditions under which reading genuinely stops.

use pdfrum_object::ObjRef;

/// A failure below the document level: a token that is not an object, a
/// cross-reference section that cannot be read, a fetch that finds nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes at this position are not the start of any object.
    #[error("no object at offset {0}")]
    NoObject(u64),

    /// Objects nested deeper than the configured limit.
    #[error("object nesting deeper than {0}")]
    TooDeep(u32),

    /// No cross-reference information could be read or reconstructed.
    #[error("no usable cross-reference information")]
    XrefBroken,

    /// The trailer named no catalog, or the object it named is not one.
    #[error("no document catalog")]
    NoCatalog,

    /// A reference names an object the cross-reference table has no entry
    /// for, has marked free, or whose body would not parse.
    #[error("no object for reference {}:{}", .0.num, .0.generation)]
    Unresolved(ObjRef),

    /// This fetch re-entered one already in progress.
    #[error("reference cycle while fetching {}:{}", .0.num, .0.generation)]
    Cycle(ObjRef),

    /// A page index past the document's page count, or one whose tree walk
    /// found nothing.
    #[error("no page at index {0}")]
    NoPage(u32),
}

impl From<Error> for pdfrum_object::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Cycle(r) => Self::RefLoop(r),
            Error::Unresolved(r) => Self::UnresolvedRef(r),
            // Everything else reaching an accessor reads as a dangling
            // reference, which is how a damaged file already behaves.
            _ => Self::UnresolvedRef(ObjRef::new(0, 0)),
        }
    }
}
