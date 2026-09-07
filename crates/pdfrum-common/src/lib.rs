#![doc = include_str!("../README.md")]
//! Shared foundation of the pdfrum workspace: the diagnostics channel for
//! damage-tolerant parsing, hard resource limits mirroring PDFium's (with the
//! [`Deadline`] a host may add and the [`LimitExceeded`] a host's own ceiling
//! answers with), the
//! two vocabulary newtypes every layer speaks in ([`PdfVersion`],
//! [`PageIndex`]), and a re-export of [`kurbo`] as the workspace-wide geometry
//! vocabulary (`Affine`, `BezPath`, `Rect`, `Point`). Deliberately tiny — anything that
//! feels like a "util" belongs in the crate that uses it.
//!
//! Both newtypes are here rather than in the crate that produces them, because
//! more than one crate does — and so is [`hex_digit`], the one hexadecimal
//! digit reader five grammars share. `PdfVersion` is read out of the header by
//! `pdfrum-parser` and written back by `pdfrum-edit`, neither of which should
//! depend on the other for a two-digit value type; `PageIndex` appears in the
//! public signatures of six crates. This is the bottom of the graph, so it is
//! the only place all of them can name.
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

mod deadline;
mod diagnostics;
mod fasthash;
mod hex;
mod limits;
mod page_index;
mod version;

pub use deadline::{Deadline, Operation};
pub use diagnostics::{DiagKind, Diagnostic, Diagnostics, Severity};
pub use fasthash::{FxBuildHasher, FxHasher};
pub use hex::hex_digit;
pub use kurbo;
pub use limits::{LimitExceeded, Limits};
pub use page_index::PageIndex;
pub use version::PdfVersion;

#[cfg(test)]
mod tests {
    #[test]
    fn kurbo_reexport_is_wired() {
        let r = crate::kurbo::Rect::new(0.0, 0.0, 2.0, 3.0);
        assert!((r.area() - 6.0).abs() < f64::EPSILON);
    }
}
