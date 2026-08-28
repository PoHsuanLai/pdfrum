//! Font handling (ISO 32000 §9): font dictionaries (Type1/TrueType/Type0/
//! Type3/CID), encodings and Differences, ToUnicode, code→CID→GID mapping,
//! glyph outlines and metrics via `skrifa`, substitution/fallback selection,
//! and the per-session glyph cache (SPEC.md §6).

#![forbid(unsafe_code)]
