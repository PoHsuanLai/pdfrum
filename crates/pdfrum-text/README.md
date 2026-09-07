# pdfrum-text

Text extraction (ISO 32000-1 §14.8.2): reading order, search, selection
geometry, word segmentation and bare-URL detection. It walks the same
interpreted page-object graph the renderer walks and never rasterizes, so
extracting text from a page costs no pixels and needs no backend.

```rust
use pdfrum_text::{FindOptions, TextIndex, TextPage};

let page = TextPage {
    search_text: "Hello, world!".chars().collect(),
    ..TextPage::default()
};
let hit = page.find("world", FindOptions::default()).next().expect("a match");
assert_eq!(hit, TextIndex::new(7)..TextIndex::new(12));
```

**Two index spaces, and mixing them is the bug this crate is shaped to
prevent.** [`CharIndex`] addresses what the page *draws* — every glyph in
drawing order, including generated spaces, hyphens at line breaks and control
characters that have geometry but no meaning. [`TextIndex`] addresses what
search and copy *see*, with those removed. A hit from [`TextPage::find`] is a
`TextIndex` range; [`TextPage::rects`] and [`TextPage::char`] want a
`CharIndex`; [`IndexMap`] is the conversion, and the two are distinct types so
the compiler refuses the mistake rather than returning a highlight in the
wrong place.

Geometry is per character, not per line. [`CharBox`] carries each glyph's
quadrilateral in page space, which is what makes [`TextPage::rects`] able to
return a selection that follows rotated or skewed text,
[`TextPage::index_at`] able to answer a click, and [`TextPage::text_in_rect`]
able to lift a column out of a two-column page.

Reading order is reconstructed, not read off the file: a content stream may
draw a page's text in any order at all, and [`extract`] sorts runs into the
order a human reads them, right-to-left when
[`ExtractOptions::rtl`] says so.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
