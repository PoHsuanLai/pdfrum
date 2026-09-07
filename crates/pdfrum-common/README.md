# pdfrum-common

`Diagnostics` is the damage channel; `Limits` are the ceilings. `PdfVersion`
is the `%PDF-m.n` header; `PageIndex` is zero-based. Geometry is
[`kurbo`](https://crates.io/crates/kurbo).

```rust
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};

let mut diags = Diagnostics::default();
diags.record(Severity::Recovered, DiagKind::XrefRebuilt, Some(1234));
assert_eq!(Limits::default().max_object_nesting, 64);
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
