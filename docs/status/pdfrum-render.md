# `pdfrum-render` status

**Updated:** 2026-08-29 · **State:** M5 pixel burn-down; the four named
feature clusters are implemented

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

## Numbers

Measured against the golden store on the full corpus (1675 files, 1628 with
a golden PNG), rendering through `tiny-skia`:

| metric | value | M3 target |
|---|---|---|
| pixel files at SSIM ≥ 0.99 | **1075 / 1628 (66.0%)** | ≥ 60% |
| at SSIM ≥ 0.95 | 1478 / 1628 (90.8%) | — |
| at SSIM ≥ 0.90 | 1544 / 1628 (94.8%) | — |
| byte-exact PNGs | 430 / 1628 | — |
| `size-mismatch` | 0 | — |
| Tier C hard failures (295-file sample) | **0** | — |

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

Metadata and pageinfo remain 100% Tier-A byte-exact; text holds at 99.0% of
pages (97.9% of non-empty ones).

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
  Tier B and never Tier A.
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
- **The tiling slow path** (`cpdf_rendertiling.cpp:151-184`), taken when the
  cell is larger than the clip. The brief records it as behaviorally
  identical to the cached-cell path and performance-only, and the engine
  takes the cached-cell path unconditionally.
- **`/K` knockout groups**, which PDFium does not implement either (D20).
- **Annotation appearance layers**, which `pdfrum-doc` now provides the dump
  for; rendering them is not yet wired into the walk.

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
