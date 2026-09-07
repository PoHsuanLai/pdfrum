# pdfrum-edit

The crate that writes (ISO 32000-1 §7.5.8). A save either rewrites the whole
file or appends a new body, cross-reference section and trailer to the bytes
already there. Page import and reordering, N-up imposition, font subsetting,
image embedding and re-encryption live here.

```rust
use std::sync::Arc;
use pdfrum_edit::{EditDoc, SaveOptions, save};
use pdfrum_parser::{LoadOptions, load};

let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/files/hello.pdf"))?;
let doc = load(Arc::from(bytes), &LoadOptions::default())?;
let mut out = Vec::new();
save(&EditDoc::new(&doc), &SaveOptions::default(), &mut out)?;
assert!(out.starts_with(b"%PDF"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

**An untouched page is copied as stored; a touched one is regenerated, and
regeneration is lossy.** This is the single fact to know before editing. The
content of a page you did not modify is written back byte for byte, so a save
is not a re-encode of the document. A page whose graph you *did* change has
its content stream rebuilt from that graph, and the rebuild does not preserve
everything the original said: colour comes back as `rg`/`RG`, and shadings,
text clipping modes and Type 3 glyph runs are dropped. Edit the pages you
mean to change and nothing else.

[`SaveMode`] chooses between the two file shapes. An incremental save appends
and leaves the original bytes intact, which is what keeps an existing digital
signature verifiable and what a reader expects of a form fill; a full save
rewrites and can drop what nothing references any more. An unencrypted save
is byte-reproducible under [`IdSource::Fixed`]. An encrypted one draws its
file key and AES vectors from the operating system: reproducible ciphertext
is reproducible secrets, which is not what a seed is for.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
