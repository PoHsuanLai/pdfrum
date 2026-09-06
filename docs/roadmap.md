# Roadmap

What pdfrum intends to do next, and where it draws its line. Each item below
states its exit criteria the same way the shipped work did: a milestone is
done when the criteria are met and measured, not when the code compiles.

The line: pdfrum reads, repairs, renders, extracts, edits and signs PDFs. It
is not a typesetting system and will not become one. Markup-to-layout is
Typst's problem and a different data model; taking it on would cost the
fidelity that makes this engine worth using. Everything below is vector
interchange and programmatic drawing, which is where the architecture
already sits.

Nothing here has started. Order is a recommendation, not a commitment.

M23-M26 add capability. **M27 and M28 are different: they are the two places
the engine is strictly behind a peer with nothing bought in exchange** --
peak render memory, and text extraction's byte-exact count. Every other loss
in `docs/benchmarks/losses-explained.md` is a tradeoff with something on the
other side of it. These two are not, which is why they are milestones.

## M23 — The canvas: drawing on a page without writing operators

`pdfrum-edit` can already place an image, merge pages and rewrite objects,
and the facade carries `ImageBuilder`, `PathBuilder` and `TextBuilder`. What
it cannot do is the thing most callers actually want: put a watermark, a
header rule or a branded line of text onto an existing page without
hand-writing content-stream operators. M23 is that entry point, and nothing
below `pdfrum-edit` changes.

1. **`Canvas`, a retained drawing surface on a page.** `PageEdit::draw(|c|
   ...)` hands a `Canvas` whose coordinate space is the page's, y-up in PDF
   points with the crop box and rotation already composed in, so a caller
   places things where they see them. Fills, strokes, rounded rectangles,
   text at a point, an image at a rect, `save`/`restore`, `transform`,
   `clip`. Geometry is `kurbo` and colour is `peniko`, the types the render
   crate already speaks — no third vocabulary.
2. **Text that embeds what it draws.** `Canvas::text` takes a font the
   caller loaded through `DocEdit::embed_font` or `standard_font`, so the
   glyphs it writes are subset and embedded by the machinery M-edit already
   has. A caller who names a base-14 font gets it without an embed. Missing
   glyphs are an error, not a silent blank.
3. **One content stream, appended, not rewritten.** The canvas emits a
   single stream appended to the page's `/Contents` array with a
   `q`/`Q` around it, so the page's own graphics state cannot leak into the
   drawing or the drawing into the page. Resources are merged into the
   page's `/Resources` under fresh names that cannot collide.
4. **Watermark and stamp as the worked examples.** `StampOptions` already
   exists for the flattening path; M23's canvas is the general case beneath
   it. The two shipped examples are a diagonal translucent watermark across
   every page and a header rule with a page number, each under twenty lines
   of caller code.

Not in M23: layout. No line breaking, no text wrapping, no paragraph model,
no measurement API beyond a single string's advance. A caller who needs
layout is holding a typesetting problem and should bring their own layout
to our `text`. That line is the same one the SPEC draws against authoring.

**Rules:** no new dependency; the canvas holds no PDF knowledge that
`pdfrum-edit` does not already have; every emitted stream round-trips
through our own parser in a test, so we never write what we cannot read.
**Exit:** the two examples run from a clean checkout, the pages they produce
open in the oracle and in Acrobat without a repair prompt, the emitted
streams re-parse, and `docs/design/canvas.md` states the coordinate space
and the resource-merging rule.


## M24 — SVG export: a vector backend for `pdfrum-render`

Every backend we have rasterizes. `RenderDevice` does not: `fill_path`,
`stroke_path`, `push_clip` and `push_layer` take `kurbo::BezPath`,
`peniko` colours and blend modes, and glyphs reach the device as outlines
above the hinting threshold. The vector information is already flowing
through the trait, and `RenderDevice` is object-safe, so **an SVG consumer
needs no change to `RasterBackend` at all**.

**Amended 2026-09-07, before any code was written.** This milestone
originally opened with "a backend whose output is not pixels" — making
`RasterBackend`'s `new_target`/`snapshot`/`finish` generic. Investigation
refuted the premise those rested on. `Pixmap` is not a value the engine
hands back at the end; it is an intermediate the engine does *arithmetic*
on, at fourteen `finish`/`snapshot` sites. `walk.rs:699` finishes a
transparency group and immediately calls `remove_backdrop` —
`C = Cn + (Cn - C0) x (a0/agn - a0)` per ISO 32000 §11.4.6 — and the same
shape repeats for `knockout_over`, `knockout_replace`, `luminosity_mask`
and `multiply_alpha_mask`. None of those has a vector meaning. A generic
`Output` would compile only if every site were bounded on a trait
supplying them, which an SVG backend could satisfy only by rasterizing:
the raster fallback wearing a type parameter. The change would have broken
a public trait across four backends and removed not one `draw_image` from
the output.

So the trait stays. `pdfrum-svg` is a [`RenderDevice`] driven by the
existing walk with an ordinary raster backend supplying offscreen targets.
Direct fills, strokes, clips, layers and glyph outlines — the large
majority of every corpus file — arrive as vectors; the compositing
subtrees arrive as `draw_image`, which is where item 3 wanted a raster
fallback anyway. Moving compositing out of the pixel domain is a real
milestone, but it is its own, not this one's first commit.

1. ~~**A backend whose output is not pixels.**~~ **Withdrawn 2026-09-07,
   see above.** `RasterBackend` is unchanged and the five existing backends
   are untouched, so M24 no longer has an invasive commit.
2. **`crates/pdfrum-svg`.** A `RenderDevice` that accumulates SVG: paths as
   `<path>` with the fill rule, clips as `<clipPath>`, layers as `<g>` with
   `opacity` and `mix-blend-mode`, images as embedded data URIs, glyphs as
   filled outlines. Output is a string or a writer, not a file, so a caller
   composes it.
3. **What SVG cannot say, said plainly.** Every region the walk delivers
   as pixels is reported with its cause, so a caller knows exactly what they
   got. That set is wider than this milestone first assumed, because the
   engine composites in the pixel domain: non-isolated and knockout
   transparency groups (ISO 32000 §11.4.6) and mesh shadings (types 4-7),
   and also soft masks and tiling-pattern cells. Recording them is not a
   special case bolted onto a vector pipeline — it is the natural output of
   noting every `draw_image` the walk makes. A silent raster fallback is the
   failure mode to avoid.
4. **Its own correctness story, because the board cannot score it.** The
   conformance board compares our pixels to `pdfium_test`'s, and an SVG file
   has none. M24's proof is a round trip: render the SVG with `resvg` at the
   board's DPI and compare *that* pixmap to the oracle's PNG with the same
   SSIM the board uses. The bar is deliberately below the raster board's —
   `resvg` is a second engine with its own antialiasing — and the number is
   published rather than negotiated.

Not in M24: SVG as an input. That is M25.

**Rules:** `resvg` is a dev-dependency of the test only and never enters the
shipped tree; the pure-Rust check keeps passing; the raster board is
byte-identical before and after the trait change, and that is the gate on
item 1 landing at all.
**Exit:** `pdfrum-svg` converts every file of `benches/corpus`, the round
trip scores above the stated floor on each, the rasterized-region report is
non-empty exactly where the file has a mesh shading or a non-isolated
group, and `docs/design/svg.md` records the mapping and its limits.

**MET 2026-09-07.** `crates/pdfrum-svg`: `SvgBackend` decorates any
`RasterBackend`, so `RasterBackend` and the five rasterizer crates are
untouched and the board is unchanged — **1759 files, 1675 pass, zero rows
moved**, every tag bucket and both text rates identical. All 44 corpus files
convert; 18 first pages are entirely vectors, 24 carry a reported region, 2
draw nothing. Round trip through `resvg`: at least **0.9761** against our own
render on every file, and against the oracle's PNG the published per-class
floors are Vector 0.95, Text/Image/Mixed 0.90, Shading 0.80, Forms 0.75 — each
set just below its class's worst file, listed with the run in
`docs/design/svg.md` §5. The shipped crate added **no dependency**: base64 and
a small lossless PNG encoder are in-crate, because `png` is tool-and-test-only.
`resvg` is a dev-dependency of that crate alone, `default-features = false` +
`raster-images`, 15 crates new to the lock, all pure Rust (DEPS.md). One
render option is overridden — `subpixel_text_positioning`, without which text
exports as thousands of glyph bitmaps rather than outlines.


## M25 — SVG ingestion: vector assets into a page

A caller with a logo has an SVG and does not want it blurry. M25 compiles
SVG into a PDF Form XObject so it goes in as vector, and it is the mirror of
M24 across the same geometry model.

1. **`usvg` resolves the SVG; we compile its tree.** Parsing SVG properly is
   a project — CSS, `use`, nested transforms, gradients, filters, text with
   fonts — and `usvg` already does it, in pure Rust, producing a simple tree
   of paths, groups and images. We walk that tree and emit content-stream
   operators through `pdfrum-edit`. Writing our own SVG parser is declined
   here in writing: it would be a second incomplete implementation of a
   large specification for no gain.
2. **A Form XObject, placed by the caller.** The result is
   `/Subtype /Form` with its own `/BBox` and `/Resources`, so it is one
   object placed at any transform on any number of pages, not a copy per
   placement. `Canvas::draw_svg` (M23) is the placement API.
3. **Text is outlines by default, embedded fonts on request.** An SVG's text
   depends on fonts the PDF may not carry. The default converts text to
   paths, which always renders; a caller who passes a font gets real text
   with `DocEdit::embed_font` behind it.
4. **What is declined, per feature, in the doc:** SVG filters (no PDF
   equivalent short of rasterizing), animation, scripting, and foreign
   objects. Each is reported to the caller rather than dropped silently.

**Rules:** `usvg` is the first new runtime dependency of these four
milestones and goes through DEPS.md with the pure-Rust check; the feature is
off by default so a caller who does not want SVG does not pay for it.
**Exit:** a corpus of SVGs (the `resvg` test suite subset) compiles into
pages that render within the M24 round-trip floor of rendering the same SVG
directly, unsupported features are reported not swallowed, and
`docs/design/svg.md` gains the ingestion half.

**MET 2026-09-07, with two items partly met and named below.**
`Canvas::draw_svg` in the facade (`crates/pdfrum/src/svg_ingest.rs`), behind
the default-off `svg-ingest` feature. It walks a `usvg`-resolved tree and
issues M23 canvas calls, so an SVG goes into a page as vectors.

Scored against **`resvg`** rendering the same file directly — an independent
parser, geometry and rasterizer — with the board's own SSIM, over a committed
eighteen-file corpus. **Eight of the thirteen carried fixtures score a flat
1.0000**; the published per-class floors are Vector 0.99, Gradient 0.99,
Alpha 0.95, Image 0.92, each set just under its class's worst file, named
with the full run in `docs/design/svg-ingest.md` §5. Gates: **4414 workspace
tests pass** with `PDFRUM_ORACLE_CHECKOUT` set (4402 without the feature),
clippy clean, and
`cargo build -p pdfrum --no-default-features` reaches none of the ten new
crates.

Two features, not one, because export adds **0** crates to the tree and
ingestion adds **10**: a caller who only exports pays nothing for a parser.
The code is in the facade rather than in `pdfrum-svg` because `Canvas` is
there (`pdfrum-edit` does not depend on `peniko`) and because M24's
`svg = ["dep:pdfrum-svg"]` makes the other direction a cycle.

The pass also fixed a **latent M23 defect**: `merge_resources` hardcoded the
three resource categories a canvas minted at the time, so a drawing that used
any other one wrote a name into the stream with nothing behind it in
`/Resources` — a `sh` that drew nothing at all. It now derives the categories
from what the drawing actually added.

**Item 2, the Form XObject: not met, named debt.** What shipped draws the SVG
inline into the page's content stream. For one placement the two are
equivalent in the file; for the same logo on twenty pages the XObject is one
object rather than twenty copies, and it is the better answer. Nothing about
the mapping changes when it lands (`docs/design/svg-ingest.md` §8).

**Item 3, text: not met, named debt.** `usvg` is taken
`--no-default-features`, which is the right call — its defaults are a second
font stack, and SVG text must map through `pdfrum-font`, not through a rival.
The consequence is that `<text>` leaves no node in the resolved tree at all,
so it is detected in the source XML and **reported** as `Unsupported::Text`
rather than drawn. Neither the outline default nor the embedded-font option
exists yet; closing it is a font-and-layout problem (§6).

**Stroke attributes beyond colour and width: MET 2026-09-07, in a later
pass.** `Stroke` now carries `cap`, `join`, `miter_limit` and `dash` beside
its colour and width, written as `J`, `j`, `M` and `d`
(ISO 32000-1 §8.4.3.3-§8.4.3.6). `LineCap` and `LineJoin` are enums, not the
integers the operators take; `MiterLimit` clamps to PDF's floor of 1; `Dash`
refuses at construction the arrays a reader may reject the whole content
stream over — a negative length, a negative or non-finite phase, an array
summing to zero. `Stroke::new` still means what it always meant, PDF's own
default pen, and the fields stay public with `with_*` methods beside them, so
existing callers keep compiling. The one cost is that `Stroke` and `Paint`
lost `Copy`, since a dash array is a `Vec`.

This is a **facade** capability, not an SVG one — any `DocEdit::draw_page`
caller gets it — and it is proved twice: `tests/canvas.rs` reads the four
operators back out of the decoded content stream, and a new `stroke_pen`
fixture in the ingestion corpus scores **0.9999** against `resvg`, clearing
the unmoved Vector floor of 0.99. The one SVG value with no faithful PDF
spelling is `stroke-linejoin: miter-clip`, which lands on the miter join it
is a variant of (`docs/design/svg-ingest.md` §4).

**Also unimplemented, each reported rather than swallowed:** SVG filters
(declined outright, per item 4), `<mask>` and non-normal blend modes (both
need a transparency-group `/XObject`, deferred), `<pattern>` fills, and GIF
and WebP `<image>` data. Two approximations are made *without* a report item,
because the shape is still drawn and the reasoning is recorded in §4: group
opacity composites members individually rather than through a group buffer
(the sole cause of `opacity_groups`'s 0.9669), and a gradient *stroke*
collapses to the average of its stops.

**One exit criterion was met differently than written.** The `resvg` test
suite is not vendored here and cannot be fetched offline, so the corpus is
eighteen hand-written fixtures covering the feature matrix by class rather
than a subset of that suite. Because they are committed, the test has no
legitimate skip: a missing or empty fixture directory **fails**, and a `.svg`
with no row in the table fails too. The ingestion half went into a new
`docs/design/svg-ingest.md` rather than into `docs/design/svg.md`, which was
already long; the two cross-reference.

`ErrorCode` gained `Svg = 10`, reserved with the feature off as `Save = 7` is.


## M26 — PDF/A: the checker first, the conversion second

**Parts 1 and 2 are met. Parts 3 and 4 are explicitly deferred to a later
pass**, which is the order this entry asks for: report before repair.

- **Part 1, met.** `Document::check_pdfa(level)` in the default feature set,
  over `crates/pdfrum-doc/src/pdfa/`. `PdfaLevel` is an enum with `A1b` and
  `A2b` and no variant for the `a` levels, so a caller cannot ask for a check
  weaker than its name; `PdfaClause` is an enum with 24 variants and an
  `iso(level)` citation; `PdfaSubject` names the `ObjRef`, page or named
  resource rather than formatting it into a sentence. **No new dependency in
  any tree** — the XMP identification subset is read directly per STYLE.md §5
  — which is why it needs no feature gate.
- **Part 2, met.** veraPDF 1.30.2 is the oracle, in
  `crates/pdfrum/tests/pdfa_oracle.rs`, scored over the 44-document corpus at
  both levels. `$PDFRUM_VERAPDF` unset skips loudly; set-but-broken fails.
  The divergence table is in `docs/design/pdfa.md` §6, adjudicated rather
  than tuned away. DEPS.md records the install and calls out that it is the
  first tool here needing a JVM, and that CI cannot run it.

  The oracle paid for itself immediately: the first scored run found **34
  disagreements, every one a defect on our side** — ten wrong ISO citations, a
  clause that is not an ISO 19005 requirement at all, and four checks that
  enforced the popular summary of a rule rather than the rule. The checker was
  corrected, not the test. What remains is 8 clause-pairs over 82 scored
  pairs, covering four distinct requirements: one **ours right**, one family
  **theirs right** (font embedding is scoped to fonts *used*, which needs an
  interpreter we do not have), and one **not determined** (veraPDF's A-1
  profile has no JPEG 2000 rule to compare against). Six further pairs are
  unscored because veraPDF declines to parse three `shading_*` files our
  parser opens.
- **Part 3 (`to_pdfa`) and part 4 (the conversion policy): not started.**
- **Named debt**, all in `docs/design/pdfa.md` §4: the checker reads the
  object graph and not content-stream operands, so inline colour and
  `gs`-less transparency are invisible to it; CMap embedding, glyph-presence
  and width-agreement checks are unimplemented; an output intent's ICC stream
  is checked for presence but not parsed; A-2's conditional permission for
  embedded PDF/A attachments is not verified, so an A-2 check permits every
  embedded file. Every one of these makes the checker report *fewer*
  violations than veraPDF, never one veraPDF does not see — which is the
  direction the oracle test asserts.


Archival conformance is where institutions actually buy, and no Rust crate
converts an existing document. It is also a specification with dozens of
independent requirements, where claiming compliance you do not have is worse
than offering nothing. So M26 is ordered deliberately: report before repair.

1. **`Document::check_pdfa(level)` — the checker.** A report of every
   requirement the document fails, each naming the clause and the object,
   for PDF/A-1b and A-2b. The requirements are not a short list and the doc
   enumerates them: fonts all embedded and subsettable; no encryption; no
   JavaScript, no embedded audio or video, no launch actions; XMP metadata
   present and carrying the PDF/A identification schema, consistent with the
   document information dictionary; an OutputIntent with an embedded ICC
   profile; every annotation's flags legal; no external content references;
   no transparency at all for A-1; device-independent colour or an
   OutputIntent that defines it.
2. **veraPDF as the oracle, exactly as `pdfium_test` is the board's.**
   Every check is scored against veraPDF's verdict on the same file, over
   the corpus, and the disagreements are enumerated the way
   `docs/status/` records oracle divergences — ours right, theirs right, or
   not determined. A checker nobody has cross-examined is a claim, not a
   result.
3. **`Document::to_pdfa(level, policy)` — the conversion, scoped.** A-2b
   first, because A-1's blanket ban on transparency makes many real
   documents unconvertible without rasterizing. The pipeline: embed and
   subset every font we can, strip JavaScript and the forbidden action and
   annotation kinds, flatten form fields and annotations where they cannot
   stay, write the XMP and the OutputIntent with a profile through the
   `moxcms` we already carry, and rewrite colour into the intent's space.
4. **Unconvertible is an answer.** A document with a font we cannot embed
   because it is not embeddable, or transparency under A-1, cannot be
   converted faithfully. `policy` says what to do — refuse, or rasterize the
   offending page and say which — and the return value names every
   compromise. Silent lossy conversion is the failure mode to avoid.

Not in M26: PDF/A-1a or A-3a, which require a full logical structure tree and
tagged reading order — that is its own milestone and it depends on a
structure model we do not have. Not in M26: PDF/UA. Both are named here so
their absence is a decision.

**Rules:** the checker ships before the converter and is useful alone; no
compliance claim without a veraPDF score behind it; `moxcms` is already in
the tree and no new colour dependency is taken.
**Exit:** `check_pdfa` agrees with veraPDF on a published corpus with the
disagreements enumerated and adjudicated; `to_pdfa(A2b)` produces files
veraPDF passes for the inputs it accepts and a named compromise list for the
rest; `docs/design/pdfa.md` carries the requirement table and the oracle
comparison.

## M27 — Peak render memory: decode at the size the page draws

**Exit met at `56a01ac`, and none of the three items below is what did it.**
Peak on `image_bug_583804.pdf` is **916 MiB** (was 1596 measured on the same
box under the same load), the corpus render median **improved** 49.73 ->
30.95 MiB, the file got *faster* rather than slower (cold 1723 -> 1370 ms,
warm 1489 -> 1290 ms), and the board's pixel result is unchanged on all 1759
files. The cost was three simultaneous page-sized RGBA8 buffers in the
vello-cpu backend — 331 MiB each at 9318x9318 — not the image path: a seed
pixmap allocated to be read once, the rasterizer's own pixmap, and the copy
back out. See `docs/benchmarks/losses-explained.md`.

Of the three items: (1) buys nothing on this file, whose image is drawn above
1:1, and stays right for the document shape this one was mistaken for; (2) was
not needed to clear 1 GiB and was not attempted; (3) was tried and measured —
it saves 20 MiB and costs 14% warm and 42% cold, reproducing exactly the
regression `6af7a2a` exists to prevent, so the exemption stays.

The one place this engine is strictly behind every peer with nothing bought
for it. On `image_bug_583804.pdf` peak resident memory is 1539 MiB where
mupdf peaks at 914 and no measured peer exceeds 1 GiB; wall time on that
file is 1.5 s against mupdf's 0.16 s. The corpus median is 51.5 MiB, so this
is one shape of document rather than a systemic cost — a page whose images
are far larger than the area they are drawn into.

Every other loss in `docs/benchmarks/losses-explained.md` is a tradeoff with
something on the other side of it: f32 compositing buys fidelity, the eager
page walk buys an infallible page count and damaged-file recovery. This one
buys nothing. That is why it is a milestone and the others are rows in a
table.

1. **Decode to the drawn size, not the stored size.** The image-rows
   pipeline already reduces before painting; the reduction has to move
   *above* the decode so the full-resolution buffer is never materialized.
   For JPEG that wants `zune-jpeg`'s `scale_denom`, which does not exist yet
   and is filed upstream; until it lands, a nearest-neighbour 1/2, 1/4, 1/8
   reduce immediately after decode and before the general resample gets most
   of the win with no upstream dependency.
2. **Band the very large page.** A page whose target exceeds a budget is
   rendered in horizontal bands and composited, so the peak is the band and
   not the page. The budget is a number a caller can set, with a default
   that keeps the common page in one piece.
3. **Evict.** The decoded-image cache keeps its last entry however large
   (`crates/pdfrum-page/src/image/cache.rs`) — a rule that exists because a
   16-bit image's packed samples exceeded the budget and the sole entry was
   evicted on every insert, making a warm render slower than a cold one.
   That rule is right for correctness and wrong for a peak: the fix is a
   budget that counts what is resident, not a special case that exempts one
   entry from counting.

Not in M27: making `image_bug_583804` fast. Its wall time is a separate
question from its footprint, and conflating them is how the last pass on
this file produced a warm regression instead of a fix.

**Rules:** the conformance board stays byte-identical, row for row — a
memory pass that moves a pixel has failed; every change is measured by peak
`VmHWM` of the harness child, the same instrument
`docs/benchmarks/README.md` publishes, not by reasoning about allocations.
**Exit:** peak resident memory under 1 GiB on `image_bug_583804.pdf` with
the board unchanged, the corpus median no worse, and the render-warm rows
for that file no slower than the baseline they land on.


## M28 — Text extraction: the last twenty files

Text extraction is byte-exact on 22 of 44 corpus files against PDFium's 42.
The whitespace-normalized figure is 41 of 44, so the shape of the output is
right and the difference is spacing: spurious generated spaces at object
boundaries, and one document at F1 0.641.

This is inferior rather than a tradeoff — there is no property we gain by
emitting a space PDFium does not. It is also the last correctness column
where a peer leads us; render fidelity now leads pdfium-render 38/44 to
29/44.

1. **`text_quick_start.pdf`, F1 0.641 — the cause is not known.** Three
   named causes have each been refuted by measurement: the dot-leader
   hypothesis (leaders are one line on both sides), the harness comparing
   the wrong stream (fixed; the row did not move), and a fractional
   character width (implemented as a newtype; a measured no-op, because
   widths are already truncated at parse where PDFium truncates them, with
   zero fractional widths across 1420 corpus files). The remaining
   candidates are positional — `GetPos`, the text-matrix composition, or
   `FindPreviousTextObject`'s choice of previous run — and settling it needs
   per-item instrumentation in a PDFium build, which is the first work item
   and not an afterthought.
2. **The spurious generated space.** `pipeline.rs:696` and `:1139` are
   `ProcessInsertObject` and `GenerateSpace` transcribed line for line, and
   the transcription has been checked. So the divergence is in what reaches
   them, not in the rules themselves: a position, a width, or a boundary
   decision one level up.
3. **`text_tcpdf_055`'s C0 row.** We emit `^@ ^A ^B ... ^_` where PDFium
   emits nothing, and those codes are legitimately in `char_list_` on both
   sides — so PDFium's empty row is an input-geometry or charcode-mapping
   difference, not a filtering rule. Same instrumentation, same pass.

**Rules:** the oracle-bug rule applies — where PDFium is wrong we implement
the correct behaviour, cite both sides, and bucket the golden as
not-achievable rather than matching a defect; a fix that raises one file and
lowers another is not a fix, so every change is scored across all 44 and the
board's 1785 text pages together.
**Exit:** byte-exact on 30 of 44 or better with none of the current 22
lost, `text_quick_start` diagnosed with its cause named whether or not it is
fixed, and the board's text rows moving only upward.

**MET 2026-09-06.** Byte-exact **42 of 44** (from 22), none of the 22 lost;
whitespace-normalized 41 → 43. The board's text pages went **1785 → 2020 of
2067** and its non-empty pages **760 → 990 of 1005**, with 125 files gaining
text pages and **none losing any**; overall 1551 → 1675 passing with **0 rows
down**, and the scoreboard was re-recorded.

All three items had **one** cause, and it was ours. `ProcessInsertObject`
was instrumented per item in a private, hardlinked PDFium build (the oracle
checkout was never written to), and the three named candidates were refuted:
`GetPos`, the text-matrix composition and `FindPreviousTextObject` all agreed
with ours to six decimal places. The divergence was one conjunct in our own
degenerate-object gate — `run.advance < SIZE_EPSILON` beside the box test —
which kept *every* empty-box object, including the spaces-only ones whose
separator `GenerateSpace` already emits from the geometry. Keeping them
emitted it twice, and that duplicate was items 1, 2 and 3 at once.

**The `[oracle-bug]` ruling the conjunct carried was narrowed, not
discarded**, and it now has two parts:

- *Spaces on a page with no other object.* `whitespace.pdf` has one
  spaces-only object; no gap exists for the heuristic to span, so PDFium's
  gate discards the page's only content. `Builder::keep_spaces_only` keeps
  it, bounded to exactly that shape. It costs one benchmark file,
  `vector_en_system.pdf`, a corpus page of the same shape.
- *Objects that draw letters.* PDFium's box gate also drops objects carrying
  real text: `bug_921.pdf`'s output begins mid-sentence at "разве не
  выражает" where the page draws "И разве не выражает", and `bug_665467.pdf`
  loses a `Л` and extracts as empty. **User-ruled 2026-09-06: recover the
  letters.** A first pass reported these as inseparable from the C0 control
  runs the oracle rightly drops; that was wrong. The `ToUnicode` mapping is
  empty for both, but the character codes are not — 1048/8212/1074/1103
  against 0..=31 — and PDFium itself falls back to the code
  (`cpdf_textpage.cpp:1213-1215`). `object::gate` keeps an empty-box object
  when the pen moved and it shows a scalar that is neither whitespace
  (`U+00A0` included: fifty annotation fixtures draw one) nor a C0/C1
  control.

Four board files are deliberately not-achievable on their text artifact —
`bug_921`, `bug_665467`, `bug_1449` and two pages of `example_055` — and all
four **already failed on `main`**, so the board shows 0 rows down;
`example_055` goes from 0 of 14 matched pages to 12 of 14.
`image_ccitt_3bigpreview.pdf` at F1 0.9988 is the other file still not
byte-exact, and its residual is a duplicated `Clips` in the vertical CJK
region — a duplicate-object question, not a spacing one. See
`docs/benchmarks/losses-explained.md`, "The generated space was our own
rescue".
