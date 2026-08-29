//! The one error type this crate returns.
//!
//! Almost nothing here can fail: a simple font always constructs, even with no
//! glyphs at all, and a damaged font program degrades into substitution rather
//! than erroring. `Err` is reserved for the four CID-font cases PDFium itself
//! treats as "this resource does not exist", and for a font program no backend
//! can read at all.

use thiserror::Error;

/// What went wrong while loading a font.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Error {
    /// A Type0 font's `/DescendantFonts` was missing, was not an array, or did
    /// not hold exactly one element. PDFium fails the load here and the
    /// content-stream interpreter then skips text using the font.
    #[error("a Type0 font's /DescendantFonts must hold exactly one dictionary")]
    BadDescendantFonts,
    /// A Type0 font's `/Encoding` was absent, or was neither a name nor a
    /// stream.
    #[error("a Type0 font's /Encoding must be a predefined name or a CMap stream")]
    BadCidEncoding,
    /// A font program was present but no backend could read it. The font falls
    /// back to substitution; this is returned only by the direct
    /// program-loading entry point.
    #[error("the embedded font program is in no format this build can read")]
    UnreadableFontProgram,
}
