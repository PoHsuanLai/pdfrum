# `pdfrum-render` status

**Updated:** 2026-08-29 · **State:** M3 exit criteria met; feature gaps named
below

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
| `color.rs` | `resolve_argb`, the `0xFFFFFFFF` invisibility sentinel, the truncating alpha, colour-mode translation, `FXRGB2GRAY` |
| `transfer.rs` | the `/TR` channel mapping, and Q1's resolution |
| `options.rs` | `RenderOptions`, `ColorMode`, `TextAa`, the oracle-conformance defaults, the type-3 and tile-cell option overrides |
| `path.rs` | `IsAvailableMatrix`, `CFX_Path::GetRect` with its normalisation, the rect-snapping ladder, `outer_rect`, the ±32000 clamp |
| `zero_area.rs` | `CheckSimpleLinePath`, `CheckPalindromicPath`, the folding-vertex scan, the `(int)c + 0.5` snap, the `>> 2` alpha |
| `stroke.rs` | the matrix1/matrix2 split, the one-device-pixel minimum, cap/join/miter mapping, the dash normalisation ladder |
| `paint.rs` | the five `DrawPath` fast paths in order, including the fill+stroke knockout buffer |
| `clip.rs` | the clip stack's reduction to device calls, the off-canvas empty-clip rect, text-clip unions |
| `blend.rs` | the twelve separable formulas, the verbatim `kColorSqrt`, `Lum`/`Sat`/`SetLum`/`SetSat`/`ClipColor`, the straight-alpha composite |
| `shading/` | all seven rasterizers, the 256-entry ramp, `component_to_shading_index`, the Coons subdivider |
| `group.rs` | the offscreen predicate, the backdrop rule, the mask/alpha ordering, the composite transparency |
| `softmask.rs` | the `/BC` backdrop, the luminosity and alpha readbacks, the `/TR` lookup |
| `text.rs` | the `Tr` mode table, the glyph matrix, the stroked-text CTM split, glyph placement |
| `image.rs` | `UseInterpolateBilinear`, the `kHugeImageSize` rule, the CMYK-overprint gate, the flip rule, sample-to-pixmap, the `/Matte` un-premultiply |
| `ctx.rs` | `RenderCtx`, the depth cap, the type-3 font *set*, `RenderCaches` |
| `walk.rs` | `render_page`, the object dispatch, the cull test, the page matrix, the group path |
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
- **The `/TR` array maps directly to R, G, B**, per the oracle's own unit
  test — see the Q1 resolution below.
- **The offscreen predicate's absences matter.** A non-isolated group with
  `/Group` present, `/I` absent, `ca` at one and no soft mask draws
  *directly*, with no group semantics at all.

## Numbers

Measured against the golden store on the full corpus (1468 files, 1421 with
a golden PNG), rendering through `tiny-skia`:

| metric | value | M3 target |
|---|---|---|
| pixel files at SSIM ≥ 0.99 | **900 / 1421 (63.3%)** | ≥ 60% |
| at SSIM ≥ 0.95 | 1230 / 1421 (86.6%) | — |
| at SSIM ≥ 0.90 | 1294 / 1421 (91.1%) | — |
| byte-exact PNGs | 346 / 1421 | — |
| `size-mismatch` | 1 | — |
| Tier C hard failures (95-file sample) | **0** | — |

Tier C's *edge* rate is reported rather than gated; `conformance/src/tierc.rs`
explains why, and the short version is that the brief's 1% budget is written
against a trace-derived mask several times larger than the neighbourhood
proxy the harness can compute today. What the contract actually gates on —
that every engine decision is identical under both rasterizers — holds at
zero failures.

Metadata and pageinfo remain 100% Tier-A byte-exact; the text tier is the
concurrent `pdfrum-text` work's and is unaffected.

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

**Q1 (the transfer-function array reversal) resolves for the observable.**
`CPDFDocRenderDataTest.TransferFunctionArray` asserts, for `[Type0, Type2,
Type4]`, that `GetSamplesR() == Type0`, and its ten `TranslateColor`
expectations agree. `pdfrum-page`'s `TransferFunc` stores the array reversed
relative to that — a defect in its storage against its own doc comment —
so `render::transfer` undoes it at the boundary rather than changing a parse
whose tests pin the reversed spelling. Worth fixing in `pdfrum-page` when
that crate is next touched.

**Q5 resolves as proposed.** `render_page` chooses white-vs-transparent from
the page's own transparency, matching `pdfium_test`; `RenderOptions.background`
is an override only. The choice is load-bearing rather than a convenience:
the engine's single compositing path relies on an opaque page's white being
real pixels, which is what makes D6's collapse of PDFium's five-armed
compositor into one arm arithmetically correct.

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

## What is not implemented yet

Each has its ported decision logic and unit tests in place; what is missing
is the wiring, and `pattern.rs` is the one genuine gap.

- **Tiling and shading patterns.** `pdfrum-page` parses the pattern and
  attaches it to `Content::pattern`, and nothing drains it into a cell
  render. This is the largest remaining pixel gap.
- **Type-3 glyph procedures**, and the blue-zone cache their horizontal
  edges snap to.
- **A soft mask's own group content.** The mask's `/BC` backdrop is honoured
  and the group's objects are not rendered into it, which is exact for a
  backdrop-only mask and wrong otherwise.
- **`/Matte` un-premultiplication** and the masked-image-on-white path: the
  arithmetic is written and tested in `image.rs`, the image walk does not
  call it.
- **Annotation appearance layers**, which await `pdfrum-doc`.

## Tests

184 in the three crates plus their integration test — 171 unit, 15
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
