# pdfrum

**A pure-Rust PDF engine: parse, render, extract text, edit.**

```rust
use pdfrum::{Document, RenderOptions};

let doc = Document::open("report.pdf")?;
for page in doc.pages() {
    let pixmap = page.render(&RenderOptions::scaled(2.0))?;
    let text = page.text().to_string();
    println!("page {}: {}x{}, {} chars",
        page.index(), pixmap.width(), pixmap.height(), text.len());
}
```

`pdfrum` is a rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/) —
the PDF engine inside Chrome — as idiomatic Rust. Not a binding and not a
transliteration: a rewrite, with PDFium kept alongside as a differential test
oracle so the behaviour that matters survives even where the code shares
nothing. Reimplementing PDF from the specification gets you a reader for files
that are *correct*; porting the recovery folklore gets you one for files that
*exist*.

**Pure Rust, all the way down.** No C or C++ is compiled into any library build
and no `-sys` crate appears anywhere in the dependency tree, checked
mechanically in CI. `unsafe` is forbidden in every crate.

Deliberately absent, permanently: XFA and any viewer behaviour. This turns
pages into pixels and text into strings.

**JavaScript is off by default**, which is a different claim: with default
features a document's scripts are read as data and never run —
`scripts/check-no-boa.nu` asserts no engine is in the tree — and the `script`
feature turns them on, behind a pure-Rust engine (boa). See the crate docs'
`# Features` section for what a script reaches today, which is not yet
Acrobat.

## The facade

This crate is composition only — `Document`, `Page`, `TextPage`, `Form`, render
and save options, and no logic of its own. Everything under it is a public
library in its own right and none of it is hidden, so reach past `pdfrum`
whenever you need to: `pdfrum-parser` for damaged-file recovery,
`pdfrum-filters` for stream codecs, `pdfrum-type1` for Type 1 fonts,
`pdfrum-crypt` for the standard security handler, and a dozen more.

Damage is a *channel*, not a failure: opening a file whose cross-reference table
had to be rebuilt succeeds and says so through `Document::diagnostics`. Every
public type is `Send + Sync`, so rendering pages in parallel needs nothing
beyond adding `rayon` to your own manifest.

Full documentation, the crate map, the conformance scoreboard and the project
documents are in the [workspace README](https://github.com/pdfrum/pdfrum).

## License

MIT OR Apache-2.0, at your option.
