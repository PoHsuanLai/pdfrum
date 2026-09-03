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

    /// A caller-supplied `/ToUnicode` CMap was empty.
    ///
    /// `FPDFText_LoadCidType2Font` rejects the same input twice — a null
    /// pointer and a zero-length string (`fpdfsdk/fpdf_edittext.cpp:488-495`)
    /// — because the font it would write has no statement of what its codes
    /// mean, and a caller who wanted generated `/ToUnicode` should have asked
    /// for [`FontEncoding::Composite`](crate::FontEncoding::Composite).
    #[error("the /ToUnicode CMap is empty")]
    EmptyToUnicodeCMap,

    /// A caller-supplied `/CIDToGIDMap` was empty, or was not a whole number
    /// of big-endian `u16` entries.
    ///
    /// `FPDFText_LoadCidType2Font` rejects the empty case
    /// (`fpdfsdk/fpdf_edittext.cpp:488-491`). The odd-length case it does not
    /// check: `LoadCustomCompositeFont` walks `i += 2` to
    /// `cid_to_gid_map_span.size()` and takes
    /// `cid_to_gid_map_span.subspan(i).first<2u>()` (`:308-313`), so a final
    /// one-byte remainder asks a 1-element span for its first 2 elements —
    /// a bounds `CHECK` in `pdfium::span`, i.e. an abort rather than a
    /// rejection. ISO 32000-1 §9.7.4.2 defines `/CIDToGIDMap` as a stream of
    /// two-byte glyph indices, so a half-entry is not a map; an error is the
    /// same answer without the crash. pdf.js is the reader that depends on
    /// it: `readCidToGidMap` pairs the bytes as
    /// `(glyphsData[j++] << 8) | glyphsData[j]` over the map's whole length
    /// (`src/core/evaluator.js:4103-4116`), so a trailing half-entry reads
    /// `undefined` as its low byte and yields a glyph index 256 times too
    /// large for the last CID.
    #[error("the /CIDToGIDMap is {0} bytes; it must be a non-empty whole number of 2-byte entries")]
    BadCidToGidMap(usize),

    /// An object the writer needed could not be fetched or made sense of.
    #[error("object model: {0}")]
    Object(#[from] pdfrum_object::Error),
}
