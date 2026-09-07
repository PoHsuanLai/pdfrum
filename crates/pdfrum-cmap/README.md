# pdfrum-cmap

CJK CMaps: 59 named tables (32 decoder stems) plus `parse_embedded`.

```rust
use pdfrum_cmap::from_encoding_name;
use pdfrum_common::Diagnostics;
use pdfrum_object::Name;

let cmap = from_encoding_name(&Name::from("Identity-H"), &mut Diagnostics::default());
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
