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

A rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/), not a binding.
No C/C++ in the library build; `unsafe` is forbidden. No XFA, no viewer.
JavaScript is off by default.

This crate is the facade — `Document`, `Page`, `TextPage`, `Form`. The rest
of the workspace is public; `Document::parser`, `Page::objects`,
`PageEdit::graph` and `Annotation::dict` are the escape hatches. Damage is
reported on `Document::diagnostics`, not as a failed open. Every public type
is `Send + Sync`.

Workspace README, crate map, and conformance numbers:
[github.com/PoHsuanLai/pdfrum](https://github.com/PoHsuanLai/pdfrum).

## License

MIT OR Apache-2.0
