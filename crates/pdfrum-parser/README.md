# pdfrum-parser

**PDF file parser: lexer, xref, object store, damage recovery.**

The file layer of ISO 32000 §7.5: a zero-copy lexer, object syntax,
cross-reference reading by every route a file can offer — classic tables, xref
streams, hybrids, prev-chains — object streams, incremental updates, encryption
hookup, and a lazy object store behind the `Resolve` trait.

```rust
use std::sync::Arc;
use pdfrum_parser::{LoadOptions, load};

let bytes: Arc<[u8]> = Arc::from(&include_bytes!("minimal.pdf")[..]);
let doc = load(bytes, &LoadOptions::default())?;
assert_eq!(doc.page_count(), 1);
```

Each stage is a layer testable on values alone: `lexer` turns bytes into tokens
and owns the byte classifier every other stage asks questions of; `syntax`
turns tokens into objects, and is where stream `/Length` repair lives; `xref`
finds and reads cross-reference information, and rebuilds it from scratch by
scanning the whole file when no route works; `doc` ties those into a document
whose object store fetches lazily, decrypts, and guards against reference
cycles.

**Damage is normal, not an error.** Nothing here refuses a malformed file that
PDFium would open. A repair is recorded in the document's diagnostics and the
read continues; `Err` is reserved for a document that cannot be opened at all.
Broken-file tolerance is this crate's reason to exist — the recovery behaviour
is ported from two decades of a browser opening whatever the web threw at it,
not derived from the specification.

## Part of pdfrum

`pdfrum-parser` is the file layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — rendering, text extraction, forms, saving — use the
`pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
