# `pdfrum-render` status

**Updated:** 2026-08-30 · **State:** M9 parity tail closure (wave 12) —
**97.4% at SSIM ≥ 0.99** (up from 96.2%), **99.4% at 0.95**, 519 byte-exact,
1624 / 1675 files passing, and the **`--annot` tier clean across all 1675
files**. The largest remaining cluster was the image downscale kernel; it is
ported, and rounding its destination size *up* rather than to nearest was worth
as much again. See "Wave 12" below for the tail, which is now 42 files in four
measured mechanisms.

*(Historical, wave 9:)* Wave 9 classified the
0.95–0.99 band before touching it, and the classification was the finding:
**31 of its 86 documents carried no font at all**, and twenty-five were missing
*ink* rather than miscovering it. The band had been recorded as "glyph
coverage" for four waves and is 36% glyph-only when measured. Five defects
followed from reading it properly — a free-text appearance described but never
drawn, the one malformed JPEG header PDFium repairs, a text clip whose
producer was missing, the form-field highlight `pdfium_test` itself paints, and
pattern-coloured text. The largest single lever, at 178 files, was not in any
document: it was the host's own configuration.

The largest cluster left — the **widget text body**, 16 documents — is out of
scope by a standing `[spec]` ruling and is escalated with measurements rather
than fixed.

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
| `image.rs` | `UseInterpolateBilinear`, the `kHugeImageSize` rule, the CMYK-overprint gate, the flip rule, sample-to-pixmap, the `/Matte` un-premultiply. Samples arrive already in device colour: `pdfrum-page`'s `unpack` resolves a `Separation`/`DeviceN` image through its tint transform first, per "Tint-space images" below |
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

## Wave 8: seven defects, and not one of them was in a rasterizer

Wave 7b left the two worst files in the store as type 6 Coons meshes and told
the next wave to start there rather than on another glyph question. That was
right, and what it found generalises past shading: **every defect this wave
fixed was a decision made in the wrong place, not an arithmetic error.** A
colour walked along the wrong axis, an alpha gated on the wrong flag, an
identity hashed from the wrong thing, an image classified by its codec rather
than by its dictionary. In each case the code doing the work was correct and
already tested; what was wrong was what it was handed.

**37 files up and none down, 30 across the 0.99 line and none back, no
byte-exact file lost its status.** 1441 → 1471 at SSIM ≥ 0.99 (88.5% → 90.4%),
1574 → 1595 at 0.95 (96.7% → 98.0%), 499 → 511 byte-exact, 1448 → 1478 passing,
`pixel-fail` 187 → 157.

### The Coons colour field was transposed against its own geometry

`2_shading_type_6_00` was the worst file in the store at 0.356 with **98% of
its pixels differing at a mean of 41 counts** — and its geometry was perfect.
Probing eight rigid transforms of our render against the golden found the
answer immediately: our output was the oracle's reflected across the patch's
anti-diagonal, at a mean of 4.3 counts.

A cell's position in the colour lattice advances on two axes, `left`/`x_scale`
and `bottom`/`y_scale`, and `bilinear` reads the four corner colours as
`c0 → c3` along the first and `c0 → c1` along the second. Those correspond to
the grid's first and second index respectively. The two subdivision functions
were named `split_horizontal` and `split_vertical` after the *visual* axis
they appear to cut, and wired to the lattice axis of the other one. Every cell
came out the right shape in the right place carrying the colour of the cell
across the patch from it.

The lesson is the naming. `SubdivideVertical` in the C++ splits the *second*
index — it is named for the direction the cut line runs, not the index it
walks — and reading it as "the vertical axis" inverts it. The two are now
`split_along_rows` and `split_along_columns`, named for the index each halves,
which is the only thing the caller needs to know.

### `full_cover` is a third mode, and reading it as "no antialiasing" punches
holes

With the axes fixed, `2_shading_type_6_00` still had white pin-holes in a
lattice down the middle of every patch — 1111 pixels the oracle paints and we
left blank.

AGG has three coverage modes and this crate had two. `aliased_path` thresholds
the integrated coverage at its midpoint; `full_cover` keeps the rasterizer's
own choice of *which* pixels a span covers and discards their coverage
**value**, writing every one at the source alpha
(`CFX_AggRenderer::GetSrcAlpha` against `GetSourceAlpha`,
`cfx_agg_devicedriver.cpp:481-491`). The difference is exactly a pixel that two
abutting cells each cover by 40%: `full_cover` paints it from both, and the
midpoint threshold drops it from both.

`AntiAlias::FullCover` is that third mode. The engine's integrator expresses it
exactly (`scanline::Coverage::Full`, a test against zero rather than against
127); `vello_cpu`'s aliasing threshold expresses it exactly at `Some(1)`;
`tiny-skia`, which has only a bool, takes **antialiased** as the nearer of its
two — reaching every pixel the oracle reaches and writing some of them light
beats not reaching them at all. `tiny_skia::convert::to_anti_alias` had been a
`matches!(aa, AntiAlias::On)`, which would have compiled silently past the new
variant; it is now an exhaustive match with the reasoning written down.

The scratch pixmap and the draw-cells-opaque discipline are unchanged and
still required: they answer a different question (D12, §5.3), which the
module doc now says.

### A mesh's ramp spans the mesh's own decode range

`shade.in` draws all seven shading types in a grid and was still 19% wrong at a
mean of 118 counts, in blocks: solid magenta where the oracle has olive.

`ColorSteps::sample(shading, 0.0, 1.0, …)` — the unit interval, hardcoded, for
both mesh families. The parametric value's domain is the mesh stream's own
`/Decode[4..6]`, and `shade.in` carries `[1 2]` and `[0 255]` and `[-128 127]`
alongside `[0 1]`. Each corner value is then mapped into the ramp's 0..255
across that same range by `ComponentToShadingIndex`, which the patch
rasterizer did not do at all — it called `Rgb::to_bytes`, which clamps to
`[0,1]` and rounds, and is accidentally right for the unit interval and wrong
for every other.

`Mesh::component_range` carries the range from the stream reader, which had it
and dropped it. Types 4 and 5 share the fix: their ramp was equally hardcoded,
and their vertex values equally unmapped — the module doc even claimed the
mapping happened at read time, which it never did.

### A Coons patch's derived interiors were transposed, and a Coons pattern drew
nothing

Two smaller ones the above were hiding. `coons_interior` returns the four
derived points in the formula's own order — p11, p12, p21, p22 — and
`from_boundary` laid the middle two into each other's grid slots. Only a curved
patch shows it, which is why the flat fixtures passed.

And a shading *pattern* reached `draw_to_pixmap`, which declines types 6 and 7
because they need a device rather than a buffer — so a Coons or tensor pattern
painted **nothing at all**, silently. Both call sites now go through one
`draw_shading_into`, which is where the fork belongs.

`2_shading_type_6_00` is byte-exact from 0.356. `example_030` — filed by wave 6
under "dense type", and by wave 3 under "DCT images at a sub-pixel offset" —
went 0.487 → 0.999, because it was a mesh shading all along.

### Optional content: a pre-pass, not a render-time decision

`OcContext` had been complete, unit-tested, exported and reachable by nobody
for three waves. Wave 7 diagnosed it exactly and named the obstacle: the
predicate needs `&mut self` and a `Resolve`, and `render_page` has neither.

The orchestrator authorised a **pre-pass**, and it is the better shape for a
reason the plumbing argument only hints at. Deciding visibility needs
indirect-object lookup and a mutable evaluation cache; consuming the answer
needs neither — it is one `bool` per object. Threading a resolver through the
render API to carry that would put a resolver in front of every rasterizer for
a question already settled.

`pdfrum_page::page_visibility(&Page, &mut OcContext, &impl Resolve, diags)`
returns a `Visibility`: plain data shaped like the page, one entry per object
in each list with a form's children nested. A page object has no id and its
position is the only thing that names it, so position is what the tree keys on.
An absent entry is **visible**, which is what lets an all-visible page collapse
to an empty tree — so the common case costs one `is_none_or` per object and
`render_page` is `render_page_with` and a default `RenderSession`.

Sub-graphs outside the page's own lists — pattern cells, soft-mask groups,
type-3 char procs — take all-visible. The tree does not describe them, and the
oracle asks no `/OC` question inside any of them either.

Two things the page graph was not carrying, and both are real:

- **A form's and an image's `/OC` live on the XObject dictionary**, which is a
  declaration site independent of any enclosing marked-content sequence. A form
  can be hidden by its own dictionary while the `Do` that drew it sits under no
  `/OC` mark at all.
- **`CheckPageObjectVisible` scans the whole mark stack**
  (`cpdf_occontext.cpp:189-200`), so nested `BDC /OC` sequences each get a veto.
  `optional_content` returned only the innermost; `optional_content_all` returns
  every one.

`octest`, `bug_40162073` and `bug_491_invisible` are byte-exact; `4_36` went
0.908 → 0.979 and its residue is now the ordinary text tail rather than a
layer. Eight files, none down.

### `bug_1402` is a table that was wired to the measurement and not the drawing

Wave 7b called this a substitution offset and told the next wave to start from
the face. It is narrower than that and it is not in the substitution at all.

`kJapan1VerticalCIDs` — 154 rows, transcribed verbatim, binary-searched, gated
on exactly the C++'s condition, unit-tested — was wired into `char_bbox` and
nothing else. The design brief §1.10.3 names *both* application sites,
`GetCharBBox` and `GetCharPosList`, and only the first was ported.

The name misleads twice. "Vertical CID" does not mean vertical *writing*:
`GetCIDTransform` gates on the charset and the absence of a font program and
never asks about the writing mode, so `/90pv-RKSJ-H` reaches the listed CIDs
perfectly well — those CIDs are vertical *forms*, a property of the glyph. And
"transform" overstates it: for CID 7888, which is where U+3002 lands, the
matrix is the **identity** and the whole content is a translation of
`(79/127, 94/127)` em. At the file's font size that is `(+22.4, +26.7)` in text
space, which after the page matrix's single y flip is the `(−23, +27)` device
displacement wave 7b measured. Right shape, right ink, right spacing, wrong
place — which is exactly what a pure translation produces and exactly what
made wave 6a read the file as a coverage problem.

The adjustment reaches the glyph's own matrix and never the pen: upstream
mutates a per-glyph `origin_`, so a run's advances are identical with the
transform and without it. `bug_1402` and `bug_1355` are both byte-exact, with
the ink bounding box matching the oracle's to the pixel.

### Four more, found worst-first, each one line of judgement

- **A JBIG2 image that declares a colour space is a picture, not a stencil.**
  The pixel layer chose the shape from which codec ran; the dictionary layer had
  already decided correctly and was overridden. Two things then go wrong at
  once: the polarity is inverted (the oracle ends `Jbig2Decoder::Decode` with
  `pix = ~pix`, turning JBIG2's "1 means black" into the PDF convention where 0
  is black) and the image composites as a mask. `transfer_function`'s
  400×400 `/IM_1bpp` decoded, in our own wrapper, to a perfectly correct
  all-black bitmap; we painted the page background over it. 0.820 → 0.9998,
  residue two counts.

- **An inline image whose `EI` never arrives takes the rest of the stream with
  it.** The scan consumes everything after the `ID` looking for a terminator,
  and when the stream runs out it has already eaten every operator that
  followed. Our scan rewound, so the parser re-read the image's *own sample
  bytes* as content and found a `re f` inside them.
  `bug_412524377.in`'s second page is a deliberate fixture and says so in its
  own comment; its first page is the same content with the `EI` present and was
  already byte-exact, which is what made the pair worth reading. Both byte-exact.

- **Two form objects with identical bytes are two forms.** The recursion guard
  hashed the decoded content, so thirty-five distinct form XObjects all reading
  `/X1 Do` collapsed to one identity and every level below the first was refused
  as a form drawing itself. Upstream keys on the decoded buffer's *address*,
  which is per stream object. The reference was already resolved at the call
  site and already handed to the image path; the form path passed `None`.
  `bug_972999` byte-exact, and skia's `xfermodes` for the same reason.

- **A group's alpha is gated on the group the object declares, not the one
  around it.** `ProcessTransparency` reads `pFormObj->form()->GetTransparency()`
  and multiplies on *that*; we passed the enclosing transparency.
  The failure is silent in one direction only — `group_alpha` is 1.0 for every
  non-form, so the wrong flag can only ever *drop* a multiply, never add one —
  which is why four waves went past it. `bug_1949.in` is the fixture that shows
  it: it draws the same 0.5-alpha square twice, once under a constant alpha and
  once under a soft mask, and asserts by construction that the two agree. The
  masked one was right and the constant one was a factor of two light.
  `bug_1949`, `bug_346598551` and `group_xobject` all byte-exact.

### What the next wave should not do again

- **Do not read `full_cover` as "antialiasing off".** It is a third mode and
  the difference is a hole through every internal seam. `scanline::Coverage`
  has all three and each is documented against the AGG line it comes from.
- **Do not assume a mesh's parametric range is `[0, 1]`.** It is `/Decode[4..6]`
  and the corpus carries `[1 2]`, `[0 255]` and `[-128 127]`.
- **Do not read `bug_1402` as a substitution or a raster question.** Both are
  settled: it was one table applied in one of its two places, and it is fixed.
- **Do not diagnose a mesh file from its SSIM.** Three separate waves filed
  `example_030` under three different wrong causes — DCT placement, then dense
  type — because a transposed colour field and a missing glyph look alike at
  that resolution. Probing rigid transforms of the output against the golden
  took one minute and answered it outright; it is worth doing first on any file
  whose *geometry* looks right and whose colours do not.
- The 0.95–0.99 band is still glyph coverage and still D7. Wave 8 did not touch
  it and did not need to: everything it fixed was below 0.95.

### `bug_1746` is diagnosed and deliberately not fixed

Worth writing down, because the diagnosis is solid and the fix is not.

The file is a Type 3 font whose char proc draws a CCITT image mask, under
`ca 0.5`, with **`/FontMatrix [1 0 0 1 0 0]`**. We draw nothing at all.

The cause is that the translucent char-proc path sizes its buffer from
`metrics.bbox` — the *declared* box, which is `d1`'s operands scaled by a
thousand and then put through the font matrix. With the conventional
thousandth matrix the scale cancels; with this file's identity matrix it does
not, and the box lands at x 8050 on a 200-pixel page. The oracle uses
`pForm->CalcBoundingBox()` instead (`cpdf_renderstatus.cpp:1020`) — the
objects' own extent, no scale, no font matrix — which is our `painted_extent`
and is unaffected.

Porting that makes the glyph draw, and the file gets **worse**: 0.835 → 0.765.
The glyph appears at half the alpha the oracle gives it — 128 where the golden
writes 192 — so a second `ca 0.5` is applied somewhere in the oracle's path
that we do not reproduce, and drawing the glyph at the wrong alpha scores below
not drawing it. The C++'s shape differs from ours in a way that is probably
related: it renders the procedure into the buffer at the text object's **own**
alpha-carrying colour and blits the buffer *opaquely*
(`SetFillColor(fill_argb)` then `SetDIBits`), where we force the buffer opaque
and re-apply at the blit. Making that change alone does not close the gap
either. Whoever picks it up has the box half already measured and should start
from where the second factor of two comes from, not from the box.

## Wave 9: the band was never glyph coverage, and half of it had no glyphs

Waves 3 through 8 each recorded the 0.95–0.99 band as "glyph coverage", "D7 to
the file", and wave 8 signed off with "89 documents of glyph coverage and about
twenty scattered singles". Wave 9 was chartered to classify the band before
fixing anything, and the classification is the finding:

**Of the 86 documents in the band, 31 contained no font dictionary at all.**
Twenty-five were missing *ink* rather than miscovering it — many with an ink
ratio of exactly zero, meaning we drew blank paper where the oracle drew a
picture. The band had been diagnosed from its SSIM and from the shape of its
worst files, and both readings were four waves old.

**1471 → 1513 at SSIM ≥ 0.99 (90.4% → 92.9%)**, 1595 → 1603 at 0.95, passing
1478 → 1510, `pixel-fail` 157 → 115. **192 files up and none down**, 42 of them
across the 0.99 line; no byte-exact file moved in either direction.

### The measurement that unlocked it was not a measurement

The first probe of `example_063` reported 15.4% of pixels differing at a mean
of 24.7 counts, which looks exactly like the coverage tail the band was filed
under. Adding the three flags the harness puts on *every* invocation —
`--time=1399672130 --croscore-font-names --font-dir=<checkout>/third_party/test_fonts`
(`conformance/src/oracle.rs:148-154`) — took the same file to 8.75% at a mean
of **2.6**.

Without `--font-dir` and `--croscore-font-names` every substitution in the
corpus picks a different face, and the resulting mismatch is a page-wide
per-glyph difference that reads precisely as a coverage tail. Any wave probing
a file by hand must reproduce the harness's determinism triple or it is
measuring its own font configuration. It is worth stating as a rule because
three of this wave's five fixes were invisible under the wrong flags and
obvious under the right ones.

### The five defects

Each was measured on its own against the scoreboard it started from, and
committed only after `conformance run --check-regressions` reported none.

| | ≥ 0.99 | ≥ 0.95 | passing | `pixel-fail` | up / down |
|---|---:|---:|---:|---:|---|
| W8 | 1471 | 1595 | 1478 | 157 | — |
| + the free-text appearance | 1475 | 1595 | 1482 | 153 | 5 / 0 |
| + the JPEG header repair | 1476 | 1595 | 1483 | 152 | 1 / 0 |
| + the text clip | 1477 | 1597 | 1484 | 151 | 4 / 0 |
| + the form-field highlight | 1509 | 1603 | 1506 | 119 | 178 / 0 |
| + pattern-coloured text | **1513** | **1603** | **1510** | **115** | 4 / 0 |

#### A generated free-text appearance was described but never drawn

`--annot` reported `freetext_annotation_without_da`'s appearance in full — the
right object count, the right content, byte-identical to the golden dump — and
the page rendered as blank paper. The two disagreed because they called
different functions: `annot_render::overlay` took `generate_appearances`, which
has no text-bearing generators at all, and only the dump path took
`generate_appearances_with_text`.

Upstream has no such split. `GenerateAPIfNeeded` reaches `GenerateFreeTextAP`
on the same annotation constructor as every other generator
(`cpdf_generateap.cpp:1603`), so an annotation that is *described* is also
*drawn*. That the text tier matched exactly is what let this survive.

A second gap sat behind it and would have kept the text invisible anyway: the
generated stream names its font in a `Tf`, and every call site built the
stream's `/Resources` with `None` for the font half, so the operator resolved
to nothing. `GenerateFreeTextAP` passes `GenerateResourceFontDict(doc,
font_name, font_dict->GetObjNum())` (`:1141-1144`).

#### PDFium repairs exactly one malformed JPEG header, and we repaired none

`bug_86459` is a progressive JPEG whose SOF2 declares height `0xffff` beside a
correct width of 612, wrapped in a dictionary that says 792. libjpeg refuses
that header — `0xffff` is above `JPEG_MAX_DIMENSION`, so `JERR_IMAGE_TOO_BIG` —
and `zune-jpeg` refuses it for its own reason, a 16384-row limit. Either way
the page came back blank.

PDFium does not accept the header either; it **rewrites** it.
`LibjpegScanlineDecoder::Start` seeds `cinfo` with the *dictionary's*
dimensions, and on a failed header read tests for one specific malformation and
patches the two height bytes before reading again
(`libjpeg_scanline_decoder.cpp:88-115`). The test is deliberately narrow — the
dimension bytes at one of exactly two offsets, five bytes past a real
start-of-frame marker, reading `ff ff` followed by the dictionary's width
*exactly* — and its own comment calls the checks "lots of possibly redundant
checks to make sure this has no false positives". It is ported in full, because
it licenses writing into image bytes.

Everywhere else the codestream still overrides the dictionary's dimensions.
This is the one case where the codestream is the wrong authority.

#### A clipping text mode clipped nothing

`ClipEntry::Text` existed, `push_text` existed, `bounds` unioned it, and
`clip::resolve` turned it into a winding-filled device path. `build.rs`
declared a `text_clip` list, initialised it, and drained it at `ET`. **Nothing
ever put anything in it.** `Tr 4` through `Tr 7` added nothing to any clip, and
every swatch drawn after a text clip painted whole — `clipping_text.pdf` is the
file named for the feature, and it and `path_9.pdf` both lay six colour blocks
over the glyphs that should have cut them out.

This is the fourth wave running to find a feature built, tested, exported and
consulted by nobody, and the tell is the same each time: the half that
*consumes* is complete and the half that *produces* is absent. It is invisible
to a test that only exercises the consumer, which is the only kind of test such
a feature ever has.

The entry now holds the **runs**, not outlines, which is what upstream holds:
`clip_text_list_` collects `CPDF_TextObject`s and `ProcessClipPath` calls
`ProcessText` on each at clip time (`cpdf_streamcontentparser.cpp:1359-1361`,
`cpdf_renderstatus.cpp:573-582`). Deriving outlines in the builder would mean a
second implementation of the advances, the kerning, the spacing and the
substituted-font width solve; holding the run means the shape that clips is the
shape `place_glyphs` would have painted, by construction.

Two rules came with it that a simpler port would have missed:

- **The mode at `ET` decides.** `Handle_EndText` re-reads the render mode when
  the text object closes, not the one each run was shown under, so
  `BT 7 Tr (x) Tj 0 Tr ET` clips with nothing. The list is cleared either way.
- **The clipping pass places fractionally.** `ProcessText`'s snapping gate is
  `if (is_clip || is_stroke)`, and `is_clip` is which *caller* is running —
  true here, false on the painting pass. So a `Tr 4` run's painted glyphs snap
  to the blit grid and its clipping glyphs do not. `text.rs`'s own doc had
  spelled this out since wave 5 and named `pdfrum-page` as where the
  accumulation would live.

#### The largest single movement was not in the document at all

**178 files up and none down**, from one flat wash of colour.

Twenty-five of the band's eighty-six documents were missing ink, and most were
missing the *same* ink: a pale-blue tint over every form field. `password.in`
is the clearest case — two text fields, no appearance streams, no content
stream at all — where the golden carries 6000 tinted pixels and we drew blank
paper.

It is a **host** decision. `pdfium_test` opens every file with

```cpp
FPDF_SetFormFieldHighlightColor(form.get(), FPDF_FORMFIELD_UNKNOWN, 0xFFE4DD);
FPDF_SetFormFieldHighlightAlpha(form.get(), 100);
```

(`pdfium_test.cc:1776-1777`), and `CPDFSDK_Widget::DrawShadow`
(`cpdfsdk_widget.cpp:982-1006`) fills each widget's `/Rect` with that colour
once the appearance is down. The goldens are what that host produces, so
reproducing them means reproducing its configuration — which is why this looked
for four waves like a coverage problem on files that have no coverage problem.

Four details decide whether it lands on the right pixels:

- **`FX_COLORREF` is BGR.** `0xFFE4DD` is blue `0xFF`, green `0xE4`, red
  `0xDD` — a pale blue, not the pink the hex reads as. Over white at 100/255
  through the truncating `AlphaMerge` that is exactly the `(241, 244, 255)` the
  goldens carry.
- **It is a hard-edged integer rect.** `ToFxRect` **truncates** every edge
  (`fx_coordinates.cpp:324-327`), so a `/Rect` of `[100 100 200 130]` on a
  200-tall page tints device rows 70..99 and columns 100..199 — 30 × 100
  pixels, no antialiasing on any side.
- **The read-only gate is the *field's* flag**, bit 0 of the inherited `/Ff`
  (`form_flags.h:13`), where the annotation's own `ReadOnly` is bit 6 of `/F`.
  Reading the wrong flag word would tint and un-tint almost disjoint sets.
- **Three kinds of field are never tinted**, each refused at a different level:
  a push button by `IsFillingAllowed`, a widget with no `/FT` by
  `IsNeedHighLight(kUnknown)`, and a **signature** by `CPDFSDK_Widget::OnDraw`
  itself, which short-circuits to `DrawAppearance` and never calls the form
  filler at all (`cpdfsdk_widget.cpp:719-724`). The signature case is the one
  that bites: without it six corpus signature files go down and four lose
  byte-exact status; with it nothing moves down anywhere.

The highlight is painted whether or not the widget had an appearance to draw,
because upstream paints it after `OnDrawDeactive` either way.

#### Pattern-coloured text is a rectangle its own glyphs clip

A run filled with a shading or tiling pattern drew nothing. The comment said
`DrawTextPathWithPattern` "returns before the ordinary draw", which is true and
was read as "so skip the run" — but the function *draws first* and returns
after.

Its unstroked arm (`cpdf_renderstatus.cpp:1290-1310`) never touches a glyph. It
builds a synthetic `CPDF_PathObject` out of the run's **bounding rectangle**,
gives it the text's colour and general state, appends the run itself to a copy
of the current clip path, and sends that through `RenderSingleObject`. The
pattern then paints the rectangle and the text clip cuts it back to the glyph
shapes.

That is a strikingly economical way to fill glyphs with a pattern, and it is
only expressible once the clip stack can hold text runs — which is why it lands
in the same wave as the clip and could not have landed before it. The stroked
arm is different and is left alone: there each glyph outline becomes its own
path object, which the ordinary draw already handles.

`text::run_rect` is `CalcPositionDataInternal`'s bounding half
(`cpdf_textobject.cpp:288-357`). The run's origin enters where `place_glyphs`
puts it, pulled back through the text matrix — starting the pen at zero puts
the rectangle 600 pixels up the page, which is how the omission was caught.

### What this wave did *not* fix, and why

**The premultiplied round trip is a real one-count loss and the fix makes the
corpus worse.** `pixmap::premultiply` uses the truncating `mul255` where its
inverse `unpremultiply_rgb` rounds, so a solid brush's colour does not
round-trip: a `/ca 0.392` fill of `(221, 228, 255)` comes back as `219` and
composites to `240` where the oracle writes `241`. Wave 6a's ruling — that
storing a straight colour into a premultiplied buffer must round — says the
rounding spelling is right, and it was tried.

Measured over the whole corpus it moves **30 files up and 105 down**, none
across a threshold, and lands the highlight on `242` against the oracle's
`241` — one over instead of one under. The truncating spelling is empirically
closer for a solid brush, so it stays. The residual count is the round trip
itself, which the oracle never performs because its buffers are straight, and
closing it properly means not making the trip rather than rounding it better.
**Do not retry the rounding change on its own.**

### The tail after wave 9, classified rather than named

83 unique documents below 0.99. Every one was classified by reading its file
and its pixels, not by its score:

| class | docs | what it is |
|---|---:|---|
| image resample residue | 27 | no font on the page at all. Measured on `FRC_1_8.2.4`: the ink agrees within 1.6%, there is no shift on either axis, and ours and the golden have **identical** gradient statistics (mean \|dx\| 6.21 both) — so it is not a filter-kernel difference. Mean absolute residue 2.0 counts with a +0.96 bias. Fourteen of the 27 are the one `FRC_8.2.4` file repeated |
| glyph-only | 20 | a font and no image. This is what the band was *called*, and it is under a quarter of it |
| **widget text body** | **16** | a `/Tx` or `/Ch` widget with no `/AP`, whose *value* the oracle lays out and we do not. See the escalation below |
| text + image | 15 | both on the page, including `example_063` (0.764) and `en_fqa` (0.885) |
| other | 5 | `same_color_knockout_fill`, `bug_1746` (diagnosed in wave 8, deliberately unfixed), assorted singles |

The median of the 83 is 0.9785 and the whole sub-0.90 population is eight
documents.

### Escalation: the widget text body is 16 documents and a standing `[spec]` ruling

This is the largest named cluster left, and it is **out of scope by rule**, so
it is reported rather than fixed.

`SPEC.md`'s 2026-08-29 doc-brief rulings say: "*NO second variable-text engine —
the `cpdfsdk_appstream`/`CPWL_EditImpl` NeedAppearances path is out of scope
(only ~5 corpus pixel files reach it; they become documented waivers)*". The
M6-implementation correction beneath it already found the scope estimate wrong —
`CPDFSDK_Widget::OnLoad` calls `ResetAppearance` **ungated** on any widget
whose appearance is not valid, so the path reaches every widget lacking a
usable `/AP`, not the five that set `/NeedAppearances` — and it responded by
widening the *chrome* while restating that "the text body remains out of scope".

The measurement this wave adds is what that costs in pixels, which neither
ruling had:

- **16 of the 83 remaining sub-0.99 documents** are in this class, from
  `listbox_form` at 0.933 to `text_form_color` at 0.989.
- **Only 2 of the 16 set `/NeedAppearances`.** The other fourteen are ordinary
  files with an unadorned field.
- The evidence is unambiguous in Tier A as well as Tier B: `--annot` reports
  **0** objects where the golden reports 1, 7 and 27 text objects on
  `bug_983867`, `bug_477200528` and `scrollable_widgets1`.
- The producer is **not** `GenerateFormAP` — that one *is* gated on
  `NeedAppearances` (`cpdf_annotlist.cpp:209`). It is
  `CPDFSDK_AppStream::SetAsTextField` / `SetAsListBox` / `SetAsComboBox`
  (`cpdfsdk_appstream.cpp:1686`, `:1604`, `:1532`), reached from
  `cpdfsdk_widget.cpp:1109-1111`. `scrollable_widgets1` proves which: it has no
  `/I`, so `GenerateListBoxAP` would highlight nothing, and the golden
  highlights Banana — `SetAsListBox` derives selection from `/V` through
  `GetSelectedIndex` (`cpdfsdk_appstream.cpp:1631-1636`).
- The dependency the ruling was protecting against **already exists**.
  `pdfrum-doc/src/vt/` is a complete variable-text engine — `layout`,
  `edit_ap::generate`, `comb.rs`, `autosize.rs`, `bidi.rs`, `split.rs` — and
  `vt::Config` already carries `multi_line`, `auto_return`, `sub_word`,
  `limit_char` and `char_array`, which are exactly the knobs the text-field
  case sets. `ap/freetext.rs` is a working consumer of it whose shape matches
  almost line for line. The gap is wiring: `widget::generate` never receives a
  `TextFont`, and `ap/mod.rs`'s text-bearing dispatch returns `None` for every
  subtype but `FreeText`.

So the ruling's stated cost — a second engine — is not the cost any more. The
orchestrator's decision is whether 16 documents justify wiring the engine this
crate already has into the widget path, and that is a `[spec]` change either
way.

### What the next wave should not do again

- **Do not probe a corpus file without the harness's determinism triple.**
  `--time=1399672130 --croscore-font-names --font-dir=<checkout>/third_party/test_fonts`
  goes on every invocation of both binaries. Without them a substituted face
  differs everywhere and reads exactly like a coverage tail; `example_063` shows
  a mean of 24.7 counts under the wrong flags and 2.6 under the right ones.
- **Do not take the band's label from the previous wave.** It was recorded as
  glyph coverage for four waves and was 36% glyph-only when finally measured.
  Classifying 86 documents took an afternoon and found four defects.
- **Do not retry the rounding premultiply on its own.** Measured: 30 up, 105
  down. The count it chases is the round trip, not the rounding.
- **Do not read `DrawTextPathWithPattern` as a skip.** It draws, then returns.
- **Do not look for a resample kernel difference in the `FRC_8.2.4` cluster.**
  Ours and the golden have identical gradient statistics; the residue is two
  counts of rounding, not a filter.

## Wave 10: the widget text body, and three things the ruling had guarded

Wave 9 measured the widget text body's price and escalated it: the largest
named cluster left, 16 of 83 documents, blocked by `SPEC.md` §10's ruling
against "a second variable-text engine". The orchestrator revised the ruling,
and the revision turned out to be the cheap half of the wave.

**1513 → 1540 at SSIM ≥ 0.99 (92.9% → 94.6%)**, 1603 → 1605 at 0.95, passing
1510 → 1569, `pixel-fail` 115 → 88. **65 files up and none down**; no
byte-exact file moved in either direction. On Tier A the `--annot` waiver
cluster went from **76 artifacts to 13**, which retires it: M6's exit criterion
is met outright at 99.4% with no exclusion.

### There was never a second engine to decline

The ruling's stated cost was writing `CPWL_EditImpl`. It is not an engine. It
is a shell over `CPVT_VariableText` — the engine `pdfrum-doc`'s `vt` module
already is, in full — and the only thing it adds that a *generated appearance*
can observe is a vertical alignment offset:

```text
to_edit(p) = (p.x - (scroll.x - plate.left),
              p.y - (scroll.y + padding - plate.top))
```

Scrolling defaults off and none of the three builders turns it on, so the
scroll term stays at the `(plate.left, plate.top)` its setter seeded and the
two cancel, leaving `-padding` on y and nothing on x. That is one argument,
and `ap/freetext.rs` had been passing it since wave 9. The whole port is
`ap/field_body.rs`: three builders, one shared wrapper, and the option and
selection accessors.

`listbox_form`'s `--annot` dump was **byte-exact on the first run** — seven
widgets, twenty-seven text objects, two selection paths interleaved in the
right places — which is what says the engine underneath was already right.

### `/V`, not `/I`, and the golden says which

Two producers in the C++ build a widget appearance out of a field's value and
they are near-copies. `GenerateFormAP` is gated on `/NeedAppearances`;
`SetAsTextField`/`SetAsComboBox`/`SetAsListBox` run on every widget lacking a
usable `/AP`. Wave 9 argued from the source that it is the second. The golden
settles it from the outside, and `listbox_form` is the file that does it:

- `Listbox_MultiSelect` has `/V (Banana)` and no `/I`. The gated generator
  reads only `/I` and would highlight nothing; the golden highlights Banana.
- `Listbox_MultiSelectMultipleIndices` has `/I [1 3]` and no `/V`. The golden
  reports five text objects and **no path** — no highlight at all.

The second is the one worth keeping in mind, because it looks like a bug and
is a consequence. With `/V` absent the selection object becomes `/I`, and the
lookup then compares each entry's *text* against the option values. An
integer's text is the empty string, which matches no option. So a list box
selected only by index highlights nothing, and reproducing that is what makes
the dump match.

### The metrics the layout stacks lines by are the substituted face's

This is the finding that cost the most and moved the most, and it was invisible
until the bodies existed.

Both callers of the appearance generators threaded in **one stock Helvetica**
for the whole page, on the reasoning — written into both files — that a
non-embedded `/DA` font substitutes to that face anyway, and that the generator
only wants metrics. The reasoning is sound about *widths*. It is wrong about
ascent and descent, and the difference is not small:

| source | ascent | descent | list-box row pitch at 12pt |
|---|---:|---:|---:|
| base-14 metric tables | 718 | −219 | 11.24 |
| the substituted face | 905 | −211 | 13.39 |

`CPDF_Font` fills its ascent and descent from the **loaded face** when the font
dictionary carries no `/FontDescriptor`, which every stock `/BaseFont
/Helvetica` in the corpus does not. Two extra rows fit in a thirty-unit list
box under the wrong numbers, and `listbox_form` sat at 0.933 with the dump
already byte-exact — the tier that would have caught it does not report
positions.

`ap::FormFonts` now loads every face the form's `/DR /Font` declares, through
the same loader and the same substitution options the rest of the page uses,
and hands each annotation the one **its own** `/DA` names. The fallback for a
name the resources lack goes through that loader too rather than through the
stock-metrics constructor, which is the same bug one level down and is where
`bug_1072440` — a field with no `/DR` at all — was auto-sizing four steps too
large.

`listbox_form` went 0.933 → 0.9997 on that change alone; `text_form_custom_font`
0.960 → 0.9999.

### Automatic font sizing was inverted, and had been since it was written

`vt/autosize.rs` opened with three paragraphs explaining that the search finds
the first size that *fits* and returns the step before it, "which by
construction is one that overflows", that this "looks like an off-by-one" and
"is the behavior". It is not the behavior. `GetAutoFontSize` is a
`std::lower_bound` whose comparator is `!IsBigger(font_size)`, and
`lower_bound` returns the first element for which the comparator is **false** —
so it finds the first size that is *bigger* than the plate, and the step before
it is the largest that fits.

The port had the predicate the other way round, so every field with no explicit
size was set at **4**. Four of the module's five tests asserted that, at
length, with prose. Nothing caught it because the only consumer was a free-text
annotation, which almost always carries a size in its `/DA`; `calculate.pdf`'s
two `/Tx` widgets carry none, and their goldens show twelve-point digits where
this produced specks two pixels tall.

The lesson is narrower than "check ports against the source", because the port
*was* checked: it is that **a comment explaining why a surprising result is
correct is evidence of nothing**, and the more confident the explanation the
more it is worth re-deriving. Three of this wave's fixes were sitting behind
one.

### A section's own origin is part of a word's position

With the size right, `calculate.pdf` set its `/Q 2` fields flush **left**.

Placement computes the alignment offset twice — once from the section's width
to fix its box, once per line from that line's width — and gives each word a
position *relative to the section box*, which is what makes the two partially
cancel. The C++ then reads a word's position as `fWordX + pSection->GetRect()
.left`. Ours added the section's `y` when stacking paragraphs and never its
`x`, so the surviving alignment — which lives entirely in that left edge — was
dropped on the floor. Every centred and right-aligned field was flush left, and
the free-text generator had the same bug for as long as it has existed.

### `GetDeflated` normalizes, and a one-unit field depends on it

`bug_765384`'s second widget is a `/Rect [0 0 1 1]` with a one-unit border.
Deflating gives `(1, 1, 0, 0)`, and `CFX_FloatRect::GetDeflated` **normalizes
afterwards**, so the client rectangle comes back as `(0, 0, 1, 1)` — the size
it started as. Ours did not, so the plate had no width, automatic sizing
answered zero, a zero size writes no font operator, a run with no font in the
text state produces no page object, and the golden's one text object was two
short of being drawn. Four steps between a normalization and a missing glyph.

### What this wave did *not* fix, and why

**The `/AP`-presence rule is deliberately left loose.** `IsAppearanceValid` is
one dictionary lookup — `!!GetDictFor("AP")` — so a widget whose `/AP /N`
resolves to nothing still counts as having an appearance and is never
regenerated. Matching that exactly is a two-line change and clears **four more
`--annot` artifacts**: `checkbox_radiobutton`'s `Widget8` then keeps having no
appearance, which is what its golden reports through the two colour lines.

It also takes `bug_707673` down, 0.9944 → 0.9910, and the monotone rule is
absolute. That file's residual is a **push-button caption** this crate does not
draw, and the chrome the current rule generates stands in for it by accident.
Removing the accident before drawing the caption is a net loss, measured. The
same applies to the grey `0xFFAAAAAA` outline `DrawAppearance` strokes over a
checkbox whose appearance is invalid: it is real behavior, it is worth +5
files, and it costs 2 — `checkbox_radiobutton_hide` hides two of its widgets
through a `/Hide` **OpenAction** we do not execute, so we outline widgets the
oracle has already hidden. **Port `SetAsPushButton` and the `/Hide` action
first; then all three land together.**

### The tail after wave 10

88 `pixel-fail`, and **69 unique documents** below 0.99 once the `.in`/`.pdf`
pairs are collapsed — 83 before this wave. It is a thin tail: eight documents
below 0.90, and **36 of the 69 sit between 0.98 and 0.99**.

| band | docs |
|---|---:|
| < 0.80 | 2 |
| 0.80 – 0.90 | 6 |
| 0.90 – 0.95 | 8 |
| 0.95 – 0.98 | 17 |
| 0.98 – 0.99 | 36 |

The 13 remaining `--annot` artifacts are three named things and none of them is
the text body: seven are the `/AP`-presence rule above, four are
`SetAsPushButton`, and two are the annotation font map's per-character fallback
to a second face, which `bug_725389` needs to set Hebrew in a field whose `/DA`
names a font its `/DR` does not carry.

#### Wave 9's largest tail cluster was mislabelled, and it is not resampling

Wave 9 filed 27 documents as "image resample residue" — "mean absolute residue
2.0 counts with a +0.96 bias", "identical gradient statistics", and a
do-not-retry note against looking for a filter-kernel difference. Fourteen of
the 27 are the `FRC_8.2.4_part1` cluster, which is one document written
fourteen times with different byte-level mutations; all fourteen score exactly
0.984551.

Measured on `FRC_1_8.2.4_Type_8.6_` under the determinism triple, the residue
is **not** two counts and **not** a resample. It is two things:

- **Edge antialiasing on the logo**, a one-pixel rim around every shape, which
  is the rasterizer's coverage and not an image filter. The page's only image
  is a vector logo drawn as paths.
- **A bold run set with a regular face.** The page's `/DA` names `Arial,Bold`,
  which substitutes to *Arimo* with a requested weight of 700 — the face is
  the regular one and the weight is what asks for synthetic emboldening. The
  visible tell is a line reading "Foxit **Reader** or Foxit **PhantomPDF**" in
  the golden and "Foxit *Reader* or Foxit *PhantomPDF*" here, alternating
  because one run's font is embedded and bold and the other's is substituted.

Wave 9's own rule applies to wave 9: do not take the band's label from the
previous wave. The correct label for these fourteen is **synthetic
emboldening**, and the next section is why it is still not implemented.

### Two negative results, measured rather than assumed

**Synthetic emboldening reaches one document, and synthetic italic reaches
none.** `SubstFont::embolden_level_for_render`, `embolden_level_for_load` and
`effective_skew` are complete, correct and pinned by their own tests, and
**none of them has a call site**. The obvious conclusion — that a fifth
producer-without-consumer had been found — was measured before it was acted
on, over all 1234 `.pdf` corpus files (the 441 `.in` templates were out of
scope):

| | fonts | files | below 0.99 | unique docs below 0.99 |
|---|---:|---:|---:|---:|
| `embolden_level_for_load() > 0` | 62 / 1589 | 38 / 1234 | 14 | **1** |
| `effective_skew() != 0` | **0** | **0** | 0 | 0 |

Every embolden hit is the same font in the same directory, and the fourteen
below-threshold files are the one `FRC_8.2.4` document repeated. The other 24
already pass at 0.9956.

Italic is structurally zero and that is not a sampling artifact. Twenty-nine
substituted fonts ask for italic — `Helvetica-Oblique`, `Times-BoldItalic`,
`Courier-Oblique` and the rest — and **all twenty-nine resolve to a real
italic face**, because `third_party/test_fonts` ships genuine `-Italic` and
`-BoldItalic` faces for all three Croscore families. `configure_external` sets
an angle only when the request is italic *and the face is not*, so the branch
never fires. Synthetic italic is unreachable against this corpus by
construction.

Porting `FT_Outline_EmboldenXY` — a fixed-point per-point dilation along
lateral bisectors, over FreeType's own point-and-tag representation, which our
`kurbo::BezPath` outlines no longer carry — for one document that is already
at 0.9846, with no guarantee the residual is even stem weight, is not
justified. Both functions stay, unused and documented as such, because they
are correct and because a different font set would reach them.

### What the next wave should not do again

- **Do not trust a port's own explanation of a surprising result.** Four tests
  and three paragraphs asserted that automatic font sizing returns a size that
  overflows on purpose. It was an inverted comparator, and it had been shipping
  since the module was written.
- **Do not measure a generated appearance with a stock face.** The widths are
  right and the ascent is not, and the ascent is what stacks the lines. This is
  the second time a "we only need the metrics" shortcut has cost a wave: the
  first was wave 9's determinism triple.
- **Do not tighten the `/AP`-presence rule, or add the invalid-appearance grey
  outline, before porting `SetAsPushButton` and the `/Hide` action.** Both are
  correct, both are measured, and both cost more than they earn today.
- **Do not look for the widget text body in the tail.** It is gone.
- **Do not look for a resample kernel — or a resample — in the `FRC_8.2.4`
  cluster.** Wave 9's do-not-retry note there was right about the conclusion
  and wrong about the subject: the fourteen files are one document, their
  residual is edge antialiasing plus a bold run set with a regular face, and
  the page's only image is a vector logo.
- **Do not port `FT_Outline_EmboldenXY`, and do not implement synthetic
  italic.** Measured over all 1234 corpus `.pdf`s: emboldening gates one unique
  document that already scores 0.9846, and italic gates nothing at all because
  the hermetic font set ships real italic faces for every family that is asked
  for. The unused `SubstFont` methods are correct; they are simply unreachable
  here.
- **Do not read a `--png` glyph trace as the whole story.** Instrumenting
  `place_glyphs` on `en_fqa` reports 32 runs and two glyphs on a page that
  visibly draws thousands, so the counting is measuring something other than
  what reaches the raster. Whatever the explanation, the trace is not one, and
  a wave that starts from it will start from a wrong number.


## Wave 11: the line, and a kern that had been one place out since M3

M5's exit criterion is **met**. The wave crossed it on a change that was not
on its worklist, and three of the four things it *was* pointed at turned out
to have been described wrongly by the wave that filed them.

### A kerning adjustment follows its string

`TextSegment::kerning` is documented — in its own field comment — as "the
adjustment following this string", and `build.rs` fills it that way: a
*leading* `TJ` number goes to the text object's starting position instead,
which is what says the field cannot also mean "before". All three pen walks in
`pdfrum-render/src/text.rs` read it at the **top** of the segment loop. Every
kerned run therefore double-counted its leading adjustment and dropped its
last one, sitting one adjustment out of place for as long as the rasterizer
has existed.

`pdfrum-text` had it right, and says so at length — `SetSegments` even zeroes
the final kerning "because there is no glyph after it to displace". The two
modules disagreed in the same repository across nine waves.

Nothing caught it because nothing could. Tier A does not report positions; a
run displaced by one kern still contains every glyph it should, in order, with
the right widths, so text extraction matched throughout. And the displacement
is a fraction of an em — large enough to move SSIM, small enough that no page
looks broken.

**`bug_1922` .9275 → .99998, and 160 further files moved.** +15 at 0.99, +7 at
0.95, +15 passing, from moving one line to the bottom of three loops.

The lesson is narrower than "check the comment against the code", because the
comment was right and the code was wrong beneath it: it is that **a field
whose meaning is stated in one crate and assumed in another is a defect
waiting for a tier that can see it**. There was no test in `pdfrum-render`
asserting where a kern lands, and there is one now.

### The three annot items landed, and the fourth was hiding under them

Wave 10 measured `SetAsPushButton`, the `/AP`-presence rule and the grey
invalid-appearance outline as individually negative and jointly positive, and
that held — but only after two corrections it could not have made from the
outside.

**The `/AP`-presence rule and the grey outline are two different tests, and
the second is the deeper one.** `IsAppearanceValid` is `!!GetDictFor("AP")`
and gates *regeneration*. `IsWidgetAppearanceValid` resolves `/AP /N /<AS>`
and requires a **stream**, and gates the grey `0xFFAAAAAA` outline
`DrawAppearance` strokes over a checkbox or radio button — nothing else.
A radio button whose `/AP /N` lists only its on-state while `/AS` reads `Off`
passes the first and fails the second: it keeps having no appearance *and*
gets outlined. Porting either alone loses; that is the whole of wave 10's
"land them together".

**And the outline is a stroke with no fill.** `DrawPath` is handed fill argb
**0** beside the grey stroke, so its `EvenOddOptions()` names a rule for a
fill that never happens. Spelling that `FillRule::EvenOdd` here paints the
box solid instead of outlining it — five files down, measured, before the
transparent fill was noticed. The joint change went from −5 to +6 on that one
enum variant.

**`/Hide` had to come first, and it is an `/OpenAction`.** `pdfium_test` runs
`FORM_DoDocumentOpenAction` before it renders anything
(`pdfium_test.cc:1779`), so a `/Hide` in the catalog's open action has already
rewritten `/F` by the time either the dump or the render reads it. That is why
`checkbox_radiobutton_hide`'s golden reports `Flags set: Hidden` for two
widgets whose file sets no `/F` at all, and why outlining them would have been
the joint change's own new defect. `nav::open_action` executes exactly that
one action type, because it is the only one of the eighteen a rendered page or
a dump can observe without a user, a script engine or a viewer chrome.

The edit is not "set the hidden bit": `Invisible` and `NoView` are cleared
whichever way the action goes, `/H` defaults to **true**, the fields are named
in `/T` rather than `/Fields`, and only a **string** entry names one — a `/T`
holding dictionary references resolves to nothing, which the test file's own
comment records as surprising and which is behavior.

**The push-button caption is a caption and nothing else.** `SetAsPushButton`
lays out a caption beside an **icon** in one of seven arrangements chosen by
`/MK /TP`, scaled by `/MK /IF`. Six of the seven and the whole icon half are
**unreachable on this corpus**: no file carries a `/MK /I`, `/RI` or `/IX`,
and with no icon stream every split arrangement collapses to `rcLabel =
rcBBox` — the caption-only box. The two files that name a `/TP` carry no
`/MK /CA`, so both produce nothing on any path. What is built is the caption,
with the three things about it that *are* behavior: both alignments are
hard-coded centred (a push button ignores the `/Q` every other text field
obeys), a missing `/DA` means size **12** rather than the automatic size the
other three read it as, and the clip is written unconditionally.

### `/NeedAppearances` always regenerates; it is usually invisible

This is the wave's one genuinely surprising finding, and it was settled with
gdb on the oracle rather than by reading.

`NewAnnot` calls `ResetAppearance` unconditionally when the form sets
`/NeedAppearances`, consulting no `/AP`. Honouring that plainly moves
`bug_707673` the right way and takes **`bug_861842` from .99986 to .93548** —
that file sets the flag, and its checkbox's golden reports one `Text` object
with no background and no border, which is the file's own ZapfDingbats stream
surviving untouched. Both files reach `ResetAppearance`. Only one of them
shows it.

The gate is a key mismatch. `SetAsCheckBox` and `SetAsRadioButton` write
exactly two sub-states, `/AP /N /<GetCheckedAPState()>` and `/AP /N /Off`,
while every reader — the dump and the renderer alike — resolves
`/AP /N /<AS>`. When `/AS` names neither, the new streams land in keys nothing
looks up.

And `GetCheckedAPState` diverges from `/AS` far more readily than it looks: it
answers the first non-`Off` key of `/AP /N`, **unless the field carries an
`/Opt` array**, in which case it answers the widget's *control index* as a
decimal string. `bug_861842` is that file — `/Opt` present, index 0, `/AS /1`
— so the rebuild writes `/0` and `/Off` while the reader keeps asking for
`/1`. `bug_707673`'s radios carry no `/Opt` and sit at `/AS /Off`, and `Off`
is always written literally, so they do rebuild.

Two one-line mutations prove it: changing `/Opt` to `/XOpt`, or `/AS /1` to
`/AS /0`, each makes `bug_861842` regenerate visibly, and to the same bytes.

With the rule in, `bug_707673` reaches **.99982** and its `--annot` dump is
byte-exact, and nothing else moves.

### Two widgets sharing a `/T` are one field

`bug_733528` lists two `/Fields` entries both named `SharedField`, the first
holding `/V (Hello, world)` and the second `/V ()`, with no `/Parent` between
them. The golden reports the **second** drawing the first's text.

`AddTerminalField` looks a name up before it builds anything and only
constructs a `CPDF_FormField` when the name is new, so the second entry
becomes a second *control* of the first's field. The value every control shows
is the field's, which is the first dictionary's. Our field tree emitted two
fields with one widget each, and the appearance generator read `/V` off
whichever widget it was drawing.

Two changes, and the split between them is the point: `Form::load` now merges
by name, and the *body* builders read their value, options and selection from
the **field's** dictionary while everything else — the plate, the colour, the
default appearance — stays read off the widget, because those are per-control.

`bug_733528` .9828 → .99978, and its `--annot` dump is byte-exact.

### A JBIG2 stencil was never handed to the codec

`decode_stencil` unpacked its stream as raw one-bit samples whatever the
filter chain said — and `decode_chain` hands a JBIG2 image back **undecoded**
by design, because an image codec needs the image dictionary the chain does
not have. So a JBIG2-coded `/ImageMask` had its *codestream* read as if it
were already the bitmap.

`bug_527174` is one data byte, `0x30`, which inverts under the default
`/Decode` to `0xCF`; its top bit is set, so the one-pixel image came out black
and `150 0 0 150 200 200 cm` painted it as a solid 150×150 square across a
region the oracle leaves white.

This was a hole, not a quirk. Every JBIG2 image *with* a `/ColorSpace` was
already on the working path — the stencil rung simply never dispatched to a
codec at all, and no corpus file had previously reached it. Our own wrapper
refuses this codestream too, so the refusal was always available; nothing
asked for it.

The rejection is also more total than it looks. `ContinueLoadDIBBase` tears
the half-built bitmap down and returns `kFail`, so a `/JBIG2Globals` of binary
garbage makes the whole image vanish even though the image is one pixel: the
globals are parsed first, and their failure is the image's failure.

**bug_527174 .94814 → byte-exact**, two more files reach byte-exact, +5
byte-exact overall.

### A knockout buffer's offset was composing inside the stroke split

The fill drew `offset * to_device * path` against an identity device matrix,
while the stroke handed the rasterizer `offset * (pre * path)` with `post` as
its transform — so what the rasterizer actually computed was
`post * offset * pre * path`, with the buffer's translation pushed *through*
`post`. `split_for_stroke` zeroes the translation there, leaving a bare
y-flip, so on a 200-high page `translate(-45,-45)` came back out as
`translate(-45,+45)`: the stroke landed ninety rows low and was clipped off
the bottom of the buffer it was drawn into.

Moving `offset` onto the transform argument composes it outermost and makes
the geometry identical to the non-knockout path, which it should always have
matched.

`same_color_knockout_fill` .8931 → .9502, and five further files cross 0.99 —
`bug_1430333`, `single_point_paths` and `path_9` among them.

### Three corrections to wave 10's map

Wave 10's do-not-retry notes were honoured, and three of them were wrong about
their subject in a way that mattered.

**The `FRC_8.2.4` cluster is 74% an image kernel, not the bold run.**
Segmenting the residual spatially: the vector-looking "logo" is a 455×455 JPEG
downscaled 2.86×, and it carries **74.1%** of the deficit; the bold text
carries 25.9%. Fixing the text alone leaves the file at .9886 — still failing.
Fixing the graphic alone reaches .9960 — passing. The prior note had the
weighting backwards.

**And the bold run is the wrong face, not a missing embolden.** Per-word ink
measurement splits cleanly: the embedded `Arial-BoldMT` words sit at ratio
1.006, the non-embedded `Arial,Bold` words at 0.699/0.683/0.704 — and the
Arimo-Regular:Arimo-Bold ink ratio is **0.687**. Rendering against a font dir
holding only `Arimo-Bold.ttf` gives 1.005 with no emboldening applied at all.
The computed embolden level for the page is 14/64 px, far too small to explain
a 30% ink deficit. There is also **no cheap stroke-based pseudo-bold in the
oracle**: `FT_Outline_Embolden` is a genuine point dilation, and the only
`IsSubstFontBold` consumer is the Skia device this build does not use.

**And `bug_725389` is not a two-slot font map.** The `--annot` path for a
widget whose `/DR` sits on the annotation rather than the AcroForm never
reaches `GenerateFormAP` at all — `cpdf_generateap.cpp:1469` reads `/DR` off
the **AcroForm** dict, finds none, and returns. What renders is
`CPDFSDK_AppStream`, over `CPDF_BAFontMap`, which is the N-slot charset-driven
map with no `IS_WIN` guards anywhere. Verified under gdb: slot 0 is
`Helvetica_00` → `Arimo-Regular.ttf`, slot 1 is **`_B1`** — the empty face
name plus `sprintf("_%02X", FX_Charset::kMSWin_Hebrew)` — resolving through
the hermetic renamer to **`Tinos-Regular.ttf`**, with a CP1255 `/Differences`
encoding. The six text objects the golden reports are one per character,
because the run is RTL and `is_rtl` disables the run-grouping optimization.

### A correct change that measured as a regression, and the feature under it

The most useful thing this wave found is a change that was right, measured
negative, and turned out to be blocked by a *different* missing feature
underneath it. Landing the blocker first turned a −2 into a +2, and the pair
is the wave's cleanest illustration that a measurement is a statement about
the whole tree rather than about the diff.

**The Croscore rename belongs at the database boundary.**
`RenameFontForTesting` lives in
`SystemFontInfoWrapper` (`testing/test_fonts.cpp:47-72`), which wraps
`SystemFontInfoIface` — **below** `FindSubstFace`, on the `face` argument of
`MapFont`/`GetFont` only, and *not* on `EnumFontList`. Ours applied it at step
0, before `subst_name`, so the whole ladder saw the renamed string.

That is why `Arial,Bold` lost its bold. The name is an entry in
`kAltFontNames`, so `GetSubstName` canonicalizes the **whole** name to
`Helvetica-Bold` — the comma never reaches the comma-split — which is base-14
index 5, and *that index* is what carries `ForceBold` and weight 700 down to
`MapFont("Helvetica-Bold")`, where the wrapper renames it to `Arimo Bold` and
`FindFont` direct-hits at the perfect score. Renaming early turned it into
`ArimoBold`, which is no alias, and base-14 recognition, the charset decision
and the bold went with it.

A `CroscoreDb` decorator renaming only `find_font`/`font_by_name` — with
`faces`/`face_bytes` passing through, mirroring the untouched `EnumFontList` —
restores all of it. Measured alone: the whole `FRC_8.2.4` family +0.0031, a
second FRC group +0.0034, and the target's ink ratio against the golden goes
**0.699 → 0.994**. And it was **−2 at 0.99 anyway**, because `bug_601362`
dropped .99078 → .98788: with the *correct* face finally chosen, that file
started depending on a correction we did not implement. The old code had been
scoring higher there by accident, on the wrong face — confirmed by forcing the
identical face into both engines through a single-font `--font-dir`, which
leaves the pixel delta unchanged.

**The correction it wanted is the §1.15 glyph-spacing heuristic**, and its
gate had been sitting in the tree as dead code since the module was written:
`SubstFont::is_actual_font_loaded`, whose doc comment reads "the glyph-spacing
heuristic of §1.15 turns on it", beside `HasFontWidths` and the
built-in-generic flag, ported for the same absent caller.

The rule applies when a non-embedded font carries its own `/Widths`, is not a
standard-14 name, did not resolve to a built-in generic, and the face that
loaded is *not* the family the document asked for — that last condition being
the whole point, since a correction only makes sense when the metrics come
from a face nobody asked for. Its two branches are asymmetric and that is
behavior: a glyph **narrower** than its declared slot is centred in it, origin
moved by half the excess and the outline untouched; a glyph **wider** than its
declared width is squeezed, scaled horizontally about its origin with the
origin left alone. Neither touches the pen, so the run's advances stay the
PDF's.

Landed first, it is +2 at 0.99 on its own (`bug_845697` .97800 → .99999,
`bug_601362` .99078 → .99357), and the Croscore fix on top of it is then 41
files up and none down, with `bug_601362` reaching **.99912**. Together: **55
files moved, every one upward.**

**The Type3 translucent buffer should be sized from the painted extent, and
cannot go in until CCITT is wired up.** This one stays reverted. `render_translucent_char_proc` sizes
its sub-buffer from `Type3Metrics::bbox`, which is the *declared* `d1` box
scaled by 1000 and then transformed by the font matrix. Those two cancel only
for the conventional `/FontMatrix [0.001 …]`; `bug_1746` declares an identity
one, so the box stays a thousand times too large, lands at device x
8050–96050, intersects to zero, and the glyph is never drawn. The oracle sizes
that rect from `matrix.TransformRect(pForm->CalcBoundingBox())` — the form's
own **painted** extent, unscaled.

Adding a separate `painted_box` (rather than patching `bbox`, which text
extraction correctly wants as the declared box) fixes the placement exactly:
the rects move to x 58.2–146.2, matching the golden's painted union. And the
file still goes **.83506 → .76463**, because `decode_ccitt` in
`pdfrum-filters` **has no caller** — `chain.rs` has no CCITT arm — so this
file's `/CCITTFaxDecode` image mask unpacks still-encoded bytes into garbage
covering 31 of its 92 rows. Confirmed independent of the change by rendering
an opaque variant of the same file, which truncates identically on the
untouched path. Blank scores better against a mostly-white golden than
correctly-placed garbage.

So `bug_1746` is two defects deep, and the outer one is a whole unwired
filter. That is the item to take first.

### The tail after wave 11

The bands, over the documents left below 0.99 once `.in`/`.pdf` pairs are
collapsed:

| band | docs |
|---|---:|
| < 0.80 | 1 |
| 0.80 – 0.90 | 5 |
| 0.90 – 0.95 | 2 |
| 0.95 – 0.98 | 13 |
| 0.98 – 0.99 | 30 |

**62 files, 51 unique documents** — down from 88 files and 69 documents. The
tail is now overwhelmingly shallow: 43 of the 51 sit above 0.95, and 30 of
those above 0.98.

Two items are external and stay documented rather than fixed:

- **`bug_867501`** (0.646) is an upstream `hayro-jbig2` gap — a segment type
  the crate does not decode. Our entry point is a wrapper (SPEC §12); the
  first-party port stays the sanctioned fallback if this ever justifies one.
- **`example_063`** cleared 0.99 on the kerning fix and is no longer in the
  tail; its residual was the displacement, not the D7 coverage difference it
  had been filed under.

Three named items account for most of what is left, and each is a feature
rather than a bug:

- **The image downscale kernel**, which is one defect with two names.
  `CStretchEngine::WeightTable::CalculateWeights` takes a 2-tap bilinear
  branch only when `|scale| < 1`; for a downscale it falls to an
  **area-average box filter** over every source pixel in the destination
  footprint. We delegate to the backend's 2-tap bilinear either way and
  under-blur. That is 74% of the `FRC_8.2.4` cluster's 14 files, all of
  `example_009`, and a share of several others in the 0.95–0.98 band. It is a
  subsystem rather than a patch, and it is the largest single item left.
- **CCITT has no caller.** `pdfrum-filters` implements `decode_ccitt` and
  `chain.rs` never dispatches to it, so a `/CCITTFaxDecode` image unpacks its
  still-encoded bytes. `bug_1746` is the visible one; it also blocks the
  Type3 `painted_box` fix above.
- **The §1.15 glyph-spacing heuristic**, which gates the Croscore boundary fix
  above and is what `SubstFont::is_actual_font_loaded` was ported for.

### What the next wave should not do again

- **Do not trust a band's label from the wave that filed it — including this
  one.** Wave 9 mislabelled the `FRC_8.2.4` cluster as resampling; wave 10
  corrected it to emboldening and got the weighting backwards *and* the
  mechanism wrong. The residual is 74% an image kernel and the text half is a
  face-selection bug. Three waves, three labels, one measurement.
- **Do not port `FT_Outline_EmboldenXY`.** Wave 10's conclusion survives its
  own wrong reasoning: emboldening is not what the bold runs need, there is no
  cheap stroke-based version of it in the oracle, and the face-selection fix
  is what moves them.
- **Do not implement `bug_725389`'s fallback as a two-slot map.** It is
  `CPDF_BAFontMap`, N slots, charset-driven, and the second face is
  `Tinos-Regular` under the alias `_B1`. The `CPVT_FontMap` reading is a
  different code path that this file never enters.
- **Do not honour `/NeedAppearances` without the sub-state rule.** The rebuild
  always runs and is usually invisible; replacing a good stream with generated
  chrome because the flag is set costs more than it earns.
- **Do not assume a page-object field means the same thing in two crates.**
  The kerning inversion sat between `pdfrum-page`'s definition and
  `pdfrum-render`'s use, with `pdfrum-text` reading it correctly the whole
  time.
- **Do not re-derive the Type3 `painted_box` change.** It is correct, it is
  measured, and it is blocked on CCITT having a caller. Land the blocker, then
  the change. (Its sibling, the Croscore boundary fix, was blocked the same way
  on the §1.15 heuristic and both landed once that was written.)
- **A change that measures negative may be right and blocked.** The Croscore
  fix was −2 at 0.99 on its own and +2 with the heuristic under it, and
  nothing about the diff said which. When a fix you have verified against the
  oracle measures backwards on one file, ask what that file was depending on.
- **Do not read `bug_1746` as a Type3 defect.** Its outer cause is that
  `decode_ccitt` is never called — `pdfrum-filters`'s chain has no CCITT arm
  at all. A correct Type3 fix makes the file *worse*, because it replaces
  "draws nothing" with "draws garbage".
- **Do not measure in the shared working tree while other agents edit it.**
  Three of this wave's measurements were confounded that way, one of them
  badly enough to invent two file movements that did not exist. Use a
  `git worktree` at HEAD with its own `CARGO_TARGET_DIR`, or copy-aside plus
  `git checkout --` on your own files. **Never `git stash`** — it is
  tree-global and will take another agent's work with it.
- **A dead-code function with a doc comment naming its future caller is a
  standing invitation.** `SubstFont::is_actual_font_loaded` was ported, tested,
  and documented as "the glyph-spacing heuristic of §1.15 turns on it" —
  and the heuristic was never written, which is what left `bug_601362`
  depending on a wrong face for its score.


## Wave 12 (M9): the downscale kernel, and three things standing behind it

M9's worklist was wave 11's tail inventory. Four of its five items landed; the
fifth — `en_fqa` — was re-derived from scratch, and the answer was that it had
never been a text file at all.

### The reduction was the largest item, and rounding it up was half of it

`CStretchEngine::WeightTable::CalculateWeights` takes a two-tap bilinear branch
only when `|scale| < 1`. For a *reduction* it falls to an area-average box
filter over every source pixel the destination pixel's footprint overlaps,
weighted by the overlap area, with the fractional residue of each weight
carried into the next and the unspent remainder landing on the final tap — so
one destination pixel's weights sum to exactly `1 << 16` and the accumulation
shifts rather than divides. That invariant is the only thing `cstretchengine_
unittest.cpp` asserts about the table, and it is what keeps a flat field flat.

We delegated to the backend either way and under-blurred. The filter now runs
as a **pre-pass in the engine** (`stretch::prescale`), not in either backend:
both must resample identically or Tier C's interior rule fails, and a pre-pass
leaves one implementation shared by construction. The backend still does the
last sub-pixel placement, which is the only part of the pipeline that knows
where the image lands — so the two agree on the low-pass, which a two-tap
filter cannot do at all, and differ by at most one two-tap interpolation.

**The destination size rounds up, not to nearest, and that is worth as much as
the filter.** A footprint of 9.72 device pixels starting at a fraction covers
eleven device rows; reducing to ten leaves the backend a sample short of the
outermost two, and an edge row arrives *empty* rather than faint. Rounding to
nearest first: **1572 at 0.99**. Rounding up: **1586**. One word, fourteen
files.

### `en_fqa` is not a text file, and wave 10's instrumentation was not why

Wave 10's trace reported 32 runs and 2 glyphs on a page it assumed drew
thousands, and was recorded as unreliable. It was not: **the page really does
draw almost no text**. Its one font is `/FirstChar 32 /LastChar 32` — a
one-glyph font that can render a space — and its six `TJ` operators each show
exactly one. Every visible line of type is a 2x2 colour swatch whose `/SMask`
is a 40x3285-wide, 81-to-86-row 1-bit bitmap carrying the glyph shapes, drawn
through a `cm` that minifies it **8.33x in both axes**, 276 times across the
document. The instrumentation was right and the inference from it was wrong.

Per-band ink ratios cluster at 0.91 with the deficit negative in 34 of 35
bands, and per-row measurement puts it precisely: interior rows match at
0.96–1.16 while the top and bottom row of every line of type collapses to
0.28–0.34. That is the rounding above, on 276 masks at once. **.885 → .921**,
with no change to anything in the text pipeline.

### CCITT had a decoder and no caller, and the repack is most of the work

`pdfrum-filters` classes `/CCITTFaxDecode` as an image codec and hands its
bytes back undecoded, because a codec needs the image dictionary the chain does
not have — and the image path answered with an error rather than a call. It now
decodes on both rungs.

The bit sense needed nothing: the fax decoder fills a row white and clears bits
for black, `/BlackIs1` inverts, and that is already what a one-bit
`/DeviceGray` sample means. The **stride** needed everything. A fax row is
padded to four bytes; every consumer here reads rows at `width.div_ceil(8)`.
At width 20 those are 4 and 3, and reading the wide buffer at the narrow stride
starts each row a byte further into the previous one — a shear that grows down
the page. A test over a uniform image cannot see it; the regression test uses
a row of eight black pixels for that reason, and fails when the stride is
sabotaged.

### `bug_1746`'s second factor of two was where the alpha is applied

Wave 8 measured the `painted_box` fix as making the file *worse* — .835 → .765
— because the glyph appeared at half the alpha the oracle gives it, and filed
it as "a second `ca 0.5` we do not reproduce". There is no second factor. There
is one factor applied in the wrong place.

The oracle renders a translucent Type 3 procedure into its buffer at the text
object's **alpha-carrying colour** and blits that buffer *opaquely*
(`SetFillColor(fill_argb)` then `SetDIBits`). We forced the buffer opaque and
re-applied at the blit. Both are one factor of alpha, and they differ wherever
the procedure covers a pixel partially: an edge pixel painted at alpha 128 and
blitted opaquely keeps 128, while one painted opaquely to coverage 128 and
blitted at half alpha becomes 64.

With the alpha moved and the buffer sized from `painted` — the objects' own
extent, which an identity `/FontMatrix` cannot push off the page the way the
declared box does — **`bug_1746` .835 → .952**, and nothing else moves. Wave 11
was right that CCITT had to land first: without it the glyph draws garbage.

### Two of the four annot artifacts were not a font map at all

`example_014` and `example_054` reported their colour keys unreadable where the
oracle reads them. The cause is one rule: **a form field carrying `/Kids` is
not a control.** `AddControl` is reached only for a field dict with no `/Kids`
and otherwise for each kid instead (`cpdf_interactiveform.cpp:969-981`), so a
`/Btn` parent with `/Rect [0 0 0 0]` has no appearance to build — and building
one anyway produced an empty stream over an empty box, invisible on the page
and enough to make the dump report the keys as unreadable, because any
appearance outranks them.

A first attempt gated on `/AS` instead, from `CPDF_AnnotList`'s `GenerateAP`.
That is a different rule for a different thing — it is an `/AS` *inheritance*
fixup, not an appearance gate — and it suppressed six real checkbox widgets.
Measured, reverted, replaced.

### `bug_725389` needed the fallthrough, not the N-slot map

Wave 11 characterised this under gdb as `CPDF_BAFontMap`: N slots, charset-
driven, slot 1 the alias `_B1` resolving to `Tinos-Regular` with a CP1255
`/Differences`. That characterisation is correct and the port is large — a
numeric `FX_Charset`, the eight hi-byte tables, a charset-driven font request,
a `HasInstalledFont` predicate, a `/Differences` writer, and a multi-font `Tf`
emitter, none of which exist.

None of it is *observable* here. The map's ladder ends in
`CPWL_EditImpl::GetPDFWordString`'s fallthrough, which appends the raw code
point as a character code when no slot knows the character — and on a hermetic
font set no second face is found, so the fallthrough is the whole of the
behaviour. Writing the raw code point takes the annotation dump from three text
objects to six and closes the tier.

The width had to follow the same code: a simple font's codes are one byte and
the emitter truncates to one, so measuring the untruncated code point advanced
the layout past characters the stream still contained.

**The `--annot` tier is now clean across all 1675 files.** It costs
`bug_725389` .0093 of pixel SSIM — the glyph that draws is wrong, the object
exists — and it stays in the same band. Whether the N-slot map is ever worth
building is a question this corpus cannot ask.

### The negative result: snapping an image to its outer integer rect

`CPDF_ImageRenderer` takes `GetUnitRect().GetOuterRect()` and stretches to
`rect.Width()` by `rect.Height()`, so every device pixel an axis-aligned image
touches is *filled* and its rim carries no partial coverage. Ours antialiases
that rim, and on `FRC_8.2.4` the boundary ring is 1.6 of a 4.74 mean residual.

Reproducing it measures **+4 files at 0.99 and +14 byte-exact — and regresses
four**: `image_foxit` .996 → .969, `2_halftone` .992 → .967, `bug_642` .992 →
.987. A guard meant to fire only on a fractional placement did not fire on
`image_foxit`, whose 364x140 image lands on 273x105 at the origin: the rect is
integral in arithmetic and not in floating point, and `273/364 * 364` is not
273. Reverted rather than tuned, because the four files it costs are real and
the files it was aimed at do not need it — see below.

### Where the remaining 42 are, measured rather than guessed

Four files sitting just under the threshold were segmented spatially, and they
share **no** cause:

| file | mechanism | evidence |
|---|---|---:|
| `bug_691967` (.9895) | image downscale, **interior** | border ring 2.8% of px differ at meanD 1.5; interior 16.5% at meanD 17.4 |
| `123` (.9892) | JPX decoder colour, not geometry | border ring **0 of 2890 px** differ; interior 97%, mode difference 3 |
| `example_057` (.9897) | vector stroke antialiasing | partial-coverage buckets carry 48.5% of error on 37% of pixels; text is 39% of pixels and 5% of error |
| `checkboxes` (.9899) | ZapfDingbats glyph *shape* | 3 of 6 widgets swap black and white outright (93% of error); the other 3 differ by one LSB of background colour |

The outer-rect issue appears in **none** of them. That is why the snap was
reverted rather than repaired: it is aimed at a residual these files do not
have.

### The tail after wave 12

**42 files, 31 unique documents** once `.in`/`.pdf` pairs collapse — down from
62 files and 51 documents. Every one, with its cause:

| ssim | document | cause |
|---:|---|---|
| .6457 | `bug_867501` | upstream `hayro-jbig2`: segment bodies sliced to a declared length of zero. Issue drafted at `docs/upstream/hayro/jbig2-segment-lengths.md` |
| .8710 | `bug_1772` | image transform: a general (sheared) matrix takes the non-axis-aligned path, which the reduction pre-pass declines |
| .8751 | `image_transformer_other` | same: `CFX_ImageTransformer`'s rotate/shear resampler is not ported |
| .9208 | `en_fqa` | 276 masks minified 8.33x; residual is the two-stage phase difference the pre-pass leaves, plus the un-snapped rim |
| .9224 | `vertical_text` | vertical writing mode: per-glyph origin displacement `/W2` defaults are approximated |
| .9325 | `2_uncolor_tiling` | uncoloured tiling pattern cell rasterized once and blitted; cell-boundary coverage differs |
| .9464 | `example_009` | image downscale interior, the same residual as `bug_691967` at larger scale |
| .9502 | `same_color_knockout_fill` | knockout group: same-colour fill and stroke composite order |
| .9508 | `bug_1442723` | text extraction disagreement (tierA) plus glyph coverage |
| .9524 | `bug_1746` | Type 3 fax mask; the alpha and box are fixed, the residual is the mask's own rim |
| .9540 | `en_system` | substituted-face glyph shapes (no embedded font) |
| .9621 | `3bigpreview` | image downscale interior |
| .9670 | `quick_start_guide` | substituted-face glyph shapes |
| .9674 | `bug_725389` | the wrong glyph draws for three Hebrew characters; see above |
| .9693 | `bug_665467` | shading mesh interior interpolation |
| .9710 | `example_010` | vector stroke antialiasing |
| .9751 | `1_image` | image downscale interior |
| .9779 | `path_7` | vector antialiasing on curve edges |
| .9805 | `bug_1395648` | glyph coverage on a small size |
| .9806 | `rotated_image` | rotated image: the non-axis-aligned resampler again |
| .9817 | `transparent1` | group transparency compositing rounding |
| .9830 | `example_025` | vector stroke antialiasing |
| .9831 | `linearized` | glyph coverage |
| .9855 | `clipping_text` | text-clip coverage at the clip edge |
| .9875 | `transformation` | vector antialiasing under a general matrix |
| .9876 | `lines` | thin-line antialiasing |
| .9879 | `example_058` | vector stroke antialiasing |
| .9892 | `123` | JPX decoder LSB colour, measured: rim is bit-exact |
| .9895 | `bug_691967` | image downscale interior, measured |
| .9897 | `example_057` | vector stroke antialiasing, measured |
| .9899 | `checkboxes` | ZapfDingbats caption glyph shape, measured |

Nine `tierA` mismatches remain, all `input.pdf.*.txt` — text extraction, not
rendering and not annotations: `1_10_watermark`, `example_055`, `example_062`,
`bug_1769`, `bug_1388_3`, `bug_1442723`.

### What the next wave should not do again

- **Do not snap an image to its outer integer rect without measuring the four
  files it costs.** The behaviour is real and upstream, the residual it removes
  is real, and it is still a net loss as written. If it is retried, the guard
  must tolerate floating-point error in "already grid-aligned", and
  `image_foxit`, `2_halftone` and `bug_642` are the files that say whether it
  worked.
- **Do not read `en_fqa` as a text file.** It is 87 image draws and six spaces.
  Wave 10's instrumentation was accurate and its conclusion was not; the
  mechanism is the image path from end to end.
- **Do not port `CPDF_BAFontMap` for `bug_725389`.** The map's whole
  observable contribution on a hermetic font set is
  `GetPDFWordString`'s raw-code-point fallthrough, which is three lines. The
  N-slot machinery would change the *glyph*, which no tier measures, and
  nothing else.
- **Do not gate widget appearance generation on `/AS`.** `CPDF_AnnotList::
  GenerateAP`'s `/Btn` arm is an `/AS` inheritance fixup, not an appearance
  gate; using it as one suppresses six real checkbox widgets. The gate that
  works is `/Kids`.
- **Do not test a repack with a uniform image.** The CCITT stride bug is
  invisible on an all-white bitmap and obvious on any row that is not constant.
  The first version of that test passed with the stride deliberately broken.
- **The remaining tail is four mechanisms, not one.** Image downscale interior
  (5 documents), vector stroke antialiasing (4), substituted-face glyph shape
  (4), and the non-axis-aligned image transformer (3). Each was measured
  spatially rather than inferred from a filename.


## Numbers

Measured against the golden store on the full corpus (1675 files, 1628 with
a golden PNG), rendering through `tiny-skia`. The W3 column is the pixel
burn-down's third wave; M5 is where the burn-down started.

| metric | M5 | M8 (wave 2) | W3/W4 | W5 | W6 | W7 | W8 | W9 | W10 | W11 | **W12 (M9)** |
|---|---|---|---|---|---|---|---|---|---|---|---|
| pixel files at SSIM ≥ 0.99 | 1075 / 1628 (66.0%) | 1243 / 1628 (76.4%) | 1247 / 1628 (76.6%) | 1387 / 1628 (85.2%) | 1403 / 1628 (86.2%) | 1441 / 1628 (88.5%) | 1471 / 1628 (90.4%) | 1513 / 1628 (92.9%) | 1540 / 1628 (94.6%) | 1566 / 1628 (96.2%) | **1586 / 1628 (97.4%)** |
| at SSIM ≥ 0.95 | 1478 / 1628 (90.8%) | 1518 / 1628 (93.2%) | 1520 / 1628 (93.4%) | 1554 / 1628 (95.5%) | — | 1574 / 1628 (96.7%) | 1595 / 1628 (98.0%) | 1603 / 1628 (98.5%) | 1605 / 1628 (98.6%) | 1616 / 1628 (99.3%) | **1618 / 1628 (99.4%)** |
| at SSIM ≥ 0.90 | 1544 / 1628 (94.8%) | 1570 / 1628 (96.4%) | 1572 / 1628 (96.6%) | 1591 / 1628 (97.7%) | — | 1597 / 1628 (98.1%) | 1615 / 1628 (99.2%) | 1616 / 1628 (99.3%) | 1616 / 1628 (99.3%) | 1619 / 1628 (99.4%) | **1623 / 1628 (99.7%)** |
| byte-exact PNGs | 430 / 1628 | 446 / 1628 | 451 / 1628 | 451 / 1628 | 487 / 1628 | 499 / 1628 | 511 / 1628 | 511 / 1628 | 511 / 1628 | 516 / 1628 | **519 / 1628** |
| files passing (all tiers) | 1146 / 1675 | 1256 / 1675 | 1260 / 1675 | 1396 / 1675 | 1411 / 1675 | 1448 / 1675 | 1478 / 1675 | 1510 / 1675 | 1569 / 1675 | 1602 / 1675 | **1624 / 1675** |
| `pixel-fail` | 495 | 385 | 381 | 241 | 225 | 187 | 157 | 115 | 88 | 62 | **42** |
| `size-mismatch` | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | **0** |
| Tier C hard failures | — | 5 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | **3** |

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

**Wave 8's seven fixes were each measured on their own** against the scoreboard
they started from, and every one was committed only after
`conformance run --check-regressions` reported none:

| | ≥ 0.99 | byte-exact | passing | `pixel-fail` | up / down |
|---|---:|---:|---:|---:|---|
| W7 | 1441 | 499 | 1448 | 187 | — |
| + the Coons axes, `full_cover`, the mesh ramp | 1450 | 501 | 1457 | 178 | 10 / 0 |
| + optional content | 1456 | 505 | 1463 | 172 | 8 / 0 |
| + the JBIG2 picture | 1456 | 505 | 1465 | 170 | 4 / 0 |
| + the Japan1 CID transform | 1462 | 505 | 1469 | 166 | 5 / 0 |
| + the unterminated inline image | 1465 | 507 | 1471 | 164 | 2 / 0 |
| + the form identity | 1468 | 507 | 1474 | 161 | 3 / 0 |
| + the group alpha | **1471** | **511** | **1478** | **157** | 5 / 0 |

Not one file moved down at any step, and **no byte-exact file lost its
status** at any step. The shading fix alone is a bigger single-wave movement in
the sub-0.80 band than every previous wave combined: it took the store's two
worst files and a third that three waves had misdiagnosed.

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

### The tail after wave 8

157 `pixel-fail` entries, **111 unique documents** — down from 129 at wave 7b
and 168 at wave 5.

| band | documents | what is in it |
|---|---|---|
| 0.95–0.99 | 89 | the coverage tail, and it is D7 to the file. Wave 8 did not touch this band and did not need to: everything it fixed was below 0.95 |
| 0.90–0.95 | 13 | `clipping_text`, `listbox_form`, `vertical_text`, `3bigpreview`, uncoloured tiling (0.9325), form-field singles |
| 0.80–0.90 | 7 | `bug_1746` (diagnosed above, deliberately not fixed), `bug_1772`, `image_transformer_other`, `same_color_knockout_fill`, `en_fqa`, two tcpdf |
| below 0.80 | **2** | `bug_867501` (0.646), the known `hayro-jbig2` gap that belongs upstream, and `example_063` (0.764), the residual stem distribution |

**Below 0.80 is down to two documents, and neither is a defect we should
fix here.** One is an upstream codec gap with a written-up reproduction; the
other is D7 in its purest form. The whole sub-0.90 population is nine
documents, of which one — `bug_1746` — has a half-measured diagnosis above and
is the only one with an obvious next step.

That is a different shape from every previous wave's tail. Waves 3 through 7b
each opened with a named cluster worth ten or twenty files; wave 8 emptied the
last of those, and what is left is **89 documents of glyph coverage** and about
twenty scattered singles. The next wave has no large lever below 0.95 and
should expect to work file by file, or else reopen the D7 question the burn-down
has deferred four times — which is a rendering-policy decision rather than a
bug hunt, and belongs to the orchestrator.
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

## Tint-space images

`corpus/fx/other/1.pdf` sat in the queue as "a coloured tiling pattern paints
grey where the oracle paints teal", carried forward from the render audit and
never isolated. **The attribution was wrong in every part except the colours.**
The file contains no `/Pattern` object at all — no `/PatternType`, no `scn`
with a name operand, nothing. The teal region is `Im1`, a **1x1 eight-bit
image** in

```
[/Separation /PANTONE#203278#20CV /DeviceCMYK
   <</FunctionType 2 /N 1.0 /Domain[0 1]
     /C0[0 0 0 0] /C1[1.0 0.0 0.600006 0.0]>>]
```

drawn under `106.45 0 0 127.84 51.38 247.45 cm` so that one sample covers a
144x146 device box. Its lone sample byte is `0xC6` — a tint of 198/255 =
0.7765, which the transform carries to CMYK `(0.7765, 0, 0.4659, 0)` and the
Adobe table to `(0, 182, 162)`.

We painted `(198, 198, 198)`: `unpack` buckets a decoded image on its
**component count** alone — `1 => Gray8`, `4 => Cmyk8`, `_ => Rgb8` — so a
one-component image was a grey plane and the tint byte *was* the grey level.
The tint transform never ran. Nothing about the diagnosis was pattern-shaped;
the only reason it read as one is that a flat expanse of a single wrong colour
looks like a pattern cell painting its fallback.

The bug was never confined to this file. Every `Separation` and every
`DeviceN` image took the same path, and a four-colorant `DeviceN` was being
read as raw `DeviceCMYK`.

**And the conversion was already written.**
`ColorSpace::translate_image_line` (`crates/pdfrum-page/src/color/mod.rs`) is
a faithful port of `CPDF_ColorSpace::TranslateImageLine` and every family
override — the `CalGray` replication, the `CalRGB` reversal, `Lab`'s byte
domain, the `ICCBased` cascade, and the generic `GetRGB`-per-pixel base that
`Separation` and `DeviceN` fall through to. It had **no caller in the image
build at all**: the only thing that touched it was
`tests/never_panics.rs:460`. So this was not a missing port but a missing
*call site* — the arithmetic was there the whole time and `unpack` never
asked for it. `tint_per_pixel` is now that call site, and the byte order is
the only adaptation: the port writes B, G, R because that is the device order
PDFium's scanline is in, and `Pixels::Rgb8` wants R, G, B.

That is also why the two arms encode differently and both are right.
`LoadPalette` rounds (`FXSYS_roundf`, `cpdf_dib.cpp:925`) and our palette
stores `Rgb` that `to_bytes` rounds; `TranslateScanline24bpp` truncates
(`static_cast<uint8_t>(B * 255)`, `:1049-1051`) and the port's `write_bgr`
uses `to_bytes_truncating`. Swapping either for the other is a visible
one-bit error on about half of all inputs.

### The oracle reaches the same conversion from two directions

Both end at `GetRGB`, and which one runs is only a question of how wide the
sample is:

| | when | what runs |
|---|---|---|
| `CPDF_DIB::LoadPalette` (`cpdf_dib.cpp:894-979`) | `bpc_ * components_ <= 8` | `GetRGB` over all `1 << bits` sample values, once |
| `CPDF_DIB::TranslateScanline24bpp` (`:1007-1054`) | anything wider | `GetRGB` per pixel |

`TranslateScanline24bpp`'s default-decode shortcut (`:1056-1075`) keeps only
`DeviceRGB`/`CalRGB` and hands every other family to `TranslateImageLine`,
whose generic base (`cpdf_colorspace.cpp:636-660`) is `GetRGB` per pixel
again — so there is no escape from the conversion for a tint space.

### Only two families, and the reason matters

`needs_image_conversion` answers `Separation` and `DeviceN` and nothing else,
because the other non-device families each have a `TranslateImageLine`
override that is **not** the scalar conversion, and folding them in would be a
new divergence rather than a fix:

| family | override | what it does |
|---|---|---|
| `CalGray` | `cpdf_colorspace.cpp:725-742` | copies the grey byte into all three channels |
| `CalRGB` | `:808-816` | a channel reversal — gamma and matrix dropped |
| `Lab` | `:899-915` | rescales out of the **byte** domain, not the decoded range |
| `ICCBased` | `:991-1010` | runs the profile over bytes |

`Indexed` is already handled by its own palette, and `Pattern` carries no
image samples at all — `TranslateScanline24bpp` skips it explicitly
(`:1043`).

### Spec and pdf.js agree, so there is no `[oracle-bug]` here

ISO 32000-1 §8.6.6.4 and §8.6.6.5 make the components of a `Separation` and a
`DeviceN` *colorant tints*, with the tint transform the thing that turns them
into colour; an image's samples are colour-space components like any other.
pdf.js runs the same conversion over image samples — `PDFImage.createImageData`
calls `colorSpace.fillRgb(...)`, and `AlternateCS.getRgbBuffer`
(`src/core/colorspace.js`) applies `tintFn` and then the base space per sample.
It builds no lookup table, which is a performance choice rather than a
different answer. All three agree; we were alone in being wrong.

### What it moved

Three rows, all `pass -> pass`, no `pass -> fail`:

| file | SSIM before | after |
|---|---|---|
| `corpus/fx/other/1.pdf` | 0.996609 | **0.998815** |
| `resources/bug_555784.pdf` | 0.998737 | 0.998736 |
| `resources/bug_555784.in` | 0.998737 | 0.998736 |

`bug_555784` is a fuzz file — a 81915x1 image over a **3277**-colorant
`DeviceN` whose type-2 transform reports `orig_outputs * inputs = 4` outputs
against a `DeviceRGB` alternate, so the space loads in both implementations.
Its samples are all zero, so the transform yields `C0` and the correct colour
is black; we now paint that instead of the dark grey the raw samples gave. The
oracle paints **white**, because it never decodes the image at all:
`ContinueInternal` (`cpdf_dib.cpp:178-181`) calls
`CalculatePitch32(bpc_ * components_, width)`, and `52432 * 81915 + 31 =
4294967311` overflows `uint32` by sixteen, so the load fails. Our `pitch()`
(`crates/pdfrum-page/src/image/dict.rs:189-194`) computes in `u64` and caps
only the whole-image product, so the image survives. **That gap is
pre-existing and independent of this change** — the fix altered which colour
the surviving pixels take, not whether they exist — and the row's -0.000001
is 139 pixels of one scanline in a 612x792 page. It is left as it is rather
than fixed by widening a colour change into a size-limit change.

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
