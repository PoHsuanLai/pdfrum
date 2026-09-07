# pdfrum-raster-tinyskia

[`tiny-skia`](https://crates.io/crates/tiny-skia) backend. Cross-check against
`vello_cpu`. Layers and clips are emulated here (tiny-skia has neither).

```rust
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::RasterBackend;

let backend = TinySkiaBackend::new();
let mut device = backend.new_target(8, 8, peniko::Color::WHITE);
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
