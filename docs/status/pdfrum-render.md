# `pdfrum-render` status

**Updated:** 2026-08-29 · **State:** M8 pixel burn-down wave 7b —
**88.5% at SSIM ≥ 0.99** (up from 86.2%), **499 byte-exact** (up from 487),
and the glyph raster reproduced as the oracle actually performs it: hinted at
a pinned 64 ppem, rasterized three times as wide, FIR5-filtered and gamma-
averaged back to gray, then cached and blitted. Three of that pipeline's four
stages had been measured *in isolation* by earlier waves and correctly
rejected; assembled, they take a 6 pt stem from twenty counts out to three

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
| `options.rs` | `RenderOptions`, `ColorMode`, `TextAa`, `subpixel_text_positioning`, the oracle-conformance defaults, the type-3 and tile-cell option overrides |
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
| `text.rs` | the `Tr` mode table, the glyph matrix, the stroked-text CTM split, glyph placement, the origin snap and its three gates, `AdjustGlyphSpace`, the substituted-font width solve, type-3 character placement |
| `glyph.rs` | the FreeType-parity glyph raster: the LCD 3× implosion, `ft_lcd_padding`, the FIR5 filter, `kTextGammaAdjust`, the subpixel phase, and the bitmap cache under the oracle's own key |
| `scanline.rs` | the analytic cell rasterizer — the engine's, so a glyph bitmap is the same under every backend |
| `image.rs` | `UseInterpolateBilinear`, the `kHugeImageSize` rule, the CMYK-overprint gate, the flip rule, sample-to-pixmap, the `/Matte` un-premultiply |
| `ctx.rs` | `RenderCtx`, the depth cap, the type-3 font *set*, `RenderCaches` |
| `walk.rs` | `render_page`, the object dispatch, the cull test, the page matrix, the group path, the soft-mask group, the type-3 char-proc walk, `DrawPatternImage`, the glyph-bitmap blit |
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

  **The orchestrator ruled: oracle parity wins**, and wave 5 implemented it.
  The cost turned out to be far smaller than that paragraph feared, because
  the grid is not whole pixels. See below.

## Wave 5: the placement is a third of a pixel, and two of the three defects
were not text at all

Wave 4 escalated one question — snap or not — and the answer was to snap.
Porting the rule then found two more defects that only a snapped render can
see, and one of them was worth more than the snap.

**1247 → 1387 at SSIM ≥ 0.99 (76.6% → 85.2%)**, passing 1260 → 1396, and
**451 byte-exact files unchanged**. Attributed by measuring each change on
its own against the same scoreboard:

| change | files at ≥ 0.99 | delta |
|---|---|---|
| baseline (wave 4) | 1247 | — |
| the substituted-font width solve alone | 1252 | +5 |
| the glyph-origin snap alone | 1307 | +60 |
| both | 1313 | +66 |
| **plus the page-matrix correction** | **1387** | **+140** |

### The x grid is thirds of a pixel; only y is whole

Wave 4 read `cfx_renderdevice.cpp:1254-1257` and stopped there:

```cpp
glyph.origin_.x = anti_alias_is_lcd ? (int)floor(device_origin.x)
                                    : FXSYS_roundf(device_origin.x);
glyph.origin_.y = FXSYS_roundf(device_origin.y);
```

That reads as "both axes quantise to whole pixels", and **porting it that way
cost 58 files** — 1247 → 1189, measured before the mistake was found. The
other half is a hundred lines further down, in the blit loop
(`cfx_renderdevice.cpp:1352`):

```cpp
int x_subpixel = static_cast<int>(glyph.device_origin_.x * 3) % 3;
```

`DrawNormalTextHelper` shifts its sampling window into the **3×-wide LCD
bitmap** by that many subpixels before averaging the triples back to gray. So
the effective origin is `floor(x) + x_subpixel/3`, which for a non-negative x
is exactly `floor(3x)/3`: **x is quantised downward to a third of a pixel**,
and only y to a whole one.

That asymmetry is the entire divergence, and it is why wave 4's *measurement*
was right while its reading of the code was not. The residual it measured was
per-line and vertical — a baseline at `y = 100.4` drawn at `100` — which is
precisely the axis that quantises to a whole pixel. The x error it inferred
was never there.

### Which rounding runs is `FontAntiAliasingMode`, not `bClearType`

This crate's own docs asserted that `bClearType = false` means "subpixel text
never runs in conformance". It does not. `bClearType` sets
`CFX_TextRenderOptions::aliasing_type`, and `DrawNormalText` derives
`FontAntiAliasingMode` **separately** (`cfx_renderdevice.cpp:1165-1206`): on a
display device at 32 bpp with a smooth aliasing type the mode is `kLcd`
whatever the flag word said, and `aliasing_type` only decides `normalize`. So
the conformance configuration takes the thirds.

`--no-smoothtext` is the one that changes it: `aliasing_type = kAliasing`
makes `IsSmooth()` false, the whole derivation is skipped, and the mode stays
at its `kMono` initialiser — one bit per pixel, no LCD triple to shift into,
so x rounds to a whole pixel like y, and `AdjustGlyphSpace` runs. Both
spellings are ported; the `TextAa` enum is the same decision under a
different name.

### Three gates keep a run off the grid, and one of them is a trap

`ProcessText` (`cpdf_renderstatus.cpp:905-928`) sends a run to
`DrawTextPath` — true fractional placement — when `is_clip || is_stroke`, and
`DrawNormalText` itself defers above `|char2device.a| + |char2device.b| > 50`.
A pattern-coloured run goes to `DrawTextPathWithPattern` and never arrives.

**`is_clip` is not a text render mode.** `ProcessText` is called twice: once
from `ProcessClipPath` with a `clipping_path`, and once from
`ProcessObjectNoClip` with `nullptr` (`cpdf_renderstatus.cpp:312`). Only the
first sets it. So a `Tr 4` fill-and-clip run *still snaps* on its painting
pass, and it is the clip accumulation — which this engine builds in
`pdfrum-page`, not in the render path — that does not. Reading `is_clip` as
"the mode has a clip bit" cost six files, 1313 → 1307, and the test that
pins it says so.

`RenderOptions::subpixel_text_positioning` is the knob, default `false`.
One field, documented as the parity/off-grid tradeoff: `true` restores
fractional placement for a caller who wants text where the PDF puts it rather
than where a golden expects it.

### The page matrix flips about the *device* box, and that was worth more

`tcpdf/example_007` got **worse** under the snap, 0.790 → 0.752, and its
render was visibly a half-pixel low. Measuring the per-line ink centroid
against the golden gave a flat **+0.503 px** across the whole body of the
page — not a rounding spread, a constant.

`CPDF_Page::GetDisplayMatrixForRect` (`cpdf_page.cpp:216-218`) builds the
matrix from an **integer** `FX_RECT` divided by the page's float size:

```cpp
CFX_Matrix matrix((x2 - x0) / page_size_.width, ...,
                  (y1 - y0) / page_size_.height, x0, y0);
```

and `CPDFSDK_RenderPageWithContext` (`cpdfsdk_renderpage.cpp:116-118`) passes
it `FX_RECT(0, 0, size_x, size_y)` — the *truncated* bitmap size. An A4 page
841.89 points tall therefore renders into 841 rows at a y scale of
`841 / 841.89`: the page is **squeezed to fit the bitmap its size was
truncated into**. We translated by the float height instead, leaving up to a
device pixel of shear between the top of a page and the bottom.

That was sub-count while every glyph was filled at its true position, which
is why it survived four waves. Once origins snap, half a pixel of shear is a
whole row. Fixing it is worth **74 files** on top of the snap, and it is what
turned the tcpdf cluster around: `example_007` 0.790 → 0.982, `example_017`
0.791 → 0.982, `example_004` 0.793 → 0.984.

The lesson generalises past this file. **A sub-pixel placement error is
invisible under fractional fills and load-bearing under snapping**, so
porting a quantisation rule is also an audit of everything upstream of it.
Two of wave 5's three fixes were found that way rather than looked for.

### The two font defects are one bug, and neither is what the triage said

Wave 4 recorded two separate defects on `5.5_simple_font.pdf`: `/Type_1_F`
rendering `abcabc` as `a ba b` — "a glyph-selection/decode bug dropping a
char and mis-advancing" — and `/Type_1_MM_F` rendering correct glyphs "at too
narrow advances". Both descriptions are wrong, and they are the same bug.

**No character was ever dropped and no advance was ever wrong.** The glyph
ids, the `/Widths` lookup and the pen arithmetic were all correct. What was
wrong is the *outline*:

`CPDF_Font::GetCharPosList` (`cpdf_font.cpp:440-444`) sets
`font_char_width_ = GetCharWidth(char_code)` for a font that is neither
embedded nor CID, and that reaches the face as `dest_width`, where
`AdjustVariationParams` (`cfx_face.cpp:941-943, 1561-1605`) solves the
built-in generic's **Multiple-Master width axis** until the glyph's own
advance equals it. Chrome Sans and Chrome Serif *are* MM Type 1 faces, so
this is not a corner — it is how every non-embedded font in the corpus is
drawn at the width its PDF declares.

`GlyphKey::plain` hardcodes `dest_width: 0`, and it was the **only** key the
renderer ever built. `GlyphSource::mm_instance` — a faithful port of
`AdjustVariationParams`, already written and already tested — took its
`dest_width == 0` early-out on every glyph in the corpus.

`/Type_1_F` declares `a = 800, b = 100, c = 400` against a fallback face
whose own advances are near 450, 470 and 480. The outlines overran their
advances and piled into each other, so `abcabc` *read* as `a ba b`: three
merged ink runs where the oracle has six. `/Type_1_MM_F` is the same thing
seen from the other side — glyphs too wide for correct advances, which looks
like advances too narrow for correct glyphs. The file goes **0.961 → 0.997**,
and two other bands on it (`/Ture_Type_F`, the AGaramond one) improve with it
because they share the cause.

The fix is four lines at the one call site: build the whole `GlyphKey`,
gating `dest_width` exactly as upstream gates it. `weight` and `italic_angle`
come from the same substitution record and were equally unwired.

### What the next wave should not do again

- Do not re-derive the glyph grid from `cfx_renderdevice.cpp:1254` alone. The
  `x_subpixel` at line 1352 is half the rule, and the whole-pixel reading
  costs 58 files.
- Do not read `is_clip` in `ProcessText` as a render mode. It is which
  *caller* is running, and the painting pass always passes `nullptr`.
- The gamma table and the LCD downsample are still dead, per wave 4.
- The remaining tail is **not text**. See the inventory below.

## Wave 6a: a rasterizer of our own, and two defects it had to find first

Wave 5 left the 0.95–0.99 band as "the whole story of the text tail" and
called it a coverage band. That was right about the *text*, and it hid a
second population: every non-axis-aligned **path** edge in the corpus was also
losing counts, for a reason the burn-down had already measured and filed as
"a backend's own business".

`tiny-skia` supersamples at `SUPERSAMPLE_SHIFT = 2` — four subsamples per
axis. A diagonal edge therefore has **seventeen** possible coverages, and a
half-covered pixel quantises to 8/16 of the range where the oracle writes the
exact half. Wave 3 measured this, priced switching to `vello_cpu` at six files,
and correctly declined: vello loses `bug_554151` outright and drops four
shading files.

The option wave 3 did not price is **not switching to anyone's rasterizer**.

### `pdfrum-raster-exact`

An analytic scanline rasterizer, a third `RasterBackend`, adding **no external
dependency**: it is written against `kurbo` and `peniko` alone, both already in
the closed set. Wrapping a fourth rasterizer would reintroduce exactly the
question it exists to settle.

kurbo flattens curves; each straight segment is integrated into per-pixel
`(cover, area)` cells; a sweep turns the sorted cells into spans of constant
coverage. The grid is the oracle's own 256ths of a pixel, the coordinate
conversion is the oracle's truncating `int(c · 256)`, and the coverage→alpha
step is the mapping wave 3 measured and told the next wave not to look for
again — `min(255, floor(cov · 256))`, re-verified against
`agg_rasterizer_scanline_aa.h:283-297`. Every intermediate value is an
integer, so the same path yields the same bytes on every machine.

Two things it deliberately does **not** do. It does not stroke: like both
wrapped backends it expands an outline through `kurbo` and fills it, so stroke
geometry is the engine's under all three. And it does not own blend
arithmetic: `blend::composite_premultiplied` is one authority for the engine
and all three backends, so a pixel a backend composites and a pixel the engine
composites in its own offscreen buffers agree by construction.

Curve flattening runs at a tenth of a pixel rather than the oracle's half
(`agg_curves.cpp:36`'s `m_distance_tolerance_square = 1/4` is a squared
distance against a squared chord). Matching the number would reproduce AGG's
*tolerance* but not its subdivision — it bisects recursively where kurbo places
points adaptively — so the finer value is the safer direction: the flattening
error stays well below the quantisation the output byte imposes.

### It found two defects before it could be measured at all

Neither is in the new crate, and both are worth more than the rasterizer.

- **`draw_image`'s transform maps the image's *pixel grid*, not its unit
  square** — and SPEC §8's own comment, plus the trait's doc, said the
  opposite. Every engine call site passes a plain translation, because the
  engine has already resampled the image to its device size before the call.
  Implementing the documented contract collapses a whole-page image onto one
  pixel, silently and totally, until the first corpus run says so. The doc now
  states the convention the code always had.

- **Premultiplying a composited colour must round, not truncate.** The
  truncating `mul255` is the oracle's product wherever the oracle performs
  one, and it stays that everywhere else. But storing a straight result into a
  premultiplied buffer is *our* round trip — the oracle's buffers are straight
  — and its inverse, ported from `CFX_DIBitmap::UnPreMultiply`'s `+ alpha / 2`,
  rounds. Truncating on the way in therefore loses a count on most values.
  Measured: straight `145` at alpha `223` premultiplies to `126` truncating and
  `127` rounding, and only `127` comes back as `145`. That count was every
  pixel of `alpha_composite`'s overlap. The replacement is proved exhaustively
  never worse than truncating and strictly better on most of the 65 280 pairs.

A third, smaller: an image's footprint must be filled **antialiased**, not
hard-edged. Hard-edging thresholds the silhouette at half a pixel, which
deletes any image thinner than that — `type3.pdf`'s sheared glyph, drawn as an
inline image mask, is exactly such a case, and the oracle paints it at its true
partial coverage. The border is not darkened twice, because the sampler returns
nothing outside the last texel, so the silhouette only modulates boundary
pixels.

### What moved

**1387 → 1403 at SSIM ≥ 0.99 (85.2% → 86.2%)**, 1554 → 1564 at 0.95,
**451 → 487 byte-exact**, passing 1396 → 1411. 899 files moved up and 85 down;
**16 crossed 0.99 upward and none downward**, and `--check-regressions`
reports none.

Five files lost byte-exact status, every one by a **single count** and every
one still passing:

- `long_dashed_line` and `bug_660850` are `kurbo`'s dash phase against AGG's
  `vcgen_dash` — 14 and 60 pixels, at dash boundaries, where the two
  expansions end a subpixel step apart.
- `xfermodeimagefilter` is a blend rounding `tiny-skia` happens to get exact
  through its own pipeline; ours is the engine's shared arithmetic, so it is
  identical under the analytic backend and the engine's own compositor.

The band itself: **167 → 161 files, 121 → 115 unique documents.** The movement
inside it is small — mean SSIM `+0.00088`, 92 files up and 10 down — and that
is the finding rather than a disappointment. The path-edge half of the band is
gone, which is what the 16 crossings are; what remains is **text**, and it is
D7 in the form wave 5 left it. `bug_1402.pdf` is the shape of the whole
residue: 6 pt type, where the oracle has glyph ink at 251/208/160 and we have
white. That is stem geometry upstream of any rasterizer, and no coverage
integral reaches it.

### Two defaults, because there are two questions

`pdfrum-tool --use-renderer=` becomes real — `exact`, `tiny-skia`, `vello` —
and `exact` is what `--png` uses by default. The `pdfrum` facade keeps
`vello_cpu`.

They had been sharing one answer to two different questions. A conformance run
asks whether the *engine* decided a page's pixels, and a rasterizer's
quantisation policy is noise in that measurement. An API user asks for a fast
production rasterizer, which is what `vello_cpu` is and what the analytic
backend does not try to be. SPEC §8 records the split.

### Tier C now has three backends and still one gating pair

`tiny-skia` against `vello_cpu` remains the gate; the analytic backend is a
reported third column. The reasoning is the tier's own: its value is that two
*independent* implementations disagree out loud, and the analytic backend is
ours and shares the engine's arithmetic, so a shared bug cannot make it
disagree. Diffing it against either wrapped backend would test less, not more.

Hard failures are **3**, unchanged. Worst edge divergence for the gating pair
is 50.0%; for the analytic backend against `tiny-skia`, 36.7% — closer to both,
which is what an exact integrator between two approximations should be.

### What the next wave should not do again

- Do not look for the coverage mapping again. It is
  `min(255, floor(cov·256))`, it is now *implemented* rather than merely
  measured, and `cell::coverage_to_alpha` is where it lives.
- Do not read `draw_image`'s transform as a unit-square map. It is the pixel
  grid, and it fails silently and totally.
- Do not expect the 0.95–0.99 band to yield to rasterization work. What is
  left in it is glyph geometry; the levers named in waves 4 and 5 still stand,
  and the codecs below 0.95 are worth more per file.

## Wave 6: the codec tail was mostly not the codecs

Wave 5 left ten documents below 0.80 and called them "codecs". Four were,
and none of the four was a defect in `hayro-jpeg2000` or `hayro-jbig2`. Every
one was a rule of PDFium's *image dictionary* handling that our wrappers
around those crates had not reproduced. Four files went byte-exact, the
store gained **15 passing files (1396 → 1411)** and **36 byte-exact
(451 → 487)**, and nothing regressed.

### `/BitsPerComponent` survives the JPX early-return

`ValidateDictParam` opens with `bpc_ = bpc_orig_;` and only *then* returns
early for `JPXDecode`. It skips the bit-depth **check**, not the assignment,
so a JPX image's declared depth is still live afterwards — and
`LoadJpxBitmap`'s `/Indexed` downshift reads exactly that field:

```cpp
} else if (color_space_ && family == kIndexed && bpc_ < 8) {
  int scale = 8 - bpc_;
  for (auto& pixel : scanline) { pixel >>= scale; }
}
```

We had been storing zero, which made `scale` eight rather than
`jpxdecode_indexed.in`'s six: every index cleared, and in a debug build the
shift overflowed outright. The distinction that makes this two rules rather
than one is which path returns where — a JPX image with **no** `/ColorSpace`
leaves `LoadColorInfo` before `ValidateDictParam` ever runs, so *that* one
really does keep a zero depth. `jpxdecode_indexed` 0.460 → **byte-exact**.

### A four-channel JPEG stays four channels

`jpeg_read_header` picks the output space from the codestream's channel
count, and PDFium only ever narrows it — pinning a three-channel image with
no Adobe marker to its own `jpeg_color_space`, never widening or reducing.
Four channels therefore stay `JCS_CMYK`. `zune-jpeg` defaults its output to
RGB instead and happily converts a CMYK or YCCK image down to three, which
then read as a component mismatch against the `/DeviceCMYK` dictionary and
**rejected the whole image**. `bug_718762` and `bug_1646` are the same
5000×5000 CMYK JPEG, and both drew nothing.

### `/Decode` reaches a codec's output, not only raw samples

`TranslateScanline24bpp` runs on the *decoder's* scanline. Those two files
carry the Adobe inversion as `/Decode [1 0 1 0 1 0 1 0]`, and we were
applying `/Decode` only on the raw-sample path — so the image arrived as its
own negative. The encode back to a byte has to **round**: PDFium keeps these
values as floats into the colour conversion and truncates only the converted
byte, and `1 − 253/255` lands a hair under `2/255`, which truncation loses.
Both files 0.633 → **byte-exact**.

### Components that disagree are refused outright

`CJPX_Decoder::Decode` walks the components and returns false on the first
that disagrees with the one before it on `dx`, `dy` or `prec`.
`LoadJpxBitmap` then returns null and the page draws *nothing*.
`bug_557223` is 904 bytes claiming a 707×6131 three-component image whose
components declare subsampling 3×7, 1×7, 1×7 at precisions 1, 2 and 3;
PDFium prints "has an empty bitmap" and paints white, while a decoder that
presses on invents a picture the file does not contain. The gate reads the
`SIZ` marker directly, because `hayro-jpeg2000` exposes a component's depth
only after a decode and its subsampling not at all. 0.773 → **byte-exact**.

### The six that were never codec problems

| file | what it actually is |
|---|---|
| `bug_718762`, `bug_1646` | the same DCT file; fixed above, and the "JBIG2/CCITT decode differences" row of wave 4's table was wrong about them |
| `bug_1396266` | a `/Mask` **stencil**, no image codec involved — **fixed**, see below |
| `bug_1236` | the JBIG2 decodes correctly — the oracle's own saved image is all-black at 400×400 and so is ours. What differs is its 100×100 `/SMask`, whose alphas cap at 25/255: the oracle's page has no black pixel anywhere, ours has an opaque quadrant. A mask-scaling defect in the image path — **fixed**, see below |
| `bug_1986` | ~~the `/Filter` array's entries are indirect references to objects terminated `enbobj` … a parser object-recovery gap~~ — **this diagnosis was wrong**, see below. **Fixed**; byte-exact |
| `bug_867501` | the one genuine hayro gap — see below |

### `bug_867501`: a genuine `hayro-jbig2` gap, and it is not truncation

Wave 5 recorded this as needing "PDFium's `DecodeSequential` truncation
tolerance". Reading the codestream says something narrower and more
structural. The 77-byte stream is:

| offset | segment | declared data length |
|---|---|---|
| 0 | page information (type 48) | **0** |
| 11 | immediate generic region (type 38) | **0** |
| 22 | symbol dictionary (type 0) | 33 554 432, with 44 bytes left |

PDFium parses every segment **body from one continuous stream, ignoring the
declared length entirely** — `ParseSegmentData` reads from the shared
`stream_` at the offset the header ended. So the zero-length page-info
segment still reads its 19 bytes (from offset 11), and the zero-length
region segment still reads its own (from offset 22), where they find a
generic region **1 pixel wide and 2 tall at (0, 0)**. The third segment's
absurd length runs `offset_` past the end, `while (getByteLeft() >=
kMinSegmentSize)` ends the loop, and `DecodeSequential` **returns success**.
The destination began zeroed and is `~`-inverted at the end, so the row the
region never reached comes back white. That is exactly the oracle's saved
image: a 1×3 grey bitmap reading `255 0 255`.

`hayro-jbig2` cannot reach that answer by any wrapper-level change:

- It slices each segment to its declared length and parses each body in
  isolation, so a zero-length page-info body is an empty slice and
  `parse_page_information` returns `UnexpectedEof`. Feeding it a prefix does
  not help — **no prefix of this stream parses**, and the longest run of
  whole segments is the 22 bytes containing only the two empty ones.
- Its `parse_segments_sequential` loops to `at_end()` and propagates the
  error, where PDFium stops at fewer than eleven remaining bytes and calls
  that success. That half *is* a wrapper-level fix and it is cheap, but on
  its own it changes nothing here.

Both halves are upstreamable and neither needs a fork. **Recorded as a known
gap** rather than fixed: the diagnosis above, the 77-byte codestream and the
expected `255 0 255` output are what an upstream issue needs, and a
first-party `pdfrum-jbig2` port is not justified by one file — SPEC §12 asks
for the narrowest faithful fix, and for this file that fix belongs upstream.

## Wave 7: four defects, and three of the four diagnoses above were wrong

The three files this wave was pointed at were each recorded as a different
kind of problem. Two turned out to be **one** bug, and the third was not in
the crate the table blamed.

### A mask has a resolution of its own, and is composited at the device's

`bug_1396266` and `bug_1236` are the same defect from opposite sides.
`to_pixmap` folded a mask into the base image's samples by indexing
`alpha_at(x, y)` at the *base's* coordinates — correct only when the two
happen to be the same size, which §1.19.9 says they generally are not: a mask
is never resolution-reduced.

PDFium never reconciles the two grids. `DrawMaskedImage`
(`cpdf_imagerenderer.cpp:375-424`) renders the base into a device-sized
buffer, renders the mask into a second buffer over the *same* device rect
through the *same* matrix (`CalculateDrawImage`, `:263-319`), and multiplies —
so each is resampled from its own resolution straight to the device and
neither is ever sampled at the other's coordinates.

Because `alpha_at` reports out-of-range as **opaque**, folding failed
asymmetrically:

| file | geometry | what folding did |
|---|---|---|
| `bug_1236` | 100×100 `/SMask` over a 400×400 base | masked the top-left quarter, left three quarters unmasked |
| `bug_1396266` | 64×64 `/Mask` stencil over a 3×3 base | kept 9 of the stencil's 4096 samples |

`render_masked_image` is the `DrawMaskedImage` path. `bug_1236` is
byte-exact; `bug_1396266` goes from 37% of pixels differing at max channel
diff 228 to 2.19% at diff 1 — resample rounding, not structure.

### `bug_1986` was never a parser gap

The table above blamed "a parser object-recovery gap" around the `enbobj`
terminator. Every part of that was wrong, and it is worth saying why, because
the wrong diagnosis pointed at the wrong crate for a whole wave:

- **`GetIndirectObject` never looks for `endobj` at all.** It reads objnum,
  gennum, the `obj` keyword and the body, then returns
  (`cpdf_syntax_parser.cpp:666-699`). Neither does ours. A malformed
  terminator is simply never read, by either engine.
- Objects 6 and 7 therefore load fine, and `decoder_list` resolves all three
  filters — it goes through `Array::get`, which follows one level.
- The JPX decode already returned the correct image: 612×792 RGB, `ff 00 00`,
  the pure red the oracle paints.

The gap was one accessor in the *image dictionary*. `last_filter` read the
`/Filter` array's last element with `name_at`, which does **not** resolve,
where `GetDecoderArray` reaches each element through `GetByteStringAt`, which
does. An unrecognised last filter meant the image's missing `/ColorSpace` —
legitimate, because a JPX codestream carries its own — forced it down the
one-bit stencil path. Byte-exact after the fix, which also made
`/DecodeParms` elements resolve by the same rule.

### `FXSYS_IsFloatZero` is 1e-4, not a machine epsilon

Both radial shading copies tested `|a| < f32::EPSILON`. The macro is
`(f) < 0.0001 && (f) > -0.0001` (`fx_system.h:36`) — about 840 times wider.

It matters because `a` is `dx² + dy² - dr²`, a catastrophic cancellation
whenever the start point sits on the end circle, and the two branches it
selects between disagree about more than precision: the linear `a == 0`
branch carries **no negative-radius skip** and the quadratic one does.
`radial_shading_point_at_border` is that geometry deliberately — its own
comment says so — with `|start| = 1 + 1.1e-7` against `r1 = 1`, giving
`a ≈ 2.4e-7`, squarely in the gap. Reading it as nonzero ran the quadratic
branch, whose skip discarded 14601 pixels the oracle paints in `C0` through
the `index < 0` extend clamp, and rendered the surviving ramp flat. 18.6% of
pixels differing at max diff 252 → 4 pixels at diff 1.

`pdfrum-doc`'s `geom.rs` already had the constant right, and documents that
the comparison widens to double as the macro's does.

### The zero-area pass precedes the fill; it does not replace it

The largest single movement of the wave, from removing one early return.

`DrawPath`'s loop calls `DrawZeroAreaPath` on each sub-path and then falls
**straight through** to the ordinary fill at the end of the function —
there is no return anywhere between them
(`cfx_renderdevice.cpp:772-804`). Our port returned as soon as any sub-path
matched.

On a sub-path that really is degenerate the two are equivalent, because
filling one paints nothing anyway — which is presumably why it read as safe.
It is not equivalent, because `GetZeroAreaPath`'s third case does not require
a degenerate sub-path at all: it scans an ordinary one for a segment that
doubles back (`IsFoldingVerticalLine` and its horizontal and diagonal
siblings) and emits **just that segment** as a hairline, leaving the rest of
the polygon to the fill that was about to happen.

`bug_1338` is five triangles each with a retraced edge; we painted the five
spikes on white. It is byte-exact now, and because a fold inside a filled
polygon is an ordinary thing for a generator to emit, **27 files** cleared
the floor.

### Optional content is implemented, and wired to nothing

`corpus/fx/layer/4_36.pdf` (0.908) and `octest.pdf` (0.879) are the two
remaining layer files, and they are not an evaluation bug. `OcContext`
answers correctly — on `4_36` its OCMD, whose `/VE` is
`[/And 20 0 R [/Not 30 0 R] [/Not 40 0 R]]` with both groups on, evaluates
`false`, exactly as the oracle behaves.

**Nothing calls it.** `grep` for `content_visible` outside `optional.rs`
finds only the `pub use` in `lib.rs`. The feature is built, unit-tested,
exported and unreachable, so every layer draws.

Wiring it is a **`[spec]` change**, not a sweep fix, and the shape is known:

- PDFium makes this purely a render-time decision — `CPDF_RenderOptions`
  holds the context and the page graph carries only the marks, so resolving
  visibility at build time would diverge structurally.
- The gate is one check at the top of `RenderSingleObject`
  (`cpdf_renderstatus.cpp:247`), which our `render_object` mirrors exactly,
  plus `/OC` on forms (`:401`) and images (`cpdf_imagerenderer.cpp:197`).
- `Mark` already carries `from_resources`, which is the C++'s
  `kPropertiesDict` test.

The obstacle is plumbing, not logic: `content_visible` needs `&mut self` and
a `Resolve`, and `render_page` takes neither — it receives a `Page` and no
document. Threading a resolver through the render API is the decision to
make, and it is the user's.

## Wave 7b: the glyph raster, and why three waves of measurement missed it

Waves 4 and 5 each measured a piece of the oracle's glyph pipeline, found it
did not help, and recorded it as dead. Both were right about what they
measured. Neither had the pipeline.

The oracle does not fill a small glyph's outline. It asks FreeType for a
**bitmap** and blits it, and producing that bitmap is four stages, each of
which moves pixels by more than the coverage integral that would otherwise
decide them. Measured on a 6 pt `H` from Arimo drawn into a 200×200 page —
the smallest probe that shows all four — every stage is load-bearing and the
whole is byte-exact on one of the glyph's five rows.

### The recipe, measured before any of it was ported

| # | stage | source |
|---|---|---|
| 1 | grid-fit the outline at a **pinned 64 ppem**, never at the drawing size | `FT_Set_Pixel_Sizes(rec, 64, 64)` once at `cfx_face.cpp:376`; the real size arrives through `FT_Set_Transform` pre-divided by 64 (`:822-825`), which FreeType applies *after* hinting |
| 2 | multiply every x by **3** and pad the box by **43/64** of a subpixel each side | `ft_smooth_raster_lcd`'s "implode" loop, `ftsmooth.c`; `ft_lcd_padding`, `ftlcdfil.c` |
| 3 | spread each span's coverage over **five** subpixel columns at `{8, 77, 86, 77, 8}`, as `(cov·w + 85) >> 8`, **accumulating** | `ft_smooth_lcd_spans`, with `FT_LCD_FILTER_DEFAULT`'s weights |
| 4 | average the triples `(r+g+b)/3` and look the result up in `kTextGammaAdjust` | `DrawNormalTextHelper` with `normalize = true`, `cfx_renderdevice.cpp:245-249` |

Then `x_subpixel` shifts the sampling window in stage 4 by 0, 1 or 2
subpixels — the third-of-a-pixel placement wave 5 already ported, now applied
where it belongs.

Hinting is enabled for every **SFNT** face and no other: `RenderGlyph` adds
`FT_LOAD_NO_HINTING` exactly when `!IsTtOt()` (`cfx_face.cpp:841-843`), so a
bare CFF — which is all fourteen base-14 blobs — is never hinted. `skrifa`'s
`Engine::Interpreter` is used rather than its default `AutoFallback`, because
the oracle's FreeType has the autofitter compiled out (`ftmodule.h`) and
falling back to one would invent grid-fitting the oracle never applies.

### The measurement, glyph by glyph

Oracle against this pipeline, on the 6 pt `H`, columns 99–104:

| row | oracle | wave 7b | unhinted |
|---|---|---|---|
| 95 | `255 239 249 255 237 252` | **`255 239 249 255 237 252`** | `255 238 248 255 236 252` |
| 96 | `253 146 211 245 128 233` | `253 144 210 247 128 233` | `253 144 210 247 128 233` |
| 97 | `253 137 135 155 102 233` | `253 135 135 155 102 233` | `253 133 126 145  99 233` |
| 98 | `253 143 182 211 119 233` | `253 140 182 211 119 233` | `253 141 191 222 122 233` |
| 99 | `253 146 211 245 128 233` | `253 144 210 247 128 233` | `253 144 210 247 128 233` |

Row 95 is byte-exact and the worst residual is **3 counts**. Across seven
glyphs at 6–12 pt and three subpixel phases the worst is **8** and the mean
under **2** — against an outline fill that was 20–30 counts off *and missing
two entire columns*, because the FIR5 filter's tails ink pixels no outline
reaches. Every glyph's bitmap box matches the oracle's exactly: no oracle ink
falls outside it in any probe.

### Why the two dead hypotheses were dead, and are not now

- **Wave 4 applied `kTextGammaAdjust` to our own coverage** and the match got
  worse. Correct: the table's input is `(r+g+b)/3` over three *FIR5-filtered
  subpixel* coverages, not a pixel's coverage. Applying a stage to the wrong
  input is not a weaker version of the pipeline — it is a different function.
- **Wave 4 simulated the LCD downsample alone** and moved the mean by under
  4%. Correct, and for the same reason: its input is a 3×-wide rasterization
  of a *hinted* outline, which wave 4 did not have.
- **Waves 4 and 5 measured hinting at ~0.04 device px and ruled it out.** The
  number is right. What it does not say is what 1/25 of a pixel is *worth*
  once the glyph is rasterized this way, and the answer is up to **10 counts
  per pixel** on a 6 pt stem: a 3×-wide grid resolves a third of the
  horizontal displacement an ordinary one does, and the filter then spreads
  that difference over five columns. Turning hinting off in the finished
  pipeline takes the worst residual from 8 counts to 25 and roughly doubles
  the mean, per-glyph:

  | glyph | hinted | unhinted |
  |---|---|---|
  | `H` 6 pt | max 3, mean 0.50 | max 11, mean 2.13 |
  | `o` 6 pt | max 8, mean 1.56 | max 20, mean 3.60 |
  | `e` 8 pt | max 7, mean 1.94 | max 25, mean 4.69 |
  | `g` 9 pt | max 8, mean 1.80 | max 20, mean 3.98 |

The general lesson is the one wave 6a's `draw_image` finding already made in
another register: **a pipeline stage measured out of its pipeline measures
something else.** Three waves rejected three real components of the oracle's
glyph raster on honest measurements of the wrong function.

### The cache is the oracle's key, and the phase is deliberately not in it

`BitmapKey` is `pdfrum-font`'s `GlyphKey` plus the device matrix quantised as
`(int)(m · 10000)` on each of `a`, `b`, `c`, `d` — `UniqueKeyGen`,
`cfx_glyphcache.cpp:62-70`, truncating rather than rounding. Two matrices
closer than one part in ten thousand share a bitmap in the oracle, so a page
whose text matrix drifts by rounding error draws identical glyphs there; a
finer key would draw very slightly different ones.

What is cached is the **3×-wide LCD bitmap**, not the gray one, which is why
the subpixel phase is absent from the key exactly as it is from the oracle's:
the phase is a window shift into that buffer applied at blit time, so one
entry serves all three thirds of a pixel instead of three entries holding the
same rasterization.

The budget is 16 MB of bitmaps per session and the policy on exhaustion is to
**stop inserting**, not to evict — the caller still gets its bitmap, the cache
just stops growing. A page's glyph repertoire is small and hammered, so the
entries an LRU would evict are the ones about to be wanted again.

### `pdfrum-raster-exact`'s `cell` module is now the engine's `scanline`

The glyph bitmap must be identical under all three rasterizers, because the
oracle's is produced by FreeType rather than by whatever draws the page's
paths. So the integrator moved from the analytic backend into the engine,
unchanged — the same argument `blend::composite_premultiplied` already makes,
and the same guarantee: Tier C's "every engine decision is identical under
both gating backends" now holds for glyph coverage **by construction**.

### What it moved

Measured with the zero-area fix already in, so this is the glyph raster alone:

| metric | before | after |
|---|---|---|
| pixel files at SSIM ≥ 0.99 | 1418 / 1628 (87.1%) | **1441 / 1628 (88.5%)** |
| at SSIM ≥ 0.95 | 1574 / 1628 | 1574 / 1628 |
| byte-exact PNGs | 497 | **499** |
| files passing (all tiers) | 1426 / 1675 | **1448 / 1675** |
| `pixel-fail` | 210 | **187** |
| Tier C hard failures | 3 | 3 |

**665 files up, 15 down**, the largest fall 0.0012 and none of the 15 across
a threshold; **23 crossed 0.99 upward and none downward**; **no byte-exact
file lost its status** and two gained it. `--check-regressions` reports none.

The band it was aimed at is where it landed: `example_064` 0.958 → 0.983,
`foxittext` 0.954 → 0.976, `ch_9_android` 0.956 → 0.968, `example_063` 0.753
→ 0.764.

### And it is faster, once `draw_image` learns what a blit is

The M8 benchmark finding predicted this and got the mechanism only partly
right; `docs/status/M8.md` carries the correction. The short version:

- Caching the bitmap is not enough on its own. `draw_image`'s general path —
  an inverse transform and two floors per pixel, plus an antialiased
  rasterization of the footprint — cost **2.56 µs** per glyph against
  **2.20 µs** to fill the outline it replaced. `pdfrum-raster-exact` now
  recognises a whole-pixel translation and takes a real blit, for every image
  rather than only for glyphs, and a test pins that the two paths agree byte
  for byte.
- **`vello_cpu` structurally cannot benefit.** It is a retained-scene
  rasterizer — the device accumulates commands and rasterizes at `finish` — so
  there is no pixel buffer to blit into and a glyph is a scene command either
  way. It gains ~1.1x where the immediate-mode backends gain 1.4–3.7x.

### `bug_1402` is not a raster problem, and never was

Wave 6a named it "the shape of the whole residue: 6 pt type, where the oracle
has glyph ink at 251/208/160 and we have white". The 251/208/160 is real and
the raster now reproduces it — total ink over the page goes from 35112 to
37473 against the oracle's 37188, over 306 pixels against 309. But the file
does not improve, because its whole run is **displaced by (−23, +27) device
pixels**: the oracle draws its three U+3002 ideographic full stops at
x 123–135 and we draw them at x 101–112, at the same relative spacing and the
same shape.

That is a substitution question — which CJK fallback face a non-embedded
`/HonMincho-M` with `/90pv-RKSJ-H` resolves to, and where in the em box that
face puts the mark — and it is the reason wave 6a's reading of this file was
wrong. Measuring a coverage residue on a run that is 23 pixels out of place
measures the offset. Anyone picking it up should start from the face, not the
raster.

### What the next wave should not do again

- Do not re-test any stage of the glyph pipeline in isolation against our own
  coverage. All four stages are now implemented together and measured
  together, and the isolated measurements that rejected three of them are in
  the table above with the reason each was measuring a different function.
- Do not read `bug_1402` as a coverage file. It is a substitution offset.
- Do not expect `vello_cpu` to gain from the glyph cache. Its scene is
  retained; there is nothing to blit into.
- Do not measure a glyph cache on `latin_extended`. Its 256 glyphs are 256
  distinct glyphs — a 0% hit rate — which is why `foxittext.pdf` joined the
  benchmark fixtures.

## Numbers

Measured against the golden store on the full corpus (1675 files, 1628 with
a golden PNG), rendering through `tiny-skia`. The W3 column is the pixel
burn-down's third wave; M5 is where the burn-down started.

| metric | M5 | M8 (wave 2) | W3/W4 | W5 | W6 | **W7** |
|---|---|---|---|---|---|---|
| pixel files at SSIM ≥ 0.99 | 1075 / 1628 (66.0%) | 1243 / 1628 (76.4%) | 1247 / 1628 (76.6%) | 1387 / 1628 (85.2%) | 1403 / 1628 (86.2%) | **1441 / 1628 (88.5%)** |
| at SSIM ≥ 0.95 | 1478 / 1628 (90.8%) | 1518 / 1628 (93.2%) | 1520 / 1628 (93.4%) | 1554 / 1628 (95.5%) | — | **1574 / 1628 (96.7%)** |
| at SSIM ≥ 0.90 | 1544 / 1628 (94.8%) | 1570 / 1628 (96.4%) | 1572 / 1628 (96.6%) | 1591 / 1628 (97.7%) | — | **1597 / 1628 (98.1%)** |
| byte-exact PNGs | 430 / 1628 | 446 / 1628 | 451 / 1628 | 451 / 1628 | 487 / 1628 | **499 / 1628** |
| files passing (all tiers) | 1146 / 1675 | 1256 / 1675 | 1260 / 1675 | 1396 / 1675 | 1411 / 1675 | **1448 / 1675** |
| `pixel-fail` | 495 | 385 | 381 | 241 | 225 | **187** |
| `size-mismatch` | 0 | 0 | 0 | 0 | 0 | **0** |
| Tier C hard failures | — | 5 | 3 | 3 | 3 | 3 |

**The W7 column is wave 7 and wave 7b together**, and the two were measured
apart before it was written, because they landed on the same tree:

| | ≥ 0.99 | byte-exact | passing | `pixel-fail` |
|---|---:|---:|---:|---:|
| W6 | 1403 | 487 | 1411 | 225 |
| + wave 7's four fixes | 1418 | 497 | 1426 | 210 |
| + wave 7b's glyph raster | **1441** | **499** | **1448** | **187** |

Wave 7's four fixes moved 15 files across the 0.99 line and none down; the
zero-area early return accounts for most of them, and three of the four were
one-line or near-one-line changes whose reach was entirely in what they had
been suppressing. Wave 7b's glyph raster moved a further 23 up and none down.

Wave 5 moved 636 files up and 18 down, none of the 18 by more than 0.0026
and none across the 0.99 line. **Every byte-exact file stayed byte-exact** —
the monotone rule's sharpest edge, and the one that says the placement port
is a refinement of where ink goes rather than a change to what is drawn.

Wave 3's four files are `bug_1693.{in,pdf}` (byte-exact, the tiling slow
path) and two the stroke outlining carried over the line; the byte-exact count
gains five and the Tier C hard count loses two. Every step was measured with
`conformance run --check-regressions`, so no previously-passing file regressed
at any point.

### The tail after wave 5, and it is no longer text

241 `pixel-fail` entries, **168 unique documents** once the `.in`/`.pdf`
pairs are collapsed. The shape has changed more than the count:

| band | documents | what is in it |
|---|---|---|
| 0.95–0.99 | 121 | the residual coverage tail — 19 tcpdf, 14 `FRC_8.2.4`, 11 `fx/text`, the rest scattered singles |
| 0.90–0.95 | 26 | shadings (`shade`, `shade-tensor`, type 6/7, radial-at-border), the `fx/layer` optional-content pair, uncoloured tiling, vertical text |
| 0.80–0.90 | 11 | `transfer_function`, `same_color_knockout_fill`, `image_transformer_other`, assorted image singles |
| below 0.80 | 10 | **codecs**: `jpxdecode_indexed` (0.460), `bug_1986` (0.546), `bug_557223` (0.773) are JPX; `bug_1396266`, `bug_718762`, `bug_867501`, `bug_1236` are decode singles; `en_fqa.pdf` (0.715) and `example_063` (0.747) are the two genuinely dense-type files left |

**Wave 6 empties four rows of that band and re-labels four more.**
`jpxdecode_indexed`, `bug_557223`, `bug_718762` and `bug_1646` are
byte-exact. Of what is left, only `bug_867501` is a codec at all:
`bug_1396266` is a `/Mask` stencil, `bug_1236` is a mask-scaling defect
around a JBIG2 that decodes correctly, and `bug_1986` is a parser
object-recovery gap. See "Wave 6" above.

**The 0.95–0.99 band is now the whole story of the text tail**, and it is a
coverage band rather than a placement one: geometry now agrees, and what is
left is the stem-edge distribution D7 has always described. Everything below
0.95 is a feature or a codec with an answer — which is exactly what wave 3
predicted the tail would resolve to once the positional half was removed, and
wave 3 was measuring a page whose placement error it could not see.

Two files sit at the boundary and are worth naming for whoever picks this up:
`en_fqa.pdf` at 0.715 and `example_063` at 0.747 are the last two files where
dense small type alone loses more than a quarter of the score. If a future
wave wants a text lever, those are the fixtures; if it does not, the codecs
above are worth more per file.

### The tail after wave 7b

187 `pixel-fail` entries, **129 unique documents** once the `.in`/`.pdf`
pairs are collapsed — down from 168 at wave 5 and 138 at wave 7.

| band | documents | what is in it |
|---|---|---|
| 0.95–0.99 | 94 | what is left of the coverage tail. It is smaller than wave 5's 121 and it is *no longer only text*: the glyph raster took 23 documents out of it and what remains is a mixture of residual stem counts, image resample rounding and small feature gaps |
| 0.90–0.95 | 15 | shadings, uncoloured tiling, vertical text |
| 0.80–0.90 | 13 | `transfer_function`, `same_color_knockout_fill`, `image_transformer_other`, `octest` (optional content, built and unwired — see above), assorted image singles |
| below 0.80 | 7 | `2_shading_type_6_00` (0.356) and `_001` (0.612) and `shade`/`shade-tensor` are **mesh shadings**, now the worst thing in the store by a wide margin; `example_030` (0.487) and `example_063` (0.764) are dense type; `bug_867501` (0.646) is the known `hayro-jbig2` gap |

**The lever has moved off text.** Wave 5 called the 0.95–0.99 band "the whole
story of the text tail" and it was; three waves later the text half of it has
been paid down and the two worst files in the entire store are **type 6 Coons
mesh shadings**, at 0.36 and 0.61. Whoever picks this up should start there
rather than on another glyph question: `2_shading_type_6_00` alone is a bigger
single-file deficit than anything text has left.

The two files wave 5 named as the text lever have split. `example_063` moved
0.747 → 0.764 and its remaining loss is genuinely the residual stem
distribution. `en_fqa.pdf` moved 0.715 → 0.885 without the glyph raster
touching it — it is unchanged across wave 7b — so whatever is left there is
not the raster either.
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

*Superseded by wave 5's inventory above; kept because the reasoning it
records is what wave 4 and wave 5 were testing. The 159 files this section
calls "D7 in its pure form" were mostly a **placement** error rather than a
coverage one, and 140 of them left the band once the placement was ported.*

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
  hinting. The oracle snaps every glyph origin before blitting
  (`cfx_renderdevice.cpp:1254-1257`) while we fill each outline where
  it truly lands, which displaces stem edges by the baseline's fractional part
  — measured at mean +0.45 px per line on `example_063.pdf`. The hinting the
  oracle does run is nearly inert, because it grid-fits at a **fixed 64 ppem**
  that is then scaled away: ~0.04 device px of point movement at 9 pt. So
  "reproducing that means porting FreeType's hinter" is false; porting the
  hinter would change almost nothing. See the wave 4 section.

  **Wave 5 corrects it twice more, and D7 is now a smaller divergence than
  it has ever been.**

  1. *The grid is not whole pixels.* Wave 4 said "snaps every glyph origin to
     a whole pixel"; only **y** does. `x_subpixel` at
     `cfx_renderdevice.cpp:1352` gives x back, through the LCD triple, what
     line 1254 floored away, so x is quantised to a **third** of a pixel. The
     whole-pixel reading is not a paraphrase of the rule, it is a different
     rule, and porting it costs 58 files.
  2. *The placement is no longer a divergence at all.* On the orchestrator's
     ruling it is now the **default policy**, reproduced exactly, and
     `RenderOptions::subpixel_text_positioning` is what asks for the old
     behaviour. So D7 has shrunk to what it was always meant to be: we fill
     outlines where the oracle blits bitmaps, glyph *positions* now agree to
     the oracle's own grid, and only stem-edge **coverage** differs. That is
     the sentence the brief's D7 should be read as carrying from here on.
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

Wave 5 adds two more, and both are about placement rather than colour.

6. **The page-to-device matrix fits the page to the *device* box, not to its
   own height.** `CPDF_Page::GetDisplayMatrixForRect` divides an **integer**
   `FX_RECT` by the page's float size (`cpdf_page.cpp:216-218`) and
   `CPDFSDK_RenderPageWithContext` hands it the truncated bitmap size, so an
   A4 page 841.89 points tall renders into 841 rows at a y scale of
   `841 / 841.89`. Both the brief and SPEC §8's item 5 describe it as a flip
   about the page height, which leaves up to a device pixel of shear from the
   top of a page to the bottom. Invisible under fractional glyph fills;
   a whole row under snapped ones. Worth 74 files.

7. **`bClearType` does not decide `FontAntiAliasingMode`.** This crate's own
   `options.rs` said `bClearType = false` meant "subpixel text never runs in
   conformance". It sets `CFX_TextRenderOptions::aliasing_type`, which is a
   different variable: `DrawNormalText` derives the anti-aliasing mode itself
   (`cfx_renderdevice.cpp:1165-1206`), and on a 32-bpp display device with a
   smooth aliasing type it is `kLcd` regardless. `aliasing_type` only decides
   `normalize`. The confusion is load-bearing for exactly one thing — which
   of the two roundings the glyph origin takes — and that is now spelled out
   where both live.

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

234 in the three crates plus their integration test — 206 unit, 28
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
