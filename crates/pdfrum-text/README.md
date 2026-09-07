# pdfrum-text

Extraction, reading order, search, selection, links. Does not render.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_text::{ExtractOptions, extract};

let text = extract(page, resolver, &ExtractOptions::default(), &Limits::default(), &mut Diagnostics::default());
```

Two index spaces: `CharIndex` (what the page draws, including controls) and
`TextIndex` (what search and copy see). `IndexMap` converts.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
