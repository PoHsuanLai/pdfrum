# `pdfrum-render` status

**Updated:** 2026-08-29 · **State:** M8 pixel burn-down wave 4 —
**76.6% at SSIM ≥ 0.99** (unchanged; wave 4 was a measurement wave and
shipped no pixels), D7 re-attributed from hinting to the oracle's integer
glyph origins, and the hinter ruled out on numbers

Contract: SPEC.md §8 including the E2/E3/E4/E10 additions and the
2026-08-29 implementation record. Behavior: `docs/design/pdfrum-render.md`;
backend rules: `docs/design/backend-verification.md`, binding.

## What landed

Three crates: the engine, and one implementation of its two traits per
rasterizer.

| module | contents |
|---|---|
| `device.rs` | the `RenderDevice` and `RasterBackend` traits, `Brush`, `AntiAlias`, `FillRule`, `ImageQuality`, the `peniko::Mix` table, `MAX_TARGET_DIMENSION` |
| `pixmap.rs` | `Pixmap` (premultiplied RGBA8) and `AlphaMask`; the straight/premultiplied conversions, the truncating and rounding alpha bytes, `AlphaMerge`, the BGRA and RGB output encodings |
| `color.rs` | `resolve_argb`, `ColorRef`'s three states and the `0xFFFFFFFF` invisibility *word*, the truncating alpha, colour-mode translation, `FXRGB2GRAY` |
| `transfer.rs` | the `/TR` channel mapping, and Q1's resolution |
| `options.rs` | `RenderOptions`, `ColorMode`, `TextAa`, the oracle-conformance defaults, the type-3 and tile-cell option overrides |
| `path.rs` | `IsAvailableMatrix`, `CFX_Path::GetRect` with its normalisation, the rect-snapping ladder, `outer_rect`, the ±32000 clamp |
| `zero_area.rs` | `CheckSimpleLinePath`, `CheckPalindromicPath`, the folding-vertex scan, the `(int)c + 0.5` snap, the `>> 2` alpha |
| `stroke.rs` | the matrix1/matrix2 split, the one-device-pixel minimum, cap/join/miter mapping, the dash normalisation ladder, the stroke-to-fill outline a pattern clip needs |
| `paint.rs` | the five `DrawPath` fast paths in order, including the fill+stroke knockout buffer |
| `clip.rs` | the clip stack's reduction to device calls, the off-canvas empty-clip rect, text-clip unions |
| `blend.rs` | the twelve separable formulas, the verbatim `kColorSqrt`, `Lum`/`Sat`/`SetLum`/`SetSat`/`ClipColor`, the straight-alpha composite |
| `shading/` | all seven rasterizers, the 256-entry ramp, `component_to_shading_index`, the Coons subdivider |
| `group.rs` | the offscreen predicate, the backdrop rule, the mask/alpha ordering, the composite transparency |
| `softmask.rs` | the `/BC` backdrop, the luminosity and alpha readbacks, the `/TR` lookup |
| `pattern.rs` | the tiling and shading pattern draws, `ClipPattern`'s two shapes, the sub-sixteen-pixel cell enlargement, the tile screen buffer |
| `text.rs` | the `Tr` mode table, the glyph matrix, the stroked-text CTM split, glyph placement, type-3 character placement |
| `image.rs` | `UseInterpolateBilinear`, the `kHugeImageSize` rule, the CMYK-overprint gate, the flip rule, sample-to-pixmap, the `/Matte` un-premultiply |
| `ctx.rs` | `RenderCtx`, the depth cap, the type-3 font *set*, `RenderCaches` |
| `walk.rs` | `render_page`, the object dispatch, the cull test, the page matrix, the group path, the soft-mask group, the type-3 char-proc walk, `DrawPatternImage` |
| `pdfrum-raster-tinyskia` | the emulated layer and clip stacks, the conversions, the `1/4096` note |
| `pdfrum-raster-vello` | the pinned `Level` and `RenderMode`, the opacity-layer wrapper for `draw_image`, the two-stack `pop` |

### The behaviors that mattered most

- **An axis-aligned rectangle is never antialiased.** It snaps to the outer
  integer rect, promotes a sub-pixel extent to one pixel, and then shrinks
  whichever side lies further from the true edge — with the strict `>`
  sending a tie to the right or bottom. This is a large share of the corpus's
  pixel-exact rectangles, and getting the tie wrong moves an edge on all of
  them.
- **A zero-area fill becomes a quarter-alpha hairline.** Three detectors —
  a there-and-back line, a palindrome, a folding vertex — each emitting a
  one-device-pixel stroke at `alpha >> 2`. `text_mode` excludes glyph
  outlines, which is why stems never turn into hairlines.
- **The minimum stroke width is one device pixel**, so `w 0` is a hairline
  and neither rasterizer's own `width == 0` path is ever reached.
- **`kColorSqrt` is hand-tuned.** It tracks ISO 32000-1's piecewise `D(x)`
  but is not any closed form of it: 35 entries differ from `round(255·D)` and
  102 from the truncating spelling. The transcription is the authority.
- **The `/TR` array is reversed**: `array[2]` drives red and `array[0]` blue.
  The oracle's unit test appears to say otherwise only because two of its
  expectation constants are misnamed — see the Q1 resolution below.
- **The offscreen predicate's absences matter.** A non-isolated group with
  `/Group` present, `/I` absent, `ca` at one and no soft mask draws
  *directly*, with no group semantics at all.

## Wave 3: what the D7 tail actually is

Wave 2 left 159 files in the 0.95–0.99 band carrying no image, shading,
pattern or soft mask, and called them "D7 in its pure form". Wave 3 opened by
trying to *disprove* that, on the hypothesis that PDFium's AGG driver applies
a coverage→alpha mapping — a gamma table or a cover scale — that our linear
backends do not. **It does not.** The hypothesis is dead, and what replaced it
is worth more than the two clusters it cost.

### The coverage mapping, measured

A shallow-slope fill rendered at 64×64 through the oracle gives the edge ramp
`223 159 95 31` on grays whose true coverages are exactly ⅛, ⅜, ⅝, ⅞. That is

```
alpha = min(255, floor(coverage · 256))
```

— a **×256 scale clamped at the top**, not ×255, and truncating rather than
rounding. It comes straight from `calculate_alpha`
(agg_rasterizer_scanline_aa.h:283-297): `cover = area >> (poly_base_shift·2 +
1 - 8)` yields 0..256 over `aa_mask = 255`, and the clamp is the only thing
keeping 256 out. There is **no gamma table anywhere on the path side**, which
is what the brief's §1.7 already said and what `grep gamma`
over `cfx_agg_devicedriver.cpp` confirms: not one hit.

The engine's own edge pixels already agree with this to within a count, so the
mapping was never the tail. Chasing it was still worth the hour, because the
answer is now measured and the next wave does not have to ask again.

### The tail is two populations, and only one of them is D7

Splitting the band by what is actually on the page:

- **Path and stroke edges** were *not* matching, for two reasons that are
  each a real defect and neither of which is a coverage curve. Both are fixed
  below.
- **Glyph edges** are D7, and D7 is exactly as unfixable as the brief says.
  On `5.5_simple_font.pdf` the ink bounding boxes agree within a pixel and the
  total ink within 2.5% — the geometry is right — while 22 013 pixels differ
  with a mean magnitude of 66 counts, in ±255 swings rather than a bias.
  Rendering the oracle with `--no-smoothtext` does not move it closer
  (14 584 differing pixels at a *worse* mean of 118), which settles that the
  difference is the hinted FreeType bitmap's **geometry**, not a filter or a
  gamma on top of our outline. Nothing short of the hinter closes it, and
  PLAN §1 rules the hinter out.

  **Wave 4 disproved that last sentence.** The geometry finding holds; the
  attribution to hinting does not. The oracle's hinting runs at a fixed
  64 ppem and moves outlines by ~0.04 device px; what actually displaces the
  stems is that the oracle blits glyph bitmaps on **integer origins**. See
  the wave 4 section.

**The brief's tail inventory was wrong about the tcpdf cluster**, and the
correction matters because it was the largest named secondary target. Those
~20 files were recorded as "DCT images at a sub-pixel placement offset". They
are not. Over `example_030.pdf`'s image region — 48 400 pixels — **not one
pixel differs by more than two counts**; the file's loss is its text, and
`example_063.pdf`, the worst of them at 0.648, is dense small type showing the
D7 signature exactly (oracle 0 where we write 16 or 82, oracle 252/253 where
we write 255). The tcpdf cluster is the text tail wearing a different hat.

### `vello_cpu` is closer to AGG on paths, and that is not a reason to switch

Measured, because the option had to be ruled out rather than assumed: on
`many_rectangles` vello differs from the oracle in 9 600 pixels against
tiny-skia's 25 200, and on a 45° fill edge it matches exactly where tiny-skia
is 32 counts light. AGG and vello both integrate analytic area; tiny-skia
supersamples at `SUPERSAMPLE_SHIFT = 2`, so a diagonal edge has **17
coverage levels** and a half-covered pixel quantizes to 6/16.

Over the whole corpus that is worth **six files**, and it costs more than it
pays: vello loses `bug_554151` outright and drops four shading files by 0.13
to 0.60. The store-wide score under vello is 1259 against tiny-skia's 1256.
The default stays where it is, and the diagonal-quantization gap stays a
backend's own business — which is what Tier C's edge population already says
it is.

### What did move: two defects that were hiding behind "AA"

Neither is a coverage mapping. Both were found by probing a stroked rectangle
against the oracle rather than by reading either rasterizer's source.

- **tiny-skia's stroker is half a count off-centre.** A unit-wide vertical
  stroke on an integer x half-covers both neighbouring columns, and AGG writes
  128 into each; tiny-skia's stroker writes 128 and 127, while its *fill* of
  the identical rectangle writes 128 and 128. The same bias costs the outer
  tip of every miter corner its pixel: AGG paints a stroked rectangle's corner
  at alpha 64 — a quarter of the pixel's area — and we painted nothing there,
  2 400 times on `many_rectangles` alone. `stroke_path` now expands the
  outline through `kurbo` and fills it. The corners become exact.

  The larger gain is Tier C's, and it is why this belongs in the backend
  rather than in a per-call correction: a stroke outline computed in the
  engine's vocabulary is identical under both rasterizers **by construction**,
  which is the argument `stroke::outline` already makes for a stroke used as a
  clip. Hard failures fall 5 → 3, both `bug_1288_2` files dropping out.

- **The tiling slow path was load-bearing after all.** The brief records
  `cpdf_rendertiling.cpp:151-184` as behaviorally identical to the cached-cell
  path and performance-only, so the engine took the cached path
  unconditionally. It is performance-only where it is also *possible*:
  `bug_1693`'s cell is 102 400 × 12 800 device pixels behind a 200×200 clip,
  `render_cell` refuses the allocation, and the whole pattern silently paints
  nothing against an oracle that fills the page. Rendering the objects once
  per tile position makes both `bug_1693` files byte-exact.

  Its predicate's third arm — cell *area* over clip area — is **redundant**:
  neither axis exceeding the clip implies the product does not either, so it
  can never be the arm that fires. It is kept, spelled as the C++ spells it,
  with a test that records why.

### What the next wave should not do again

- Do not look for a gamma or a cover-scale LUT. There is none; the mapping is
  `min(255, floor(cov·256))` and we already agree with it.
- Do not spend the budget on the 0.95–0.99 band expecting path work. After
  the two fixes above it is glyph coverage almost to the file, tcpdf included.
- Do not switch the default backend for the diagonal-coverage gap. Six files,
  and it loses more elsewhere.
- The remaining tail worth attacking is **not** antialiasing: `jpxdecode_*`
  and `bug_1986` (JPX, 4 files), the JBIG2/CCITT decode differences, and the
  ~20 assorted singles. Those are codec and compositing questions with
  answers.

## Wave 4: D7 is not a hinting problem, and the hinter would not have helped

Wave 3 left the 0.95–0.99 band as "glyph coverage almost to the file" and
named the hinter as the only thing that could close it. Wave 4 was chartered
to test that — the font crate's OQ-4 ruling was reopened on the argument that
PLAN §1 rules out FreeType the C library but not hinting, and that `skrifa`
ships a pure-Rust bytecode interpreter built for FreeType parity.

**It measured, and it did not build.** Hinting is not the cause, and the
oracle's own hinting is very nearly a no-op. What the tail is instead is
measured below.

### The oracle does hint — on a far wider set of faces than OQ-4 said

Below the `|char2device.a| + |char2device.b| > 50` threshold the oracle
rasterizes glyph *bitmaps*, and that path hints every SFNT face:

```cpp
// cfx_face.cpp:841-843, CFX_Face::RenderGlyph
int load_flags = FT_LOAD_NO_BITMAP | FT_LOAD_PEDANTIC;
if (!IsTtOt()) {                       // !(face_flags & FT_FACE_FLAG_SFNT)
  load_flags |= FT_LOAD_NO_HINTING;
}
```

The `tricky`-list predicate OQ-4 relied on belongs to the *outline* path
(`cfx_face.cpp:948-951`), which ordinary text does not reach. No
`FT_LOAD_TARGET_*` is ever passed, so the target is `NORMAL` by omission, and
the autofitter is compiled out, so non-SFNT faces get no hinting at all. For
`pdfium_test --png` the anti-alias mode resolves to `kLcd` with
`normalize = true`: FreeType renders a 3×-wide `FT_RENDER_MODE_LCD` bitmap
under `FT_LCD_FILTER_DEFAULT`, and `DrawNormalTextHelper` averages the triples
back to grayscale through a 256-entry `kTextGammaAdjust` table.

### The hinting runs at a fixed 64 ppem, which makes it nearly inert

`CFX_Face::New` calls `FT_Set_Pixel_Sizes(face_rec, 64, 64)` once
(`cfx_face.cpp:376`) and nothing ever changes it. The real size arrives
through `FT_Set_Transform`, with the matrix pre-divided by 64
(`cfx_face.cpp:822-825`) — and FreeType applies the transform *after* hinting.
So the interpreter always grid-fits to a 64-pixel grid whose alignment is then
scaled away.

Measured with the pinned `skrifa` 0.46.2 `HintingInstance`
(`Engine::Interpreter`, `Target::Smooth`/`Normal`), comparing hinted against
unhinted point positions on real corpus faces:

| face | oracle's config: hinted at 64 ppem, scaled to 9 pt | hinted at 9 ppem |
|---|---|---|
| `example_063.pdf` `/FontFile2` | *interpreter errors — see below* | — |
| `en_fqa.pdf` `/FontFile2` | mean **0.040** device px | mean 0.306 device px |
| DejaVuSans | mean **0.037** device px | mean 0.260 device px |
| LiberationSerif | mean **0.049** device px | mean 0.619 device px |

About **1/25 of a device pixel** — an order of magnitude below what hinting at
the true size would do, and nowhere near enough to explain differences whose
mean magnitude is 66 counts. `example_063`'s own face is sharper evidence
still: its `prep` program fails in skrifa's interpreter with
`InvalidDefinition`, and under `FT_LOAD_PEDANTIC` that is precisely the case
where `cfx_face.cpp:849-857` reloads the glyph **unhinted**.

And the two fixtures wave 3 named are largely unhinted in the oracle anyway.
`5.5_simple_font.pdf` embeds no font program at all — every run is a
substitution onto a Foxit base-14 face, and those ship as **bare CFF**
(header `01 00 04 02`, no table directory) with the MM fallbacks as Type 1
PFB. Neither is SFNT, so `IsTtOt()` is false and both take
`FT_LOAD_NO_HINTING`.

### What the tail actually is: the oracle blits glyphs on integer origins

The oracle does not fill an outline at its fractional device position. It
snaps every glyph to a whole pixel before blitting the bitmap
(`cfx_renderdevice.cpp:1254-1257`):

```cpp
glyph.origin_.x = anti_alias_is_lcd ? (int)floor(device_origin.x)
                                    : FXSYS_roundf(device_origin.x);
glyph.origin_.y = FXSYS_roundf(device_origin.y);
```

We fill each outline where it really lands. A baseline at `y = 100.4` is drawn
by the oracle at `100` and by us at `100.4`, displacing every horizontal stem
edge by 0.4 px — which on 9-pixel type is a full-count swing on each edge, in
both directions, with geometry that still matches to within a pixel. That is
exactly the signature wave 3 recorded and attributed to the hinter.

Measured on `example_063.pdf`, per text line, as the ink-centroid offset
between our render and the golden:

- mean **+0.446 px**, sd 0.300, range −0.20 to +0.92 — *fractional and
  per-line*, not a constant, which rules out a matrix error.
- Re-aligning each line's baseline onto the oracle's cuts mean |diff| from
  71.0 to 49.3 (**−30.5%**) and removes **63%** of the ±128 swings
  (24 431 → 9 070).
- Fitting an independent sub-pixel shift per 8-pixel band recovers **43.6%**
  of the residual, with the fitted shifts clustered at |0.5| in 91 of 94
  bands.

It generalises. Across ten files sampled from the 0.95–0.99 band, every
text-bearing one shows non-zero per-line sub-pixel offsets — |mean| 0.22 to
0.55 px — and re-alignment recovers 8–48% of the residual.

Two candidate explanations were tested against the same pixels and **both
failed**, so neither should be retried: the `kTextGammaAdjust` table applied
to our coverage makes the match *worse* (the ≤2-count share falls from 42% to
5% on `5.5_simple_font`), and simulating the LCD pipeline — 3× horizontal
supersample, FreeType's FIR5 default filter, box-average back down — moves
mean |diff| by under 4% on either fixture. The residual is positional, not a
coverage curve.

### One thing that is not D7 at all

`5.5_simple_font.pdf`'s worst band is not antialiasing. Its `/Type_1_F` run
draws `(abcabcType 1 fonts -- AGaramond-Semibold)`, a non-embedded Type 1 that
falls to substitution; the oracle renders `abcabc` and we render `a ba b` —
we drop the `c` and then mis-advance, so the rest of the line drifts. Its
`/Type_1_MM_F` band renders the right glyphs but visibly **too narrow**, an
advance-width defect on the MM fallback. Both are glyph-selection and metrics
bugs sitting inside a file the triage had filed under "coverage", and both are
worth more than any coverage work on that file.

### What the next wave should not do again

- **Do not port a hinter.** Measured at the oracle's own fixed 64 ppem it
  moves outlines by ~0.04 device px. OQ-4's ruling stands; only its stated
  reason was wrong, and `pdfrum-font.md` now carries the corrected one.
- Do not re-test the text gamma table or the LCD downsample as the cause of
  the tail. Both were measured here and both make the match no better.
- The lever on D7, if one is ever wanted, is the **integer glyph origin** —
  and it is a deliberate divergence with real costs, because snapping glyph
  positions to whole pixels is exactly what D7 declined in order to keep text
  geometry exact. It is a rendering-policy decision, not a bug fix, and it
  belongs to the orchestrator rather than to a burn-down wave.

## Numbers

Measured against the golden store on the full corpus (1675 files, 1628 with
a golden PNG), rendering through `tiny-skia`. The W3 column is the pixel
burn-down's third wave; M5 is where the burn-down started.

| metric | M5 | M8 (wave 2) | **W3** |
|---|---|---|---|
| pixel files at SSIM ≥ 0.99 | 1075 / 1628 (66.0%) | 1243 / 1628 (76.4%) | **1247 / 1628 (76.6%)** |
| at SSIM ≥ 0.95 | 1478 / 1628 (90.8%) | 1518 / 1628 (93.2%) | **1520 / 1628 (93.4%)** |
| at SSIM ≥ 0.90 | 1544 / 1628 (94.8%) | 1570 / 1628 (96.4%) | **1572 / 1628 (96.6%)** |
| byte-exact PNGs | 430 / 1628 | 446 / 1628 | **451 / 1628** |
| files passing (all tiers) | 1146 / 1675 | 1256 / 1675 | **1260 / 1675** |
| `pixel-fail` | 495 | 385 | **381** |
| `size-mismatch` | 0 | 0 | 0 |
| Tier C hard failures | — | 5 | **3** |

Wave 3's four files are `bug_1693.{in,pdf}` (byte-exact, the tiling slow
path) and two the stroke outlining carried over the line; the byte-exact count
gains five and the Tier C hard count loses two. Every step was measured with
`conformance run --check-regressions`, so no previously-passing file regressed
at any point.
| Tier C hard failures (full store) | 11 | **5** |

Every step was measured with `conformance run --check-regressions` against
the scoreboard it started from, so no previously-passing file regressed at
any point.

### What M8 found, and what it says about the metric

The burn-down opened on the three concrete defects the M5 tail-cleanup had
named, and all three turned out to be the *same shape*: a feature fully built
and tested, with nothing consulting it.

- **`/Decode` was being applied to samples the oracle never decodes.**
  `CPDF_DIB::GetScanline` fills a zeroed *output* buffer and returns before
  `TranslateScanline24bpp` when a row begins past the end of the stream. We
  zero-padded the raw samples and decoded those, which on `bug_554151.in` —
  `/Decode [1.0]` over a 612×104000 image with a kilobyte of data — painted
  484 704 red pixels where the oracle paints black.
- **The degenerate-path family was one nudge.** `BuildAggPath` moves a
  one-point subpath's line endpoint a device pixel right so `vcgen_stroke`
  has two vertices; the dot the oracle paints is the *stadium* that leaves,
  21 pixels across rather than 20. All four files it explains dropped out of
  Tier C's hard count.
- **`bug_1288_2`'s rounding bias was not in `AlphaMerge`.** That function was
  already exact. The bias was upstream: an uncoloured tile composites through
  `CompositeMask`, which carries one flat colour and merges only alpha, and
  we recoloured the cell into premultiplied RGBA and blitted source-over —
  blending the colour against itself once per overlap, truncating upward each
  time. Fixing the shape halved the outliers; the residual one count is the
  final `draw_image`, which a rasterizer rounds where PDFium truncates.

Three more clusters were found the same way and were larger than any of them:

- **Annotations were not rendered at all** — 280 of the 489 remaining pixel
  failures mentioned `/Annots`. `pdfium_test` seeds its flags with
  `FPDF_ANNOT` unconditionally, so an appearance stream is page content.
  Wiring the M6 appearance machinery into the render pass moved 90 files.
- **A `/Mask` array was parsed and never evaluated.** The predicate is on raw
  samples, so it cannot run at draw time; PDFium runs it inside `GetScanline`
  and so do we now, resolving the key into an alpha plane at decode.
- **A `/TR` reached fill colours but not image samples.**
  `StartRenderDIBBase` wraps the source in a `CPDF_TransferFuncDIB`, so every
  sample goes through the tables.

### Where the remaining 385 are

`pixel-fail` is still a threshold cut through a continuous distribution, and
the distribution has changed shape: 275 of the 385 sit in the 0.95–0.99 band
and **159 of those carry no image, shading, pattern or soft mask at all.**
They are D7 in its pure form — geometry that matches exactly, stem coverage
that does not, because the oracle rasterizes hinted FreeType bitmaps with LCD
filtering below `|a| + |b| > 50` and PLAN §1 rules out porting the hinter.
The goldens read 252/253 where we read 63/191: the same ink, distributed the
way two different rasterizers distribute it.

Below 0.9 there are 58 files and they are **not** one cluster:

| files | shape |
|---|---|
| ~20 | `corpus/third_party/tcpdf/example_*`, DCT images at a sub-pixel placement offset — feature edges land a fraction of a pixel apart across the whole page |
| 4 | JPX codestreams: `jpxdecode_indexed`, `bug_1986`, `bug_557223`. `bug_1986`'s filter chain reaches JPX through two indirect `/Filter` names |
| 3 | the tiling *slow path* (`bug_1693`), taken when the cell is larger than the clip, which the engine still declines |
| ~10 | JBIG2 and CCITT decode differences |
| ~20 | assorted single files |

The marginal cluster is under three files, which is where the budget said to
stop. The tail inventory above is what remains.

**Wave 3 corrects two rows of that table.** The tiling row is fixed — the
slow path is implemented and `bug_1693` is byte-exact. The tcpdf row's
*diagnosis* was wrong: those files' images are accurate to within two counts
over their whole extent, and what fails is their text. See "Wave 3: what the
D7 tail actually is" above; the row is left standing because the file count is
still right and the correction is worth reading next to it.

The golden store widened between M3 and M5, so the store-wide rate is not a
like-for-like comparison. Over the **same 1421 files** M3 measured, the
count rises **909 → 944 (64.0% → 66.4%)**, and the M5 work is monotone
against every scoreboard it was measured on: zero previously-passing files
regress and no file's SSIM falls.

Tier C's *edge* rate is reported rather than gated; `conformance/src/tierc.rs`
explains why, and the short version is that the brief's 1% budget is written
against a trace-derived mask several times larger than the neighbourhood
proxy the harness can compute today.

The hard-failure count needs the same care, because M3's reported **0** was
over a 295-file sample and the store is now 1628. Over the *full* store M3
measures **10** and M5 measures **13**, and the three new ones are all tiling
patterns: `xfermodes3.pdf` and `bug_1288_2.{in,pdf}`.

They are a blind spot in the metric rather than an engine divergence, and the
shape says so. A tile cell is *rasterized* — the C++ antialiases it like any
other content — and the finished cell is then blitted at every tile position
into a screen buffer, which is exactly what `CPDF_RenderTiling` does. So the
cell's own antialiased edges arrive at the device inside a composited image,
where `edge_mask`'s neighbourhood test can no longer see them as edges: the
differing pixels recur at precisely the tile period (every 4 rows on
`bug_1288_2`, whose `/YStep` is 4; every 31 and 9 columns on `xfermodes3`)
and their magnitude is the ordinary two-integrations-of-one-ramp spread the
edge tolerance exists for. This is the same class the M3 report already
accepts in prose — "a stem `tiny-skia` writes 127/127 and `vello_cpu` writes
104/150, the same ink distributed differently" — reaching the interior
population only because a blit moved it there.

Instrumenting `bug_1288_2` settles which half is which: under both backends
the engine computes the *same* tile range (`min_col 0, max_col 1, min_row
-20, max_row 29`) and the *same* cell size (100x100), while the two cell
buffers hash differently. The decision is identical; the ink inside the cell
is not, and that is the rasterizers' business.

Making it disappear would mean not rasterizing the cell, which is not what
the oracle does. What the contract actually gates on is that every *engine*
decision is identical under both rasterizers, and every one this work added —
the step and range ladders, the cell sizing, the tile positions, the
recolouring, the screen buffer's compositing — is integer arithmetic in the
engine and identical by construction.

### Corrected 2026-08-29: the edge rule, and what the 13 actually were

`edge_mask` now dilates, seeded from the **intersection** of the two
per-image boundary masks. The brief's mask is `dilate(edges, 1px)` and the
proxy simply omitted the growth; seeding from the union instead would be
worse than not growing at all, because a differing region carries its own rim
in one image and dilating that rim inward excuses the very difference it
marks. `conformance/README.md` documents the rule and the tests that pin both
directions. Hard failures over the full store: **13 → 11**.

The account above turns out to be wrong about the cluster, and the correction
matters more than the two files. Measuring all thirteen rather than the three
named ones shows they are **not one class**:

- `bug_554151.{in,pdf}` — `tiny-skia` fills the page **red** and `vello_cpu`
  fills it **white**: 484 704 pixels at 255 counts, immune to dilation at any
  radius because there is no agreed boundary anywhere on the page. This is a
  real defect and the metric was right to flag it. It is the single most
  valuable thing the tier-C run is currently saying, and it was sitting
  unread under a heading that called all thirteen a blind spot.
- `bug_1983.{in,pdf}`, `single_point_paths.{in,pdf}`, `bug_0_length_line.pdf`,
  `bug_0_width_line.pdf` — one backend paints a degenerate mark (zero-length
  line, zero-width line, single-point path) and the other paints nothing.
  The localized form of the `one_painted_nothing` inversion.
- `bug_1288_2.{in,pdf}` — tiling, but **not an edge phenomenon**: `vello_cpu`
  writes a constant 127 where `tiny-skia` writes 129/130 on a 50% blend, a
  systematic one-sided compositing rounding bias two counts over the interior
  tolerance. Widening the edge mask would hide it rather than fix it.
- `xfermodes3.pdf` — the genuine blit-seam residue, 2x2 cell-corner blocks two
  pixels in from agreed structure.

So the tiling story explains at most two files, one of which it explains
incorrectly. The remaining count should **not** be driven toward zero by
widening this rule: three of the four classes above are things the interior
guarantee exists to catch.

**M8 update: they were, and it caught them.** Every one of the first two
classes was a real engine bug, and fixing them took the hard count from 11 to
**5** without touching the edge rule at all. `bug_554151` was `/Decode` being
applied to samples the oracle never decodes; the four degenerate-path files
were one missing nudge in `BuildAggPath`. `bug_1288_2` shrank rather than
cleared — the bias was not in `AlphaMerge`, which was already exact, but in
recolouring an uncoloured tile into premultiplied RGBA before blitting it,
which blends the colour against itself once per overlap. What is left is a
rasterizer's rounding in one `draw_image`, three counts at most.

The lesson stands for the metric rather than for these five files: a rule
that reports five engine bugs and mislabels one is doing exactly what it is
for, and the correct response to a hard failure is to read it rather than to
widen the mask that raised it.

Metadata and pageinfo remain 100% Tier-A byte-exact; text holds at 99.6% of
pages (99.1% of non-empty ones).

### M5: the four feature clusters

Each cluster is counted over the set of files that carried the feature *and*
failed at M3, so the denominators are M3's triage and the numerators are how
many of those now pass at SSIM ≥ 0.99.

| cluster | M3 | M5 |
|---|---|---|
| tiling patterns | 0 / 13 | **7 / 13** |
| shading patterns | 0 / 29 | **19 / 29** |
| `/Type3` | 0 / 10 | **7 / 10** |
| `/ExtGState` `/SMask` | 0 / 8 | **5 / 8** |
| `/Matte` | 0 / 5 | 0 / 5 |

`/Matte` is the one that did not move, and the reason is that the cluster was
never really a `/Matte` cluster: the un-premultiplication is implemented and
tested, and the five files that mention `/Matte` each fail for something else
— `transfer_function.in` on its transfer functions, `quick_start_guide.pdf`
and `en_system.pdf` on glyph coverage. The arithmetic reaches a pixel now
where before it did not, which is what the cluster asked for.

Four bugs surfaced while wiring these up, each one wider than the cluster
that exposed it:

- **`GetNamedColors` was off by one.** A named `scn` was reading the pattern
  name as a component and dropping the oldest real one, so `1 1 1 /P1 scn`
  came out `[1, 1, 0]`. Invisible until a pattern actually paints, and then
  it paints yellow where the file says white.
- **A resolved white was read as the invisibility sentinel.** `FXSYS_BGR`
  packs a colour into the low 24 bits, so white is `0x00FFFFFF`, while the
  word `GetFillArgb` compares against is `0xFFFFFFFF`. Every white object in
  the corpus was painting nothing.
- **`scn` installs the `/Pattern` space itself**, so a `/P1 scn` with no
  preceding `/Pattern cs` is a pattern colour rather than a `DeviceGray` one.
- **`FindPattern` and a pattern's `Load` fail differently.** The first
  decides whether a pattern colour is installed; the second decides whether
  it paints. Collapsing them made an unusable pattern a no-op, and the four
  `bug_481_*` fuzz files rendered as entirely black pages. All four are now
  byte-exact.

`kMaxType3FormLevel` was also declared and never enforced, which the type-3
work exposed as a stack overflow on `bug_651304.pdf`; `BuildContext` now
counts the depth.

### Where the remaining 553 pixel failures are

`pixel-fail` is the predicate `ssim < 0.99`, so it is a threshold cut through
a continuous distribution rather than a set of distinctly broken files: the
overwhelming majority sit in the shallow 0.9–0.99 band.

The four clusters now account for 27 of them. What remains is dominated by
glyph coverage — D7's deliberate divergence, where geometry matches exactly
and only stem-edge antialiasing differs — plus the shading-pattern residue
(10 files, mostly type 6/7 patch meshes reached through a pattern rather than
through `sh`) and six tiling files whose cell rasterization differs in
antialiasing rather than in placement.

## Deliberate divergences

The design brief's D1–D17 are implemented as written. The three worth
repeating here:

- **D7 — text is always outline fills.** Below `|char2device.a| +
  |char2device.b| > 50` the oracle rasterizes hinted FreeType glyph bitmaps
  with LCD filtering and a gamma table; reproducing that means porting
  FreeType's hinter, which PLAN §1 rules out. Glyph *geometry* matches
  exactly and only stem-edge coverage differs, which is why text pixels are
  Tier B and never Tier A. Wave 3 measured the size of it and confirmed the
  cause is the hinted bitmap's geometry rather than any filter or gamma over
  our outline — the mapping the AGG driver applies to path coverage is plain
  `min(255, floor(cov·256))` and there is no gamma table on that side at all.
  See the wave 3 section.

  **Wave 4 corrects the attribution.** It is the bitmap's *placement*, not its
  hinting. The oracle snaps every glyph origin to a whole pixel before
  blitting (`cfx_renderdevice.cpp:1254-1257`) while we fill each outline where
  it truly lands, which displaces stem edges by the baseline's fractional part
  — measured at mean +0.45 px per line on `example_063.pdf`. The hinting the
  oracle does run is nearly inert, because it grid-fits at a **fixed 64 ppem**
  that is then scaled away: ~0.04 device px of point movement at 9 pt. So
  "reproducing that means porting FreeType's hinter" is false; porting the
  hinter would change almost nothing. See the wave 4 section.
- **D5 — non-isolated groups double-count their backdrop.** PDFium seeds
  such a group's buffer with a copy of the page and never removes it before
  compositing back. That is not ISO 32000 §11.4.6, and it is what the oracle
  does.
- **D10 — the ±32000 coordinate clamp is kept**, in the engine rather than a
  backend, because it distorts rather than clips and both rasterizers must
  agree.

## `[spec]` changes

SPEC §8 gains an implementation record; the six items are summarised there.
The two that a reader should know without opening it:

**Q1: the `/TR` array reversal is real.** `array[2]` drives red and
`array[0]` drives blue, exactly as `pFuncs[2 - i] = Load(array[i])` reads.
`CPDFDocRenderDataTest.TransferFunctionArray` looks like it says the opposite
only because two of its expectation constants are **misnamed**:
`kExpectedType0FunctionSamples` is the type 4 program's sine ramp — it
matches `sin(v/255 · 360°)/2` on all 128 positive samples — and
`kExpectedType4FunctionSamples` is the type 0 function's flat one. The ten
`TranslateColor` pairs settle it without naming a function.

This crate briefly had it backwards and compensated by remapping channels at
the boundary, which cancelled a correct parse and swapped red and blue on
every rendered `/TR` array. Both halves are gone; the concurrent
`pdfrum-text` work caught it, and `pdfrum-page::transfer` now carries the
derivation.

Porting the oracle's fixture verbatim — its actual type 0, type 2 and type 4
functions, and all ten `TranslateColor` pairs, now
`transfer::tests::the_oracle_fixtures_ten_translate_colour_pairs` — then
exposed a second, independent bug in the parse: **`sample_channel` clamped
where PDFium wraps.** The C++ rounds `output[0] * 255` into a `size_t` and
stores it into a `uint8_t` with no clamp, so a function whose `/Range` admits
negatives folds its lower half to the top of the byte range. The fixture's
type 4 program produces `-121.26` at input `0xCC`, which the oracle stores as
`0x87` and we stored as `0x00`; two of the ten pairs turn on it. See
`docs/status/pdfrum-page.md`.

**Q5 resolves as proposed, with a correction to what the predicate is.**
`render_page` chooses white-vs-transparent itself and `RenderOptions.
background` is an override only. But `FPDFPage_HasTransparency` is **not**
the page's `/Group`: it is `BackgroundAlphaNeeded`, which only an
`/ExtGState` blend mode above `Multiply` sets. The choice is load-bearing
rather than a convenience — the engine's single compositing path relies on an
opaque page's white being real pixels, which is what makes D6's collapse of
PDFium's five-armed compositor into one arm arithmetically correct.

## Brief errata

Two claims in `docs/design/pdfrum-render.md` do not survive measurement, and
the tests that would have encoded them assert the truth instead:

1. **§1.15 / Q7: `kColorSqrt` is not `round(255·D(i/255))` for all 256
   entries.** 35 differ from that and 102 from the truncating spelling. The
   table is hand-tuned; `blend::tests::color_sqrt_is_piecewise_d_not_sqrt`
   now asserts a one-count envelope around `D` plus the entry-1 value that a
   naive square root would get wrong, which is what the test was for.
2. **Test 33's premise (`SoftLight` divides twice) is not a trap.** On that
   branch `255 - 2·src ≥ 1`, so the numerator is non-negative and truncating
   twice by 255 equals truncating once by 65025 — exhaustively, over every
   input. The `/255/255` spelling is still ported verbatim, and the test now
   proves the equivalence rather than a difference.

A third, smaller one: §1.7 step 2's ladder checks `rect_i.valid()` *before*
the sub-pixel promotion, so a rectangle whose outer rect is already
degenerate is dropped rather than widened. The brief's ordering reads the
other way.

M5 adds two more, both about the colour ref rather than about a rasterizer.

4. **§1.3's "white is the sentinel" is a paraphrase, and the paraphrase is
   wrong.** `FXSYS_BGR` packs a resolved colour into the low 24 bits, so a
   white fill is `0x00FFFFFF`; the value `GetFillArgb` compares against is
   `0xFFFFFFFF`, the whole 32-bit word, and only `value_or(0xFFFFFFFF)` on an
   unresolvable colour and `SetPattern`'s fallback ever produce it. Testing
   the colour for white instead of the word for the sentinel makes every white
   object in the corpus paint nothing. `color::ColorRef` names the three
   states the ref distinguishes.

5. **The brief's §1.9 does not say that `scn` installs the `/Pattern` space
   itself.** `CPDF_Color::SetValueForPattern` calls `SetColorSpace(kPattern)`
   when the current space is not already a pattern space, so a `/P1 scn` with
   no preceding `/Pattern cs` *is* a pattern colour. Nor does the brief
   separate `FindPattern` — which decides whether a pattern colour is
   installed at all, and checks only that the resource is a dictionary or a
   stream — from the pattern's own `Load`, which decides whether it paints.
   `pdfrum_page::FoundPattern` names the three outcomes; collapsing them made
   an `scn` naming an unusable pattern a no-op, which painted four of the
   corpus's fuzz files entirely black.

## What is not implemented yet

The four M5 clusters are wired; what remains are the narrower paths each one
declines, plus the work that awaits another crate.

- **The type-3 glyph *bitmap* path**, and the blue-zone cache its horizontal
  edges snap to. An uncoloured char proc whose sole object is a *colour*
  image has that image lifted out and blitted as an 8bpp mask in the text
  colour; the char-proc path declines such a glyph rather than painting the
  image's own samples. A sole-image *stencil* is not declined, because
  painting it as a char proc lands on the same pixels the mask blit would —
  and that is the overwhelmingly common case, every bitmap font in the corpus.
- **`/K` knockout groups**, which PDFium does not implement either (D20).

The tiling slow path used to be listed here. Wave 3 implemented it: the brief
called it performance-only, but a cell larger than the clip is sometimes a
cell that cannot be allocated at all, and the cached path then paints nothing
rather than slowly. See the wave 3 section above.
**Annotation appearance layers now render** (M8). They are appended to the
page's object list by `pdfrum-tool`'s `annot_render`, after the content
stream and in `/Annots` order, using `pdfrum-doc`'s appearance ladder,
placement matrix and ten-subtype AP generator — the machinery M6 built for
the `--annot` dump.

Two things about the wiring are worth knowing, because both were paid for:

- **It belongs to the render pass, not to the shared page builder.** Putting
  it in `content::build` cost 74 text pages and 67 annotation dumps at once:
  `--txt` reads the content stream's text and `--annot` describes the
  annotations rather than drawing them, and both double-count an appearance
  the page graph has absorbed.
- **PDFium draws them in two passes with different visibility rules.** The
  page render's `DisplayAnnots(bShowWidget=false)` skips every `/Widget`;
  widgets come from `FPDF_FFLDraw`, which `pdfium_test` calls after every
  bitmap render with no flag guard. Both end in the same `AnnotGetMatrix`
  arithmetic, so one traversal reproduces them — but Pass B's `IsVisible`
  tests `kInvisible` and Pass A's `DisplayPass` does not. `is_visible` keys
  on the subtype so each annotation gets its own pass's rules.

Wiring this also exposed that **a form's `/BBox` was never applied as a
clip**. A form reached from a content stream inherits an enclosing `q`/`Q`
and the clip rides along in the graphics state; an annotation appearance has
no enclosing anything. `build_form_object` pushes it, which is why
`ink_annot.in`'s golden — an entirely white page, because its two `/InkList`
strokes run far outside the `/Rect` they are clipped to — is byte-exact.

## Tests

211 in the three crates plus their integration test — 185 unit, 26
end-to-end, plus the backends' own. The end-to-end tests render synthetic
pages through **both** rasterizers and assert that anything the engine
decided is bit-identical while only antialiased edges may differ, which is
Tier C's contract in miniature.

Ported C++ unit-test assertions: the transfer-function trio, the twelve
separable blend formulas, the non-separable four through `ClipColor`'s
ordering, `CFX_Path`'s rect normalisation, the premultiplication round trip,
and `CStretchEngine`'s resample-selection and degenerate-size rules. The
heuristics upstream has no test for — the rect-snapping ladder, the three
zero-area detectors, the radial root selection, the Gouraud quirks, the Coons
thresholds, the overprint gate — each get a hand-written test built from a
synthetic input, per the brief's §7.2.

M5 adds one regression test per fix, each on a synthetic target small enough
to assert exact pixels. The end-to-end ones are the load-bearing half,
because they run through both rasterizers: a coloured tile paints its cell
across a fill in visible stripes; an uncoloured one takes the `scn` colour
and not the cell's; a pattern stays inside its object's geometry; a zero step
paints nothing; a pattern that did not resolve paints nothing rather than
black; a luminosity mask's group reveals where it is white and the `/BC`
backdrop hides where it is black; and a white fill paints white. The unit
tests pin the arithmetic underneath — the `/Matte` inverse blend's three
cases, the sub-sixteen-pixel cell rule, the tile-offset overflow, the
source-over blit, the box filter — and `pdfrum-page` gains the named-`scn`
operand fix, the `d0`/`d1` colouring rule, and the char proc's objects.
