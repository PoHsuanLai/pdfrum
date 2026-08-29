# pdfrum-edit

**PDF serializer: full/incremental save, page import, subsetting.**

Writing PDF back out, per ISO 32000 §7.5.8: a deterministic object serializer,
full rewrite and incremental append, cross-document page import with reference
remapping, N-up imposition, content-stream generation from a page-object graph,
and CID font subsetting.

This is the crate that *writes*. Everything beneath it derives values from
bytes; this turns values back into bytes another reader must be able to open.

```rust
use std::sync::Arc;
use pdfrum_edit::{EditDoc, SaveOptions, save};
use pdfrum_parser::{LoadOptions, load};

let doc = load(bytes, &LoadOptions::default())?;
let edit = EditDoc::new(&doc);

let mut out = Vec::new();
save(&edit, &SaveOptions::default(), &mut out)?;
assert!(out.starts_with(b"%PDF-1.7\r\n"));

// What we wrote, another reader opens.
let reloaded = load(Arc::from(&out[..]), &LoadOptions::default())?;
assert_eq!(reloaded.page_count(), doc.page_count());
```

**Determinism is a parameter.** Conventional PDF writers draw the second half
of `/ID` and the six-letter font-subset tag from a process-global random
source, which makes their output unreproducible. `IdSource` makes that an
argument: `Random` by default, matching the observable behaviour, and `Fixed`
when the same input must produce the same bytes.

Regenerating a page's content is lossy and deliberately so, reproducing a
specific emitter's output: only `rg`/`RG` colours survive, patterns and
shadings do not, and text keeps only `Tm`, `Tf`, `Tr` and `TJ`. A page whose
objects were never touched is not regenerated at all, so none of that applies to
an ordinary save.

## Part of pdfrum

`pdfrum-edit` is the writing layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — `Document::save` and page copying without assembling
the layers — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
