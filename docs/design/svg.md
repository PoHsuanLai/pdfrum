# SVG export — the mapping, and its limits

`crates/pdfrum-svg`, M24. What a PDF page becomes when the destination is
vectors rather than pixels, what it cannot become, and how we know.

## 1. The shape of the thing

`RenderDevice` is already vector-typed. `fill_path`, `stroke_path`,
`push_clip` and `push_layer` take `kurbo::BezPath`, `peniko` colours and
`pdfrum_page::BlendMode`; glyphs above the size threshold arrive as filled
outlines. The vector information is already flowing through the trait, and
the trait is object-safe. **So `RasterBackend` did not change and neither did
any of the five rasterizer crates.** M24's roadmap entry records why the
original plan to make the backend generic over its output was withdrawn: the
engine does *arithmetic* on `Pixmap` at fourteen `finish`/`snapshot` sites —
`remove_backdrop` per ISO 32000 §11.4.6, `knockout_over`, `knockout_replace`,
`luminosity_mask`, `multiply_alpha_mask` — and none of those has a vector
meaning.

`SvgBackend<B>` is therefore a **decorator**, not a replacement. It wraps any
`RasterBackend` and passes every call through, so the pixels the engine works
on are exactly the ones the wrapped rasterizer would have produced and every
piece of that compositing arithmetic still comes out right. What is added is a
second recording of the *page* target's calls, as text.

```
                     ┌─ SvgDevice (Root)      → both: tiny-skia AND the document
render_page ── walk ─┤
                     └─ SvgDevice (Offscreen) → tiny-skia only, and a fingerprint
                                                (its finished pixmap comes back
                                                 to the root as one draw_image)
```

The first target the engine asks for is the page; every later one is
offscreen. That is not a heuristic — `render_page_inner` allocates the root
before it walks a single object.

## 2. The mapping

| PDF / device call | SVG |
|---|---|
| `fill_path`, solid brush | `<path d fill="#rrggbb" [fill-opacity] [fill-rule] [transform] [shape-rendering]>` |
| `stroke_path`, solid brush | `<path d fill="none" stroke stroke-width stroke-linecap stroke-linejoin [stroke-miterlimit] [stroke-dasharray stroke-dashoffset] [transform]>` |
| `FillRule::Winding` / `EvenOdd` | `fill-rule` omitted (nonzero is the default) / `fill-rule="evenodd"` |
| `AntiAlias::On` | no attribute |
| `AntiAlias::Off` | `shape-rendering="crispEdges"` |
| `AntiAlias::FullCover` | `shape-rendering="crispEdges"` — an approximation, and one that never ships; see §4 |
| `push_clip(path, rule)` | `<clipPath id><path d [clip-rule]></clipPath>` in `<defs>`, plus `<g clip-path="url(#id)">` |
| `push_clip_rect(rect)` | the same with a `<rect>`, which is what a renderer can fast-path |
| `push_layer(blend, alpha, None)` | `<g [opacity] [style="mix-blend-mode:…"]>` |
| `pop` | `</g>` |
| `draw_image` | `<g data-cause="…"><image x=0 y=0 width height preserveAspectRatio="none" xlink:href="data:image/png;base64,…" [opacity] [transform]></g>` |
| glyph, above the threshold | a `<path>` fill, like any other — this is what makes the text vector |
| a colour's alpha | `fill-opacity` / `stroke-opacity`, kept out of the hex so a `<g opacity>` multiplies cleanly |
| the identity transform | no `transform` attribute at all |

All sixteen `BlendMode` variants map one to one onto CSS `mix-blend-mode`,
because the CSS blend modes were taken from PDF's. `Normal` and `Compatible`
produce no attribute rather than `normal`.

Coordinates are written at three decimal places — a hundredth of a device
pixel at 72 dpi, below any rasterizer's sampling grid — and trimmed.

### `subpixel_text_positioning` is forced on

**The one render option this crate overrides**, and it is not a preference.
With it off — the default, because that is what reproduces the oracle — the
engine rasterizes a *glyph bitmap* for every run below fifty device units per
em and blits it at a snapped origin. At one pixel per PDF point that is nearly
all body text, so an export that honoured the default would be some thousands
of small embedded PNGs with no vector text in it. Measured on
`benches/corpus/text_foxittext.pdf`: **2891 rasterized regions with the option
off, zero with it on.** The cost is that a glyph lands where the PDF puts it
rather than where a golden expects it, which is the trade the option exists to
offer and the right side of it for a vector format.

### What is not in the shipped tree

Nothing new. Writing SVG is string formatting over `kurbo` and `peniko`, both
already there. Two things that would ordinarily be a dependency are written
in-crate instead, per STYLE.md §5:

- **base64** (`src/xml.rs`), nineteen lines, checked against the RFC 4648
  vectors.
The PNG behind an `<image>` `data:` URI is *not* one of them. It was, once —
an in-crate encoder writing stored deflate blocks, on the reading that `png`
is tool-and-test-only under DEPS.md — but `pdfrum-render` already carries the
`png` crate behind its own `png` feature for `Pixmap::encode_png`, so the
choice was never between a dependency and none: it was between one encoder in
the workspace and two. This crate turns that feature on and hands the encoder
a `Pixmap`. The regions that are pixels are now really deflated rather than
stored, which is where the export's size went, and the pixels are still
exactly the bytes the engine produced.

The one subtlety is alpha: the engine's pixmaps are premultiplied and PNG's
alpha is straight, so `SvgDevice::draw_image` unpremultiplies first and wraps
the result in a `Pixmap` purely to reach the encoder.

## 3. The rasterized-region report

`SvgPage::report` lists every region the walk delivered as pixels, each with a
device-space rectangle and a `RasterCause`. An empty report means the page
converted entirely to vectors. A silent raster fallback is the failure mode
this crate exists to avoid, so the report is a deliverable rather than a
nicety, and each region's element carries a `data-cause` attribute so a person
reading the file sees the same answer.

`RenderDevice` carries no cause channel — the engine hands a device a pixmap
and a transform, not a reason — and M24 keeps the trait unchanged. The cause
is worked out instead from the *shape of the engine's own behaviour*, which
`RasterBackend` makes visible for free: the offscreen targets that opened and
closed since the previous root draw are that draw's evidence.

| Cause | Evidence | Exact? |
|---|---|---|
| `NonIsolatedGroup` | a target seeded through `new_target_with_backdrop` | yes — only `needs_backdrop` asks for one |
| `MeshShading` | a fill with `AntiAlias::FullCover` | yes — `shading/patch.rs:546` is the only site in the engine |
| `Knockout` | two sibling targets, the first fill-only and the second stroke-only | yes for the fill+stroke buffer (`paint.rs:333-362`) |
| `SoftMask` | a target holding exactly one image and nothing else | yes — a coverage plane rendered alone to be multiplied into an alpha channel |
| `CompositedGroup` | one offscreen subtree, ordinary drawing inside | **a family**: an isolated group, a knockout group, a type-3 glyph procedure, a tiling-pattern cell |
| `SampledSource` | no offscreen work at all | the region is exact; the *kind* is a family: a sampled image, an axial or radial shading's evaluated buffer, a hinted glyph bitmap |

**The limit, stated plainly.** Four of the six causes are exact. The other two
name a family rather than a single origin, because from the backend's side an
isolated group and a type-3 glyph procedure are the same event: one target
opened, drawn into, finished, blitted. The *region* is right in every case —
that is the part a caller acts on. Making the last two exact needs a cause
channel on the trait, which is a change to `RenderDevice` and so a change M24
deliberately does not make.

## 4. What has no faithful SVG expression

Named, rather than approximated silently.

- **Mesh shadings, types 4-7.** The engine subdivides a Coons or tensor patch
  into cells and fills each with `AntiAlias::FullCover` so abutting cells show
  no seam. SVG has no full-cover fill: `shape-rendering="crispEdges"` is the
  nearest keyword and it is not the same thing. This is moot in practice — a
  mesh is a raster region, so those full-cover fills never reach a shipped
  element — but it is the one place in the mapping table where the answer is
  "close" rather than "exact", and it is why the arm exists at all.
- **A device-sized alpha mask on a layer.** `push_layer`'s `mask` parameter is
  a coverage plane. SVG's `<mask>` could carry it only as an image, which is a
  raster region — so the engine's own choice is followed instead: it passes
  `None` here and multiplies every mask into a pixmap's alpha channel, which
  is why `SoftMask` is a *cause* rather than a mapping row.
- **An image brush on a fill or stroke.** It reaches `fill_path` only from the
  tiling-pattern path, which is rasterized as a whole region, so the document
  never needs a `<pattern>`. The fill is still drawn by the wrapped
  rasterizer; it is simply not written as an element.
- **Axial and radial shadings.** These have a faithful SVG expression —
  `<linearGradient>` and `<radialGradient>` — and this pipeline does **not**
  use it. They arrive at the device as an already-evaluated pixel buffer
  (`shading::draw_to_pixmap`), so what the backend sees is an image and what
  it writes is an image. They are reported as `SampledSource` and they are the
  largest single opportunity left in this crate. Turning them into gradients
  means reading them from the page graph rather than from the device, which is
  a different pipeline, not a patch to this one.

Beyond these five, no part of the mapping is an approximation.

## 5. The round trip, and the floors

The conformance board compares our pixels to `pdfium_test`'s and an SVG file
has none, so M24's proof is a round trip:
`crates/pdfrum-svg/tests/roundtrip.rs` converts each of `benches/corpus`'s 44
first pages, renders the SVG with **`resvg`** at the board's one-pixel-per-
point scale, and scores that pixmap with the board's own SSIM — the same
formula, transcribed rather than called because the conformance harness is a
binary with no library target.

Two comparisons are run:

- **vs ours** — against our own `tiny-skia` render of the same page under the
  same options. Isolates what the *conversion* loses.
- **vs oracle** — against `pdfium_test`'s PNG from `conformance/goldens`.
  Carries the conversion's loss, `resvg`'s own antialiasing, and the
  glyph-placement difference `subpixel_text_positioning` makes, all at once.

Seven of the 44 bench-corpus documents are not in the conformance corpus and
have no golden (`forms_combo_box`, `forms_list_box`, `forms_number`,
`forms_push_button`, `forms_signature`, `forms_text_field`,
`mixed_formfield`). **37 are scored against the oracle.**

### Where the store comes from, and what a missing one means

`$PDFRUM_GOLDENS`, else the in-repo `conformance/goldens`. The store is **not
committed**: it is generated locally from a built `pdfium_test`, and
`.github/workflows/ci.yml` states outright that it will not do that
multi-hour build. So a hosted runner has no goldens and the oracle half
cannot run there.

The test treats that as a missing *input*, not as a pass, and keeps two cases
apart — collapsing them is exactly what produces a vacuous green:

- **No store**: the oracle half is skipped with a note on stdout, and the
  self-consistency half still runs over all 44 files with its floors enforced.
  This is the CI configuration.
- **A store that is present**: every file that has a golden must be scored,
  and the count 37 is pinned. A broken lookup fails with `left: 0, right: 37`
  rather than quietly shrinking the proof to nothing.

### Published floors — measured, not chosen

Set just below the worst file in each class on the run below. The worst file
is named beside each.

| Class | vs ours (floor) | vs oracle (floor) | worst file, vs oracle |
|---|---|---|---|
| Text | 0.95 | 0.90 | `text_tcpdf_063` 0.9240 |
| Vector | 0.95 | 0.95 | `vector_font_size14` 0.9647 |
| Image | 0.95 | 0.90 | `image_ccitt_3bigpreview` 0.9317 |
| Shading | 0.95 | 0.80 | `shading_tcpdf_030` 0.8532 |
| Forms | 0.95 | 0.75 | `forms_widgets_407` 0.7902 |
| Mixed | 0.95 | 0.90 | `mixed_tcpdf_045` 0.9628 |

Against our own render every one of the 44 scores **at least 0.9761**, which
is why one flat floor covers all six classes there.

The shading and forms classes are the low end, and the reasons are the two in
§4 and §2: a mesh is an embedded PNG that `resvg` resamples with its own
filter, and `forms_widgets_407` is a page of dense small text where placing
glyphs at their true origins rather than the oracle's snapped grid costs the
most. Both are the pipeline being honest about a real difference rather than a
bug.

### Vector coverage on the corpus

Of the 44 first pages: **18 convert entirely to vectors** (empty report), 24
carry at least one rasterized region, and 2 draw nothing at all
(`forms_widgets_407`, whose content is all in widget annotations, and
`mixed_formfield`).

The cause breakdown across the 24 matches §3's prediction: `SampledSource`
dominates (embedded images, and the axial/radial buffers of §4);
`MeshShading` appears on exactly the three files with type 4-7 shadings
(`shading_axial_radial` x16, `shading_tcpdf_030` x2, `shading_tensor` x13);
`SoftMask` on the three files with soft masks or `/SMask` images
(`image_en_fqa` x87, `shading_tcpdf_058` x13, `image_ccitt_3bigpreview` x1).
No corpus first page exercised `NonIsolatedGroup`, `Knockout` or
`CompositedGroup`; those paths are covered by the unit tests in
`src/backend.rs` and `src/evidence.rs` instead.

## 6. `resvg`, and what it pulls

A **dev-dependency of `pdfrum-svg` only**. It never enters the shipped tree,
and `scripts/ci.nu`'s pure-Rust check covers that.

```toml
resvg = { version = "=0.47.0", default-features = false, features = ["raster-images"] }
```

Per the DEPS.md rule — take what we need, not what a crate defaults to. The
defaults are `text`, `system-fonts`, `memmap-fonts` and `raster-images`; the
first three exist to *shape text*, which our SVG never asks for because every
glyph is already a filled outline. Dropping them drops `fontdb`, `rustybuzz`,
`ttf-parser`, `unicode-bidi`, `unicode-script`, `unicode-vo` and `memmap2`.
`raster-images` is the one feature kept: it is what admits `<image>` decoding,
and our images travel as PNG data URIs.

0.47.0 resolves `tiny-skia` **0.12.0**, the version the workspace already
pins, so no duplicate rasterizer enters the graph. Everything it pulls is pure
Rust.
