# pdfrum-common

**Shared error/diagnostic types and geometry vocabulary for pdfrum.**

Three things, and deliberately nothing else:

- `Diagnostics` — a recording channel for damage-tolerant parsing. A parser
  that recovers from a malformed input records what it bent rather than
  returning an error, and the caller reads the record afterwards if it cares.
  `Severity` and `DiagKind` classify each entry; a byte offset locates it.
- `Limits` — hard resource ceilings mirroring PDFium's: object nesting depth,
  maximum decoded stream length, and the rest of the guards that keep a hostile
  file from exhausting memory.
- A re-export of [`kurbo`](https://docs.rs/kurbo) as the workspace-wide
  geometry vocabulary — `Affine`, `BezPath`, `Rect`, `Point` — so that every
  crate above speaks one set of geometric types rather than converting between
  its neighbours'.

```rust
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};

let limits = Limits::default();
assert_eq!(limits.max_object_nesting, 64);

let mut diags = Diagnostics::default();
diags.record(Severity::Recovered, DiagKind::XrefRebuilt, Some(1234));
assert_eq!(diags.len(), 1);
```

The crate is tiny on purpose. Anything that feels like a "util" belongs in the
crate that uses it, not here.

## Part of pdfrum

`pdfrum-common` is the foundation crate of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — open a file, render its pages, read its text — use the
`pdfrum` facade rather than assembling the layers yourself.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
