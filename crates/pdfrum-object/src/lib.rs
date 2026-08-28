//! The PDF object model as plain values (ISO 32000 §7.3): the `Object` enum
//! (Null/Bool/Int/Real/String/Name/Array/Dict/Stream/Ref), dictionary-key
//! name constants, and the `Resolve` trait for indirect-reference lookup.
//! Construction and typed access only — no parsing lives here (SPEC.md §2).

#![forbid(unsafe_code)]
