//! What pdfrum costs, and the machinery that keeps it from getting worse.
//!
//! This crate is the M12 performance program's harness. It publishes nothing —
//! `publish = false`, and no library crate depends on it — and exists so that
//! four things describe the *same* work on the *same* files:
//!
//! - `benches/engine.rs`, the criterion suite (open, render across three
//!   backends, text extraction, save);
//! - `src/bin/profile.rs`, which runs one operation in a loop with nothing
//!   else in the process, for `scripts/profile.sh` to record;
//! - `src/bin/ratchet.rs`, which compares a criterion run against the
//!   committed `baseline.json` and fails on a regression outside the noise
//!   band;
//! - `scripts/bench-oracle.sh`, which times `pdfium_test` over the same files.
//!
//! [`corpus`] is what makes that true: one list of documents with one class
//! each, so a table row in `docs/status/M12.md` means the same thing in every
//! column.

#![forbid(unsafe_code)]

pub mod corpus;
