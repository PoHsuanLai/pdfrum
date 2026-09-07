//! PDF/A conformance checking (ISO 19005): what a document fails, and why.
//!
//! # Report before repair
//!
//! This module only *reports*. Nothing here rewrites a document, and
//! conversion is a later pass deliberately: a
//! checker that has been cross-examined against an independent implementation
//! is a result, and one that has not is a claim. The oracle is veraPDF, and
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
mod report;
mod xmp;

pub use check::check;
pub use report::{Clause, Level, Report, Subject, Violation};
