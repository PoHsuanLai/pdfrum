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
through the trait; only the two ends of `RasterBackend` — `new_target`,
`snapshot`, `finish` — insist on a `Pixmap`. M24 opens that end and writes
one vector consumer.

1. **A backend whose output is not pixels.** `RasterBackend`'s three
   pixmap-typed methods become generic in what a backend produces, or a
   sibling trait carries the vector case; whichever lands, the five existing
   backends compile unchanged and their behaviour is byte-identical. This is
   the only invasive part of M24 and it goes in its own commit, with the
   conformance board byte-identical across it.
2. **`crates/pdfrum-svg`.** A `RenderDevice` that accumulates SVG: paths as
   `<path>` with the fill rule, clips as `<clipPath>`, layers as `<g>` with
   `opacity` and `mix-blend-mode`, images as embedded data URIs, glyphs as
   filled outlines. Output is a string or a writer, not a file, so a caller
   composes it.
3. **What SVG cannot say, said plainly.** Non-isolated and knockout
   transparency groups (ISO 32000 §11.4.6) have no SVG equivalent; mesh
   shadings (types 4-7) have none either. Each such subtree is rendered to a
   pixmap by an ordinary raster backend and embedded as an image, and the
   crate reports which regions it had to rasterize so a caller knows what
   they got. A silent raster fallback is the failure mode to avoid.
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


## M26 — PDF/A: the checker first, the conversion second

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
