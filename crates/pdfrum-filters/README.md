# pdfrum-filters

Stream `/Filter` values (ISO 32000-1 §7.4) as functions over bytes: Flate
with its PNG and TIFF predictors, LZW, RunLength, ASCIIHex, ASCII85 and
CCITT Group 3/4. DCT, JPX and JBIG2 are *classified* here and decoded in
`pdfrum-page`, because they produce an image rather than bytes.

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
# Ok::<(), Box<dyn std::error::Error>>(())
```

**A broken stream is `Ok` plus a diagnostic.** Damage tolerance is the point:
in the files this crate exists to open, a truncated Flate stream or an LZW
code table that runs off the end is normal, and every byte decoded before the
break is still worth having. The `Err` cases are only three — a size
computation that overflows, output past `Limits::max_decoded_stream_len`, and
the two malformations PDFium itself rejects outright.

A `/Filter` is a chain, and [`decode_chain`] walks it left to right, handing
each filter the previous one's output. It stops at the first filter this crate
does not decode and returns [`DecodeOutput::Image`] with the bytes produced so
far — which is how `/Filter [/ASCII85Decode /DCTDecode]` arrives at the image
decoder as JPEG rather than as ASCII85.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
