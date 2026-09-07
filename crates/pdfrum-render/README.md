# pdfrum-render

Walks a page-object graph onto a `RasterBackend` (ISO 32000-1 §8.4–8.7).
An opaque page paints onto white; a transparent one onto nothing.
Undrawable objects are skipped with a diagnostic.

```rust
use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::{RenderOptions, render_page};

let pixmap = render_page(
    &Page::empty(),
    &RenderOptions::default(),
    &TinySkiaBackend::new(),
    &mut Diagnostics::default(),
)?;
assert_eq!((pixmap.width(), pixmap.height()), (612, 792));
# Ok::<(), pdfrum_render::Error>(())
```

The engine does not know which rasterizer it has. Undrawable objects are
skipped with a diagnostic.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
