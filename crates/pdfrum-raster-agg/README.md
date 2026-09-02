# pdfrum-raster-agg

**An analytic scanline rasterizer implementing pdfrum-render's RenderDevice.**

A rasterizer backend with no rasterizer dependency: it *is* the rasterizer.
It implements `pdfrum-render`'s `RenderDevice` and `RasterBackend` traits —
path fills and strokes, images, clips, layers — and hands back premultiplied
RGBA8 pixmaps.

## Why a third backend

The other two backends wrap third-party rasterizers, and each brings its own
idea of what a partially covered pixel is worth. `tiny-skia` supersamples at
four subsamples per axis, so a diagonal edge has **seventeen** distinct
coverage levels and a half-covered pixel quantises to 8/16 of the range rather
than to a half. PDFium integrates the covered area analytically and writes the
exact value.

This crate computes the same integral, on the same 256ths-of-a-pixel grid, and
maps coverage to alpha through the same measured formula:

```text
alpha = min(255, floor(coverage * 256))
```

a ×256 scale clamped at the top, truncating — measured from a shallow-slope
fill whose true coverages of ⅛, ⅜, ⅝ and ⅞ come back as 32, 96, 160 and 224.

## How it works

Curves are flattened by `kurbo`; each straight segment is integrated into
per-pixel `(cover, area)` cells; a sweep turns the sorted cells into spans of
constant coverage. Every intermediate value is an integer, so the same path
produces the same bytes on every machine — a determinism a SIMD-dispatched
rasterizer cannot promise for free.

The compositing arithmetic is **`pdfrum-render`'s**, not this crate's, so a
pixel this backend blends and a pixel the engine blends in its own offscreen
buffers agree by construction rather than by coincidence.

## What "exact" means, and what it does not

Exact refers to the coverage integral and the arithmetic downstream of it. It
does not mean a whole page matches PDFium byte for byte: glyph rendering, image
resampling and the engine's own decisions are all upstream of this crate.

```rust
use kurbo::Affine;
use pdfrum_render::{AntiAlias, Brush, FillRule, RasterBackend, RenderDevice};
use pdfrum_raster_agg::AggBackend;

let backend = AggBackend::new();
let mut device = backend.new_target(4, 1, peniko::Color::TRANSPARENT);
// ... fill a rectangle covering exactly half of column 0 ...
let pixmap = backend.finish(device);
// Exactly half, not the nearest of seventeen supersampled levels.
```

## Part of pdfrum

`pdfrum-raster-agg` is a backend for
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API with a backend already wired up, use the `pdfrum`
facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
