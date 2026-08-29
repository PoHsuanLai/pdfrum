//! What can go wrong at this level.
//!
//! Very little can. Nearly every reader here is damage-tolerant by design —
//! a missing key, a mistyped value or a broken tree yields an absent or empty
//! answer plus a diagnostic, never an error — so this enum covers only the
//! two cases where a caller genuinely cannot proceed.

use pdfrum_object::ObjRef;

/// A failure that stops a document-level operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The document has no catalog, so there is nothing to read from.
    #[error("the document has no catalog")]
    NoCatalog,
    /// An indirect object a walk depends on could not be fetched.
    #[error("object {0:?} could not be resolved")]
    Unresolved(ObjRef),
}
