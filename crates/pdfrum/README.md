# pdfrum

**A pure-Rust PDF engine: parse, render, extract text, edit.**

```toml
pdfrum = "0.1"
```

```rust
use pdfrum::{Document, RenderOptions, VelloCpuBackend};

let doc = Document::open(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hello_world.pdf"
))?;
for page in doc.pages() {
    let pixmap = page.render(&VelloCpuBackend::new(), &RenderOptions::scaled(2.0))?;
    let text = page.text().to_string();
    assert_eq!((pixmap.width(), pixmap.height()), (400, 400));
    assert!(text.contains("Hello, world!"));
}
# Ok::<(), pdfrum::Error>(())
```

A rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/), not a binding.
No C/C++ in the library build; `unsafe` is forbidden. No XFA, no viewer.
JavaScript is off by default (`javascript` feature, [boa](https://boajs.dev/)).

This crate is the facade — [`Document`], [`Page`], [`TextPage`], [`Form`].
The rest of the workspace is public; [`Document::parser`], [`Page::objects`],
[`PageEdit::graph`] and [`Annotation::dict`] are the escape hatches. Damage
is reported on [`Document::diagnostics`], not as a failed open. Every public
type is `Send + Sync`.

## Features

| feature | default | adds |
|---|:---:|---|
| `vello-cpu` | on | default rasterizer |
| `tinyskia`, `agg` | off | extra CPU rasterizers |
| `vello-gpu` | off | GPU rasterizer over `wgpu` |
| `edit` | on | save, edit, subsetting |
| `forms` | on | form reader and session |
| `javascript` | off | run the document's own scripts |
| `codecs-all` | on | JPX, JBIG2, CCITT |
| `system-fonts` | on | host font fallback (not on wasm32) |
| `markdown` | off | `Page::markdown` |
| `svg` | off | SVG export |
| `svg-ingest` | off | draw an SVG into a page |
| `png` | off | `Pixmap::encode_png` |

`default-features = false` is a parser and extractor. A render always names
its backend.

## License

MIT OR Apache-2.0. Crate map and conformance numbers:
[github.com/PoHsuanLai/pdfrum](https://github.com/PoHsuanLai/pdfrum).
