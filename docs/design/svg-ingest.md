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

### Three features, not one

`pdfrum-svg`'s export adds **no dependency at all** — it is string formatting
over the vocabulary the engine already speaks. Ingestion adds a parser.
Ingesting *text* adds a layout engine on top of that. Charging an exporting
caller for a parser they never reach, or a logo-ingesting caller for a
shaper, would both be the wrong trade, so they are three features:

| feature | what it turns on | new crates, cumulative |
|---|---|---|
| `svg` | `pdfrum-svg`, export | **0** |
| `svg-ingest` | `Canvas::draw_svg`, `DocEdit::compile_svg` | **22** |
| `svg-text` | `<text>` laid out and drawn as outlines | **+10** |

All three are default-off. `svg-ingest` implies `edit`, because a canvas is a
drawing session over an editable document; `svg-text` implies `svg-ingest`.

The 22 is measured against a **default** `pdfrum`, which is the number a
caller actually pays; DEPS.md carries the crate list, the reproduction
command, and the correction — this table said **10** until 2026-09-07, on a
count of `usvg`'s own subtree that omitted the `png` decoder's six, two
crates the default tree turns out not to carry, and a `roxmltree` that
enters at a second version.

## 2. The mapping

| SVG / `usvg` node | PDF |
|---|---|
| `<svg>` root, placed by `SvgFit` | `q` … `Q` around the whole drawing, with a clip to the destination rect and one `cm` carrying the fit and the y flip |
| `Group` `transform` | `cm` inside a `q`/`Q` |
| `Group` `opacity` | `/ExtGState` with `/ca` and `/CA` |
| `Group` `clip-path` | the children's outlines concatenated, then `W n` (or `W* n`) |
| `Path` solid `fill` | `rg` + `f` / `f*` |
| `Path` solid `stroke` | `RG` + `w` + `S` |
| `stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit` | `J`, `j`, `M` |
| `stroke-dasharray`, `stroke-dashoffset` | `d` |
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

**Only one stroke value has no faithful spelling: `stroke-linejoin:
miter-clip`.** The other four cross whole — `stroke-linecap`,
`stroke-linejoin`'s miter, round and bevel, `stroke-miterlimit`,
`stroke-dasharray` and `stroke-dashoffset` become `J`, `j`, `M` and `d`
(ISO 32000-1 §8.4.3.3-§8.4.3.6), carried by `Stroke`'s `cap`, `join`,
`miter_limit` and `dash`. SVG 2's `miter-clip` clips the miter at the limit
where PDF bevels it, and `j` has no third spelling, so it lands on the miter
join it is a variant of rather than on a visibly blunter bevel. Unreported,
for the same reason group opacity is: the shape is there and is mitered, and
the difference shows only past the limit.

An all-zero `stroke-dasharray` is legal SVG and means solid, but the same
array through `d` is invalid PDF — a reader may reject the whole content
stream over it. `Dash::new` refuses it (along with a negative length, a
negative phase and a non-finite one), and the stroke stays solid, which is
what the SVG asked for.

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
is eighteen small hand-written documents, not multi-megabyte artifacts
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
| Text (`svg-text` only) | 0.99 | every file 1.0000 |

The Text class is the one whose floor is not set just under a worst file,
because there is no file below 1.0000 to set it under: all four documents
carrying `<text>` — the three `text_*` fixtures and `declined_text` — score a
flat **1.0000**, which is the highest agreement of any class in the corpus.
0.99 is chosen so that a face's hinting or a rasterizer's antialiasing
changing under us is a review rather than a red build, and that choice is
stated here rather than dressed up as a measurement.

The run, all eighteen fixtures:

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
| `stroke_pen` | 0.9999 | — |
| `opacity_groups` | 0.9669 | — |
| `gradient_linear` | 0.9991 | — |
| `gradient_radial` | 0.9992 | — |
| `image_png` | 0.9315 | — |
| `declined_filter` | 0.8280 | `filter` |
| `declined_text` | 1.0000 | `text` |
| `declined_pattern` | 0.5230 | `pattern` |
| `declined_mask` | 0.8229 | `mask` |
| `declined_blend` | 0.8543 | `blend-mode` |
| `text_basic` | 1.0000 | — |
| `text_anchored` | 1.0000 | — |
| `text_styled` | 1.0000 | — |

The three `text_*` rows are the `svg-text` run; without the feature they
report `text` and draw nothing, exactly as `declined_text` does. Both sides
are given the *same* committed face — `tests/fixtures/roboto.ttf`, already in
the repository for the font tests — and neither is allowed to reach the
host's installed fonts, so the score compares two layouts of one face rather
than two machines.

Eight of the thirteen carried fixtures score a flat **1.0000** against an
independent implementation — paths, fill rules, nested transforms, `use`
expansion, the CSS cascade and clipping are translations rather than
approximations, and the numbers say so. The five that are not fall to three
causes §4 explains: stroke joins and caps inked slightly differently, group
opacity composited differently, and a raster resampled by a different
filter.

**The five `declined_*` fixtures carry no floor** and their scores are not
evidence of quality — the two renderers are drawing different pictures by
design. They are scored only so the table is complete; what is asserted about
them is that each names its own construct in the report.

`declined_text` scoring 1.0000 **without** `svg-text` needs saying plainly:
it is not evidence that text renders. Both sides omit the text — we because
`usvg` dropped it (§6), `resvg` because it was given no font database — so
the two blank regions agree exactly. The assertion that matters on that
fixture in that build is the report item.

With `svg-text` the same 1.0000 means the opposite thing: both sides *draw*
the text, in the same face, and agree exactly. That two builds produce the
same number for opposite reasons is why the fixture's expectation is a
constant the feature selects rather than a literal in the table.

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

### `svg-text`: what closes it, and what was weighed

That was the state M25 shipped, and it was named debt. It is closed by a
third feature, `svg-text`, which turns `usvg/text` on — **and nothing else**.

Three ways to close it were on the table.

**(a) Enable `usvg/text` and take its layout.** Measured: **+10 crates** over
`svg-ingest`, one of which is a genuine duplicate — `usvg 0.47` pins
`fontdb 0.23`, this workspace pins `fontdb 0.24`, and 0.x versions do not
unify, so the build compiles fontdb twice. `rustybuzz` and `ttf-parser` come
too, a second shaper and a second font parser beside `skrifa`/`read-fonts`.
DEPS.md carries the list.

**(b) Resolve `<text>` ourselves through `pdfrum-font`.** This is the option
that sounds right — one font stack, our own — and it is the one that does not
survive being written down. `usvg` has already run the CSS cascade and thrown
the element away, so we would be starting from the source XML: a second XML
parse, a second cascade to recover the inherited `fill`, `transform` and font
properties, our own implementation of `x`/`y`/`dx`/`dy` lists, `tspan`,
`text-anchor`, `textLength`, `textPath` and bidi reordering, and a family
matcher `pdfrum-font` does not expose (its outline getters are `pub(crate)`,
reachable only through a PDF font dictionary, and it has no shaper at all).
That is **a second incomplete implementation of a large specification** —
precisely what §1 declines in writing about SVG parsing, for exactly the same
reason. Declined.

**(c) Amend the exit criterion and keep text reported forever.** Legitimate
if the cost of (a) were not worth paying. It is not the answer here, because
the cost is not what the original decision assumed it was.

**(a), with the objection taken seriously rather than dropped.** The original
refusal was of a *rival font stack* — a second source of faces, competing
with `pdfrum-font` over which font a document gets. `usvg/text` alone is not
that. `system-fonts` and `memmap-fonts` stay **off**, so nothing scans the
host and `fontdb`'s `fs`, `fontconfig` and `memmap` paths are never compiled.
What is left is a layout engine with an **empty** database, and the faces come
from the caller:

```rust
let mut fonts = SvgFonts::new();
fonts.register(std::fs::read("Inter.ttf")?);
edit.set_svg_fonts(fonts);
```

So `usvg` shapes text in faces *we handed it*. It is not choosing fonts; it
is doing the arithmetic. That is a layout dependency, which this project has
always been willing to take, rather than a font-source dependency, which it
is not — and it has a property the system-font path could not have had: a
document's appearance cannot depend on which fonts the build machine happens
to carry, so a reproducible save stays reproducible.

### What text becomes: outlines

`usvg::Text::flattened()` hands back the laid-out text as an ordinary
`Group` of filled `Path`s. So the mapping is one line — walk that group like
any other — and **nothing in §2's table changes**, because glyph outlines
*are* paths. The `Node::Text` arm that was unreachable is now the entry point.

Outlines rather than embedded text is the roadmap's own default and it is the
honest one: the page carries no font, has no encoding to get wrong, and
renders identically in every viewer. What it costs is **selectable text** — an
ingested `<text>` leaves nothing for `Page::text` to find, and
`ingested_text_is_outlines_rather_than_an_embedded_font` asserts exactly that
rather than leaving it to be discovered. The roadmap's "embedded fonts on
request" half is not this pass's: it needs the SVG's family mapped to a
`DocEdit::embed_font` program and `usvg`'s per-glyph positions replayed as
`Tj` runs through the *positioned* glyphs (`Text::layouted`), which is a
different piece of work from drawing the outlines it already computed.

### A missing face is still reported

The module's guarantee is that nothing is dropped silently, and text has two
ways to go missing. Both are reported:

| situation | what happens | how it is reported |
|---|---|---|
| `svg-text` off | no text stack; `<text>` leaves no tree node | source-XML scan, before the parse |
| `svg-text` on, **no face registered** | `usvg` with an empty database also drops the element, leaving no node | the same source-XML scan |
| `svg-text` on, face registered, family unknown | `usvg` falls back to the default family and **draws** it | not reported — nothing was lost |

The middle row is the one worth stating, because it was a defect first: an
empty font set makes `usvg` behave exactly as a `usvg` without the feature
does, so trusting the walk to notice would have made the loss silent. The
pre-parse scan therefore runs whenever the session has no face, not merely
whenever the feature is off, and `text_is_reported_when_no_face_is_registered`
holds it.

The third row is a measured surprise rather than a design choice. `usvg`
falls back to `Options::font_family` for a family it does not know, so with
*any* face registered no `<text>` is ever declined for a missing family — it
is set in the fallback face. `resvg` given the same database does the same
thing, which is why `declined_text` scores a flat 1.0000 with the feature on
and is a *carried* fixture there rather than a declined one.

## 7. `usvg`, and what it pulls

Twenty-two crates new to a **default** `pdfrum`'s normal tree, behind the
default-off `svg-ingest`:

```
arrayref  base64  bitflags  crc32fast  data-url  fdeflate  flate2
float-cmp  imagesize  memchr  miniz_oxide  pico-args  png  roxmltree
simd-adler32  simplecss  siphasher  strict-num  svgtypes  tiny-skia-path
usvg  xmlwriter
```

Fourteen behind `usvg`, six behind the `png` decoder, plus `tiny-skia-path`
and `strict-num`, which only the `tinyskia` backend feature would otherwise
bring. `svg-text` adds ten more on top; DEPS.md lists them and names the
`fontdb` duplicate.

All pure Rust; `scripts/ci.nu`'s pure-Rust check walks the whole workspace and
still passes, because none of them is a `-sys` crate and none has a native
build dependency. `kurbo 0.13.1` is the version the workspace already pins,
so no duplicate geometry crate enters the graph — but `roxmltree` does enter
at **0.21.1** beside the **0.20.0** that `fontdb 0.24` already resolves under
`system-fonts`, and that is recorded rather than left to be discovered.

`pico-args` and `xmlwriter` are non-optional dependencies of `usvg`'s
*library* despite serving its CLI and its writer; they are carried rather than
patched around.

This section said **ten** until 2026-09-07. The figure counted `usvg`'s own
subtree rather than the delta a caller pays against a default build; DEPS.md
carries the correction and the reproduction command.

## 8. The Form XObject: compile once, place many

The roadmap's M25 item 2 asks for a **Form XObject** — `/Subtype /Form` with
its own `/BBox` and `/Resources`, so one SVG placed on twenty pages is one
object and twenty `Do` calls rather than twenty copies. M25 shipped the
inline drawing and named this as debt; it is closed here.

Both spellings exist, because they answer different questions:

| you want | you call | what the file gets |
|---|---|---|
| this SVG, on this page | `Canvas::draw_svg` | the operators, inline in that page's content |
| this SVG, on many pages | `DocEdit::compile_svg` then `Canvas::place_svg` | one `/Subtype /Form` object, one `Do` per placement |

```rust
let (logo, report) = edit.compile_svg(svg)?;
edit.draw_pages(|c| c.place_svg(&logo, corner, SvgFit::Contain))?;
```

`draw_svg` is not deprecated by this and should not be: for a single
placement the two produce equivalent files, and the inline spelling needs no
value to carry between calls. The form is what a *repeated* placement wants.

### What is the same, and what is not

**The mapping in §2 is untouched.** A form's content stream holds the same
operators the inline path writes, which is not merely asserted:
`a_compiled_form_draws_what_the_inline_path_draws` scores every carried
fixture rendered through the form against the same fixture rendered inline,
and all twelve come back **1.0000** — gradients and the embedded raster
included. What changes is where the operators are written and how resources
are scoped.

**Resources are scoped to the form, not merged into a page.** A page canvas
chooses names against the page's own `/Resources` so a merged name cannot
collide; a form's `/Resources` starts empty and is its own, so the name
allocator has nothing to avoid. That is the structural reason a form is one
object: the content does not need to know which page it will land on.

**The fit is a placement property, not a compilation one.** The form's
`/BBox` is the SVG's own coordinate box, in a y-**up** space — the y flip is
composed once, inside the form's stream, exactly as `SvgFit::place` composes
it for the inline path. A placement is then a plain rectangle-to-rectangle
map, so the same compiled object can be `Contain`ed on one page and `Cover`ed
on another; `one_form_serves_two_different_fits` holds that, and it is what
makes the object genuinely reusable rather than merely shared.

### Proving the deduplication

The claim is about the **file**, so rendering correctly does not prove it —
the inline spelling renders correctly too. `tests/svg_form.rs` decodes every
stream in the saved document and counts the ones carrying the SVG's own path
operator: **two** under `draw_svg` on a two-page document, **one** under
`compile_svg`, with one `Do` per page either way. Decoding rather than
searching the raw bytes matters, because the save flate-compresses what it
writes and a byte search would find nothing and pass vacuously.

### The canvas gained a second surface

`Canvas` is no longer only a page's. It carries a private `Surface` — `Page`
or `Form` — and `Canvas::page()` therefore returns `Option<PageIndex>` rather
than a `PageIndex`: a form is being compiled for no page in particular, and
there is no honest index to hand back. That is the one API break this pass
makes, and an `Option` is the truthful spelling; a sentinel index would have
been a lie the type system could not catch.

A caller inside `draw_page` or `draw_pages` always gets `Some`, because the
form-compiling canvas is never handed to caller code — `compile_svg` drives
the walk itself.
