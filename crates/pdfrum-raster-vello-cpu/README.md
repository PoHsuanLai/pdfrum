# pdfrum-raster-vello-cpu

[`vello_cpu`](https://crates.io/crates/vello_cpu) backend. Default rasterizer.

```rust
use pdfrum_raster_vello_cpu::VelloCpuBackend;
use pdfrum_render::RasterBackend;

let backend = VelloCpuBackend::new();
let mut device = backend.new_target(4, 4, peniko::Color::WHITE);
```

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
