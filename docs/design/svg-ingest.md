# SVG ingestion — the mapping, and its limits

`crates/pdfrum/src/svg_ingest.rs`, M25. What an SVG document becomes when it
is drawn into a PDF page, what it cannot become, and how we know.

The mirror of `docs/design/svg.md`, which is the same geometry crossing in
the other direction.

## 1. The shape of the thing

Export decorates a `RenderDevice` and writes `<path>` elements. Ingestion has
no device to decorate: an SVG is not a render, it is a *document*, and the
first problem is resolving it — the CSS cascade, `use` expansion, `viewBox`
fitting, the two gradient coordinate systems, presentation attributes losing
to stylesheet rules, `href` chains. That is a large specification, and
`usvg` already implements all of it in pure Rust. **Writing a second
incomplete SVG parser is declined here in writing**; the roadmap's M25 entry
declines it too.

What `usvg` hands back is a tree of groups, paths and images with every
transform and every reference already resolved. Ingestion walks that tree and
issues M23 `Canvas` calls:

```
usvg::Tree ── Walk ─┬─ Group  → Canvas::saved + transform + clip + opacity
                    ├─ Path   → Canvas::draw, or Canvas::shade for a gradient
                    └─ Image  → Canvas::image, or a nested walk for an <svg>
```

`Canvas` already writes content-stream operators, merges resources under
collision-checked names and scopes every graphics-state change to a closure,
so ingestion writes no PDF syntax of its own except the shading dictionaries
of §2.

### Where the code lives, and why the facade

`Canvas` is in the **facade** (`crates/pdfrum/src/canvas.rs`), not in
`pdfrum-edit`, because `pdfrum-edit` does not depend on `peniko` and a canvas
speaks `peniko::Color`. Ingestion targets `Canvas`, so it inherits that
position.

It could not have gone in `pdfrum-svg` in any case. `pdfrum-svg` is a
*dev*-dependant of `pdfrum` today, and M24's facade feature makes `pdfrum`
depend on `pdfrum-svg`; a normal dependency the other way would close the
cycle. The seam is real rather than incidental: export is a `RenderDevice`
consumer and belongs beside the engine, ingestion is a `Canvas` producer and
belongs beside the editor. They share a subject, not a layer.

### Two features, not one

`pdfrum-svg`'s export adds **no dependency at all** — it is string formatting
over the vocabulary the engine already speaks. Ingestion adds `usvg` and nine
crates behind it. Charging an exporting caller for a parser they never reach
would be the wrong trade, so the two are separate features:

| feature | what it turns on | new crates in the tree |
|---|---|---|
| `svg` | `pdfrum-svg`, export | **0** |
| `svg-ingest` | `Canvas::draw_svg`, ingestion | **10** |

Both are default-off. `svg-ingest` implies `edit`, because a canvas is a
drawing session over an editable document.

## 2. The mapping

| SVG / `usvg` node | PDF |
|---|---|
| `<svg>` root, placed by `SvgFit` | `q` … `Q` around the whole drawing, with a clip to the destination rect and one `cm` carrying the fit and the y flip |
| `Group` `transform` | `cm` inside a `q`/`Q` |
| `Group` `opacity` | `/ExtGState` with `/ca` and `/CA` |
| `Group` `clip-path` | the children's outlines concatenated, then `W n` (or `W* n`) |
| `Path` solid `fill` | `rg` + `f` / `f*` |
| `Path` solid `stroke` | `RG` + `w` + `S` |
| both | `B` / `B*` |
| `fill-rule="evenodd"` | the starred operator |
| `fill-opacity`, `stroke-opacity`, `stop-opacity` | folded into the colour's alpha, which becomes an `/ExtGState` |
| `<linearGradient>` fill | `/ShadingType 2`, the path as a clip, `sh` |
| `<radialGradient>` fill | `/ShadingType 3`, likewise |
| gradient stops | one `/FunctionType 3` stitching function over `/FunctionType 2` exponentials, one per adjacent pair, stitched at the stop offsets |
| `spreadMethod` | `/Extend [true true]` — always; see §4 |
| `<image>` PNG | decoded to samples, `/DeviceRGB` or `/DeviceGray`, alpha to an `/SMask` |
| `<image>` JPEG | passed through whole as `/DCTDecode`, nothing decoded |
| `<image>` nested `<svg>` | walked as a subtree, so it stays vectors |
| a quadratic segment | raised to the identical cubic, never flattened |

### The y flip, composed once

SVG's y runs **down** from a top-left origin; the canvas's runs **up** from a
bottom-left one. The flip is composed into the single placement transform
`SvgFit::place` returns, applied to the whole subtree, rather than negated at
each draw. That is the only spelling that also gets nested `transform`
attributes and gradient coordinate systems right — negating per-primitive
would mirror every nested rotation.

`SvgFit` is an enum, not a `preserve_aspect: bool`: `Contain` (the default,
and what `preserveAspectRatio`'s own default `xMidYMid meet` means), `Cover`,
and `Stretch` (`preserveAspectRatio="none"`).

## 3. The unsupported report

The report is the deliverable, not a nicety. **Nothing the walk cannot carry
is dropped silently** — that is the property this module exists to guarantee,
and it is the same one `pdfrum-svg`'s `RasterReport` guarantees going the
other way.

`Canvas::draw_svg` returns an `SvgIngestReport` of `UnsupportedItem`s, each an
`Unsupported` variant and the element's `id`. `Unsupported` is an enum rather
than a message string (STYLE.md §2): a caller that will refuse a filter but
tolerate a dropped `<text>` matches on the variant, and adding a case makes
every such match fail to compile.

`Unsupported::is_dropped` separates the two kinds of loss a caller acts on
differently:

| variant | dropped? | what actually happens |
|---|---|---|
| `Filter` | no | the subtree draws unfiltered |
| `Mask` | no | the group draws unmasked |
| `BlendMode` | no | the subtree draws with normal blending |
| `OffsetFocalGradient` | no | the focus is moved to the centre |
| `Text` | **yes** | nothing is drawn — see §6 |
| `Pattern` | **yes** | the shape is not painted |
| `ImageFormat` | **yes** | a GIF or WebP `<image>` is not drawn |

A dropped construct leaves a visible hole; an approximated one still draws
something close. A caller that will accept an approximation but not a hole
tests `is_dropped`.

## 4. What has no faithful PDF expression

**Filters.** PDF has no filter model at all. The only faithful rendering
would be to rasterize the filtered subtree — which is exactly the blurry
result that ingesting an SVG as vectors exists to avoid. Declined, per the
roadmap, and reported.

**Masks.** A PDF soft mask *is* expressible, through a `/SMask` luminosity
group whose own content is a second form — a whole second compilation path
with its own resource scope. Deferred rather than declined; the group draws
unmasked and the loss is reported.

**Blend modes.** PDF has the same separable and non-separable modes — CSS's
were taken from PDF's — but setting one meaningfully needs a transparency
group `/XObject` around the subtree, not a bare `/ExtGState`. Same shape of
deferral as masks.

**Patterns.** A PDF tiling pattern is expressible and is future work. Today
the shape is skipped rather than filled with an approximation, because a
wrong colour over a large area is a worse answer than a reported gap.

**Group opacity is an approximation, and it is the one carried case that is.**
SVG renders a group to its own buffer and composites that buffer once at the
group's opacity. We set `/ca` and `/CA` and draw the members into the page one
at a time. Where two translucent members *overlap*, those are different
numbers — the overlap is composited twice. `opacity_groups` scores 0.9669 for
exactly this reason and no other, which is why its class floor is the second
lowest in §5. It is not reported, because the shape is still there and still
translucent; a caller who needs the exact SVG semantics needs the same
transparency-group `/XObject` masks and blend modes need.

**Gradient strokes.** `S` takes a colour, not a shading. The PDF spelling
would be to convert the stroke to its outline and shade that, which needs a
stroke expander this module does not have. The stops are averaged so the shape
stays visible and roughly right. Also unreported, and for the same reason.

**`spreadMethod` reflect and repeat.** A PDF shading extends its end colours
or does not; there is no reflect or repeat. All three spreads therefore emit
`/Extend [true true]`, and `spread_method()` is not read at all — a parameter
that every branch discards is the dead option STYLE.md §4 forbids, so it is
not taken. Also unreported: `pad` is exact, and the other two are wrong in a
way a caller can see. Reflect and repeat need a tiling pattern over the
gradient, which is the same work `<pattern>` needs.

**A `clipPath` with its own nested `clip-path`.** SVG intersects the two; `W`
narrows the clip to one path's subpaths, and we concatenate the children into
a single outline. The nested-intersection case is not expressible this way.

**Stroke attributes beyond colour and width.** `M23`'s `Canvas::Stroke`
carries exactly those two, so `stroke-linecap`, `stroke-linejoin`,
`stroke-miterlimit`, `stroke-dasharray` and `stroke-dashoffset` are dropped —
every stroke inks with the PDF default cap and join and no dash. PDF has an
operator for each of them (`J`, `j`, `M`, `d`), so this is a **canvas** gap
rather than a mapping one: closing it means widening `Stroke`, which is M23's
type and is used by callers who are not ingesting SVG.

It is unreported, and that is a judgement rather than an oversight: a
butt-capped line where the source asked for a round cap is a half-pixel
difference at the ends, not a missing shape, and reporting it on every dashed
or round-joined stroke would bury the report items that mean something. It is
named debt in the roadmap instead. The one case where it is visible rather
than subtle is a **dashed** stroke, which draws solid.

**No fixture exercises it**, deliberately: a dashed stroke would fail the
Vector floor, and correctly so. Adding one and then lowering the floor to
accommodate it would be the floor being chosen rather than measured, which is
the thing §5 exists to prevent. The gap is recorded here and in the roadmap
instead, and a fixture belongs in the pass that closes it.

`paint-order` and `shape-rendering` are likewise not read: the first because
the canvas paints fill-then-stroke through `B`, which is the SVG default and
the only order `Paint::FillStroke` expresses, and the second because the
canvas has no antialiasing control to map it onto.

## 5. The round trip, and the floors

The conformance board scores PDFs against `pdfium_test`'s pixels, and an SVG
input has none — there is no `pdfium_test` for SVG. So the proof is the mirror
of M24's: `crates/pdfrum/tests/svg_ingest.rs` draws each fixture into a
200×200 page, renders the **saved** PDF with `tiny-skia`, and scores it
against **`resvg`** — an independent implementation with its own parser, its
own geometry and its own rasterizer — rendering the same SVG directly, with
the board's own SSIM.

Rendering the *saved* document rather than the editing session is deliberate:
it puts the content stream, the resource merge and the shading dictionaries
through a real write-and-reparse, so a stream this module emits but cannot
read back fails here instead of passing on an in-memory shortcut.

### The fixture store, and what a missing one means

Unlike M24's, this corpus **is committed**: `crates/pdfrum/tests/fixtures/svg`
is seventeen small hand-written documents, not multi-megabyte artifacts
generated from a build of the oracle. There is therefore no legitimate "the
store is absent" case, and the test says so — a missing or empty directory
**fails**, and a `.svg` with no row in the `FIXTURES` table fails too, so a
fixture cannot be added and then silently scored under a floor nobody chose
for it.

`$PDFRUM_GOLDENS` is not consulted at all here. There is no oracle artifact
for an SVG, so its absence changes nothing about what this test proves.

### Published floors — measured, not chosen

Set just below the worst file in each class on the run below. The worst file
is named beside each.

| Class | floor | worst file |
|---|---|---|
| Vector | 0.99 | `shapes_stroked` 0.9989 |
| Gradient | 0.99 | `gradient_linear` 0.9991 |
| Alpha | 0.95 | `opacity_groups` 0.9669 |
| Image | 0.92 | `image_png` 0.9315 |

The run, all seventeen fixtures:

| fixture | vs `resvg` | report |
|---|---|---|
| `shapes_basic` | 1.0000 | — |
| `shapes_stroked` | 0.9989 | — |
| `paths_curves` | 0.9990 | — |
| `fill_evenodd` | 1.0000 | — |
| `transforms_nested` | 1.0000 | — |
| `use_and_defs` | 1.0000 | — |
| `css_styles` | 1.0000 | — |
| `clip_path` | 1.0000 | — |
| `opacity_groups` | 0.9669 | — |
| `gradient_linear` | 0.9991 | — |
| `gradient_radial` | 0.9992 | — |
| `image_png` | 0.9315 | — |
| `declined_filter` | 0.8280 | `filter` |
| `declined_text` | 1.0000 | `text` |
| `declined_pattern` | 0.5230 | `pattern` |
| `declined_mask` | 0.8229 | `mask` |
| `declined_blend` | 0.8543 | `blend-mode` |

Eight of the twelve carried fixtures score a flat **1.0000** against an
independent implementation — paths, fill rules, nested transforms, `use`
expansion, the CSS cascade and clipping are translations rather than
approximations, and the numbers say so. The three that are not are the three
§4 explains: stroke joins inked slightly differently, group opacity
composited differently, and a raster resampled by a different filter.

**The five `declined_*` fixtures carry no floor** and their scores are not
evidence of quality — the two renderers are drawing different pictures by
design. They are scored only so the table is complete; what is asserted about
them is that each names its own construct in the report.

`declined_text` scoring 1.0000 needs saying plainly: it is **not** evidence
that text renders. Both sides omit the text — we because `usvg` dropped it
(§6), `resvg` because it was given no font database — so the two blank
regions agree exactly. The assertion that matters on that fixture is the
report item.

## 6. Text, and the dependency decision behind it

`usvg` is taken with `default-features = false`. The defaults are `text`,
`system-fonts` and `memmap-fonts`, and all three exist to **shape text** —
which would mean a second font stack (`fontdb`, `rustybuzz`, `ttf-parser`,
`unicode-bidi`, `unicode-script`, `unicode-vo`) standing beside the one
pdfrum already carries. Dropping them is the point rather than a saving:
DEPS.md's rule is to take what we need from a dependency, not what it
defaults to, and a rival font stack is not something this project needs.

**The consequence is stated rather than hidden.** Without the `text` feature,
`usvg`'s converter has no arm for `EId::Text` — a `<text>` element leaves *no
node* in the resolved tree. The walk cannot notice something that is not
there, so `<text>` is detected in the **source XML**, before the parse, and
reported as `Unsupported::Text`. That is why it is the one variant carrying no
element id: there is no tree node left to name.

This is named debt, not a finished story. The roadmap's M25 item 3 asks for
text as outlines by default and embedded fonts on request; what shipped is
the report. Closing it means mapping SVG text through `pdfrum-font` — which
is the *right* design and the reason the second stack was refused — and it is
a font-and-layout problem rather than an SVG one.

`Node::Text` still has a match arm in the walk, reporting the same variant.
It is unreachable in this build; it is there because the enum is `usvg`'s and
not ours, and a future build that did carry text must not silently drop it.

## 7. `usvg`, and what it pulls

Ten crates new to the workspace's normal tree, behind a default-off feature:

```
usvg  base64  data-url  float-cmp  imagesize  pico-args
simplecss  siphasher  svgtypes  xmlwriter
```

All pure Rust; `scripts/ci.nu`'s pure-Rust check walks the whole workspace and
still passes, because none of them is a `-sys` crate and none has a native
build dependency. `kurbo 0.13.1` and `tiny-skia-path 0.12.0` are the versions
the workspace already pins, so no duplicate geometry or path crate enters the
graph.

`pico-args` and `xmlwriter` are non-optional dependencies of `usvg`'s
*library* despite serving its CLI and its writer; they are carried rather than
patched around.

DEPS.md records the decision in full.

## 8. What ingestion does **not** produce

The roadmap's M25 item 2 asks for a **Form XObject** — `/Subtype /Form` with
its own `/BBox` and `/Resources`, so one SVG placed on twenty pages is one
object and twenty `Do` calls rather than twenty copies.

What shipped draws the SVG **inline** into the page's content stream. For the
single-placement case that this API's shape encourages — `draw_page(i, |c|
c.draw_svg(…))` — the two are equivalent in the file, and the inline form is
the one `Canvas` already expresses. For the repeated-placement case the
XObject is genuinely better and is named debt in the roadmap.

Nothing about the mapping in §2 changes when it lands: a form's content stream
holds the same operators. What changes is where they are written and how
resources are scoped.
