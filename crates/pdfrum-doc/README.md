# pdfrum-doc

Bookmarks, dests, links, annotations, AcroForm, structure tree, metadata.
No JavaScript.

Appearance generation returns an `AnnotOverlay` — it does not mutate the
store. Readers take `Option<&AnnotOverlay>`.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
