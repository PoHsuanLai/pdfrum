#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

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
