//! The damage-tolerance channel.
//!
//! Opening broken PDFs is the behavior this project exists to reproduce, so a
//! recovery is *not* an error: a function that can proceed past damage takes a
//! `&mut Diagnostics`, records what it repaired, and returns the best-effort
//! value. `Err` is reserved for "cannot continue". Nothing here ever fails, so
//! recording a diagnostic never changes control flow.

/// How badly a recorded event bends the file's meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The reader repaired the damage and is confident about the result
    /// (a rebuilt cross-reference table, a `/Length` corrected by scanning
    /// for `endstream`).
    Recovered,
    /// The reader proceeded, but the file said something it should not have
    /// and information was dropped (a malformed dictionary entry skipped, an
    /// out-of-range cross-reference-stream field ignored).
    Suspicious,
}

/// What was repaired.
///
/// Grows as each crate lands, but **a variant earns its place by having a
/// recording site**: it names a condition some crate actually detects and
/// recovers from, and there is a `record` call to prove it. `#[non_exhaustive]`
/// makes both directions non-breaking, and the enum has shrunk as well as
/// grown: a variant is added when a port reaches the condition, not before.
///
/// A variant with no recording site is worse than no variant: it reads as a
/// promise that `Document::diagnostics()` reports the condition, and a caller
/// matching on it waits for a row that never comes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagKind {
    /// The `%PDF-` header was not at offset 0; the given offset became the
    /// origin for every offset in the file.
    HeaderOffset,
    /// `startxref` was missing or pointed at something that is not a
    /// cross-reference section.
    BadStartXref,
    /// The cross-reference table was rebuilt by scanning the whole file.
    XrefRebuilt,
    /// The `/Prev` chain of cross-reference sections looped back on itself.
    XrefPrevLoop,
    /// A cross-reference table's entries did not agree with the objects found
    /// at the offsets they name.
    XrefEntriesShifted,
    /// A cross-reference stream entry was dropped (bad field type, generation
    /// beyond `u16`, or a segment running past the stream).
    XrefStreamEntryDropped,
    /// The trailer's `/Root` was missing or unusable and the catalog was found
    /// another way.
    RootRecovered,
    /// A stream's `/Length` did not match the bytes before `endstream`.
    LengthMismatch,
    /// An `endstream`/`endobj` keyword was missing and the reader resynced.
    KeywordResync,
    /// A dictionary body was malformed: closed by `endobj`, or a key/value
    /// pair was unparsable and skipped.
    MalformedDict,
    /// An array element was unparsable and the array was kept partially.
    MalformedArray,
    /// A stream appeared as a dictionary value or array element, which
    /// ISO 32000-1 §7.3.8.1 forbids; it was dropped.
    StreamInCompositeDropped,
    /// The object at a cross-reference offset carried a different object
    /// number than the table claimed.
    ObjNumMismatch,
    /// An object-stream offset pair was garbage and that entry was skipped.
    ObjStmEntryDropped,
    /// A stream's filter chain was invalid, so the raw bytes were used.
    UndecodableStream,
    /// The page tree needed repair: a guessed `/Type`, a wrong `/Count`, or a
    /// kid that pointed back at an ancestor.
    PageTreeRepaired,
    /// The page tree was deeper than the depth cap and the walk stopped.
    PageTreeDepthExceeded,
    /// The password was accepted only after re-encoding it.
    PasswordReencoded,
    /// An `/Encoding` named no built-in CMap; the font fell back to two-byte
    /// codes mapped to themselves.
    CMapNameUnknown,
    /// An `/Encoding` name matched a known CMap family but no built-in table
    /// carries that exact name, so the decoder is right and the CID map is not.
    CMapTableMissing,
    /// An embedded CMap's `usecmap`, or a stream's `/UseCMap`, named a CMap
    /// that is not one of the built-in ones, so nothing was inherited.
    CMapUsecmapUnknown,
    /// A `/UseCMap` chain ran deeper than `Limits::max_name_tree_depth`; the
    /// rest of the chain was not followed.
    CMapUsecmapDepth,
    /// A codespace range's bounds were discarded: a block declaring exactly
    /// one range keeps only its width.
    CMapCodespaceDropped,
    /// A codespace bound had no closing `>` and was read at whatever width its
    /// digits implied.
    CMapTruncatedCodespace,
    /// A `begincidrange` named a start code above its end code, so it mapped
    /// nothing.
    CMapReversedRange,
    /// Character-code mappings at or above `0x1_0000` were dropped because the
    /// CMap's coding scheme cannot produce codes that wide.
    CMapWideMappingsDropped,
    /// A CMap program declared more ranges than `Limits::max_cmap_ranges`
    /// allows; the rest were dropped.
    CMapRangeLimit,
    /// More operands arrived for one CMap construct than it takes.
    CMapOperandOverflow,
    /// A PFB container's segment chain ended early: a length running past the
    /// blob, a missing `0x80` marker, or no end-of-file record. The segments
    /// read so far were kept.
    Type1PfbTruncated,
    /// A PFA font program's hexadecimal private section ended at a byte that
    /// is not a hex digit, so the tail was dropped.
    Type1HexTruncated,
    /// A Type 1 `/Encoding` entry named a glyph the `/CharStrings` dictionary
    /// does not define, so that character code maps to nothing.
    Type1EncodingGlyphMissing,
    /// A Type 1 charstring could not be interpreted to completion — an
    /// unknown operator, a stack underflow, a missing subroutine, or a
    /// recursion depth cap. Whatever path had been built is kept.
    Type1CharstringAborted,
    /// A Multiple-Master font's `/WeightVector`, `/BlendDesignPositions`,
    /// `/BlendDesignMap` and `/BlendAxisTypes` did not agree on the number of
    /// axes or masters, so the font was treated as non-variable.
    Type1BlendInconsistent,
    /// A `/ToUnicode` `bfchar` or `bfrange` block declared a different number
    /// of entries than it contained, or contained a character code the format
    /// cannot express, so **every mapping in that block** was discarded.
    ToUnicodeBlockRejected,
    /// An embedded font program could not be read by any backend, so the font
    /// was treated as if it had none and went to substitution.
    FontProgramUnreadable,
    /// A `/CIDToGIDMap` stream was shorter than the CIDs indexing into it, so
    /// glyphs past its end resolve to nothing.
    CidToGidStreamShort,
    /// A `/W`, `/W2` or `/Widths` array was malformed and parsing stopped
    /// early or dropped a record; the widths read so far were kept.
    FontWidthsTruncated,
    /// An OpenType `GSUB` table could not be read, so vertical glyph
    /// substitution is unavailable and upright forms are drawn instead.
    GsubUnreadable,
    /// No system or embedded face could be found for a font, and even the
    /// built-in fallback failed to parse.
    FontSubstitutionFailed,

    // ---- Content streams and page building (`pdfrum-page`) ----
    /// A content-stream keyword named no operator; it and its operands were
    /// dropped.
    UnknownOperator,
    /// More than sixteen operands accumulated before one operator, so the
    /// oldest were evicted and the rest silently renumbered.
    OperandsDropped,
    /// An operator that demands an exact operand count did not get it, so it
    /// did nothing at all.
    OperandCountMismatch,
    /// A `Q` arrived with no matching `q`; the graphics state was left alone.
    UnbalancedRestore,
    /// An `EMC` arrived with no matching `BMC`/`BDC`.
    UnbalancedMarkedContent,
    /// A form `XObject` was refused because it re-entered a content buffer
    /// already being parsed, or because too many parses were in flight. The
    /// stream was consumed and contributed no objects.
    FormRecursionRefused,
    /// A `BI` was followed by a keyword other than `ID`, so the inline image
    /// was abandoned and the bytes re-read as ordinary content.
    InlineImageAbandoned,
    /// The scan for an inline image's `EI` absorbed bytes past the end of its
    /// inferred sample data.
    InlineImageResync,
    /// An inline image named a filter whose length cannot be inferred
    /// (`JPXDecode`, `JBIG2Decode`, or an unknown name), so it produced
    /// nothing.
    InlineImageUnsupported,
    /// A `Tr` operand outside 0..=7 was ignored, leaving the previous text
    /// rendering mode in place.
    BadTextRenderMode,
    /// A dash pattern was abandoned in favour of a solid line: an element was
    /// not finite, or the whole cycle fell below the device threshold.
    DashPatternDropped,
    /// A dash element at or below one part in a million was replaced by 0.1.
    DashElementClamped,
    /// A colorspace could not be built, so the operator naming it did
    /// nothing.
    ColorSpaceUnsupported,
    /// An ICC profile was not usable and its `/Alternate` space was used
    /// instead.
    IccAlternateUsed,
    /// An ICC profile was not usable and no `/Alternate` served, so the stock
    /// device space for its `/N` was used.
    IccStockFallback,
    /// An ICC space's `/Alternate` declared a different component count than
    /// its `/N`, so the alternate was discarded.
    IccAlternateMismatch,
    /// An `/Indexed` space's `/hival` was outside 0..=255 and was clamped.
    IndexedHivalClamped,
    /// A `Separation` space's tint transform failed to load or produced too
    /// few outputs; the space kept working without it.
    TintTransformDropped,
    /// A function could not be built, so whatever named it has no transform.
    FunctionUnsupported,
    /// A PostScript calculator program pushed past its stack or popped an
    /// empty one; the values involved were dropped or read as zero.
    PostScriptStackAbuse,
    /// A PostScript `if` or `ifelse` was not preceded by the procedures it
    /// needs, aborting that procedure.
    PostScriptMalformedProc,
    /// A shading failed validation and paints nothing.
    ShadingUnsupported,
    /// A mesh shading's `/Decode` array was not exactly the length its
    /// component count requires.
    MeshDecodeMalformed,
    /// A mesh stream ran out mid-record; the vertices read so far were kept.
    MeshTruncated,
    /// A tiling pattern's `/XStep` or `/YStep` was zero or not finite, so it
    /// draws nothing.
    TilingStepInvalid,
    /// A tiling pattern's tile indices did not fit an `i32`, so it draws
    /// nothing.
    TilingRangeOverflow,
    /// An image's `/BitsPerComponent` was not one of 1, 2, 4, 8 or 16.
    ImageBadBitDepth,
    /// An image's `/Width` or `/Height` was zero, negative, or beyond the
    /// dimension cap.
    ImageBadDimensions,
    /// A codec reported dimensions differing from the image dictionary's, and
    /// the codec's were used.
    ImageDimensionsFromCodec,
    /// A JPEG 2000 codestream's own colour space replaced the one the image
    /// dictionary named.
    JpxColorSpaceOverride,
    /// A codec refused an embedded image, so it paints nothing.
    ImageDecodeFailed,
    /// An image's mask could not be loaded; the base image was kept unmasked.
    MaskDropped,
    /// An image's sample data ended before its last scanline; the remainder
    /// was zero-filled.
    ImageStreamTruncated,
    /// A colour-key `/Mask` array held fewer than two entries per component,
    /// so the ranges it did not state default to zero.
    ColorKeyArrayShort,
    /// A page's `/MediaBox` was missing or empty, so US Letter was used.
    MediaBoxDefaulted,
    /// An optional-content membership dictionary named a `/P` policy that is
    /// none of the four defined ones, which makes its content invisible.
    OptionalContentPolicyUnknown,

    // ---- Text extraction (`pdfrum-text`) ----
    // `core/fpdftext/` has no error channel at all: every damaged input there
    // is a silent skip or a default value. These are those silences, named.
    /// A text object's bounding box had no width, so the object was dropped
    /// whole and contributed no characters.
    TextObjectDegenerate,
    /// A text object was dropped because the one before it showed no glyphs —
    /// a quirk of the batching, not a property of the dropped object.
    TextObjectDropped,
    /// A text object repeated one of the five text objects before it closely
    /// enough to be a redraw, and was dropped.
    TextObjectDuplicate,
    /// Character codes in one text object had no Unicode mapping and were
    /// emitted as raw code points. Carries how many, because a font with a
    /// broken `/ToUnicode` would otherwise record one per character and
    /// flood the sink.
    TextCharcodesUnmapped(u32),
    /// Character code zero appeared, which emits a NUL into the character
    /// stream and nothing into the text.
    TextCharcodeZero,
    /// A marked-content `/ActualText` held no printable character, so the
    /// object it covered emitted nothing at all.
    TextActualTextUnprintable,
    /// An `/ActualText` character at or above `U+FFFD` was skipped, though
    /// the box progression still stepped past it.
    TextActualTextCharDropped,
    /// A soft hyphen was called for with no preceding character to attach it
    /// to. The C++ dereferences an empty container here; we emit nothing.
    TextHyphenNoPrevChar,

    // ---- Document features: navigation, annotations, forms, structure ----
    /// An outline, `/Next` action chain, or field `/Parent` walk revisited a
    /// node it had already seen; the walk stopped there.
    NavigationCycle,
    /// A name, number, structure or field tree exceeded its depth cap. The
    /// lookup answers "not found" rather than recursing further.
    TreeDepthExceeded,
    /// A name-tree node's `/Limits` array was shorter than two entries, or
    /// held its bounds the wrong way round, and was read as repaired.
    NameTreeLimitsRepaired,
    /// A name-tree leaf's `/Names` array had an odd length, so its last key
    /// has no value.
    NameTreeMalformed,
    /// A named destination resolved only through the pre-1.2 `/Dests`
    /// dictionary, not the name tree.
    LegacyNamedDest,
    /// A destination's page could not be turned into an index.
    DestPageUnresolved,
    /// An annotation's `/Subtype` matched no known spelling.
    AnnotSubtypeUnknown,
    /// An appearance stream was generated for an annotation that had none.
    AppearanceGenerated,
    /// A `/QuadPoints` array's length is not a multiple of eight; the tail is
    /// ignored.
    QuadPointsTruncated,
    /// An `/InkList` sub-array was too short to draw, or had an odd length.
    InkPathDropped,
    /// A `/DA` string held no `Tf` operator, so the font name is empty and
    /// the size is zero.
    DefaultAppearanceMalformed,
    /// `/DR /Font` is not a dictionary of font dictionaries, so no form
    /// appearance can be generated.
    FormResourcesInvalid,
    /// A form field carries no `/FT` on itself or its parent.
    FieldSkippedNoType,
    /// A form field's fully-qualified name came out empty, so it was dropped:
    /// the name is a field's identity and its only address.
    FieldSkippedNoName,
    /// A structure element was dropped: its page did not match, or its parent
    /// could not be linked.
    StructElementDropped,
    /// A page label's `/S` names no known numbering style, so the label is
    /// its prefix alone.
    PageLabelStyleUnknown,

    /// A document's script exhausted one of the [`Limits`](crate::Limits)
    /// script bounds — loop iterations, recursion depth or stack — and was
    /// stopped.
    ///
    /// **The hook it was running then takes its *refusing* answer**, not its
    /// permissive one: a script that ran out of budget did not say "accept",
    /// and inventing an acceptance on its behalf is what would let a hostile
    /// file walk past a validator. A build without the script feature can
    /// never record this.
    ScriptLimitReached,
    /// A document's script threw, or would not parse, and was abandoned.
    ///
    /// An ordinary outcome for untrusted input rather than an error: the rest
    /// of the document is unaffected, and the hook takes its refusing answer
    /// for the same reason as [`DiagKind::ScriptLimitReached`].
    ScriptFailed,
    /// `Limits::deadline` passed inside an operation that cannot fail — the
    /// content interpreter or the text extractor — which stopped where it was
    /// and returned what it had. The result is partial: the objects before
    /// the stop, or an empty text page.
    ///
    /// Recorded once per stop. The fallible entry points (open, page load,
    /// render) answer `LimitExceeded::Time` instead of recording this, so a
    /// render that fails on the deadline still carries the interpreter's
    /// record of where the build stopped.
    TimeLimitReached,
}

impl core::fmt::Display for Severity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Recovered => "recovered",
            Self::Suspicious => "suspicious",
        })
    }
}

impl DiagKind {
    /// One lower-case clause naming what was repaired, for a human reading a
    /// report.
    ///
    /// The `Debug` spelling is the variant identifier and is what `--json`
    /// carries, because a machine consumer wants a token that survives
    /// rewording; this is the other half, and the two are deliberately not
    /// interchangeable.
    ///
    /// The match is exhaustive with no wildcard arm: `#[non_exhaustive]`
    /// binds downstream crates, not this one, so a variant added without a
    /// clause here fails to compile. That is the point — it is the same gate
    /// the enum's own doc comment asks for, applied to the wording.
    ///
    /// ```
    /// use pdfrum_common::DiagKind;
    ///
    /// assert_eq!(
    ///     DiagKind::XrefRebuilt.message(),
    ///     "cross-reference table rebuilt by scanning the file",
    /// );
    /// ```
    #[must_use]
    // One arm per variant: the length is the enum's, not the function's.
    #[allow(clippy::too_many_lines)]
    pub fn message(&self) -> &'static str {
        match self {
            Self::HeaderOffset => "%PDF- header was not at the start of the file",
            Self::BadStartXref => "startxref was missing or did not name a cross-reference section",
            Self::XrefRebuilt => "cross-reference table rebuilt by scanning the file",
            Self::XrefPrevLoop => "cross-reference /Prev chain looped back on itself",
            Self::XrefEntriesShifted => "cross-reference entries disagreed with the objects found",
            Self::XrefStreamEntryDropped => "cross-reference stream entry dropped",
            Self::RootRecovered => "trailer /Root was unusable; catalog found another way",
            Self::LengthMismatch => "stream /Length did not match the bytes before endstream",
            Self::KeywordResync => "missing endstream/endobj keyword; reader resynced",
            Self::MalformedDict => "malformed dictionary body",
            Self::MalformedArray => "malformed array element; array kept partially",
            Self::StreamInCompositeDropped => "stream inside a dictionary or array dropped",
            Self::ObjNumMismatch => "object number did not match the cross-reference table",
            Self::ObjStmEntryDropped => "object-stream entry skipped",
            Self::UndecodableStream => "stream filter chain was invalid; raw bytes used",
            Self::PageTreeRepaired => "page tree repaired",
            Self::PageTreeDepthExceeded => "page tree deeper than the depth cap; walk stopped",
            Self::PasswordReencoded => "password accepted only after re-encoding",
            Self::CMapNameUnknown => "/Encoding named no built-in CMap; fell back to two-byte codes",
            Self::CMapTableMissing => "no built-in CMap table carries that exact name",
            Self::CMapUsecmapUnknown => "usecmap named a CMap that is not built in",
            Self::CMapUsecmapDepth => "/UseCMap chain ran past its depth cap",
            Self::CMapCodespaceDropped => "codespace range bounds discarded",
            Self::CMapTruncatedCodespace => "codespace bound had no closing >",
            Self::CMapReversedRange => "begincidrange start code was above its end code",
            Self::CMapWideMappingsDropped => "character mappings too wide for the coding scheme dropped",
            Self::CMapRangeLimit => "CMap declared more ranges than the limit allows",
            Self::CMapOperandOverflow => "too many operands for one CMap construct",
            Self::Type1PfbTruncated => "PFB segment chain ended early",
            Self::Type1HexTruncated => "PFA hexadecimal private section ended early",
            Self::Type1EncodingGlyphMissing => "Type 1 /Encoding named an undefined glyph",
            Self::Type1CharstringAborted => "Type 1 charstring could not be interpreted to completion",
            Self::Type1BlendInconsistent => "Multiple-Master blend arrays disagreed; treated as non-variable",
            Self::ToUnicodeBlockRejected => "/ToUnicode block rejected; its mappings were discarded",
            Self::FontProgramUnreadable => "embedded font program unreadable; substituted instead",
            Self::CidToGidStreamShort => "/CIDToGIDMap stream was shorter than the CIDs indexing it",
            Self::FontWidthsTruncated => "font widths array was malformed; parsing stopped early",
            Self::GsubUnreadable => "OpenType GSUB table unreadable; upright forms drawn",
            Self::FontSubstitutionFailed => "no face found and the built-in fallback failed to parse",
            Self::UnknownOperator => "content-stream keyword named no operator",
            Self::OperandsDropped => "more than sixteen operands accumulated; oldest evicted",
            Self::OperandCountMismatch => "operator did not get its exact operand count",
            Self::UnbalancedRestore => "Q with no matching q",
            Self::UnbalancedMarkedContent => "EMC with no matching BMC/BDC",
            Self::FormRecursionRefused => "form XObject refused as re-entrant",
            Self::InlineImageAbandoned => "BI was not followed by ID; inline image abandoned",
            Self::InlineImageResync => "inline image EI scan ran past the inferred sample data",
            Self::InlineImageUnsupported => "inline image filter length cannot be inferred",
            Self::BadTextRenderMode => "Tr operand outside 0..=7 ignored",
            Self::DashPatternDropped => "dash pattern abandoned for a solid line",
            Self::DashElementClamped => "dash element below the threshold replaced by 0.1",
            Self::ColorSpaceUnsupported => "colorspace could not be built",
            Self::IccAlternateUsed => "ICC profile unusable; /Alternate space used",
            Self::IccStockFallback => "ICC profile unusable; stock device space used",
            Self::IccAlternateMismatch => "ICC /Alternate component count disagreed with /N",
            Self::IndexedHivalClamped => "/Indexed /hival outside 0..=255 was clamped",
            Self::TintTransformDropped => "Separation tint transform dropped",
            Self::FunctionUnsupported => "function could not be built",
            Self::PostScriptStackAbuse => "PostScript calculator over- or under-ran its stack",
            Self::PostScriptMalformedProc => "PostScript if/ifelse lacked its procedures",
            Self::ShadingUnsupported => "shading failed validation and paints nothing",
            Self::MeshDecodeMalformed => "mesh shading /Decode array had the wrong length",
            Self::MeshTruncated => "mesh stream ran out mid-record",
            Self::TilingStepInvalid => "tiling pattern /XStep or /YStep was zero or not finite",
            Self::TilingRangeOverflow => "tiling pattern tile indices did not fit an i32",
            Self::ImageBadBitDepth => "image /BitsPerComponent was not 1, 2, 4, 8 or 16",
            Self::ImageBadDimensions => "image /Width or /Height was zero, negative, or too large",
            Self::ImageDimensionsFromCodec => "codec dimensions differed from the dictionary's",
            Self::JpxColorSpaceOverride => "JPEG 2000 codestream colour space replaced the dictionary's",
            Self::ImageDecodeFailed => "codec refused an embedded image",
            Self::MaskDropped => "image mask could not be loaded; base image kept unmasked",
            Self::ImageStreamTruncated => "image data ended early; remainder zero-filled",
            Self::ColorKeyArrayShort => "colour-key /Mask array was short; missing ranges default to zero",
            Self::MediaBoxDefaulted => "page /MediaBox was missing or empty; US Letter used",
            Self::OptionalContentPolicyUnknown => "optional-content /P policy is not one of the four defined",
            Self::TextObjectDegenerate => "text object had no width and was dropped",
            Self::TextObjectDropped => "text object dropped because the one before it showed no glyphs",
            Self::TextObjectDuplicate => "text object was a redraw of a recent one and was dropped",
            Self::TextCharcodesUnmapped(_) => "character codes had no Unicode mapping",
            Self::TextCharcodeZero => "character code zero emitted a NUL",
            Self::TextActualTextUnprintable => "/ActualText held no printable character",
            Self::TextActualTextCharDropped => "/ActualText character at or above U+FFFD skipped",
            Self::TextHyphenNoPrevChar => "soft hyphen had no preceding character to attach to",
            Self::NavigationCycle => "navigation walk revisited a node it had already seen",
            Self::TreeDepthExceeded => "tree exceeded its depth cap; lookup answered not-found",
            Self::NameTreeLimitsRepaired => "name-tree /Limits array was short or reversed",
            Self::NameTreeMalformed => "name-tree /Names array had an odd length",
            Self::LegacyNamedDest => "named destination resolved through the pre-1.2 /Dests dictionary",
            Self::DestPageUnresolved => "destination page could not be turned into an index",
            Self::AnnotSubtypeUnknown => "annotation /Subtype matched no known spelling",
            Self::AppearanceGenerated => "appearance stream generated for an annotation that had none",
            Self::QuadPointsTruncated => "/QuadPoints length was not a multiple of eight",
            Self::InkPathDropped => "/InkList sub-array was too short or had an odd length",
            Self::DefaultAppearanceMalformed => "/DA string held no Tf operator",
            Self::FormResourcesInvalid => "/DR /Font is not a dictionary of font dictionaries",
            Self::FieldSkippedNoType => "form field carries no /FT",
            Self::FieldSkippedNoName => "form field's qualified name came out empty",
            Self::StructElementDropped => "structure element dropped",
            Self::PageLabelStyleUnknown => "page label /S names no known numbering style",
            Self::ScriptLimitReached => "script exhausted a limit and was stopped",
            Self::ScriptFailed => "script threw or would not parse and was abandoned",
            Self::TimeLimitReached => "time limit passed; the result is partial",
        }
    }
}

impl core::fmt::Display for DiagKind {
    /// The [`message`](DiagKind::message) clause, with any count the variant
    /// carries appended: `character codes had no Unicode mapping (3)`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())?;
        match self {
            Self::TextCharcodesUnmapped(n) => write!(f, " ({n})"),
            _ => Ok(()),
        }
    }
}

/// One recorded recovery.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// How badly the file was bent.
    pub severity: Severity,
    /// What was repaired.
    pub what: DiagKind,
    /// Byte offset into the file where the damage was seen, when known.
    /// Offsets are header-relative, matching how the reader indexes the file.
    pub at: Option<u64>,
}

/// A bounded sink of [`Diagnostic`]s.
///
/// Bounded because a sufficiently broken file produces a diagnostic per
/// object: past the limit the entries are counted but not stored, so a fuzz
/// case cannot turn a diagnostic channel into an out-of-memory condition.
///
/// ```
/// use pdfrum_common::{DiagKind, Diagnostics, Severity};
///
/// let mut diags = Diagnostics::with_limit(1);
/// diags.record(Severity::Recovered, DiagKind::XrefRebuilt, None);
/// diags.record(Severity::Suspicious, DiagKind::MalformedDict, Some(42));
/// assert_eq!(diags.len(), 1); // second entry counted, not stored
/// assert_eq!(diags.recorded(), 2);
/// assert!(diags.dropped() > 0);
/// ```
#[derive(Debug, Clone)]
pub struct Diagnostics {
    entries: Vec<Diagnostic>,
    limit: usize,
    recorded: usize,
}

impl Diagnostics {
    /// Default number of diagnostics kept before the sink starts counting
    /// only. Chosen to survive a pathological file without unbounded growth.
    pub const DEFAULT_LIMIT: usize = 4096;

    /// An empty sink keeping at most `limit` entries.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit,
            recorded: 0,
        }
    }

    /// Record a recovery. Never fails; past the limit the entry is counted
    /// but not stored.
    pub fn record(&mut self, severity: Severity, what: DiagKind, at: Option<u64>) {
        self.recorded = self.recorded.saturating_add(1);
        if self.entries.len() < self.limit {
            self.entries.push(Diagnostic { severity, what, at });
        }
    }

    /// Fold another sink's diagnostics into this one, keeping their order.
    ///
    /// A call that reads through the stack collects into its own sink; the
    /// caller that owns the longer-lived record folds it in when the call
    /// returns. This crate's own limit still applies, so merging a full sink
    /// into a full one counts the entries without storing them, exactly as
    /// [`Diagnostics::record`] does.
    ///
    /// ```
    /// use pdfrum_common::{DiagKind, Diagnostics, Severity};
    ///
    /// let mut whole = Diagnostics::default();
    /// let mut part = Diagnostics::default();
    /// part.record(Severity::Recovered, DiagKind::XrefRebuilt, Some(9));
    ///
    /// whole.extend(&part);
    /// assert!(whole.contains(&DiagKind::XrefRebuilt));
    /// // `part` is unchanged: this reads it rather than draining it.
    /// assert_eq!(part.len(), 1);
    /// ```
    pub fn extend(&mut self, other: &Diagnostics) {
        for entry in other.entries() {
            self.record(entry.severity, entry.what.clone(), entry.at);
        }
    }

    /// The stored diagnostics, in the order they were recorded.
    #[must_use]
    pub fn entries(&self) -> &[Diagnostic] {
        &self.entries
    }

    /// Number of stored diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether anything was stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total number of recoveries recorded, including those dropped past the
    /// limit.
    #[must_use]
    pub fn recorded(&self) -> usize {
        self.recorded
    }

    /// How many recoveries were counted but not stored.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.recorded.saturating_sub(self.entries.len())
    }

    /// Whether any stored diagnostic matches `what`.
    #[must_use]
    pub fn contains(&self, what: &DiagKind) -> bool {
        self.entries.iter().any(|d| &d.what == what)
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::with_limit(Self::DEFAULT_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagKind, Diagnostics, Severity};

    #[test]
    fn records_in_order() {
        let mut d = Diagnostics::default();
        d.record(Severity::Recovered, DiagKind::HeaderOffset, Some(7));
        d.record(Severity::Suspicious, DiagKind::MalformedArray, None);
        assert_eq!(d.len(), 2);
        assert_eq!(d.entries()[0].what, DiagKind::HeaderOffset);
        assert_eq!(d.entries()[0].at, Some(7));
        assert_eq!(d.entries()[1].severity, Severity::Suspicious);
        assert!(d.contains(&DiagKind::MalformedArray));
        assert!(!d.contains(&DiagKind::XrefRebuilt));
        assert_eq!(d.dropped(), 0);
    }

    #[test]
    fn empty_by_default() {
        let d = Diagnostics::default();
        assert!(d.is_empty());
        assert_eq!(d.recorded(), 0);
    }

    #[test]
    fn limit_counts_without_storing() {
        let mut d = Diagnostics::with_limit(0);
        for _ in 0..5 {
            d.record(Severity::Recovered, DiagKind::KeywordResync, None);
        }
        assert!(d.is_empty());
        assert_eq!(d.recorded(), 5);
        assert_eq!(d.dropped(), 5);
    }
}
