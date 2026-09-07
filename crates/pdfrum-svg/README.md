# pdfrum-svg

SVG export: a [`RenderDevice`](pdfrum_render::RenderDevice) that records
vectors while a real rasterizer does the pixels. It is a second consumer of
the same page-object graph the renderer walks, not a separate interpreter,
so an SVG and a raster render of one page agree by construction.

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
assert!(converted.report.is_empty());
# Ok::<(), pdfrum_render::Error>(())
```

Paths, clips, layers and glyph outlines become SVG elements. What SVG has no
way to say — mesh shadings, knockout groups, soft masks, tiling cells — is
composited in the pixel domain by the backend you pass and embedded as a PNG.
That is why a rasterizer is an argument rather than an implementation detail:
a conversion with no backend would have to either drop those regions or
render them wrong.

Every such region is listed in [`RasterReport`] with its [`RasterCause`], so
a caller can tell an all-vector page from one that is half a bitmap before
shipping it. [`RasterReport::is_empty`] is the "purely vectors" test and
[`RasterReport::counts`] tallies the causes.

The document's `viewBox` is the device box the same page would render into
under the same [`RenderOptions`](pdfrum_render::RenderOptions), so the two
outputs are in one coordinate system and can be diffed pixel for pixel.
[`SvgPage::svg`] is a `String`, not a file — this crate has no opinion about
where it goes.

From the facade: `Page::to_svg`, feature `svg`.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
