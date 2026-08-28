//! Editing and saving (ISO 32000 §7.5.8 incremental updates and full
//! rewrite): the deterministic object serializer (`ryu` shortest floats),
//! full and incremental save, page import/reorganization, font subsetting,
//! and content-stream generation (SPEC.md §11).

#![forbid(unsafe_code)]
