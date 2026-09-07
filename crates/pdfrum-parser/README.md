# pdfrum-parser

Lexer, xref (tables, streams, hybrids, rebuild), object streams, encryption,
lazy `Resolve` store.

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
