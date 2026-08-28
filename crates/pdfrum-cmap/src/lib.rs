//! Character-code to CID mapping (ISO 32000 §9.7.5): compact
//! build-script-generated static tables for the predefined CJK CMaps, plus a
//! parser for embedded CMap streams (codespace ranges, `cidrange`/`cidchar`,
//! `usecmap` chaining) (SPEC.md §6).

#![forbid(unsafe_code)]
