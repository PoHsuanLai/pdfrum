# pdfrum-raster-agg

An analytic scanline rasterizer implementing [`pdfrum_render::RasterBackend`].
No third-party rasterizer crate: coverage is exact area on a
256ths-of-a-pixel grid, mapped to alpha by
`alpha = min(255, floor(coverage * 256))` — PDFium's AGG path, by
construction rather than by approximation.

```rust
use pdfrum_raster_agg::AggBackend;
use pdfrum_render::RasterBackend;

let backend = AggBackend::new();
let mut device = backend.new_target(8, 8, peniko::Color::WHITE);
```

That formula is the reason this backend exists. The other two CPU backends
wrap someone else's rasterizer and bring its idea of a partly covered pixel
with them — `tiny-skia` supersamples four times per axis, so a diagonal edge
has seventeen coverage levels and a half-covered pixel quantises to 8/16
rather than to a half. Over the corpus that is a persistent few-count spread
along every non-axis-aligned edge. `AggBackend` closes it; `vello_cpu` stays
the facade default, because an API caller wants a fast production rasterizer
rather than an oracle-matching one.

[`AggBackend`] is stateless — every [`AggDevice`] target is independent, so
one backend value can be shared across threads. Layers and clips are both
explicit stacks here, as they must be for a backend with neither natively: a
layer is an offscreen target composited on `pop`, and a clip is a coverage
plane pushed so `pop` restores the previous one rather than recomputing an
intersection.

Facade feature `agg`.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
