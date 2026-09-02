//! What pdfrum costs, and the machinery that keeps it from getting worse.
//!
//! This crate is the M12 performance program's *certificate suite* — the part
//! of the harness that is about the corpus as a whole rather than about any one
//! crate. It publishes nothing (`publish = false`, and no library crate depends
//! on it) and holds:
//!
//! - `baseline.json`, the committed medians and their per-group noise bands;
//! - `src/bin/ratchet.rs`, which compares a criterion run against that file and
//!   fails on a regression outside the band;
//! - `src/bin/profile.rs`, which runs one operation in a loop with nothing else
//!   in the process, for `scripts/profile.nu` to record;
//! - `src/bin/scaling.rs`, one rayon thread count per process, for
//!   `scripts/bench-scaling.nu`;
//! - `corpus/`, the 44 documents themselves.
//!
//! The criterion groups no longer live here. Since M12's per-crate split each
//! belongs to the crate whose code it measures — `pdfrum-parser` owns `open`,
//! `pdfrum-page` owns `build`, `pdfrum-render` owns the six render groups,
//! `pdfrum-text` owns `text`, `pdfrum-edit` owns `save` — so that
//! `cargo bench -p pdfrum-render` is a command with a meaning. The two binaries
//! above stay because they need the facade and the engine/rasterizer seam in
//! one process, which is not any single crate's business.
//!
//! [`corpus`] is what keeps all of that describing the same work: one list of
//! documents with one class each, so a table row in `docs/status/M12.md` means
//! the same thing in every column.

#![forbid(unsafe_code)]

/// The shared corpus list.
///
/// Re-exported from the `pdfrum-corpus` leaf crate, where it moved in the M12
/// per-crate bench split so that five library crates could dev-depend on the
/// document list without any of them depending on the facade that depends on
/// them. The path `pdfrum_bench::corpus` is kept because the three binaries
/// here and `scripts/` both spell it that way.
pub use pdfrum_corpus as corpus;
