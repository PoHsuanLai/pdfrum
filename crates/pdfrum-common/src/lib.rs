//! Shared foundation of the pdfrum workspace: the diagnostics channel for
//! damage-tolerant parsing, hard resource limits mirroring PDFium's, and a
//! re-export of [`kurbo`] as the workspace-wide geometry vocabulary
//! (`Affine`, `BezPath`, `Rect`, `Point`). Deliberately tiny — anything that
//! feels like a "util" belongs in the crate that uses it (SPEC.md §1).

#![forbid(unsafe_code)]

pub use kurbo;

#[cfg(test)]
mod tests {
    #[test]
    fn kurbo_reexport_is_wired() {
        let r = crate::kurbo::Rect::new(0.0, 0.0, 2.0, 3.0);
        assert!((r.area() - 6.0).abs() < f64::EPSILON);
    }
}
