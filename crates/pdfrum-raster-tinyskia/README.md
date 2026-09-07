# pdfrum-raster-tinyskia

[`tiny-skia`](https://crates.io/crates/tiny-skia) behind
[`pdfrum_render::RasterBackend`]. Carried as a second CPU rasterizer so the
engine's output can be cross-checked against
[`vello_cpu`](https://crates.io/crates/vello_cpu): the engine does not know
which backend it has, and two independent rasterizers agreeing is what makes
that true rather than aspirational.

```rust
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::RasterBackend;

let backend = TinySkiaBackend::new();
let mut device = backend.new_target(8, 8, peniko::Color::WHITE);
```

tiny-skia has neither layers nor clips, so both are emulated here — a layer
is an offscreen pixmap composited on `pop`, and a clip is an alpha mask
multiplied in with the same truncating product PDFium's
`CFX_AggClipRgn::IntersectMask` uses.

What this backend deliberately does not do is correct tiny-skia's `1/4096`
drop. Fixing it per call would make this backend's geometry differ from
`vello_cpu`'s and break the premise of the cross-backend contract. The engine
ports PDFium's rectangle snapping and zero-area detection instead, which
removes the bulk of the cases, and the harness turns a residual drop into a
hard failure rather than a quiet drift.

Facade feature `tinyskia`.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
