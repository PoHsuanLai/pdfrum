# pdfrum-text

**Text extraction: reading order, search, link detection.**

Turns an interpreted PDF page into the characters a reader can select, search
and copy — reading order, generated spaces, line breaks and all — without ever
rendering anything.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_text::{ExtractOptions, extract};

fn demo(page: &pdfrum_page::Page, resolver: &impl pdfrum_object::Resolve) {
    let mut diags = Diagnostics::default();
    let text = extract(page, resolver, &ExtractOptions::default(), &Limits::default(), &mut diags);
    println!("{text}");
}
```

**There are two texts, and they are not the same sequence.** `TextPage` carries
both, deliberately as different types, because conflating them is the single
easiest way to get text extraction wrong. The *character stream* holds one
entry per character the page draws or the extractor invents, geometry attached,
including control characters, a `\0` for an unmappable code, and a sentinel
where a word was hyphenated across a line — this is what a raw text dump
emits, unfiltered, in order. The *text* is what a search matches and a
selection copies: control characters and placeholders dropped, ligatures
expanded. The two index spaces are `CharIndex` and `TextIndex`, distinct types
in every signature that names one, and `IndexMap` converts between them.

Reading order comes from sorting text objects by their transformed x within a
batch and flushing the batch when the baseline jumps. Spaces are generated from
geometry, never read from the content stream. Right-to-left runs are reversed
into logical order and brackets in them mirrored; `/ActualText` marks replace
the glyphs they cover; web and mail addresses are recognized in the result.

## Part of pdfrum

`pdfrum-text` is the text layer of [pdfrum](https://crates.io/crates/pdfrum), a
pure-Rust PDF engine. For a batteries-included API — open a file and read a
page's text in two calls — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
