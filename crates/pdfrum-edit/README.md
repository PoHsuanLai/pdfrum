# pdfrum-edit

Serializer: full rewrite, incremental append, page import, N-up, subsetting.

```rust
use std::sync::Arc;
use pdfrum_edit::{EditDoc, SaveOptions, save};
use pdfrum_parser::{LoadOptions, load};

let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/files/hello.pdf"))?;
let doc = load(Arc::from(bytes), &LoadOptions::default())?;
let mut out = Vec::new();
save(&EditDoc::new(&doc), &SaveOptions::default(), &mut out)?;
assert!(out.starts_with(b"%PDF"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
