# pdfrum-raster-tinyskia

**`tiny-skia` implementation of pdfrum-render's RenderDevice.**

A rasterizer backend built on [`tiny-skia`](https://crates.io/crates/tiny-skia):
it implements `pdfrum-render`'s `RenderDevice` and `RasterBackend` traits —
path fills and strokes, images, clips, layers — and hands back premultiplied
RGBA8 pixmaps. It serves as the cross-check rasterizer and determinism
baseline, diffed against the `vello_cpu` backend to tell a backend bug from an
engine bug.

tiny-skia has neither a layer stack nor a clip stack, so both are emulated
here rather than in the engine — the engine must not know which rasterizer it
has. A layer is an offscreen pixmap composited on `pop`; a clip is a coverage
mask, and pushing one intersects a clone of the current mask, which is literally
`a * b / 255`, the same truncating product PDFium's own clip region uses.

```rust
use kurbo::{Affine, Rect};
use pdfrum_render::{AntiAlias, Brush, FillRule, RasterBackend, RenderDevice};
use pdfrum_raster_tinyskia::TinySkiaBackend;

let backend = TinySkiaBackend::new();
let mut device = backend.new_target(8, 8, peniko::Color::WHITE);

device.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
// ... fill a path covering the whole target ...
device.pop();

let pixmap = backend.finish(device);
```

**What this backend deliberately does not do.** tiny-skia silently drops any
path fill or clip whose bounding box is thinner than `1/4096` in either axis,
turning a degenerate *clip* into a no-op clip rather than an empty one — a
correctness inversion. Fixing it here, per call, would make this backend's
geometry differ from `vello_cpu`'s and break the premise of the cross-backend
comparison. The engine ports the rectangle snapping and zero-area detection
instead, and the harness turns a residual drop into a hard failure rather than
a quiet drift.

## Part of pdfrum

`pdfrum-raster-tinyskia` is a backend for
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API with a backend already wired up, use the `pdfrum`
facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
