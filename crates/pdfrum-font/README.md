# pdfrum-font

A font dictionary names a base font, an encoding, and usually a program.
Which glyph a byte selects, what character it stands for, and how wide it
is are three different ladders (ISO 32000-1 §9). `Font::decode` answers all
three.

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

A simple font always constructs. Only Type0 can fail to load.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
