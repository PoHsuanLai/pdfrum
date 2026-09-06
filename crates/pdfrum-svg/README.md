# pdfrum-svg

SVG export for the `pdfrum` engine: a `RenderDevice` that accumulates
vectors while an ordinary rasterizer does the pixels.

`RenderDevice` is already vector-typed — `fill_path`, `stroke_path`,
`push_clip` and `push_layer` speak `kurbo::BezPath`, `peniko` colours and
blend modes, and glyphs reach it as outlines — so an SVG consumer needs no
change to the engine or to any of the five rasterizer backends.
`SvgBackend` wraps whichever `RasterBackend` you already have and records
the page target's calls on the way past: paths become `<path>` with the
fill rule, clips become `<clipPath>`, layers become `<g>` with `opacity`
and `mix-blend-mode`, glyphs become filled outlines, and images become an
embedded PNG data URI. The output is a `String`, so a caller composes it.

Every region the engine composites in the *pixel* domain — a mesh shading,
a non-isolated or knockout transparency group, a soft mask, a
tiling-pattern cell — arrives at the device as pixels and is embedded as
one. Each of those is recorded in a `RasterReport` with a typed cause and
tagged in the document with a `data-cause` attribute. A silent raster
fallback is the failure mode this crate exists to avoid, so the report is
part of the answer rather than an aside.

```rust
use pdfrum_common::Diagnostics;
use pdfrum_page::Page;
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_render::RenderOptions;
use pdfrum_svg::page_to_svg;

# fn main() -> Result<(), pdfrum_render::Error> {
let converted = page_to_svg(
    &Page::empty(),
    &RenderOptions::default(),
    &TinySkiaBackend::new(),
    &mut Diagnostics::default(),
)?;
assert!(converted.svg.contains(r#"viewBox="0 0 612 792""#));
assert!(converted.report.is_empty(), "an empty page is all vectors");
# Ok(())
# }
```

`page_to_svg` takes the engine's own `Page` and `RenderOptions`, which is
what a caller who already holds a page-object graph has. A caller coming
from the `pdfrum` facade holds the facade's wrappers instead — different
types on purpose — and reaches the same conversion through
`pdfrum::Page::to_svg`, behind the facade's default-off `svg` feature. That
is the shorter path, and `examples/convert.rs` takes it: nothing in it names
an engine crate.

The shipped crate depends on nothing beyond the geometry vocabulary the
engine already speaks; base64 is written in-crate rather than pulled in. The
PNG behind an embedded image is not: it is `pdfrum-render`'s own encoder,
reached by turning on that crate's `png` feature rather than by adding a
second edge to the `png` crate.

`docs/design/svg.md` records the full mapping table, the five things that
have no faithful SVG expression, and the round-trip scores the export
achieves against `pdfium_test`'s own output.

Licensed under either of Apache-2.0 or MIT at your option.
