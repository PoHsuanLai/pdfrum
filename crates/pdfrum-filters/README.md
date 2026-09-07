# pdfrum-filters

Flate (with predictors), LZW, RunLength, ASCIIHex/85, CCITT. DCT / JPX /
JBIG2 are classified here and decoded elsewhere.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, NoResolve};
use pdfrum_filters::{DecodeOutput, Filter, decode};

let out = decode(
    Filter::from_name(&Name::from("AHx")).unwrap(),
    b"48656C6C6F>",
    &Dict::new(),
    &NoResolve,
    &Limits::default(),
    &mut Diagnostics::default(),
)?;
assert!(matches!(out, DecodeOutput::Bytes(b) if b == b"Hello"));
```

A broken stream is `Ok` plus a diagnostic. `decode_chain` walks `/Filter`
left to right.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
