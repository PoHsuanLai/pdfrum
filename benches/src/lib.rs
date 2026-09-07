//! Corpus, ratchet, profile, scaling. Criterion groups live on the crate
//! they measure — `cargo bench -p pdfrum-render` has a meaning.
//!
//! [`corpus`] is the same 44 files everywhere.

#![forbid(unsafe_code)]

/// Shared corpus list (`pdfrum-corpus`). A leaf so library crates can
/// share it without depending on the facade.
pub use pdfrum_corpus as corpus;
