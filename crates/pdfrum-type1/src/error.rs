//! The crate's error type.
//!
//! Very little here is fatal. A Type 1 program that names a subroutine it does
//! not have, blends with a weight vector of the wrong arity, or runs off the
//! end of a charstring yields *no outline for that glyph* — the font is still
//! usable for every other glyph, so those cases surface as `None` from
//! [`outline`](crate::Type1Font::outline) rather than as an `Err`.
//!
//! `Error` is reserved for "this blob is not a font we can use at all".

/// Why a Type 1 font program could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The blob is shorter than the smallest thing that could be a font.
    #[error("font program is empty")]
    Empty,

    /// A PFB container's segment headers did not describe the blob: a segment
    /// length running past the end, a tag byte other than `0x80`, or a segment
    /// type outside 1–3.
    ///
    /// Carries the offset of the offending header, which is what a fuzz
    /// reduction wants.
    #[error("PFB segment header at {at} is not usable")]
    PfbSegment {
        /// Byte offset of the header that failed.
        at: usize,
    },

    /// No `eexec` token was found, so the font has no private portion — and a
    /// Type 1 font's charstrings live entirely inside it.
    #[error("no eexec section")]
    NoEexec,

    /// The `eexec` section decrypted to something that is not a PostScript
    /// private dictionary, which means the wrong bytes were fed to the cipher.
    #[error("eexec section did not decrypt to a private dictionary")]
    EexecGarbage,

    /// The program parsed but declared no `/CharStrings`, so there is nothing
    /// to draw.
    #[error("font program has no CharStrings dictionary")]
    NoCharStrings,
}
