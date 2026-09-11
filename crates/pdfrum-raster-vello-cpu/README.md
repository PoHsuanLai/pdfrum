# pdfrum-raster-vello-cpu

[`vello_cpu`](https://crates.io/crates/vello_cpu) behind
[`pdfrum_render::RasterBackend`]. The facade's default rasterizer: pure Rust,
no threads, no GPU handshake, and the one backend a `wasm32` build can use.

```rust
use pdfrum_raster_vello_cpu::VelloCpuBackend;
use pdfrum_render::RasterBackend;

let backend = VelloCpuBackend;
let mut device = backend.new_target(4, 4, peniko::Color::WHITE);
```

Determinism is pinned rather than assumed. A baseline-vs-AVX2 pair differs
by one count on a handful of pixels, reproducibly, in the shared flattening
stage; that is inside the cross-backend rounding budget but it would make
our own output depend on the host CPU, so the SIMD path is not left to
runtime detection.

Facade feature `vello-cpu`, on by default.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
