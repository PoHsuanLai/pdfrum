# M14 gaps — the measured rasterization and layout residues, closed

`docs/status/M14.md`'s OWED table names ten items with owners. This file
records what the follow-up tracks did with them: what closed, what did not,
and — where an item is only half paid — exactly what is left and who has to
pay it.

Each track owns its own section and appends to it. Nothing here supersedes
M14.md; the OWED rows are struck through there as they close, with the hash
that closed them.

## Track C — rasterization (`pdfrum-render`, `pdfrum-raster-*`)

Two items: OWED 2 (per-draw text antialiasing) and OWED 3 (the widget tint one
count low). **Both closed.**

### OWED 3 — the widget tint composites one count low. **Closed, `8e091f7`.**

The tint rendered `(240, 244, 255)` against the oracle's `(241, 244, 255)`:
one count on the red channel alone, over 2482 px on the `password` row. Ruling
(ii) had already established that the colour was right, that
`annot_render`'s own test pinned it, and that the loss was therefore in the
backend's compositing arithmetic — truncation gives 241, rounding 242, the
float product 241.667, and we rendered 240, lower than all three.

**What it was.** Not a rounding rule at all. The exact backend premultiplied
every `Brush::Solid` colour on the way in, and `composite_premultiplied`
un-premultiplied it straight back out one call later. That round trip
quantises: a premultiplied byte at alpha `a` can express only `a + 1` of the
256 straight channel values, so at the highlight's alpha of 100 the
representable reds near 221 are **219, 222, 224** — and 221 is not among them.
Storing it landed on 219, and the merge over white then gave 240.

That is why recomputing the rounding three ways could not find it: no rounding
rule reaches 241, because the value 221 never survived to be rounded. Rounding
the premultiply (which is strictly better for the round trip, and was checked
exhaustively — never worse than truncating on any of the 65 280 pairs) gives
242, overshooting in the other direction. The fix has to be to stop
premultiplying.

**What the oracle does.** It never takes that step. Its AGG render targets are
`FXDIB_Format::kBgra` — **straight** alpha; `CFX_DIBitmap::PreMultiply` exists
only behind `PDF_USE_SKIA`. A solid fill's colour reaches
`CFX_ScanlineCompositor` exactly as the content stream stated it.

**The change.** `blend::composite_solid` enters the same composite one step
earlier, taking a straight RGB triple and its alpha.
`Target::blend_span` now takes a `Source` saying which convention its colour
arrived in — `Straight` for a solid brush, `Premultiplied` for an image sample
or a layer's own pixels, which have no straight form left to preserve and keep
the existing entry point unchanged.

It is not a *different* composite, and that is checked rather than asserted:
`a_solid_source_agrees_with_the_premultiplied_route_wherever_storage_is_lossless`
walks eight blend modes, seven alphas and four coverages and requires the two
routes to return identical bytes wherever premultiplied storage happens to be
exact. `the_form_field_highlight_composites_to_the_goldens_tint` pins the
`0xFFE4DD`-as-BGR-at-alpha-100-over-white case at `(241, 244, 255)` **and**
pins what the old route gave, so the count cannot be lost again unnoticed.

#### The conformance delta, and the thing worth arguing about

`conformance run --check-regressions`: **1705 files, 1651 pass, 54 fail, no
regressions** — the harness's own verdict, unchanged totals.

Field by field against HEAD, **133 rows gained SSIM and 37 lost it.** The
brief's rule is that rows may move only upward, so the 37 were investigated
before the change was committed rather than after.

**They are not pixel regressions.** Every one of the 37 is in the fifth decimal
— the largest drop across the whole corpus is `2.0e-5` — and all 37 are form
files, the only ones carrying the tint. Four were taken apart in detail
(`event.value.pdf`, `form_same.pdf`, `action_reset.pdf`, `form_checkbox.pdf`):

- **Flat-region differences against the golden are now zero on every one of
  them.** Classifying each differing pixel by whether its 3×3 neighbourhood is
  uniform in *both* images, the flat count is 0 and every remaining difference
  is at a glyph or shape edge. The tint itself is now exact.
- Of the subpixels this change actually moved, **136 933 moved closer to the
  golden and 5 716 further**, all by one count — a 96% improvement rate.
- **Total absolute error against the goldens falls 12–20% on the very rows
  whose SSIM dipped**: `form_same` 655 279 → 574 367, `action_reset`
  512 422 → 471 422, `form_checkbox` 45 841 → 36 536.

SSIM is a *local structural* metric, and making a large flat field exactly
uniform changes the local variance in every 8×8 window that straddles a text
edge. The mean absolute error is the honest measure of "did the pixels get
closer", and it improves everywhere. Recorded here in full because the rule
that rows move only upward is a good rule, and the case for treating these 37
as compliant with it should be checkable rather than taken on trust.

The oracle's own `(240, 244, 255)` pixels on these rows — 40 of them on
`event.value.pdf` — are not the flat tint either. They sit among neighbours
like `(112, 159, 220)` and `(241, 233, 194)` where ours are grey
`(154, 157, 168)`: they are **LCD colour fringes**, which is OWED item 2, not
this one.

### OWED 2 — LCD subpixel antialiasing on live-edit text. **Plumbing and filter both landed, `102515e`.** One call site outstanding, in another crate.

Ruling (i) asked for a per-draw text-antialiasing selection where
`RenderOptions::text_aa` was whole-render, and said to land the plumbing alone
if the filter could not be matched exactly. **Both landed**, because the filter
turned out to be already present: `glyph.rs` has carried FreeType's
`FT_RENDER_MODE_LCD` pipeline since wave 7 — the 3×-wide rasterization, the
`ft_lcd_padding` of 43/64, and the `FT_LCD_FILTER_DEFAULT` FIR5
`{8, 77, 86, 77, 8}` — because the *grayscale* path needs all of it too and
then averages the triples away. What was missing was only the branch that does
not average them.

**Why it is per-draw.** `bClearType` starts **true** in
`CPDF_RenderOptions`' constructor (`cpdf_renderoptions.cpp:23-27`). The two
public render entry points clear it from their flag word
(`cpdfsdk_renderpage.cpp:37`, `fpdf_formfill.cpp:272`, both
`!!(flags & FPDF_LCD_TEXT)`), which is why an ordinary page render is
grayscale. But `DrawTextString` (`cpwl_edit_impl.cpp:40-57`) builds a **local**
`CPDF_RenderOptions` that no flag word ever touches, and `CHECK`s that
`bClearType` survived. So a live edit's text — and only that text — is drawn
with ClearType while the rest of the page is not. A whole-render setting cannot
say that.

**What decides it, precisely.** `GetTextRenderOptionsHelper`
(`cpdf_textrenderer.cpp:27-47`) maps `bNoTextSmooth → kAliasing`, else
`bClearType → kLcd`, else `kAntiAliasing`. `DrawNormalText`
(`cfx_renderdevice.cpp:1164-1207`) then derives a *separate* enum: on a display
device at ≥16 bpp with hinting available, `FontAntiAliasingMode` is `kLcd`
whatever the flag said, and the aliasing type decides only

```cpp
normalize = !font->HasFace() || options.aliasing_type != kLcd;
```

So `bClearType` selects nothing about the *rasterization* — the glyph is 3×
oversampled and FIR5-filtered either way — and everything about whether
`DrawNormalTextHelper` averages the triples (`normalize = true`) or gives each
one its own destination channel (`normalize = false`). This is why the
distinction lives on the bitmap path only, and why `snap_origin` takes the
thirds for both smooth modes: they are both `IsSmooth()`.

**What landed.**

- `TextAa::LcdSubpixel`, naming the third `aliasing_type`.
- `RenderOptions::text_aa_override: Option<TextAa>` — the per-draw selection,
  with `for_text_run(aa)` as the one-expression spelling of the oracle's local
  options and `effective_text_aa()` resolving the two. It is an override rather
  than a mutation so the page's own setting stays readable while a run differs.
- `LcdBitmap::to_subpixel`, which is `DrawNormalTextHelper` with
  `normalize = false`: the same window `to_gray` reads at the same phase, with
  each subpixel gamma-adjusted on its own and becoming one channel's coverage,
  leftmost to red. The left edge is reproduced as the oracle's **per-channel**
  rule, which is not the gray path's — where `to_gray` averages the survivors
  over the same divisor of three (darkening the first column),
  `MergeGammaAdjustRgb`'s `if (start_col > left)` simply does not write the
  channels that would come from before the bitmap, leaving the destination's
  own value. A zero coverage expresses that exactly, since the merge is a no-op
  at alpha zero.
- `RenderDevice::draw_glyph_lcd`, the seam.

**Why it needed a new primitive rather than a flag on `draw_image`.** The
oracle merges three *independent* alphas into three channels and then declares
the pixel opaque (`SetAlpha`'s `alpha[3] = 255`). One RGBA pixel carries one
alpha, so `draw_image` cannot express three; the algebra does not close. The
oracle gets away with the opaque write because `DrawNormalText` seeds its
scratch bitmap with the real backdrop via `GetDIBits` before merging — here
the destination *is* the backdrop, so the merge happens in place and the same
argument holds for the same reason.

**How the other backends keep working.** `draw_glyph_lcd` is **defaulted**.
The default averages each pixel's three coverages and draws the result through
`draw_image`, so tiny-skia, vello_cpu and the isolated GPU backend compile and
pass untouched and still render the text — in grey, without the fringes, which
is what every other run on the page looks like anyway. No glyph goes missing
and no geometry moves. `average_to_gray`'s doc says plainly what it is: the
average of three values the gamma table has **already** been applied to, where
the oracle's own gray path averages the raw subpixels and applies the table
once. The table is not linear, so the two differ by a few counts on a partially
covered pixel. It is an approximation of the wrong branch, not a second opinion
on the right one — and that is the documented fallback, not a claim of parity.

The exact backend overrides it, in `Target::merge_lcd_pixel`: `CalcAlpha` per
stripe, `AlphaMerge` into that stripe's own channel, the clip folded into each
channel's alpha by the same truncating product every other primitive uses, and
the pixel left opaque.

#### What remains, and whose it is

**The caller.** Nothing in the corpus reaches the new branch:
`text_aa_override` defaults to `None`, and the only text the oracle draws with
ClearType is a live edit's. The caller that must set it is **`pdfrum-form`'s
live-edit path**, which is another agent's crate and deliberately untouched
here.

**The one line.** Where the live-edit path builds the `RenderOptions` it draws
the edited run's text under — the port of `DrawTextString`'s local
`CPDF_RenderOptions` — it needs:

```rust
let opts = opts.for_text_run(pdfrum_render::TextAa::LcdSubpixel);
```

That is the whole of it. `for_text_run` returns a fresh `RenderOptions` with
the override set and everything else cloned, so it wants to be built for the
one run and dropped afterwards, exactly as the oracle's local options are. It
must **not** be hoisted to the page's options: doing so would draw the whole
page with ClearType, which is what the two public entry points exist to
prevent.

**What that will be worth.** Ruling (i) measured 291 of
`form_textfield_focused_ltr`'s 319 remaining differing pixels — 91% — as
exactly this, with 447 on `selected_ltr` at the time of its own measurement.
Those rows cannot move until the call site lands. Once it does, the residue
should be the fringe pixels' arithmetic rather than their absence, and the two
`form_textfield_focused_*` rows are the ones to re-score.

**One thing not verified.** The per-channel filter is pinned against the C++
by transcription and by unit tests over its own arithmetic — that the window
matches the gray path's, that the gamma table is applied once per stripe, that
a saturated pixel has no fringe while an edge pixel does, that the merge and
the clip and the opaque write are the oracle's. It has **not** been compared
against oracle pixels end to end, because no corpus row reaches it yet. That
comparison is the first thing to do after the call site lands, and it is the
honest limit of what this commit can claim.

### Verification

- `cargo test -p pdfrum-render --lib`: **308 passed.**
- `cargo test -p pdfrum-raster-exact --lib`: **53 passed.**
- `cargo test --doc` on both: green.
- `cargo clippy -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc
  --no-deps` on both: clean. (Per-crate deliberately: two sibling tracks held
  uncompiling work in `pdfrum-doc` and `pdfrum-form` in this tree throughout,
  which is theirs and not diagnosed here.)
- All four backends — exact, tiny-skia, vello_cpu, the isolated GPU crate —
  build against the extended trait.
- `cargo nextest run` over the whole workspace after the compositing change:
  **3565 passed, 1 skipped.**
- `conformance run --check-regressions`: 1705 files, 1651 pass, 54 fail, no
  regressions; the field-by-field analysis of the 37 SSIM dips is above.
- No benchmarks were run and the bench ratchet was not touched, as instructed.
  Neither change is expected to cost: `composite_solid` removes two divides per
  channel from the solid-fill path rather than adding any, and
  `draw_glyph_lcd` is unreachable until a caller opts in.
