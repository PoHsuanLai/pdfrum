//! The damage-tolerance channel (STYLE.md §3).
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

/// What was repaired. Grows as each crate lands; every recovery PDFium
/// performs silently gets a variant here (see the diagnostics tables in
/// `docs/design/`).
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
    /// An embedded CMap's `usecmap` was recognised and ignored, so whatever
    /// the named base map would have contributed is absent.
    CMapUsecmapIgnored,
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
    /// A `/ToUnicode` destination held an unpaired UTF-16 surrogate, which a
    /// Rust `char` cannot represent; it became U+FFFD (font brief D3).
    ToUnicodeLoneSurrogate,
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
