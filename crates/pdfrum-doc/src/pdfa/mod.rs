//! PDF/A conformance checking (ISO 19005): what a document fails, and why.
//!
//! # Reporting and repair policy
//!
//! [`check`] reports what a document fails; it rewrites nothing. [`Policy`]
//! and its outcome types describe what a caller will accept in exchange for
//! conformance, and [`packet`](xmp_packet) serializes the XMP a converted
//! file carries. Applying a repair belongs to the writer.
//!
//! The checker is cross-examined against veraPDF:
//! `crates/pdfrum/tests/pdfa_oracle.rs` scores every check against it exactly
//! as the conformance board scores rendering against `pdfium_test`.
//!
//! # What a failure is
//!
//! A [`Violation`] names three things: the [`Clause`] of ISO 19005 it breaks,
//! the [`Subject`] in the file that breaks it, and a short human sentence.
//! The clause is an enum rather than a formatted string because the whole
//! point of a report is that a caller can act on it — count by clause, filter
//! to the ones a converter can repair, or map to another vocabulary. A report
//! that can only be printed is worth much less than one that can be matched.
//!
//! # Scope
//!
//! [`Level::A1b`] and [`Level::A2b`], the two *basic* conformance levels. The
//! `a` levels (A-1a, A-2a, A-3a) additionally require a tagged logical
//! structure tree with a defined reading order, which is a different kind of
//! claim resting on a structure model this engine does not yet have; they are
//! absent by decision, not by oversight, and [`Level`] has no variant for
//! them so a caller cannot ask for a check that would silently be weaker than
//! its name.

mod check;
mod policy;
mod report;
mod xmp;
mod xmp_write;

pub use check::check;
pub use policy::{Compromise, Concession, Conversion, Dpi, Policy, RasterCause, Refusal};
pub use report::{Clause, Level, Report, Subject, Violation};
pub use xmp_write::{InfoFields, packet as xmp_packet};
