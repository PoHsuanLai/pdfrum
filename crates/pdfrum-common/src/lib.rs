//! Shared foundation of the pdfrum workspace: the diagnostics channel for
//! damage-tolerant parsing, hard resource limits mirroring PDFium's, the
//! version a file declares ([`PdfVersion`]), and a re-export of [`kurbo`] as
//! the workspace-wide geometry vocabulary (`Affine`, `BezPath`, `Rect`,
//! `Point`). Deliberately tiny — anything that
//! feels like a "util" belongs in the crate that uses it (SPEC.md §1).
//!
//! `PdfVersion` is here rather than in the crate that produces it because more
//! than one crate does: `pdfrum-parser` reads it out of the header and
//! `pdfrum-edit` writes one back, and neither should depend on the other for a
//! two-digit value type. This is the bottom of the graph, so it is the only
//! place both can name (`docs/design/idiomatic-api.md` §WP1).
//!
//! ```
//! use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
//!
//! let limits = Limits::default();
//! assert_eq!(limits.max_object_nesting, 64);
//!
//! let mut diags = Diagnostics::default();
//! diags.record(Severity::Recovered, DiagKind::XrefRebuilt, Some(1234));
//! assert_eq!(diags.len(), 1);
//! ```

#![forbid(unsafe_code)]

mod diagnostics;
mod fasthash;
mod limits;
mod version;

pub use diagnostics::{DiagKind, Diagnostic, Diagnostics, Severity};
pub use fasthash::{FxBuildHasher, FxHasher};
pub use kurbo;
pub use limits::Limits;
pub use version::PdfVersion;

#[cfg(test)]
mod tests {
    #[test]
    fn kurbo_reexport_is_wired() {
        let r = crate::kurbo::Rect::new(0.0, 0.0, 2.0, 3.0);
        assert!((r.area() - 6.0).abs() < f64::EPSILON);
    }
}
