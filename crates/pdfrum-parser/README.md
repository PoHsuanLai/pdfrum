# pdfrum-parser

Lexer, xref (tables, streams, hybrids, rebuild), object streams, encryption,
lazy `Resolve` store.

```rust
use std::sync::Arc;
use pdfrum_parser::{LoadOptions, load};

let doc = load(Arc::from(&include_bytes!("minimal.pdf")[..]), &LoadOptions::default())?;
assert_eq!(doc.page_count(), 1);
```

A malformed file PDFium would open is not an error here. Repairs go to
diagnostics; `Err` means it cannot be opened at all.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
