# pdfrum-parser

The file layer (ISO 32000-1 §7.5). A PDF is `%PDF-m.n`, a body of objects, a
cross-reference table or stream at the end, and a trailer pointing at both;
an incremental update appends another body, xref and trailer in front of the
last. [`load`] finds that chain, or gives up on it and rebuilds the table by
scanning the whole file for `N G obj`.

```rust
use std::sync::Arc;
use pdfrum_parser::{LoadOptions, load};

let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/files/minimal.pdf"))?;
let doc = load(Arc::from(bytes), &LoadOptions::default())?;
assert_eq!(doc.page_count(), 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

**A malformed file PDFium would open is not an error here.** That is the
whole design constraint: real PDFs have wrong `/Length` values, xref offsets
off by a few bytes, tables that point at nothing, and generations that do not
match. Every one of those is repaired and recorded in
the document's diagnostics, and `Err` is reserved for a file that cannot be
opened at all. A caller that wants strictness reads the diagnostics; a caller
that wants the document gets the document.

Loading is lazy. [`load`] reads the header, the xref chain and the trailer,
and leaves object bodies as byte offsets until something asks for them — so
opening a thousand-page file to read its page count does not parse a thousand
pages, and the object store is `Arc`-backed shared bytes rather than a parsed
tree in memory.

The stages are separable on purpose and each is testable on values alone: the
lexer turns bytes into tokens and owns the byte classifier everything else
asks questions of, syntax turns tokens into `Object`s and is where stream
`/Length` repair lives, and xref finds cross-reference information by every
route a file can offer before rebuilding from scratch.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
