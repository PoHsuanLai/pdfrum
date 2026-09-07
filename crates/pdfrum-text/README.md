# pdfrum-text

Extraction, reading order, search, selection, links. Does not render.

```rust
use pdfrum_text::{FindOptions, TextIndex, TextPage};

let page = TextPage {
    search_text: "Hello, world!".chars().collect(),
    ..TextPage::default()
};
let hit = page.find("world", FindOptions::default()).next().expect("a match");
assert_eq!(hit, TextIndex::new(7)..TextIndex::new(12));
```

Two index spaces: `CharIndex` (what the page draws, including controls) and
`TextIndex` (what search and copy see). `IndexMap` converts.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
