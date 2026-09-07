# pdfrum-render

`RenderDevice` / `RasterBackend` seam, page walk, compositor, resampling.

```rust
use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_render::{RenderOptions, render_page};

let pixmap = render_page(&Page::empty(), &RenderOptions::default(), &backend, &mut Diagnostics::default())?;
assert_eq!((pixmap.width(), pixmap.height()), (612, 792));
```

The engine does not know which rasterizer it has. Undrawable objects are
skipped with a diagnostic.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
