# pdfrum-filters

**PDF stream filters: Flate, LZW, RunLength, ASCIIHex/85, CCITT.**

The stream filters of ISO 32000-1 §7.4, as pure functions over byte slices:
Flate with PNG and TIFF predictors, LZW, RunLength, ASCIIHex, ASCII85, and
CCITT Group 3/4 fax. Image codecs (DCT, JPX, JBIG2) are recognized and
classified here but decoded elsewhere, since they need an image's dimensions.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, NoResolve};
use pdfrum_filters::{DecodeOutput, Filter, decode};

let filter = Filter::from_name(&Name::from("AHx")).expect("a filter we know");
let mut diags = Diagnostics::default();
let out = decode(
    filter,
    b"48656C6C6F>",
    &Dict::new(),
    &NoResolve,
    &Limits::default(),
    &mut diags,
)?;
assert!(matches!(out, DecodeOutput::Bytes(b) if b == b"Hello"));
```

**Damage tolerance is the point.** A broken filter stream is not an error here,
because it is not an error in the files this crate exists to open. Flate stops
at the first byte it cannot inflate and returns the prefix; LZW discards a
trailing partial code; ASCIIHex skips non-hex bytes; RunLength zero-fills a run
that overruns its input. Each records a diagnostic and returns `Ok`. `Err` is
reserved for "this cannot produce bytes at all" — overflowing size arithmetic,
output past the configured limit, or a malformation rejected outright.

A stream's `/Filter` is a chain, and `decode_chain` walks it left to right,
stopping at the first filter this crate does not decode and returning what it
produced — which is how `/Filter [/ASCII85Decode /DCTDecode]` reaches a JPEG
decoder already de-ASCII'd.

## Part of pdfrum

`pdfrum-filters` is the codec layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — open a document and get decoded streams without
touching a filter — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
