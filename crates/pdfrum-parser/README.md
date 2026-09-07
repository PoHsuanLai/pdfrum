# pdfrum-parser

A PDF file is `%PDF-m.n`, a body of objects, a cross-reference table (or
stream) at the end, and a trailer that points at both (ISO 32000-1 §7.5).
Incremental updates append another body+xref+trailer. `load` recovers that
table, or rebuilds it by scanning for `N G obj`.

```rust
use std::sync::Arc;
use pdfrum_parser::{LoadOptions, load};

let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/files/minimal.pdf"))?;
let doc = load(Arc::from(bytes), &LoadOptions::default())?;
assert_eq!(doc.page_count(), 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

A malformed file PDFium would open is not an error here. Repairs go to
diagnostics; `Err` means it cannot be opened at all.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
