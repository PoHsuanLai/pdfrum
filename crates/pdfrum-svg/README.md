# pdfrum-svg

`RenderDevice` that records vectors while a rasterizer does the pixels.

Paths, clips, layers, glyph outlines become SVG. Pixel-domain compositing
(mesh shadings, knockout groups, soft masks, tiling cells) is an embedded
PNG, each listed in `RasterReport`.

```rust
use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::RenderOptions;
use pdfrum_svg::page_to_svg;

let converted = page_to_svg(
    &Page::empty(),
    &RenderOptions::default(),
    &TinySkiaBackend::new(),
    &mut Diagnostics::default(),
)?;
assert!(converted.svg.contains("<svg"));
# Ok::<(), pdfrum_render::Error>(())
```

From the facade: `Page::to_svg`, feature `svg`.

MIT OR Apache-2.0
