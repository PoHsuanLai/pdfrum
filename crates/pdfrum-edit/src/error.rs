//! What can go wrong while writing a document back out.

use std::io;

use pdfrum_common::PageIndex;

/// A failure that stops the editor producing output.
///
/// Damage in the *input* is not an error here: a broken object silently
/// vanishes from the output the way the C++ writer drops it, and every such
/// recovery is recorded as a [`pdfrum_common::Diagnostic`]. `Err` is reserved
/// for "cannot continue".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The sink refused the bytes.
    #[error("write failed: {0}")]
    Io(#[from] io::Error),

    /// A page size for [`blank_document`](crate::blank_document) that is not
    /// finite and positive.
    #[error("a page must have a finite, positive size, not {0:?}")]
    BadPageSize(kurbo::Size),

    /// A glyph font drew more than 65 535 distinct glyphs, all an Identity-H
    /// code can name.
    #[error("a glyph font can name at most 65535 glyphs")]
    TooManyGlyphs,

    /// A glyph font needs instancing — a CFF2 face, or a variable instance
    /// other than the default — and the `variable-fonts` feature is off.
    #[error("this face needs the variable-fonts feature")]
    VariableFontsDisabled,

    /// A glyph run named a [`GlyphFont`](crate::GlyphFont) this session did not
    /// embed.
    #[error("the glyph font belongs to another editing session")]
    ForeignGlyphFont,

    /// A PNG [`EditDoc::embed_png`](crate::EditDoc::embed_png) cannot pass
    /// through as stored: alpha, `tRNS`, interlacing or 16-bit samples.
    #[error("this PNG must be decoded before it is embedded")]
    PngNeedsDecoding,

    /// The blank document did not read back. Not expected: the bytes are this
    /// crate's own; the message is the parser's.
    #[error("the blank document did not parse: {0}")]
    BlankDocument(String),

    /// The document declares `/Encrypt`, this reader derived no key for it
    /// (an `/Identity` crypt filter, or a handler opened as
    /// [`pdfrum_crypt::SecurityHandler::Identity`]), and `remove_security`
    /// was not set. Re-declaring a cipher over plaintext would produce a file
    /// nothing could open.
    #[error(
        "cannot save an encrypted document without its key; \
         set `SaveOptions::remove_security` to save it decrypted"
    )]
    EncryptedSaveUnsupported,
    /// A password given for a new encryption is not valid UTF-8, or does
    /// not survive `SASLprep` (ISO 32000-2 §7.6.4.3.3).
    #[error("a password for encryption must be text")]
    PasswordNotText,
    /// The operating system's cryptographic generator is unavailable, so the
    /// file key and AES vectors a new encryption needs cannot be drawn.
    /// Only an encrypting save can raise this; every other save needs no
    /// randomness it cannot derive.
    #[error("the operating system's random generator is unavailable")]
    NoEntropy,

    /// A document with no usable catalog cannot be the destination of an
    /// import: there is nowhere to attach the pages.
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

    /// An SVG would not resolve. Only produced with the `svg-import`
    /// feature, whose ingestion parses it.
    #[cfg(feature = "svg-import")]
    #[error("the SVG would not resolve: {0}")]
    Svg(#[source] usvg::Error),

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
    /// The font it would write would carry no statement of what its codes
    /// mean. A caller who wanted a *generated* `/ToUnicode` should ask for
    /// [`FontEncoding::Composite`](crate::FontEncoding::Composite) instead.
    #[error("the /ToUnicode CMap is empty")]
    EmptyToUnicodeCMap,

    /// A caller-supplied `/CIDToGIDMap` was empty, or was not a whole number
    /// of big-endian `u16` entries.
    ///
    /// `[oracle-bug]` ISO 32000-1 §9.7.4.2 defines `/CIDToGIDMap` as a stream
    /// of two-byte glyph indices, so a half-entry is not a map. Both the
    /// empty and the odd-length case are rejected here, before any of it is
    /// written.
    //
    // [oracle-bug] `FPDFText_LoadCidType2Font` rejects the empty case
    // (`fpdfsdk/fpdf_edittext.cpp:488-491`) but not the odd-length one:
    // `LoadCustomCompositeFont` walks `i += 2` to
    // `cid_to_gid_map_span.size()` and takes
    // `cid_to_gid_map_span.subspan(i).first<2u>()` (`:308-313`), so a final
    // one-byte remainder asks a 1-element span for its first 2 elements —
    // a bounds `CHECK` in `pdfium::span`, i.e. an abort rather than a
    // rejection. An error is the same answer without the crash. pdf.js is the
    // reader that depends on the map: `readCidToGidMap` pairs the bytes as
    // `(glyphsData[j++] << 8) | glyphsData[j]` over the map's whole length
    // (`src/core/evaluator.js:4103-4116`), so a trailing half-entry reads
    // `undefined` as its low byte and yields a glyph index 256 times too
    // large for the last CID.
    #[error("the /CIDToGIDMap is {0} bytes; it must be a non-empty whole number of 2-byte entries")]
    BadCidToGidMap(usize),

    /// The bytes are not a JPEG or JPEG 2000 codestream a PDF may hold.
    ///
    /// Raised when no JPEG header can be read, when the component count is
    /// not 1, 3 or 4, or when the sample precision is not 1, 2, 4, 8 or 16.
    /// The oracle reaches the same outcome by producing no stream at all;
    /// this is the same refusal with the reason attached.
    #[error("not a JPEG or JPEG 2000 image")]
    UnrecognisedImageData,

    /// An image was asked for with a zero width or height.
    #[error("an image needs a non-zero width and height")]
    EmptyImage,

    /// The sample buffer is not the length the dimensions and pixel format
    /// require.
    ///
    /// The oracle cannot reach this: its API takes a bitmap object that
    /// already knows its own pitch, so a short buffer is not expressible.
    /// Loose bytes are, and reading past them is not an option.
    #[error("image data is {found} bytes; {expected} are needed")]
    ImageDataLength {
        /// Bytes the dimensions and format require.
        expected: usize,
        /// Bytes the caller supplied.
        found: usize,
    },

    /// A drawing refused a character the font has no glyph for.
    ///
    /// Raised by [`EmbeddedFont::encode_checked`](crate::EmbeddedFont::encode_checked)
    /// and by the facade's canvas, which encodes through it. A blank where a
    /// glyph should be is a worse answer than a refusal, because nothing
    /// downstream can tell the two apart.
    #[error("cannot draw the text: {0}")]
    MissingGlyph(#[from] crate::MissingGlyph),

    /// A page written inline in its parent's `/Kids` has no object of its own
    /// for an appended content stream to reach.
    #[error("page {0} has no object of its own to draw on")]
    InlinePage(PageIndex),

    /// A highlight or underline was asked for with no `/QuadPoints`.
    ///
    /// ISO 32000-1 §12.5.6.10 requires the array; an empty one is not a
    /// markup of anything.
    #[error("a highlight or underline needs at least one quadrilateral")]
    EmptyQuadPoints,

    /// `update_annotation` / `delete_annotation` named an object that is not
    /// listed in that page's `/Annots`.
    #[error("annotation {}:{} is not on page {}", .0.num, .0.generation, .1)]
    AnnotNotOnPage(pdfrum_object::ObjRef, PageIndex),

    /// `update_annotation_at` / `delete_annotation_at` named an index outside
    /// the page's `/Annots` array (or the page has no `/Annots`).
    #[error("annotation index {0} is out of range on page {1}")]
    AnnotIndexOutOfRange(usize, PageIndex),

    /// `add_form_field` was given a name the document already uses.
    ///
    /// A field's fully qualified name is its identity, not a label: the reader
    /// merges two fields sharing one into a single field with two controls,
    /// and every lookup, action and script then addresses both at once. So a
    /// collision is refused rather than written.
    #[error("the document already has a form field named {0:?}")]
    DuplicateFieldName(String),

    /// `add_form_field` was given an empty name.
    ///
    /// The reader drops a field with no name anywhere in its ancestry, so such
    /// a field would be written and then silently lost on the next load.
    #[error("a form field needs a name")]
    EmptyFieldName,

    /// `add_form_field` was given a radio group with no buttons.
    #[error("a radio group needs at least one button")]
    EmptyRadioGroup,
    /// `set_attachment_name` was given a name another attachment already has.
    ///
    /// The embedded-files tree is keyed by name, so writing a duplicate would
    /// leave one of the two unreachable by the only lookup there is.
    #[error("another attachment is already named {0:?}")]
    DuplicateAttachmentName(String),

    /// An object the writer needed could not be fetched or made sense of.
    #[error("object model: {0}")]
    Object(#[from] pdfrum_object::Error),
}
