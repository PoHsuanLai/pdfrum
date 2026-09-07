# pdfrum-font

Font dictionaries and the glyph pipeline (ISO 32000-1 §9): Type 1, TrueType,
Type 0 composite, Type 3 procedure and CID fonts; simple and `/Differences`
encodings, CMaps, `/ToUnicode`, the base-14 substitutes, and the widths
ladder.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::{FontCache, load};
use pdfrum_object::NoResolve;

# fn demo(dict: &pdfrum_object::Dict) {
let Some(font) = load(
    dict,
    &NoResolve,
    &FontCache::default(),
    &Limits::default(),
    &mut Diagnostics::default(),
) else {
    return;
};
for item in font.decode(b"Hello") {
    let _ = (item.code, item.gid, item.width, &item.unicode);
}
# }
```

**Which glyph a byte selects, what character it stands for, and how wide it
is are three different ladders**, and a font can answer any one of them
without the others. The glyph comes from the encoding and the font program;
the character comes from `/ToUnicode`, then the encoding's glyph names, then a
CID-to-Unicode table; the width comes from `/Widths` or `/W`, then the
program's own metrics, then a default. A file routinely disagrees with itself
across the three — text that draws correctly but copies as mojibake is
exactly that disagreement. [`Font::decode`] answers all three at once, per
code, so a caller never has to re-derive one from another.

A simple font always constructs. Only `Type0` can fail to [`load`], because
only it depends on a CMap that may be unresolvable; everything else falls
back — a missing program becomes a base-14 substitute, a missing width
becomes the default — because a page with an unloadable font still has to
draw. [`FontCache`] is shared across pages, since a document's fonts are
per-document and re-parsing an embedded program per page is the difference
between a fast and a slow render.

Feature `system-fonts` adds host font fallback via
[`fontdb`](https://crates.io/crates/fontdb); it is not available on `wasm32`,
where the bundled base-14 faces carry the text.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
