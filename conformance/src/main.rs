//! Conformance harness (PLAN.md §5): runs `pdfrum-tool` over the corpus and
//! resource PDFs, compares against the golden store in `goldens/` (Tier A
//! byte-exact, Tier B perceptual, Tier C cross-backend), and emits
//! `scoreboard.json` — the fitness function every burn-down loop optimizes.

#![forbid(unsafe_code)]

fn main() {
    eprintln!("conformance: M0 stub - harness v1 not implemented yet");
}
