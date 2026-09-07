# pdfrum-edit

Serializer: full rewrite, incremental append, page import, N-up, subsetting.

```rust
use pdfrum_edit::{EditDoc, SaveOptions, save};

let mut out = Vec::new();
save(&EditDoc::new(&doc), &SaveOptions::default(), &mut out)?;
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
