//! The fidelity-critical file parser (ISO 32000 §7.5): zero-copy lexer,
//! object syntax, xref reading (classic tables, xref streams, hybrids,
//! prev-chains) with PDFium's full-file recovery rebuild, object streams,
//! incremental updates, encryption hookup, and the lazy object store behind
//! `Resolve`. Broken-file tolerance is this crate's superpower (SPEC.md §5).

#![forbid(unsafe_code)]
