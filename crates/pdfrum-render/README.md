# pdfrum-render

Walks a page-object graph onto a rasterizer (ISO 32000-1 §8.4–8.7): paths,
clips, images, shadings, transparency groups, soft masks, patterns and glyph
runs. It is the engine, not a rasterizer — it owns *what* to paint and the
order to paint it in, and knows nothing about how a pixel gets set.

```rust
use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::{RenderOptions, render_page};

let pixmap = render_page(
    &Page::empty(),
    &RenderOptions::default(),
    &TinySkiaBackend,
    &mut Diagnostics::default(),
)?;
assert_eq!((pixmap.width(), pixmap.height()), (612, 792));
# Ok::<(), pdfrum_render::Error>(())
```

The seam is two traits. [`RenderDevice`] is six drawing primitives plus a
hard-edged rectangle clip, object-safe, and the engine holds one per target;
[`RasterBackend`] is the factory that makes a second target, which is what
soft masks, transparency groups, pattern cells and the fill-plus-stroke
knockout buffer all need. A backend implements both and is otherwise
invisible: because the engine never learns which rasterizer it has, two
independent CPU backends can be run over the same corpus and made to agree,
and that agreement is the test that the engine is not compensating for one of
them.

The background is a decision, not a default: an opaque page paints onto
white, a transparent one onto nothing, and
[`needs_alpha_background`] is the query. Getting it backwards is the
difference between a page composited over a viewer's chrome and one with a
white card behind it.

Damage never fails a render. An object the engine cannot draw is skipped with
a diagnostic; `Err` is reserved for a target that is empty or too large under
`opts`. [`RenderSession`] carries the caches a repeated render reuses — give
each worker thread its own.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
