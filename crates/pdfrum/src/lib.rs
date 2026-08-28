//! The idiomatic user-facing API: `Document`, `Page`, `TextPage`, `Form`,
//! and render options, composed from the pdfrum-* crates. This crate contains
//! no logic of its own — only composition and ergonomics; rendering pages in
//! parallel with rayon Just Works (SPEC.md §13).

#![forbid(unsafe_code)]
