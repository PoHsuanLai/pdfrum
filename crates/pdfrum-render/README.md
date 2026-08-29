# pdfrum-render

**Rendering engine and the RenderDevice/RasterBackend seam.**

The realization half of ISO 32000 §8.4–8.7: the `RenderDevice` and
`RasterBackend` traits — the only seam between engine and rasterizer, spoken in
`kurbo` and `peniko` vocabulary — plus the page-graph walker, the layer
compositor for isolated groups and soft masks, and image resampling.

`render_page` takes an interpreted page and any `RasterBackend`, and returns a
premultiplied RGBA8 `Pixmap`:

```rust
use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_render::{RenderOptions, render_page};

let page = Page::empty();
let mut diags = Diagnostics::default();
let pixmap = render_page(&page, &RenderOptions::default(), &backend, &mut diags)?;

// An empty US Letter page at the default scale.
assert_eq!((pixmap.width(), pixmap.height()), (612, 792));
```

The engine never learns which rasterizer it has. A backend implements the two
traits — fill, stroke, image, clip, layer push and pop — and everything above
is written once. Two implementations ship separately:
[`pdfrum-raster-vello`](https://crates.io/crates/pdfrum-raster-vello) over
`vello_cpu` and
[`pdfrum-raster-tinyskia`](https://crates.io/crates/pdfrum-raster-tinyskia)
over `tiny-skia`; anything else that can fill a path can be dropped in.

Damage is never an error. An undrawable matrix, a shading whose function will
not evaluate, a pattern with degenerate steps — each is a skipped object and a
diagnostic. `Error` is reserved for a target that cannot exist at all.

## Part of pdfrum

`pdfrum-render` is the rendering layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — open a file and render a page in two calls, backend
already chosen — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
