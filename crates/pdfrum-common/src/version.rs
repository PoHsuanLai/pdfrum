//! The PDF version a file declares in its header, as a pair of digits.
//!
//! `%PDF-1.7` is `PdfVersion { major: 1, minor: 7 }`. The parser reads the
//! header, the writer emits one, and the facade forwards both — so the type
//! lives here, at the bottom of the dependency graph, where all three can name
//! it without any of them depending on either of the others.

use core::fmt;

/// The version digits from a `%PDF-M.N` header.
///
/// Two independent digits, not a packed integer. The packed form — `17` for
/// 1.7 — survives only as a private conversion beside the header parser
/// (`pdfrum-parser`'s `doc::read_version`), and appears in no public
/// signature anywhere in the workspace.
///
/// Ordering is lexicographic on `(major, minor)`, so `PDF_1_4 < PDF_1_7 <
/// PDF_2_0` — which is what "at least version X" means, and what a packed
/// comparison gets wrong the moment a major digit reaches two of its own
/// (`PdfVersion::new(11, 0)` orders above 1.9; `110 > 19` only by luck of the
/// digit count).
///
/// ```
/// use pdfrum_common::PdfVersion;
///
/// assert_eq!(PdfVersion::PDF_1_7.to_string(), "1.7");
/// assert!(PdfVersion::PDF_1_4 < PdfVersion::PDF_2_0);
/// assert_eq!(PdfVersion::new(1, 5), PdfVersion::PDF_1_5);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PdfVersion {
    /// The digit before the dot.
    pub major: u8,
    /// The digit after it.
    pub minor: u8,
}

impl PdfVersion {
    /// PDF 1.0, the oldest version the writer will emit on request.
    pub const PDF_1_0: Self = Self { major: 1, minor: 0 };
    /// PDF 1.4 — the last version before object and cross-reference streams.
    pub const PDF_1_4: Self = Self { major: 1, minor: 4 };
    /// PDF 1.5, which introduced cross-reference and object streams.
    pub const PDF_1_5: Self = Self { major: 1, minor: 5 };
    /// PDF 1.7, ISO 32000-1, and the writer's fallback for a document that
    /// declares no version of its own.
    pub const PDF_1_7: Self = Self { major: 1, minor: 7 };
    /// PDF 2.0, ISO 32000-2.
    pub const PDF_2_0: Self = Self { major: 2, minor: 0 };

    /// A version from its two digits.
    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for PdfVersion {
    /// `1.7`, the way the header spells it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::PdfVersion;

    #[test]
    fn display_spells_the_header() {
        assert_eq!(PdfVersion::PDF_1_0.to_string(), "1.0");
        assert_eq!(PdfVersion::PDF_1_4.to_string(), "1.4");
        assert_eq!(PdfVersion::PDF_1_7.to_string(), "1.7");
        assert_eq!(PdfVersion::PDF_2_0.to_string(), "2.0");
    }

    // Ordering is on the pair, so a major bump orders above every minor of the
    // version below it. This is the property a packed `u8` carries only while
    // the major digit stays single, and the reason `Ord` is derived on the
    // struct rather than delegated to the packed form.
    #[test]
    fn ordering_is_major_then_minor() {
        assert!(PdfVersion::PDF_1_0 < PdfVersion::PDF_1_4);
        assert!(PdfVersion::PDF_1_4 < PdfVersion::PDF_1_5);
        assert!(PdfVersion::PDF_1_5 < PdfVersion::PDF_1_7);
        assert!(PdfVersion::PDF_1_7 < PdfVersion::PDF_2_0);
        assert!(PdfVersion::new(1, 9) < PdfVersion::new(2, 0));
        assert!(PdfVersion::new(1, 9) < PdfVersion::new(11, 0));
    }

    #[test]
    fn the_named_constants_are_their_digits() {
        assert_eq!(PdfVersion::PDF_1_0, PdfVersion::new(1, 0));
        assert_eq!(PdfVersion::PDF_1_4, PdfVersion::new(1, 4));
        assert_eq!(PdfVersion::PDF_1_5, PdfVersion::new(1, 5));
        assert_eq!(PdfVersion::PDF_1_7, PdfVersion::new(1, 7));
        assert_eq!(PdfVersion::PDF_2_0, PdfVersion::new(2, 0));
    }

    // Every field is public and `Copy`, so struct-update construction works
    // the way asks configuration to (this is not a config struct,
    // but the same freedom applies and a reader will try it).
    #[test]
    fn a_minor_can_be_bumped_by_struct_update() {
        let v = PdfVersion {
            minor: 5,
            ..PdfVersion::PDF_1_7
        };
        assert_eq!(v, PdfVersion::PDF_1_5);
    }
}
