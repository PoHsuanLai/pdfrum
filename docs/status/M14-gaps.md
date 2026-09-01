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
