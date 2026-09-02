# pdfrum-raster-vello-cpu

**`vello_cpu` implementation of pdfrum-render's RenderDevice.**

A rasterizer backend built on [`vello_cpu`](https://crates.io/crates/vello_cpu):
it implements `pdfrum-render`'s `RenderDevice` and `RasterBackend` traits —
path fills and strokes, images, clips, layers with blend modes and soft masks —
and hands back premultiplied RGBA8 pixmaps. This is the primary backend.

*Renamed 2026-09-02, was `pdfrum-raster-vello` (type `VelloBackend`).* The
crates now take vello's own names — upstream ships `vello` (GPU, on `wgpu`),
`vello_cpu` and `vello_hybrid` — so the bare name is the GPU backend and this
one says which vello it wraps.

The wrapper is total: `vello_cpu`'s still-moving API never leaks through into
the engine above, so a version bump here is not a version bump everywhere.

```rust
use kurbo::Affine;
use pdfrum_render::{ImageQuality, Pixmap, RasterBackend, RenderDevice};
use pdfrum_raster_vello_cpu::VelloCpuBackend;

let backend = VelloCpuBackend::new();
let mut device = backend.new_target(4, 4, peniko::Color::WHITE);

let image = Pixmap::filled(4, 4, peniko::Color::from_rgba8(255, 0, 0, 255));
device.draw_image(&image, Affine::IDENTITY, ImageQuality::Nearest, 0.5);

let pixmap = backend.finish(device);
assert_eq!(pixmap.pixel(1, 1).expect("in bounds")[3], 255);
```

**Determinism is pinned, deliberately.** `vello_cpu` is bit-exact across thread
counts but *not* across SIMD feature levels — a baseline-versus-AVX2 pair
differs by one count on a handful of pixels, reproducibly, in the shared
flattening stage. That is inside any reasonable rounding budget, but it would
make output differ between a developer machine and CI, so this backend pins the
baseline level and the f32 quality pipeline rather than detecting either.

One workaround worth knowing: `vello_cpu` 0.2.0 panics rather than honouring a
non-1.0 image sampler alpha, so a translucent image draw is wrapped in an
opacity layer here and the sampler is always handed `alpha = 1.0`. A regression
test pins that.

## Part of pdfrum

`pdfrum-raster-vello-cpu` is a backend for
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API with the backend already wired up, use the `pdfrum`
facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
