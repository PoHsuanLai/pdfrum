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

#### The call site: a correction, and where it actually went

**The first plumbing commit named a call site that does not exist.** It said
`pdfrum-form`'s live-edit path should call
`opts.for_text_run(TextAa::LcdSubpixel)`. That crate cannot: it emits an
appearance **stream** (`UpdateKind::LiveEdit(GeneratedAp)`), never sees a
`RenderOptions`, and does not depend on `pdfrum-render` at all. The claim was
made without checking the dependency edge, and it was wrong.

Worse, the shape was wrong wherever it was put. By the time anything
rasterizes, the live edit's appearance is one form XObject among the page's
objects — merged in by `annot_render::overlay_with` — and both `pdfrum-tool`'s
`render::render` and the facade build exactly **one** `RenderOptions` and make
exactly **one** `render_page_with_visibility` call for the whole page. A
`for_text_run` there would hoist ClearType onto every run on the page, which is
the precise thing the oracle's two public entry points exist to prevent.

**So the override travels with the object.** `FormObject::live_edit` records
*where the form came from* — a fact about the document, not an instruction to a
renderer, which is why `pdfrum-page` can hold it without naming a rasterizer's
type — and `walk.rs`'s `form_options` folds it into that subtree's options,
the same shape `for_type3_char_proc` already uses to force options around one
glyph procedure. A caller's own `text_aa_override` outranks it: the flag
describes the document, the override describes the request.

The chain, end to end:

```
AnnotOverlay::set_live_edit(index)          // pdfrum-doc, the session's signal
  -> annot_render, at the supplied-appearance site
  -> build_form_object_with(.., live_edit)  // pdfrum-page
  -> FormObject::live_edit
  -> walk.rs form_options                   // pdfrum-render, folds in the override
  -> to_subpixel + draw_glyph_lcd
```

Every step is **additive**: `build_form_object` keeps its signature and
delegates with `false`, so every existing caller compiles unchanged and every
existing producer — the file's own appearance streams, and a session's
*regenerated* ones — stays ordinary.

**The producer line, for `pdfrum-tool` (Track B's crate, not edited here).** In
`render.rs`'s `session_overlay`, inside the loop that already sets appearances:

```rust
if update.kind.is_live_edit() {
    overlay.set_live_edit(update.annot.index as usize);
}
```

`UpdateKind::is_live_edit()` already exists. The API name to wire is
**`pdfrum_doc::AnnotOverlay::set_live_edit(index)`**, taking the same raw
`/Annots` index `overlay.set` takes.

#### The filter's first corpus evidence

Wired end to end (the tool line applied locally to measure, then reverted), the
whole board moves **1651 pass / 54 fail -> 1652 pass / 53 fail**, with
`bug_736695_2.in#form-events` going fail -> pass. Every `form_textfield_*` row
improved:

| row | before | after |
|---|---|---|
| `focused_ltr#form-events` | 0.999311 (max diff 89) | **0.999513** (89) |
| `focused_rtl#form-events` | 0.997558 (156) | **0.997777** (156) |
| `selected_ltr#form-events` | 0.999682 (84) | **0.999936** (31) |
| `selected_rtl#form-events` | 0.999385 (189) | **0.999758** (149) |
| the five plain `form_textfield_*` | 0.999984 (1) | **1.000000** (0) |

On `focused_ltr` against its golden, the fringes themselves:

- **non-grey pixels: 0 -> 288, against the oracle's 291.** Before this, our
  render had *no* coloured pixel anywhere on the row; ruling (i)'s 291 was
  exactly the count of the oracle's.
- **231 of our 288 are co-located** with one of the oracle's fringe pixels.
- total absolute error **74838 -> 70500**.

**What that does not say.** No fringe is byte-exact, and the differing-pixel
count is essentially flat (370 -> 374). The reason is measurable and it is not
the filter: comparing the *pre-LCD* render to the golden in pure **luminance**,
before any colour is involved, the glyphs already disagree by a mean of **62.6
counts and up to 218**. The glyph shapes are wrong on this row — that is OWED
1's font-substitution territory — and a subpixel filter can only colour ink
that is already in the right place. Testing the filter where that confound is
absent, on the 37 pixels whose pre-LCD shape already agreed within 8 counts,
absolute error falls **1908 -> 1451, a 24% improvement**. That is the fair test
of the filter and it passes; 37 pixels is suggestive rather than conclusive,
and it stays that way until the glyphs land correctly.

So the honest status of the filter is: **it produces fringes, in the right
columns, in roughly the right count, and it moves every row that reaches it
upward — but it has not been shown byte-exact against the oracle, and cannot be
on this corpus until OWED 1's glyph shapes are right.** Re-scoring these rows is
the thing to do after that lands.

### Verification

- `cargo nextest run -p pdfrum-page -p pdfrum-doc -p pdfrum-render -p
  pdfrum-raster-exact`: **1282 passed**, after the per-object carrier landed.
- `cargo test --doc` on the touched crates: green.
- `cargo clippy -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc
  --no-deps` on all four: clean; `cargo build --workspace` clean.
  (Per-crate deliberately: sibling tracks held uncompiling work in
  `pdfrum-doc` and `pdfrum-form` in this tree for part of the window, which is
  theirs and not diagnosed here.)
- All four backends — exact, tiny-skia, vello_cpu, the isolated GPU crate —
  build against the extended trait.
- `cargo nextest run` over the whole workspace after the compositing change:
  **3565 passed, 1 skipped.**
- `conformance run --check-regressions`, compositing alone, measured from a
  clean worktree: 1705 files, 1651 pass, 54 fail, no regressions; the
  field-by-field analysis of its 37 SSIM dips is above.
- With the live-edit carrier wired end to end: **1652 pass, 53 fail**, one row
  better, no status regressions, and the same 35 non-form-events fifth-decimal
  dips as the compositing change alone.
- No benchmarks were run and the bench ratchet was not touched, as instructed.
  Neither change is expected to cost: `composite_solid` removes two divides per
  channel from the solid-fill path rather than adding any, and
  `draw_glyph_lcd` is unreachable until a caller opts in.

## Track A — fonts and metrics (`pdfrum-font`, `pdfrum-doc`)

Three items: OWED 1 (`char_width` against the substituted face), OWED 6
(`caret_rect` and `is_rtl`), OWED 9 (the seven unported charset tables). Two
closed, one was already closed before this track started, and one item from
another track's queue was landed here because only this crate can hold it.

| OWED | verdict | commit |
|---|---|---|
| 1 | **closed** — but the recorded mechanism was one layer off | `0f77c8e` + `8b1f76e` |
| 6 | **already closed** at M14's own close; verified, no code | `bb53fa7` (M14-doc §8.1) |
| 9 | **closed** | `f5dc65b` |
| 8 | closed on this crate's side, at Track B's request | `6e87424` |

### OWED 1 — the caret was measured at the size that was *asked for*

**The recorded mechanism is not the defect.** The OWED row reads it as
`Font::char_width` answering the base-14 table where the substituted face
draws — 311/1000 for `'*'` under a `/Helvetica` with no `/Widths`, against
Arimo's 389. Measured, `char_width` already answers **389**, under a default
`SubstitutionOptions` and under the hermetic one alike. It is
`LoadCharMetrics`'s `use_font_width_` branch
(`core/fpdfapi/font/cpdf_facebasedsimplefont.cpp:46-86`), and
`SimpleFont::char_width` implements it: no `/Widths` sets `use_face_widths`,
and the face substitution has already replaced `glyphs` by the time a width is
asked for. The row's C++ citation, `CPDF_Font::GetCharWidthF`, does not exist
in the oracle checkout at all — the accessor is `GetCharWidth`, and it is
where our port already agrees.

What was actually wrong is one layer up, and it is the same *class* of error:
measuring with the wrong number.

`Config::font_size` is a **request**; `0.0` is the request for automatic
sizing. `vt::layout` resolves it and records the answer in
`Layout::font_size` — which is exactly `CPVT_VariableText::Rearrange`
(`core/fpdfdoc/cpvt_variabletext.cpp:834-846`) calling
`SetFontSize(GetAutoFontSize())`, writing the resolved size into the field
every later `GetFontSize()` reads, `CPVT_Word::CaretX`'s advance and
`CPVT_Section::SearchWordPlaceImpl`'s midpoint included. `vt::hit` read the
**config's** in three places, so on an auto-sized field it measured with zero:

- `caret_x` added a zero advance, so every caret sat on the leading edge of
  the character it should trail — one advance short of the gap it names, the
  left-to-right branch collapsed onto the right-to-left one;
- `word_at_x`'s midpoints collapsed onto each character's own origin, so the
  bisection answered by position alone rather than by the midpoint rule;
- `line_extent`'s fallback answered a caret of no height.

`vt::edit_ap` already drew with `layout.font_size`, which is why this stayed
invisible until a caret was drawn beside glyphs that disagreed with it.

**How it was found.** Rendering `password` with `--send-events` under the
harness flags and profiling the ink column by column: our five asterisks land
on the oracle's columns *exactly* — 150, 160, 170, 180, 190, peak for peak —
while our caret sits at column 189 and the oracle's at 198–199. Glyphs right,
caret wrong, and the gap is one advance of 389/1000 at 25pt. A probe over the
fixture's own geometry then reproduced 189.275 for place `word: 4` at a config
size of zero and 199.0 at the layout's 25. `password` carries **no `/DA` and
no `/DR` at all**, which is what makes the request zero and the fixture the
one that shows it.

#### The third site was reverted, because it was reasoned rather than measured

Isolated on the board from M14's close (`9352635`), three release builds:

| change | rows up | rows down | `password#form-events` |
|---|---:|---:|---|
| `caret_x` + `word_at_x` | **2** | **0** | 0.991764 → **0.999016** |
| + `line_extent`'s fallback | 133 | **37** | 0.999022 |

`line_extent`'s fallback is reached only for a place naming a line the layout
does **not** have, and there the two sizes say different things: the layout's
invents a height for a line that does not exist, the config's gives an
auto-sized caller nothing. The second is upstream's —
`CPWL_EditImpl::GetCaretRect` reads an empty word range for a missing place,
so the caret it returns has no height. Thirty-five of the 37 drops are
non-form-events rows, which the brief's gate does not permit, and the whole of
`password`'s gain from it is +0.000006 against the +0.007252 the other two are
worth. So it stays on the config's size, its doc now carries the numbers, and
the `caret_rect` test asserts the missing-line caret has **no** height — so
substituting the layout's size turns a test red rather than moving 170 rows
silently.

**On the two 37s.** Track C's tint change reports the same 133-up/37-down
signature on the same rows. That is a coincidence of *population*, not a
contaminated measurement: both changes touch every tinted form widget, so the
same 170 rows respond to either. The trees were checked — the raster crates
are byte-identical across all three of this track's builds — and this track's
kept change moves two rows and no others.

#### The delta, measured

Against `9352635`, with the reverted fallback in place:

| row | before | after |
|---|---|---|
| `resources/pixel/password.in#form-events` | 0.991764 | **0.999016** |
| `resources/pixel/password.pdf#form-events` | 0.991764 | **0.999016** |

**Exactly two rows move, both upward. The other 1703 are byte-identical**,
tags and text rates unchanged; totals stay 1705 files, 1651 pass, 54 fail, and
`--check-regressions` reports none. Measured with release tools built from
trees archived out of git, because the working tree carries two other tracks'
uncommitted work. The baseline reproduced `HEAD`'s committed scoreboard row
for row, and a **second** baseline run reproduced itself row for row — the
board carries no run-to-run noise, so every difference above is real.

What is left on `password` is not this crate's: the widget tint (OWED 3, Track
C's, now fixed) and one character of `ScrollToCaret` scroll in `pdfrum-form`.

### OWED 6 — already closed, verified rather than re-fixed

`vt::hit::caret_rect` does **not** ignore `is_rtl`. Both places the OWED row
names were ported in `bb53fa7` and are recorded in M14-doc §8.1: `caret_x` is
`if word.is_rtl { word.x } else { word.x + advance }`, which is
`CPVT_Word::CaretX` (`core/fpdfdoc/cpvt_word.h:42`), and `line_caret_x`
answers `line.x + line.width` when the line's first word is rtl, which is
`GetLineCaretX` (`core/fpdfdoc/cpvt_variabletext.cpp:103-118`). The exact test
the item asks for — `בחר` at 12pt in a plate starting at x = 1, carets
`19, 13, 7, 1` — exists as
`a_right_to_left_words_carets_walk_leftward_from_its_right_edge`, with
`the_drawn_caret_rectangle_follows_the_same_right_to_left_walk` asserting the
same four through `caret_rect`. Both pass. The OWED row is stale: it was
written from the state before §8.1 landed and never struck.

No code was written for this item. Both RTL rows still pass, and the board
diff above shows neither moved.

### OWED 9 — the seven tables, and how a table nothing renders is checked

`ap::font_map::charset_unicodes` now carries all eight rows of
`kFX_CharsetUnicodes` (`core/fxcrt/fx_codepage.cpp:208-217`) — Thai, Eastern
European, Cyrillic, Greek, Turkish, Hebrew, Arabic, Baltic — transcribed
verbatim. The `_` arm that swallowed every other charset is spelled out, so a
new `Charset` variant is a compile error rather than a silent `None`.

The item's own objection was that a table nothing exercises is a table nothing
can catch a typo in. What answers it is that the transcription is checked
against sources that are not the Rust file:

1. **One FNV-1a-32 digest per row**, computed from the C++ rather than from
   the transcription. A single flipped nibble anywhere in a row fails that
   row's test — which a set of spot checks over 128 values cannot promise.
2. **Each row's own script run**, which is checkable by eye against the
   Unicode standard where a digest is not: Cyrillic's `0xC0..=0xFF` is
   U+0410..=U+044F, Hebrew's `0xE0..=0xFA` is U+05D0..=U+05EA, Greek's two
   runs split at `0xD2` where U+03A2 is unassigned, Arabic's `0xC1..=0xD6` is
   U+0621..=U+0636.
3. **The hole count**, which catches a value dropped or gained without
   shifting the rest. Arabic has none at all — 1256 encodes every code — and
   that is asserted too.

All eight were additionally decoded byte for byte under Python's `cp874`,
`cp1250`, `cp1251`, `cp1253`, `cp1254`, `cp1255`, `cp1256` and `cp1257`
codecs, **zero mismatches**; the same check run against the already-landed
Hebrew row round-tripped it exactly, which is what established the extraction
was faithful before the other seven were taken from it. That fourth source is
recorded in the test module's doc rather than run there: it needs a codec
table DEPS.md has no dependency for, and restating 1024 literals would only
verify the copy against itself.

**`SUBSTITUTABLE_CHARSETS` deliberately still names Hebrew alone.** The tables
say how a charset's characters would be *written*; that list decides which
charsets a field actually reaches a second face for, and widening it changes
what the corpus renders. Each added charset also needs M14-doc §8.2's
default-face question answered for it — Hebrew's answer, the serif fallback
via `RenameFontForTesting`'s empty-name mapping, is not automatically the
others'. Having the table is what makes adding one a one-line change rather
than a transcription. So this is **behaviour-neutral**: outside the tests,
`charset_code` and `substitute_font_dict` are reached only for
`SUBSTITUTABLE_CHARSETS`, and the board confirms no row moved.

Two existing tests used Cyrillic as "the charset with no table" and could no
longer. They move to a CJK charset, which `kFX_CharsetUnicodes` has no row for
and is not expected to grow one, its encoding being multi-byte and a 128-entry
high half unable to express it. Both now also assert the positive side, so
each is about the table rather than about the function.

### OWED 8 — landed here at Track B's request

`ap::widget::LiveInput` grows `appearance_state: Option<&[u8]>`, and
`is_checked` grows a sibling `is_checked_with` that takes it. Track B owns the
state that needs it and cannot edit this crate.

The reason a seam is needed at all: a radio group is **one field with several
kid controls, each carrying a different on-state name**. Clicking one is
`CPDF_FormField::CheckControl` (`core/fpdfdoc/cpdf_formfield.cpp:683-716`),
which sets the clicked control's `/AS` to that control's own on-state and
every *other* control's to `Off` — one click restating the state of every kid,
in as many different names. A session holds one record per *field*, so all it
can say is which control was chosen; turning that into what each kid draws is
per-kid, and the dictionary on disk still names the state before the click.

`build` now takes the whole `LiveInput` rather than a seventh positional
`Option`, which is what that record's doc already promised. `is_checked` keeps
its public signature — `pdfrum/src/form.rs` and `pdfrum-doc/src/form/field.rs`
both call it with no override — and is now that function handed `None`.
Byte-identical for every current caller: `generate` and `generate_with_text`
pass a default record, `generate_with_live` forwards `appearance_state: None`
beside the `substitute: None` it already forwarded, both spelled out so a
fifth field is a compile error there rather than a silent change. The
byte-identity golden `tests/data/unfocused_field_bodies.txt` is untouched and
green.

**One thing the next reader needs.** `LiveInput` has no `#[non_exhaustive]`,
so adding a field breaks any *exhaustive* struct literal. `route.rs` at
`0f77c8e` carried one, and `pdfrum-form` did not compile against `6e87424`
until Track B's own edit — already in its working tree — switched that literal
to `..Default::default()`. Whoever adds a sixth field should either mark the
struct `#[non_exhaustive]` first or coordinate the same way.

### A process note, recorded because it cost another track

Commit `0f77c8e` was made with a bare `git add` and swept in six files
belonging to Tracks B and C — `crates/pdfrum-form/{popup.rs, route.rs,
page.rs, field/mod.rs, lib.rs, tests/combo_popup.rs}`,
`crates/pdfrum-raster-exact/{lib.rs, target.rs}` and
`crates/pdfrum-render/blend.rs` — under a message naming only `vt::hit`. It
was already pushed when it was noticed, so it was left alone rather than
amended or reverted; the content is correct and the other tracks record the
attribution. Every commit after it in this track is path-scoped
(`git commit -- <paths>`) with `git status --short` checked first. The
constraint exists because concurrent tracks share one worktree, and the same
sweep would have caught half-finished work just as easily.

### Gates

- `cargo fmt -p pdfrum-doc -- --check`, `cargo fmt -p pdfrum-font -- --check`:
  clean. Deliberately per-crate; `--all` would reformat the other tracks'
  uncommitted work, and `scripts/ci.sh` was not run for the same reason.
- `cargo clippy -p pdfrum-doc --all-targets -- -D warnings`: clean.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p pdfrum-doc`: clean.
- `cargo nextest run -p pdfrum-doc`: **440 pass**, up from 402 at M14's close
  and from 424 before this track — ten for the charset tables, four for the
  auto-sized caret, two for the appearance-state override.
- `cargo test --doc -p pdfrum-doc`: 3 pass.
- `cargo build -p pdfrum-form -p pdfrum -p pdfrum-tool`: clean against the
  final state of this track's crates.
- Conformance, **this track alone** (base `9352635` vs this track's crates):
  1705 files, 1651 pass, 54 fail, `--check-regressions` clean, **two rows
  moved and both upward**, 1703 byte-identical.
- Conformance, **all three tracks' committed work** (a clean archive of
  `c35cd76`, scored against the committed scoreboard): **1705 files, 1652
  pass, 53 fail, no regressions** — one row crosses,
  `bug_736695_2.in#form-events` fail → pass, which is Track B's. `password`'s
  two rows read **0.999022** there rather than this track's 0.999016; the
  extra +0.000006 is Track C's tint fix on the same page. The 38 dips on that
  joint board are Track C's, analysed in their section above.
- **The scoreboard is not re-committed from this track.** This track's own
  delta leaves the totals unchanged, and the joint board above is a
  three-track result whose dips another track has already ruled on — so
  recording it is a cross-track decision rather than this one's, and a board
  written from here would overwrite whatever the others measure next. The
  numbers are here so whoever does commit it can check them.
- No benchmarks and no ratchet update, as instructed.

## Track B — the popup as an API (`pdfrum-form`, `pdfrum`, `pdfrum-tool`)

Three items: OWED 4 (the open combo dropdown), OWED 8 (`clear_siblings` cannot
draw the difference) and OWED 10 (the last `generate_with_live` note). **All
three closed.**

### The ruling this track implements

STYLE §2b, 2026-09-01: *a library has no business creating a dropdown window*.
`fpdfsdk/pwl` is a closed list of five chrome pieces — caret, selection band,
focus rectangle, scroll bar, combo dropdown — and the line between them is
whether the piece falls **inside** the widget's `/Rect`. The first three do,
so they arrive with the appearance stream, which is fitted onto that rectangle.
The last two do not, and drawing outside it means creating a window.

So: **no `FormChrome` trait and no fourth seam.** The seam list stays
`RenderDevice`, `Resolve`, `Cascade`. What ships is state + geometry + intent —
`PopupView`, `ScrollView`, `choose`, `close_popup` — written up as SPEC §15.10
in `4d883d4`, including the four places the shipped shape differs from the
ruling's sketch and why each one differs.

### A record before the items: where the first block actually landed

The state-and-geometry half of OWED 4 — `popup.rs`, and the changes to
`route.rs`, `page.rs`, `field/mod.rs`, `lib.rs` and `tests/combo_popup.rs` —
is committed inside **`0f77c8e`**, whose message describes only Track A's
`vt::hit` caret work and says nothing about a dropdown. It got there because a
concurrent track ran a non-path-scoped commit (`git add -A` or `git commit -a`)
while those six files were staged and about to be committed under their own
message.

The content is correct and `0f77c8e` was already pushed, so it stays where it
is: the fix for a wrong attribution is the record, not a rewrite of shared
history. M14.md's OWED row for item 4 therefore cites `0f77c8e` alongside
`1e9365a` and `1475455`, and this paragraph is here so a reader following that
row is not surprised by a commit message that never mentions the thing they
came looking for.

This is also the concrete cost of `git add -A` in a shared tree, which is why
the constraint against it exists.

### OWED 4 — the open combo dropdown. **Closed, `0f77c8e` + `1e9365a` + `1475455`.**

Four parts, and the third is the one that mattered most.

**(a) State.** `ChoiceState` gains `popup_open` and `hovered`. Which events
open and close it is read from the C++ rather than assumed, and two of the four
answers are counter-intuitive:

- **`Escape` does not close it.** `CPWL_Edit::OnCharInternal`
  (`cpwl_edit.cpp:586-590`) filters `kEscape` out, and a combo box never
  reaches `CFFL_TextField`'s escape handler because `CFFL_ComboBox::OnChar`
  forwards to `CFFL_TextObject`. Only a text field's filler destroys its
  window on Escape.
- **`Space` opens a gated combo and never shuts it**, where `Return` toggles.
  `CPWL_ComboBox::OnChar` (`:452-472`) runs `SetPopup(true)` only when the list
  is already closed, and returns `true` either way. A symmetric implementation
  gets this wrong in the direction that looks tidier.

The four that *do* close it: kill-focus (`:52-58`), a release on a list row
(`:505-516`), a second press on the drop button (`:497-503`, a toggle), and
`Return`. `SetPopup`'s **failure returns are reproduced too** — a list with no
options and one with no room both leave it shut while the click stays
*consumed*, which is a different answer from ignored.

**(b) Geometry.** `popup::place` is `SetPopup`'s clamp (`:325-377`) followed by
`QueryWherePopup` (`cffl_interactiveformfiller.cpp:670-729`), as one function
over numbers. Three parts are easy to get backwards and are pinned by tests:
the three-row floor applies only **above** three options (`> 3`), it
**outranks** the 140-unit cap because upstream clamps the constant into
`[min, max]` rather than clamping the wanted height, and a popup squeezed on
both sides takes the larger side's **room** rather than the height it asked
for.

Verified against both goldens' own pixels. `bug_736695_2`'s list is
28.784 units — two rows of 13.392 plus a 2-unit border — hanging from the
widget's bottom edge at page y 315.9, which is device rows 26..54 in a 342-tall
page: exactly the band the golden paints. `bug_1372651`'s is 42.176, three
rows, device rows 65..107.

**(c) Routing — the real correctness gap, and it was invisible to the metric.**
A mouse-down inside the popup's computed rectangle now **selects that row**.
Upstream this is not a special case at all: `CPWL_Wnd::OnLButtonDown` walks its
child windows before anything else and the list is a child. Here the widget hit
test is rect containment over `/Annots`, which the list is not in — so the same
click read as a **miss**, and a miss kills focus.

`bug_736695_3.in#form-events` scored **0.997003 while selecting nothing**, and
M14.md recorded it as a false pass for exactly that reason: the whole
disagreement is a 150×15 box on a 595×342 page, which SSIM cannot resolve.
Two tests in `crates/pdfrum-form/tests/combo_popup.rs` —
`a_click_inside_the_open_list_selects_that_row` and
`choosing_the_second_row_selects_it` — fail on the old behaviour. Verified by
disabling the two `popup_hit` interceptions and watching exactly those two go
red, and nothing else.

Down hover-selects and **up commits** (`CPWL_CBListBox::OnLButtonUp` →
`NotifyLButtonUp`), which is what makes a press that drags off the list leave
the field alone. Hover is recorded apart from selection because dismissing the
list must leave the stored value untouched — which is precisely what
`bug_736695_4` renders, and why that row was already passing at 0.999998.

`1475455` finished the commit half: `SetSelectText` (`:518-523`) *ends* on a
`SelectAllText()` and `NotifyLButtonUp` calls another one, so a chosen row
lands in the text half **with every character selected**. And `highlight_of`
answered `None` for anything but a text field, so even a selected editable
combo had no band to draw. An editable combo's text half **is** a `CPWL_Edit`
(`:190-203`), read-only only when the box is gated (`:115-118`), so it gets the
same caret and the same bands — measured in the plate left of the drop button,
which is the box `with_combo_edit` lays the text out in. A gated combo still
answers nothing, which is right.

**(d) Oracle parity, in the tool only.** `crates/pdfrum-tool/src/chrome.rs`.
The oracle's `pdfium_test` **is** a host — it composites `FPDF_FFLDraw` over
its bitmap — so the tool has to be one too, and it draws the list the library
declines to.

It draws it by **describing the popup as a list box and asking the ordinary
appearance generator to draw that**. `ap::field_body`'s list-box body already
has the row stack from the plate's top, the laid-out line height, the navy
`(0, 51, 113)` band, the white text on it and the clip; reusing it is what
keeps a popup's rows and a list box's rows from drifting apart, and it is a
synthetic dictionary rather than a second painter. Placed by `MatchRect` into
a rectangle of its own and appended after the annotation pass — never folded
into the widget's `/AP`, which is fitted onto `/Rect` and would have squeezed
twenty-nine units into fourteen.

Two details cost a measurement each and are worth the record:

- **The band is written as `/V`, the row's text, never as `/I`.**
  `ap::field_body::selected_indices` is `/V`-first and matches by *value*.
  That is correct for its own job — OWED item 7 settles it: without
  `/NeedAppearances` the producer is `CPDFSDK_AppStream::SetAsListBox` →
  `GetSelectedIndex`, and making the appearance reader index-first would flip
  `listbox_form.{in,pdf}` to fail. So an `/I [2]` here matched nothing and
  banded no row at all. A synthetic dictionary should speak the channel the
  reader speaks rather than ask the reader to change.
- **A widget with no `/DA` anywhere falls through to `FormFonts`' own
  fallback face** rather than giving up. `bug_736695_4.pdf` has no `/DA` on
  the widget and none on the form, and reaching for `default_appearance`
  alone drew no list at all on it — which was the whole of `bug_736695_2`.

The 12-unit scroll-bar reservation **does not apply**: `GetListRect`
(`cpwl_list_box.cpp:352-355`) is what `SetPlateRect` receives and therefore
what the rows are measured against, and it deflates by the border alone. Only
`GetClientRect` subtracts a bar, and `GetScrollBarWidth` answers **zero** while
the bar is invisible, which it is whenever the content fits. Neither target
fixture scrolls.

### OWED 8 — `clear_siblings` records the chosen control and cannot draw it. **Closed, `1e9365a`, on `6e87424`.**

The `pdfrum-doc` half was requested additively and landed as `6e87424`:
`ap::widget::LiveInput` gained `appearance_state: Option<&[u8]>`, threaded to
the single `is_checked` call site, with `None` reading the widget's own `/AS`
byte for byte.

The `pdfrum-form` half is `ToggleState::state_for_control`, which is the
per-kid answer a field-level record can give:
`CPDF_FormField::CheckControl` (`cpdf_formfield.cpp:683-716`) sets the clicked
control's `/AS` to **that control's own** on-state name and every other
control of the field to `Off`. A session holds one record per field, so all it
can store is which control was chosen — and turning that back into a per-kid
state is three cases: the chosen kid its own name, a sibling `Off`, and a
group nothing has clicked `None`, which is the file's `/AS` unchanged so a
loaded group renders exactly as written.

M14 recorded that storing the chosen control now would make this a one-line
change once the seam existed. It was: one `match` in `route.rs`'s `generate`.

### OWED 10 — the last `generate_with_live` caller. **Closed, `1e9365a`.**

Half of this was already paid in M14 (`8e45897` migrated `generate`); what
remained was, on inspection, **only the note**. `route.rs` had no
`generate_with_live` caller left — its one live call site was already
`generate_with_live_faces` — and the survivor was a doc comment on
`clear_siblings` naming the migration as still owed. That comment is now the
paragraph describing how the `/AS` override actually works, so the note is
gone because the thing it pointed at is done rather than because it was
deleted.

### Conformance: six rows, all of them form-events, all upward

Measured in isolation, which is the only honest way to attribute a delta with
three tracks in one tree. A worktree at this track's HEAD with **only** Track
B's three commits reverted (and `6e87424`'s construction site patched shut, so
the baseline builds at all) was scored against the same corpus and goldens,
and the two boards diffed field by field:

```
rows 1705 -> 1705,  0 added, 0 removed
differing rows: 6
rows with no `#form-events` in the path that Track B moved: 0
```

| status | ssim | delta | row |
|---|---|---|---|
| fail → fail | 0.910500 → 0.981546 | **+0.071046** | `bug_1372651.in#form-events` |
| fail → fail | 0.910500 → 0.981546 | **+0.071046** | `bug_1372651.pdf#form-events` |
| fail → **pass** | 0.979073 → 0.999650 | **+0.020577** | `bug_736695_2.in#form-events` |
| pass → pass | 0.997017 → 0.999787 | +0.002770 | `bug_736695_3.in#form-events` |
| pass → pass | 0.995299 → 0.999998 | +0.004699 | `combobox_form.in#form-events` |
| pass → pass | 0.995299 → 0.999998 | +0.004699 | `combobox_form.pdf#form-events` |

Those are the popup's own numbers, measured before the ClearType call site
below existed. The final board, with it, reads `bug_1372651.{in,pdf}`
**0.981722**, `bug_736695_2.in` **0.999673** and `bug_736695_3.in`
**0.999800**; the `combobox_form` pair is unchanged at 0.999998.

**All 1699 rows without `#form-events` in their path are byte-identical** —
status, tags, tier A, tier B and notes. (The `tags` field is empty on passing
rows, a scoreboard convention, so the path is the reliable discriminator and
the count above uses it.)

Two of the six deserve a word. `bug_736695_3` is the **false pass M14 recorded
becoming a real one**: it now selects `Spain`, leaves it selected, and draws
the navy band the golden draws — 157 band pixels against the golden's 135, in
the right place with the glyphs knocked out. And `combobox_form.{in,pdf}` is
one of M14's three starred sub-1% false passes, closing as a side effect of the
editable combo's selection band.

`bug_1372651` does not cross the 0.99 floor and this is the honest reason: its
residue is glyph antialiasing. Measured against its golden directly, differing
pixels went **3590 → 1044** and max channel **255 → 250**, with the `Item3`
navy band matching pixel for pixel and the list at the right size in the right
place. What is left is subpixel-filter arithmetic, and the ClearType call site
below closed the larger half of it: 1044 → 875 differing pixels, with the
list's coloured fringe pixels going from 1092 to **1509 against the golden's
1511**. The remainder — 315 in the widget's own text at max channel 250, 560 in
the popup's glyphs and 295 on the sibling push button's caption, all at 105 —
is the LCD filter's precision rather than anything about the dropdown, whose
geometry, border and selection band are exact.

### OWED 2's call site — landed here, `703a94a` + the popup's own `live_edit`

Track C's first plumbing commit named a call site in `pdfrum-form` that could
not exist, this track said so, and `f505534` replaced it with a per-object
carrier: `AnnotOverlay::set_live_edit` → `annot_render` →
`build_form_object_with` → `FormObject::live_edit` → `walk.rs`'s form options,
which folds the override into that subtree alone. The two call sites are both
in `pdfrum-tool` and both are one line:

- `session_overlay` marks the annotation whose appearance is a `LiveEdit`;
- `chrome::push_popup` builds its form object with `live_edit = true`.

The second is not an afterthought. The oracle's list is a `CPWL_Wnd`, and
`CPWL_ListBox::DrawThisAppearance` (`cpwl_list_box.cpp:66-84`) sets every row
through `CPWL_EditImpl::DrawEdit` → `DrawTextString`
(`cpwl_edit_impl.cpp:40-57`) — the same local `CPDF_RenderOptions` a focused
text field's glyphs take. Drawing the popup grey left 419 of the golden's
fringe pixels unmatched inside the list alone.

Board effect of the pair, on rows that are not this track's: `focused_ltr`
0.999311 → 0.999513, `selected_ltr` 0.999682 → **0.999936** with max channel
84 → **31**. That is the first corpus evidence the LCD filter has had — Track
C could pin its arithmetic in unit tests but had no oracle pixels to compare
against — and the fringes do match: within two pixels on the popup's count,
and a max channel that more than halves on the row with the most selected
text.

### Verification

- Final board, all three tracks in tree: **1705 files, 1652 pass, 53 fail**,
  `--check-regressions` clean. One row crosses and it is this track's:
  `bug_736695_2.in#form-events` fail → pass. Four form-events rows still fail —
  `bug_1372651.{in,pdf}` at 0.981722 and `scrollable_widgets1.{in,pdf}` at
  0.989732, the latter being OWED 5's scroll-bar chrome, out of M14 by the
  brief's appendix.
- `cargo nextest run -p pdfrum-form -p pdfrum -p pdfrum-tool`: **612 passed**,
  up from 566 at M14's close. `pdfrum-form` alone is 324, up from 307 before
  the select-all half.
- `cargo test --doc -p pdfrum`: 43 passed.
- `cargo clippy -p pdfrum-form -p pdfrum -p pdfrum-tool --all-targets
  -- -D warnings`: clean.
- `cargo fmt -p <crate>` per crate, never `--all`, and `scripts/ci.sh` was not
  run: two other tracks hold uncommitted work in this tree and either would
  have reformatted or re-scored their files.
- **The scoreboard is not committed.** The board this track measured is the
  three-track tree's, so committing it from here would attribute two other
  tracks' movements to this one and overwrite whatever they measure next. The
  isolated six-row delta above is the number that belongs to Track B.
- No benchmarks and no ratchet update, as instructed.

### One durable note for whoever touches `LiveInput` next

`ap::widget::LiveInput` is **not** `#[non_exhaustive]`, and `6e87424`'s new
`appearance_state` field therefore broke this crate's exhaustive struct literal
and left `pdfrum-form` failing to build for a period. The answer is *not* to
spell the literal loosely — naming every field is what makes an upstream
addition a compile error at the call site rather than a silent default, which
is the property that caught this one. It is to mark the struct: whoever adds a
sixth field should put `#[non_exhaustive]` on it first and give it a `Default`,
so a caller outside `pdfrum-doc` can still build one. The note also lives at
the construction site in `route.rs`, where a reader will actually hit it.
