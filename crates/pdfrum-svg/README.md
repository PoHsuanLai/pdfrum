# pdfrum-svg

`RenderDevice` that records vectors while a rasterizer does the pixels.

Paths, clips, layers, glyph outlines become SVG. Pixel-domain compositing
(mesh shadings, knockout groups, soft masks, tiling cells) is an embedded
PNG, each listed in `RasterReport`.

```rust
use pdfrum_svg::page_to_svg;
use pdfrum_raster_tinyskia::TinySkiaBackend;

let converted = page_to_svg(&page, &opts, &TinySkiaBackend::new(), &mut diags)?;
```

From the facade: `Page::to_svg`, feature `svg`.

MIT OR Apache-2.0
