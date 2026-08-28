//! Text extraction (behavioral twin of the oracle's `fpdftext`): character
//! boxes with unicode/bbox/origin, reading-order and whitespace-insertion
//! heuristics, line breaks, rotated text, search, and web-link detection.
//! Depends only on parser/font/page — never on rendering (SPEC.md §9).

#![forbid(unsafe_code)]
