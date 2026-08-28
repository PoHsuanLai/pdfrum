//! Type 1 font parsing — the one outline format Fontations does not read:
//! PFA/PFB containers, eexec/charstring decryption, and a Type 1 charstring
//! interpreter producing glyph outlines for `pdfrum-font` (SPEC.md §6).

#![forbid(unsafe_code)]
