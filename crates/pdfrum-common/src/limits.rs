//! Hard resource limits, defaulting to PDFium-equivalent values.
//!
//! Every field is a cap the reader enforces against untrusted input. The
//! defaults are the C++ constants consolidated in
//! `docs/design/pdfrum-parser.md` §1.20; where PDFium has no cap at all
//! (string, array and dictionary sizes — its outputs are already bounded by
//! file size) the field exists for future hardening and fuzz budgets and
//! defaults to "unbounded".

/// Caps applied while reading a document. Plain configuration data: pass it
/// down, never store it in a parser struct that also owns state.
///
/// ```
/// use pdfrum_common::Limits;
///
/// // PDFium-equivalent defaults, with one cap tightened for a fuzz budget.
/// let limits = Limits { max_array_len: 1 << 20, ..Limits::default() };
/// assert_eq!(limits.max_object_nesting, 64);
/// assert_eq!(limits.max_object_number, 25_165_824);
/// ```
// Deliberately *not* `#[non_exhaustive]`: STYLE.md §4 makes struct-update
// syntax over `Default` the way callers configure options, and the attribute
// forbids exactly that outside this crate. New fields are additive here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum depth of nested arrays/dictionaries accepted while parsing an
    /// object body. Enforced at parse time so access code may recurse freely.
    /// PDFium: `kParserMaxRecursionDepth` (`cpdf_syntax_parser.h`).
    pub max_object_nesting: u32,
    /// Maximum length of a single string object. PDFium has no such cap.
    pub max_string_len: usize,
    /// Maximum number of elements in one array. PDFium has no such cap.
    pub max_array_len: usize,
    /// Maximum number of entries a cross-reference section may declare;
    /// one past the largest legal object number.
    pub max_xref_size: u32,
    /// Largest legal object number. PDFium: `kMaxObjectNumber` (24·2²⁰).
    pub max_object_number: u32,
    /// How far from the start of the file the `%PDF-` header is searched for.
    pub header_scan: u64,
    /// How far back from the end of the file `startxref` is searched for.
    pub startxref_scan: u64,
    /// Maximum number of bytes kept from one syntax token (names, keywords).
    /// Longer tokens are truncated, matching PDFium's word buffer.
    pub max_word_len: usize,
    /// Maximum depth of the page tree walk before it gives up.
    pub max_page_tree_depth: u32,
    /// Maximum number of pages a document may report.
    pub max_page_count: u32,
    /// Maximum number of bytes any single stream filter may produce.
    ///
    /// PDFium has *no* cap here: Flate and LZW decode until they stop, and
    /// only the size it *reports* saturates, at `kMaxTotalOutSize` = 1 GiB
    /// (`flatemodule.cpp`), silently truncating anything larger. We decline to
    /// inherit that zip-bomb surface and turn the same ceiling into a hard
    /// rejection instead; past 1 GiB the oracle's reported size has already
    /// stopped tracking its content, so no stream that decodes faithfully in
    /// the C++ changes behavior. `RunLengthDecode` keeps its own, much
    /// smaller and behaviorally load-bearing 20 MiB cap in `pdfrum-filters`.
    pub max_decoded_stream_len: usize,
}

impl Limits {
    /// The object number that means "no object" in a cross-reference table.
    /// PDFium: `kInvalidObjNum` (`cpdf_object.h`).
    pub const INVALID_OBJ_NUM: u32 = 0xFFFF_FFFF;
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_object_nesting: 64,
            max_string_len: usize::MAX,
            max_array_len: usize::MAX,
            max_xref_size: 25_165_825,
            max_object_number: 25_165_824,
            header_scan: 1024,
            startxref_scan: 4096,
            max_word_len: 256,
            max_page_tree_depth: 1024,
            max_page_count: 0x000F_FFFF,
            max_decoded_stream_len: 1024 * 1024 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Limits;

    #[test]
    fn pdfium_equivalent_defaults() {
        let l = Limits::default();
        assert_eq!(l.max_object_nesting, 64);
        assert_eq!(l.max_xref_size, 25_165_825);
        assert_eq!(l.max_object_number, 25_165_824);
        assert_eq!(l.max_xref_size, l.max_object_number + 1);
        assert_eq!(l.header_scan, 1024);
        assert_eq!(l.startxref_scan, 4096);
        assert_eq!(l.max_word_len, 256);
        assert_eq!(l.max_page_tree_depth, 1024);
        assert_eq!(l.max_page_count, 1_048_575);
        assert_eq!(l.max_decoded_stream_len, 1024 * 1024 * 1024);
        assert_eq!(l.max_string_len, usize::MAX);
        assert_eq!(l.max_array_len, usize::MAX);
        assert_eq!(Limits::INVALID_OBJ_NUM, 0xFFFF_FFFF);
    }

    #[test]
    fn struct_update_syntax_keeps_the_rest() {
        let l = Limits {
            max_array_len: 8,
            ..Limits::default()
        };
        assert_eq!(l.max_array_len, 8);
        assert_eq!(l.max_object_nesting, 64);
    }
}
