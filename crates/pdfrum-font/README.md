# pdfrum-font

Font dictionaries, encodings, ToUnicode, outlines (`skrifa`), substitution
(`fontdb`). `Font::decode` is the entry point.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::{FontCache, load};

let font = load(dict, doc, &FontCache::default(), &Limits::default(), &mut Diagnostics::default())?;
for item in font.decode(b"Hello") {
    let _ = (item.code, item.gid, item.width, &item.unicode);
}
```

A simple font always constructs. Only Type0 can fail to load.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
