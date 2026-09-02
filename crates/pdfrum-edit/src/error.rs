//! What can go wrong while writing a document back out.

use std::io;

use pdfrum_common::PageIndex;

/// A failure that stops the editor producing output.
///
/// Damage in the *input* is not an error here: a broken object silently
/// vanishes from the output the way the C++ writer drops it, and every such
/// recovery is recorded as a [`pdfrum_common::Diagnostic`]. `Err` is reserved
/// for "cannot continue" (SPEC.md §0, STYLE.md §3).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The sink refused the bytes.
    #[error("write failed: {0}")]
    Io(#[from] io::Error),

    /// The document declares `/Encrypt` and the save was not asked to remove
    /// it. v1 writes encrypted documents decrypted (SPEC.md §11's 2026-08-29
    /// ruling E3); preserve-encryption save is deferred past M7.
    #[error("saving an encrypted document requires remove_security (SPEC §11 E3)")]
    EncryptedSaveUnsupported,

    /// A document with no usable catalog cannot be the destination of an
    /// import (`cpdf_pageorganizer.cpp:41-44`, the one hard failure there).
    #[error("the destination document has no catalog to import into")]
    NoDestinationCatalog,

    /// A page index named by an import is outside the source document.
    #[error("page index {0} is outside the source document")]
    PageIndexOutOfRange(PageIndex),

    /// A page-range string the grammar of ISO 32000 viewers accepts could not
    /// be parsed; the C++ treats one bad entry as discarding everything.
    #[error("malformed page range")]
    BadPageRange,

    /// N-up was asked for a grid or sheet size with a zero dimension.
    #[error("N-up needs a non-zero grid and sheet size")]
    BadNupParams,

    /// The font program could not be subset.
    #[error("font subsetting failed: {0}")]
    Subset(String),

    /// The bytes are not a TrueType, OpenType, or Type 1 font program.
    #[error("unrecognised font program")]
    UnrecognisedFontProgram,

    /// The program parsed but declares no glyphs.
    #[error("font program has no glyphs")]
    EmptyFontProgram,

    /// An object the writer needed could not be fetched or made sense of.
    #[error("object model: {0}")]
    Object(#[from] pdfrum_object::Error),
}
