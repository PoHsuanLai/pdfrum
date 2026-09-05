# Backend verification — `vello_cpu 0.2.0` vs `tiny-skia 0.12.0`

**Closes render brief Q4.** The render brief (`docs/design/pdfrum-render.md`)
§5 flagged its entire `vello_cpu` column as **INFERRED** — written on a machine
with no network and no `vello_cpu`/`peniko` in the cargo registry. This document
replaces that inference with verification against the real crate sources, plus
executable probes that were compiled and run against both backends.

**Do not edit SPEC.md from this document.** §3 lists the SPEC §8 adjustments
this verification implies; the orchestrator rules on them.

---

## 0. Method and source provenance

The pinned crates were fetched from `static.crates.io` and unpacked. Two
locations are cited below; both are the same bytes:

| Crate | Cited path prefix |
|---|---|
| `vello_cpu 0.2.0` | `$V/vello_cpu-0.2.0/` |
| `vello_common 0.2.0` | `$V/vello_common-0.2.0/` |
| `peniko 0.6.1` | `$V/peniko-0.6.1/` |
| `color 0.3.3` | `$V/color-0.3.3/` |
| `tiny-skia 0.12.0` | `$R/tiny-skia-0.12.0/` |
| `tiny-skia-path 0.12.0` | `$R/tiny-skia-path-0.12.0/` |
| `kurbo 0.13.1` | `$R/kurbo-0.13.1/` |

where
`$R = ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`
and `$V` is the unpack directory
a scratch directory outside the repo.
`vello_common-0.2.0`, `peniko-0.6.1` and `color-0.3.3` were additionally
materialised under `$R` by cargo when the probe crate was built, so
`$R/vello_common-0.2.0/src/encode.rs:494` etc. also resolve. `vello_cpu`'s own
source is only under `$V` (cargo unpacked it there for the probe build under a
different target dir).

**Probes.** A throwaway crate depending on `vello_cpu = "=0.2.0"` (with
`features = ["multithreading", "f32_pipeline"]`), `tiny-skia = "=0.12.0"`,
`peniko = "=0.6.1"`, `kurbo = "=0.13.1"` was built and run. Every row marked
**probed** below carries an observed pixel value, not a reading of the source.
This matters: two of the most consequential findings (B1, B2) are behaviours
that the source alone does not make obvious.

---

## 1. Blocking findings (read these first)

### B1 — `draw_image(..., alpha)` **panics** on vello_cpu 0.2.0

SPEC §8 declares
`fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32)`.
The natural vello_cpu implementation is `peniko::ImageSampler { alpha, .. }`.
That field exists (`$V/peniko-0.6.1/src/image.rs:103`) but vello_cpu refuses it:

```rust
// $V/vello_common-0.2.0/src/encode.rs:492-495
if sampler.alpha != 1.0 {
    // If the sampler alpha is not 1.0, we need to force alpha compositing.
    unimplemented!("Applying opacity to image commands");
}
```

**Probed:** `set_paint(Image { sampler: ImageSampler { alpha: 0.5, .. }, .. })`
followed by `fill_rect` →
`panicked at vello_common-0.2.0/src/encode.rs:494: not implemented: Applying opacity to image commands`.

This is not a rounding difference; it is a hard panic on a code path the engine
takes for **every** image drawn with a non-1.0 `ca`/`CA` — which is common in
the corpus (`alpha_composite`, `transparent*`, every watermark).

tiny-skia has no such problem: `PixmapPaint::opacity`
(`$R/tiny-skia-0.12.0/src/shaders/pattern.rs:38`) is honoured.
**Probed:** `draw_pixmap` of opaque red over opaque green at `opacity: 0.5`
→ `(128, 128, 0, 255)`. Correct.

**Recommendation — engine-side workaround, no trait change.** Keep
`draw_image`'s `alpha: f32` in the trait; it is the right vocabulary and
tiny-skia implements it directly. `pdfrum-raster-vello-cpu` implements it as
`push_opacity_layer(alpha)` → draw the image with `sampler.alpha = 1.0` →
`pop_layer()`. This is verified to work (see B4's combined-layer probe, which
exercises `push_layer` with an opacity). Cost: one layer per non-opaque image
draw, which vello_cpu handles natively. Add a backend unit test that asserts a
non-1.0 alpha image draw does **not** panic, so a future vello_cpu that
implements `sampler.alpha` cannot silently change the rounding.

### B2 — hard-edged fills do **not** give `full_cover` when alpha < 1, and the two backends disagree about what "hard-edged" means

Brief §5.3 proposes emulating AGG's `full_cover` by drawing Coons/tensor patch
cells with anti-aliasing off. **That is only correct for opaque cells.**

**Probed**, two abutting cells splitting at x = 3.5, drawn over white:

| | x=0..2 | **x=3 (the shared edge)** | x=4..7 |
|---|---|---|---|
| vello_cpu, AA on, alpha 1.0 | 255,0,0 | **255,64,64** (the ~75% seam) | 255,0,0 |
| vello_cpu, `aliasing_threshold=Some(128)`, alpha 1.0 | 255,0,0 | **255,0,0** ✓ | 255,0,0 |
| tiny-skia, AA on, alpha 1.0 | 255,0,0 | **191,0,0** (the ~75% seam) | 255,0,0 |
| tiny-skia, `anti_alias=false`, alpha 1.0 | 255,0,0 | **255,0,0** ✓ | 255,0,0 |
| vello_cpu, `aliasing_threshold=Some(128)`, **alpha 0.5** | 255,127,127 | **255,64,64** ✗ double-written | 255,127,127 |
| tiny-skia, `anti_alias=false`, **alpha 0.5** | 255,127,127 | **255,127,127** ✓ | 255,127,127 |

The mechanism, probed per-cell in isolation (alpha channel of a single cell):

```
vello_cpu  thr=Some(128)  left cell:  255 255 255 255   0   0   0   0
vello_cpu  thr=Some(128)  right cell:   0   0   0 255 255 255 255 255   <-- x=3 claimed by BOTH
tiny-skia  anti_alias=false left:     255 255 255 255   0   0   0   0
tiny-skia  anti_alias=false right:      0   0   0   0 255 255 255 255   <-- x=3 claimed by ONE
```

- **vello_cpu** binarizes each path's *own* coverage:
  `$V/vello_common-0.2.0/src/strip.rs:492-498`,
  `u8_vals.simd_ge(splat(aliasing_threshold)) ? 255 : 0`. A pixel with 0.5
  coverage from each of two cells is written at full alpha **twice**.
- **tiny-skia** `anti_alias=false` runs Skia's non-AA scan converter, a strict
  pixel-centre rule, so exactly one cell owns each pixel. The cells **tile**.

Note which one matches the oracle's *binarization*: PDFium's `aliased_path`
thresholds coverage at `> 127 → 255`
(brief §1.7, `agg_rasterizer_scanline_aa.h:283-297`) — i.e. **vello_cpu's**
rule, not tiny-skia's. But PDFium's `full_cover` is a *different* flag that
bypasses coverage entirely (`src_alpha = alpha`, brief §1.17), which is what
makes abutting cells seam-free *at any alpha*. Neither backend exposes that.

**Recommendation — engine-side, and §5.3 must be corrected.** Brief §5.3 already
says to render the patch into a scratch `Pixmap` and `draw_image` it onto the
target with the shading's alpha. **Keep that scratch pixmap — it is not
optional, it is what makes the emulation correct.** Draw every cell into the
scratch at **alpha 1.0** with AA off; the cells then tile at full opacity on
both backends (rows 2 and 4 above), and the shading's alpha is applied *once*,
by the single `draw_image` of the scratch. The alpha-0.5 double-write in row 5
never arises because no cell is ever drawn at alpha < 1.

This makes §5.3 correct as written *provided* the "with the shading's alpha"
clause in its step 3 is read as strictly excluding per-cell alpha. Make that
explicit in the brief, and add it to §6.4's Tier-C fixture notes: with this
rule the two backends still differ at x=3 for a *scratch-internal* cell edge
(vello 255/255 double-write vs tiny-skia single write) — but since both write
opaque red, the composited result is identical. Add a Tier-C assertion on
`2_shading_type_6_00` that the two scratch pixmaps are byte-identical.

### B3 — tiny-skia **silently drops** thin path fills; vello_cpu does not

```rust
// $R/tiny-skia-0.12.0/src/painter.rs:218-222   (Pixmap::fill_path)
let path_bounds = path.bounds();
if path_bounds.width().is_nearly_zero() || path_bounds.height().is_nearly_zero() {
    log::warn!("empty paths and horizontal/vertical lines cannot be filled");
    return;
}
```
`is_nearly_zero` is `|x| <= 1.0/4096.0` (`$R/tiny-skia-path-0.12.0/src/scalar.rs:12,64`).
The identical guard is in `Mask::fill_path` (`$R/tiny-skia-0.12.0/src/mask.rs:262-266`),
so a **degenerate clip path silently becomes a no-op clip** rather than an
empty clip — a correctness inversion, not just a missing pixel.

**Probed**, a 5px-wide rect of height *h* filled with AA on:

| h | tiny-skia `fill_rect` | tiny-skia `fill_path` | vello_cpu `fill_path` (max alpha) |
|---|---|---|---|
| 0.5 | drawn | drawn | 128 |
| 0.1 | drawn | **dropped** | 26 |
| 0.05 | drawn | **dropped** | 13 |
| 0.01 | drawn | **dropped** | 3 |
| 0.001 | **dropped** | **dropped** | 0 |

`fill_rect` survives longer only because its identity-transform fast path
(`$R/tiny-skia-0.12.0/src/painter.rs:192-210`) skips the bounds guard; with any
non-identity transform it routes to `fill_path` and the guard applies again.

How much of this the engine already absorbs: PDFium's axis-aligned rect snapping
(brief §1.7 step 2) integer-snaps such rects to ≥1px *before* the backend, and
the zero-area rule (§1.7 step 3) converts degenerate sub-paths into 1-device-pixel
hairline **strokes**. So the exposure is the residue: thin fills that are neither
axis-aligned nor zero-area, and rect fills reached with `rect_aa` set.

**Recommendation — engine-side guard, no trait change, no backend avoidance.**
`pdfrum-raster-tinyskia` must not paper over this per-call (a per-call fixup
would diverge from vello and break §6.1's "geometry is identical"). Instead:
1. The engine ports §1.7 steps 2 and 3 faithfully (already planned, D14) — this
   removes the bulk of the cases.
2. Add a Tier-C harness assertion: if either backend produced zero touched
   pixels for a `fill_path` call that the other backend painted, that is a
   **hard_fail**, not an edge-budget difference. This turns the silent drop into
   a loud test failure instead of a quiet 1% drift.
3. Document in §6.2 that sub-1/4096-px path extents are **out of contract**;
   the engine must not emit them.

---

## 2. Full verification table

Legend: **CONFIRMED** = brief's claim holds; **REFUTED** = brief's claim is
wrong; **REFINED** = true but the brief's reasoning or wording is materially
off; **GAP** = real capability gap.

### 2.1 SPEC §8 `RenderDevice` methods

| SPEC §8 method | vello_cpu 0.2.0 | tiny-skia 0.12.0 | Verdict |
|---|---|---|---|
| `fill_path(path, t, brush, rule, aa)` | `RenderContext::fill_path` (`$V/vello_cpu-0.2.0/src/render.rs:278`) + `set_fill_rule(Fill)` (`:650`) + `set_transform` (`:675`) + `set_paint` (`:601`) + `set_aliasing_threshold` (`:570`). Note: transform/paint/rule are **context state**, not per-call args. | `Pixmap::fill_path(path, &Paint, FillRule, Transform, Option<&Mask>)` (`$R/tiny-skia-0.12.0/src/painter.rs:217`); `Paint::anti_alias` (`:47`) | **CONFIRMED** both. B3 caveat on tiny-skia. |
| `stroke_path(path, t, brush, stroke, aa)` | `stroke_path` (`render.rs:299`) + `set_stroke(kurbo::Stroke)` (`:581`). Stroking is delegated to `kurbo::stroke_with` (`$V/vello_common-0.2.0/src/flatten.rs:256`), incl. dashes. | `Pixmap::stroke_path(path, &Paint, &Stroke, Transform, Option<&Mask>)` (`painter.rs:330`) | **CONFIRMED** both |
| `draw_image(img, t, quality, alpha)` | quality/extend: yes (`peniko::ImageQuality::{Low,Medium,High}` all implemented — `$V/vello_cpu-0.2.0/src/fine/common/image.rs:244,275` bilinear/bicubic, `:150` nearest; `Extend::{Pad,Repeat,Reflect}` at `:442-450`). **`alpha`: PANICS** (B1). | `Pattern::new(pixmap, SpreadMode, FilterQuality, opacity, Transform)`; `FilterQuality::{Nearest,Bilinear,Bicubic}` (`shaders/pattern.rs:19-25`); `PixmapPaint::opacity` (`:38`) | **GAP on vello (B1)**; CONFIRMED tiny-skia |
| `push_clip(path, rule)` | Two distinct mechanisms: `push_clip_path`/`pop_clip_path` (`render.rs:726,740`) — a clip *stack*, may be left pending at render time; and `push_clip_layer`/`pop_layer` (`:522`) — an isolated layer. Use the former. Intersection is native. | No clip stack. `Mask::intersect_path(path, FillRule, anti_alias, Transform)` (`$R/tiny-skia-0.12.0/src/mask.rs:352`); intersection is literally `premultiply_u8(a,b)` = `a·b/255` (`:360-363`) — **identical to `CFX_AggClipRgn`**, as the brief claims. `Mask: Clone` (probed). | **CONFIRMED** both, incl. the `CFX_AggClipRgn` equivalence |
| `push_clip_rect(rect)` — **E2, hard-edged** | `set_aliasing_threshold(Some(128))` then `push_clip_path`. The threshold is threaded through `Dispatcher::push_clip_path`'s `aliasing_threshold` param (`$V/vello_cpu-0.2.0/src/dispatch/mod.rs:47-53`) and `ClipContext::push_clip` (`$V/vello_common-0.2.0/src/clip.rs:90-96`). **Probed:** clip to `[0.5,4.5]` on an 8px row → alphas `0 255 255 255 0 0 0 0`. Truly hard. | `Mask::fill_path(rect_path, Winding, anti_alias=false, …)`. **Probed:** same clip → mask row `0 255 255 255 255 0 0 0`. Truly hard. | **CONFIRMED** both. Note the two backends pick **different** pixels at the 0.5 boundary (vello 4 px, tiny-skia 4 px but shifted) — see §2.4. |
| `push_layer(blend, alpha, mask)` | **NATIVE and complete.** `RenderContext::push_layer(clip_path: Option<&BezPath>, blend_mode: Option<BlendMode>, opacity: Option<f32>, mask: Option<Mask>, filter: Option<Filter>)` (`render.rs:471-478`). All of clip+blend+alpha+mask in one call. **Probed:** a layer with clip circle + `Mix::Multiply` + opacity 0.5 + an alpha mask composited correctly (centre `(191,223,191,255)`, outside-clip `(255,255,255,255)`). | **ABSENT** — must be emulated. Brief §5.2's recipe **probed and confirmed**: offscreen `Pixmap`, scale premultiplied bytes by alpha, `draw_pixmap(.., &PixmapPaint{blend_mode, ..}, .., Some(&clip_mask))`. Observed `(127,127,255)` inside clip, unchanged outside. | **CONFIRMED** — brief was right on both counts. Q4(a) **answered: yes**. |
| `pop()` | `pop_layer()` (`render.rs:575`) / `pop_clip_path()` (`:740`) | pop the `Vec<LayerFrame>` / restore the previous `Mask` | **CONFIRMED** |

### 2.2 SPEC §8 `RasterBackend` methods (the E2/E3/E4/E10 additions)

| SPEC §8 method | vello_cpu 0.2.0 | tiny-skia 0.12.0 | Verdict |
|---|---|---|---|
| `new_target(w, h, clear)` — **E10** | `RenderContext::new(w: u16, h: u16)` (`render.rs:209`) + a `Pixmap` pre-filled with `clear` + `RasterizerSettings { composite_mode: CompositeMode::SrcOver }` (`render.rs:101-107, 131`). **Probed:** green-filled target + a 50%-red fill under `SrcOver` → covered `(128,64,0,255)`, uncovered `(0,128,0,255)`. The clear colour survives. | `Pixmap::new(w,h)` + `Pixmap::fill(Color)` (`$R/tiny-skia-0.12.0/src/pixmap.rs:220`) | **CONFIRMED** both. See §2.5 for the `u16` constraint this imposes on vello. |
| `new_target_with_backdrop(base)` — **E4** | Same mechanism: seed the destination `Pixmap` with `base`'s pixels, render with `CompositeMode::SrcOver`. **Probed above.** | `Pixmap::from_vec` (`pixmap.rs:62`), or `draw_pixmap` with `BlendMode::Source`. **Probed:** `Source` blit of red over green → `(255,0,0,255)`. Exact overwrite. | **CONFIRMED** both |
| `snapshot(d)` — **E3, mid-render** | `RenderContext::render(&self, target, resources)` takes **`&self`** (`render.rs:756`), so a scene may be rasterized, then drawn on further, then rasterized again. **Probed:** filled a red quadrant, `flush()`, `render()` → red at (0,0); then filled a blue quadrant on the *same* context, `flush()`, `render()` → red at (0,0) **and** blue at (5,5). Mid-render snapshot works. One constraint: `render_with` asserts `!self.dispatcher.has_layers()` (`render.rs:800-803`) — **all layers must be popped before a snapshot**. `flush()` is mandatory in multithreaded mode (`:749`, and `assert!(self.flushed)` at `$V/vello_cpu-0.2.0/src/dispatch/multi_threaded.rs:646`). | `Pixmap::data()` / `clone_rect(IntRect)` (`pixmap.rs:230,285`) — trivially, at any time. | **CONFIRMED** both. Brief's "**verify (Q4)**" resolves to **yes**. Q4(b) **answered: no `RasterBackend` change needed.** |
| `finish(d) -> Pixmap` (RGBA8 premul) | `vello_common::pixmap::Pixmap` is `Vec<PremulRgba8>` (`$V/vello_common-0.2.0/src/pixmap.rs:18,337`). `PixelFormat::Rgba8` is the only variant and is documented "Premultiplied RGBA8" (`render.rs:111-115`). `take_unpremultiplied() -> Vec<Rgba8>` exists (`pixmap.rs:416`). | `Pixmap` is premultiplied RGBA8 (`PremultipliedColorU8`), `take_demultiplied()` (`pixmap.rs:269`) | **CONFIRMED** both, incl. §5.4's premultiplication story and the `finish`-must-unpremultiply conclusion. **Neither is BGRA.** |

### 2.3 Brief §5.1 capability-matrix rows

| Brief row | Brief's claim | Verified finding | Verdict |
|---|---|---|---|
| Fill path, winding/even-odd, AA on/off | exact both | Confirmed. vello: `Fill::{NonZero,EvenOdd}`, `set_aliasing_threshold`. tiny-skia: `FillRule`, `Paint::anti_alias`. | **CONFIRMED** |
| Stroke with cap/join/miter/dash | exact both, dash caveat | Confirmed. vello_cpu delegates to `kurbo::stroke_with` (`flatten.rs:256`), which dashes internally (`$R/kurbo-0.13.1/src/stroke.rs:289-297`). | **CONFIRMED** |
| Solid colour brush | exact both | Confirmed | **CONFIRMED** |
| Image brush, nearest vs bilinear | "**exact**" both | Both have three levels (Nearest/Bilinear/Bicubic ≡ Low/Medium/High). But vello silently **downgrades Medium→Low** for integer-only translations (`$V/vello_common-0.2.0/src/encode.rs:500-509`); tiny-skia does not. A pure-translation image draw will therefore be nearest on vello and bilinear on tiny-skia — a **Tier-C divergence the brief does not mention**. | **REFINED** — see §3 item 5 |
| Clip by path, AA, intersecting | exact / native | Confirmed, incl. the `premultiply_u8` = `CFX_AggClipRgn` equivalence | **CONFIRMED** |
| Clip by integer rect, hard-edged (E2) | exact both | Confirmed, probed both | **CONFIRMED** |
| Layer with blend+alpha+mask | **gap on tiny-skia**, native on vello | Confirmed exactly. vello's `push_layer` takes all four (clip, blend, opacity, mask) plus a filter. tiny-skia emulation probed working. | **CONFIRMED** |
| The 16 PDF blend modes | `peniko::Mix` has all 16; tiny-skia's `BlendMode` has all 16 | Confirmed. `peniko::Mix` = 16 variants (`$V/peniko-0.6.1/src/blend.rs:11-87`). tiny-skia `BlendMode` (`$R/tiny-skia-0.12.0/src/blend_mode.rs:5-64`) has all 12 separable + Hue/Saturation/Color/Luminosity. vello_cpu implements all 16 in `highp` (`$V/vello_cpu-0.2.0/src/fine/highp/blend.rs:98-113`); the default `lowp` u8 pipeline covers 9 natively and **falls back to the f32 path** for ColorDodge/ColorBurn/SoftLight/Luminosity/Color/Hue/Saturation (`$V/vello_cpu-0.2.0/src/fine/lowp/blend.rs:18-31, 72-78`) — a fallback, not a gap. **Probed:** per-fill `Mix::Luminosity` of blue over white → `(28,28,28,255)`; `Mix::Multiply` → `(0,0,255,255)`. Both correct. | **CONFIRMED** |
| Blend-mode on a blit | `PixmapPaint::blend_mode` / `push_layer` + `draw_image` | Confirmed, and **better than claimed** on vello: `set_blend_mode` (`render.rs:621`) applies to a plain `fill_path`/`fill_rect` with **no layer at all** (probed). Fewer layers than the brief assumed. | **CONFIRMED / better** |
| Multiply a target's alpha by a scalar | "**none**" on tiny-skia; iterate `pixels_mut` | **REFUTED for vello**: `Pixmap::multiply_alpha(u8)` exists (`$V/vello_common-0.2.0/src/pixmap.rs:210`) and scales all four premultiplied channels — exactly §5.4's row. tiny-skia genuinely has none; `pixels_mut()` (`pixmap.rs:250`) is the answer, and the probe confirms hand-scaling premultiplied bytes composites correctly. | **REFINED** |
| Multiply a target's alpha by a mask | `Pixmap::apply_mask` / `push_layer(mask)` | Confirmed. `apply_mask` (`$R/tiny-skia-0.12.0/src/painter.rs:518`) requires identical size (`:520`), as does vello's `push_layer`, which **silently drops** a mismatched mask (`$V/vello_cpu-0.2.0/src/render.rs:479-485`). **E8's device-sized-and-aligned invariant is mandatory on both, and vello fails silently if violated** — the brief calls E8 "a documented invariant"; it needs to be an asserted one. | **CONFIRMED**, with a hardening note (§3 item 4) |
| Read back pixels (E3, E4) | "**verify (Q4)**" | **Resolved: yes**, see §2.2 | **CONFIRMED** |
| Seed a target with existing pixels | exact both | Confirmed, probed both | **CONFIRMED** |
| `full_cover` / constant alpha ignoring coverage | "**absent** — gap on both" | Confirmed absent on both, **and the emulation is subtler than §5.3 says** — see B2. | **CONFIRMED gap; emulation REFINED** |
| Premultiplied RGBA8 output | exact both | Confirmed | **CONFIRMED** |
| Determinism across runs | vello "multithreaded and SIMD-dispatched — must be verified" | See §2.6. Multithreading is **bit-exact**; SIMD level and pipeline choice are **not**, by ±1. | **REFINED** |

### 2.4 Gradients — `/Extend` semantics

| Aspect | vello_cpu / peniko | tiny-skia | PDF need |
|---|---|---|---|
| Axial | `GradientKind::Linear(LinearGradientPosition{start,end})` (`$V/peniko-0.6.1/src/gradient.rs:144,271`) | `LinearGradient::new(start, end, stops, SpreadMode, Transform)` (`$R/tiny-skia-0.12.0/src/shaders/linear_gradient.rs:35`) | ✓ |
| Radial (two-point conical) | `RadialGradientPosition{start_center,start_radius,end_center,end_radius}` (`gradient.rs:164`); implementation is a port of Skia's `SkConicalGradient` (`$V/vello_common-0.2.0/src/encode.rs:122-161`) | `RadialGradient::new(start_point, start_radius, end_point, end_radius, …)` (`radial_gradient.rs:120`) — also Skia | ✓ both, and they are the **same algorithm**, which is good for Tier C |
| Extend | `Gradient::extend: Extend` — **one** mode for the whole gradient (`gradient.rs:304`); `Extend::{Pad,Repeat,Reflect}` (`$V/peniko-0.6.1/src/brush.rs:159-166`) | `SpreadMode::{Pad,Reflect,Repeat}` (`$R/tiny-skia-0.12.0/src/shaders/mod.rs:27-39`) — also one mode | PDF `/Extend [bool bool]` is **asymmetric**: extend-start and extend-end are independent. **Neither backend can express `[true false]` or `[false true]`.** |
| Degenerate handling | `validate()` returns the first stop's colour as a solid (`$V/vello_common-0.2.0/src/encode.rs:281-329`); negative radii, coincident points, unsorted/out-of-range offsets all → solid | `LinearGradient::new`/`RadialGradient::new` return `Option`; degenerate → `Shader::SolidColor` or `None` (`linear_gradient.rs:45-60`, `radial_gradient.rs:132-150`) | Different fallbacks. Engine must decide. |

**The `/Extend` finding is not in the brief at all.** PDF's asymmetric extend is
common (`/Extend [true false]` on a shading that should fade out at one end).

**Recommendation — engine-side, no trait change.** The engine already owns
gradient construction. For an asymmetric `/Extend`:
- Use `Extend::Pad` / `SpreadMode::Pad` (which covers `[true true]` exactly, the
  common case).
- For a `false` on either side, `Pad` over-paints beyond the axis. The engine
  must **clip** the gradient fill to the half-plane (axial) or the annulus
  (radial) that `/Extend` permits, using the clip it already has. This is a
  `push_clip` of an engine-computed path, so both backends agree by
  construction (D14's principle) — and it is exactly what the oracle does
  (brief §1.10's per-pixel `t` range check produces the same silhouette).
- The engine must also normalise stops (sorted, `[0,1]`, first at 0.0 and last
  at 1.0) itself rather than relying on either backend's fallback, since the two
  fallbacks differ. `peniko` will synthesise the 0.0/1.0 stops itself
  (`encode.rs:89-105`); tiny-skia will not. Normalise engine-side so both see
  the same input.

### 2.5 Constraints the brief does not record

| Constraint | Source | Consequence |
|---|---|---|
| **vello_cpu dimensions are `u16`.** `RenderContext::new(width: u16, height: u16)` (`$V/vello_cpu-0.2.0/src/render.rs:209`), `Pixmap::new(u16,u16)` (`$V/vello_common-0.2.0/src/pixmap.rs:81`), `Mask` is `u16`-sized (`mask.rs:92,98`). `ImageSource::from_peniko_image_data` **asserts** `width/height <= u16::MAX` (`paint.rs:135-139`). | source | SPEC §8's `new_target(&self, w: u32, h: u32, …)` cannot be honoured above 65535 px on vello. tiny-skia is `u32` (`pixmap.rs:43`). A `RasterBackend` must return an error or the engine must cap. **PDFium's own `kMaxImageDimension` and the render-target size limits (brief §1.14) should be checked against 65535.** |
| **vello_cpu's `Mask` luminance uses BT.709** (`0.2126/0.7152/0.0722`, `$V/vello_common-0.2.0/src/mask.rs:74-78`), as does tiny-skia's `MaskType::Luminance` (`$R/tiny-skia-0.12.0/src/mask.rs:86`). PDFium uses `FXRGB2GRAY` (0.3/0.59/0.11). | source | The engine **must not** use `Mask::new_luminance` / `Mask::from_pixmap(_, Luminance)`. Build the 8-bit buffer with the oracle's coefficients and use `Mask::from_parts(Vec<u8>, w, h)` (`mask.rs:43`) / `Mask::from_vec(Vec<u8>, IntSize)` (`$R/tiny-skia-0.12.0/src/mask.rs:98`). Both accept raw bytes, so this is free. Worth stating explicitly in §1.13, since the convenience constructors are the obvious wrong choice. |
| **vello_cpu is a retained scene, not an immediate-mode painter.** Drawing calls record into a `Dispatcher`; pixels appear only at `render()`. | `render.rs:278-296, 756-831` | Harmless for us (the engine is already a one-pass emitter) but it means `RasterBackend::Device` for vello is `(RenderContext, Pixmap)`, and `snapshot` costs a full rasterization of the scene so far — **O(scene) per snapshot, not O(rect)**. With E3/E4 firing once per non-isolated group and once per fill+stroke knockout, a pathological file could be quadratic. **Recommend** the engine render each group into its **own** `RenderContext` (which it must do anyway for `new_target_with_backdrop`) rather than snapshotting a shared one, so snapshots stay small. |
| **`push_layer` and the clip stack are different stacks.** `render_with` asserts `!has_layers()` (`render.rs:800`) but explicitly permits pending `push_clip_path`s (`:737-739`). | source | The backend's `pop()` must know which stack it is popping. Track it. |
| **`multithreading` is not a default feature** (`$V/vello_cpu-0.2.0/Cargo.toml:42-48`); `RenderSettings::num_threads` defaults to `available_parallelism()-1` capped at 8 **only when the feature is on** (`render.rs:194-203`). | source | `Cargo.toml` must opt in deliberately. Given §2.6, single-threaded is the safe default for conformance runs. |
| **Filters panic in multithreaded mode** (`render.rs:468-470, 552-554`). | source | We never use filters. Do not enable the code path. |

### 2.6 Determinism (Q4(c))

**Probed**, a 256×256 scene (rotated circle fill + 2.7px stroked rect), rendered
under every combination and byte-compared:

| Comparison | Differing bytes / 262144 | Max Δ |
|---|---|---|
| single-threaded vs 4-threaded, same SIMD level | **0** | 0 |
| 4-threaded, run to run | **0** | 0 |
| `Level::baseline()` vs `Level::Avx2` (u8 pipeline) | 2 | **1** |
| `Level::baseline()` vs `Level::Avx2` (f32 pipeline) | 2 | **1** |
| u8 pipeline vs f32 pipeline (same level) | 24 | **1** |

The SIMD-level difference is **reproducible** (identical byte indices
`[39184, 39185]` across three repeats) and affects the u8 and f32 pipelines
**identically**, which locates it in the shared flattening/tiling stage
(`$V/vello_common-0.2.0/src/{flatten_simd,tile}.rs`), not in compositing. On a
denser scene (60 overlapping primitives) the difference was **0 bytes**.

**Verdict — the brief's gap #4 is REFUTED as stated, REFINED in substance:**
- Multithreading is **bit-exact**. The brief's headline worry is unfounded.
- SIMD feature level is **not** bit-exact, by ±1 on a handful of pixels.

**Recommendation — no trait change; adjust §6.3's gate.** ±1 is already inside
§6.2's "Rounding of the composite: ±1, everywhere" budget, so the Tier-C
contract absorbs this without modification. But §6.1's third row demands
**zero tolerance** on engine-computed values, and §6.3's `hard_fail` gate is
`> tol(px)` on *interior* pixels — a flattening difference can move an edge and
thus land on a pixel the trace-derived `edge_mask` did not dilate over. Two
mitigations, both cheap:
1. **Pin `RenderSettings::level` explicitly** in `pdfrum-raster-vello-cpu` (do not
   call `Level::try_detect()`), and expose it. For golden/snapshot tests, pin
   `Level::baseline()`. This makes our own output reproducible across
   developer machines and CI, which is the property that actually matters.
2. Pin `RenderMode` explicitly too — prefer `OptimizeQuality` (f32) for
   conformance, since `f32_pipeline` is the documented "more accurate…
   especially useful for rendering test snapshots" path (`$V/vello_cpu-0.2.0/src/lib.rs`).
   Note this requires enabling the non-default `f32_pipeline` feature.

With both pinned, vello_cpu is deterministic and §6.3 needs no relaxation.
**Do not** make the Tier-C baseline asymmetric as Q4(c) contemplated — it is
not needed.

### 2.7 Brief §5.5's ranked gap list, re-ranked

| # | Brief's gap | Status after verification |
|---|---|---|
| 1 | `full_cover` on both | **Confirmed gap**, and the emulation needs the correction in B2. Still #1. |
| 2 | tiny-skia has no layers | **Confirmed**, fully emulable, probed. Not a blocker. Correctly ranked. |
| 3 | vello_cpu API unverified | **Closed.** The API is *better* than inferred (native combined layers, per-fill blend modes, hard-edged clips, mid-render snapshot) — except B1. |
| 4 | vello_cpu determinism | **Refuted for threading, refined for SIMD.** Demoted; fixed by pinning (§2.6). |
| 5 | Dash arrays | **Confirmed exactly.** `StrokeDash::new` rejects `len < 2`, odd length, negatives, and non-positive total (`$R/tiny-skia-path-0.12.0/src/dash.rs:48-60`). Engine-side normalisation as the brief specifies. kurbo has no such rejection, so normalising engine-side is also what keeps the two backends identical. |
| 6 | Stroke geometry at joins | **Confirmed.** `kurbo::Join::Miter` falls through to the bevel branch when `2·hypot >= (hypot+dot)·miter_limit²` (`$R/kurbo-0.13.1/src/stroke.rs:461-486`); tiny-skia's `LineJoin::Miter` is documented "Extends to miter limit, then switches to bevel" (`$R/tiny-skia-path-0.12.0/src/stroker.rs:107-110`). Matches AGG's `miter_join_revert`. `MiterClip` exists in tiny-skia only — the brief's "do not use it" stands. |
| 7 | Hairlines | **Confirmed.** tiny-skia `width == 0` → hairline (`stroker.rs` doc, `:44-46`). Engine never passes 0 (D14). Dead code for us. |
| **NEW 8** | **`draw_image` alpha panics on vello** (B1) | New, blocking, engine-side workaround available |
| **NEW 9** | **tiny-skia silently drops thin fills** (B3) | New, engine-side guard + Tier-C assertion |
| **NEW 10** | **Asymmetric `/Extend` unrepresentable on both** (§2.4) | New, engine-side clip |
| **NEW 11** | **vello downgrades Medium→Low quality on integer translations** (§2.3) | New, Tier-C divergence, engine-side |
| **NEW 12** | **vello dimensions are `u16`** (§2.5) | New, API-shape consequence for SPEC §8 |
| **NEW 13** | **Both backends' luminance masks use BT.709, not `FXRGB2GRAY`** (§2.5) | New, engine must use the raw-bytes constructors |

---

## 3. Concrete SPEC §8 adjustments needed

**The orchestrator rules on these; SPEC.md is untouched by this document.**

The headline is good news: **E2, E3, E4 and E10 as freshly added to SPEC §8 are
all verified implementable on both backends, with no signature change.** The
adjustments below are refinements, not rewrites.

**1. `new_target`'s dimensions must be `u16`-safe.** (§2.5)
SPEC §8 has `fn new_target(&self, w: u32, h: u32, clear: Color) -> Self::Device`.
vello_cpu cannot exceed 65535 in either axis. Options, in order of preference:
- (a) **Recommended — no signature change**, document the invariant
  "`w, h <= 65535`" on `RasterBackend` and have `render_page` enforce it against
  PDFium's own render-size limits (brief §1.14). PDFium's page raster sizes are
  bounded well below this in practice.
- (b) Change to `-> Result<Self::Device, BackendError>`. Heavier; only if the
  corpus actually contains a >65535px target, which should be checked.

**2. `snapshot`'s signature: drop the `rect`, or document the cost.**
Brief §4's E3 sketch is `fn snapshot(&self, d: &Self::Device, rect: Rect) -> Pixmap`;
the SPEC §8 text landed on `fn snapshot(&self, d: &Self::Device) -> Pixmap`
(no rect). Both are implementable. **Recommend keeping the no-rect form** as
SPEC has it, and instead documenting that the engine renders each group into its
**own** target (§2.5) rather than snapshotting sub-rects of a shared one — on
vello a snapshot costs a full rasterization of the recorded scene, so a `rect`
parameter would be misleading about the cost. Add to SPEC §8's comment:
> `snapshot` may cost O(scene-so-far), not O(rect); the engine renders each
> transparency group into its own target rather than snapshotting sub-rects.

**3. `snapshot` requires all layers popped.** (`render.rs:800-803`)
Add to SPEC §8: `snapshot` is only valid when every `push_layer` has been
matched by a `pop`. This is a real precondition on vello (an assert), free on
tiny-skia. Document it; a debug assertion in both backends is cheap.

**4. E8's `AlphaMask` invariant must be asserted, not merely documented.**
Brief E8 concludes "**No spec change; a documented invariant**". Verified
correct in substance — but vello_cpu **silently ignores** a wrongly-sized mask
(`render.rs:479-485` maps it to `None`), and tiny-skia's `apply_mask` only
`log::warn!`s (`painter.rs:520`). A wrong size therefore produces *unmasked
output*, the worst possible failure mode. **Recommend** SPEC §8 state that
`AlphaMask` is device-sized-and-aligned and that **backends must assert it**,
not defer to the underlying crate.

**5. `ImageQuality` needs a third state, or a documented normalisation.** (§2.3)
vello downgrades `Medium`→`Low` for integer-only translations
(`encode.rs:500-509`); tiny-skia does not. Since SPEC §8 already passes
`quality: ImageQuality` per call and brief §1.16 says the *selection* is the
engine's job, **recommend** the engine detect the integer-translation case
itself (it already computes the device matrix) and pass `Low` explicitly in that
case, so both backends receive the same value and vello's internal downgrade
becomes a no-op. **No signature change**; a one-line note in SPEC §8's engine
decisions and a Tier-C fixture.

**6. No change needed for `draw_image`'s `alpha`.** (B1) Keep it. The vello
backend implements it with an opacity layer. Recommend a note in SPEC §8:
> `draw_image`'s `alpha` is a per-draw constant alpha; backends without native
> support implement it as an opacity layer around the draw.

**7. No change needed for E1, E5, E6, E7, E9.** All four resolutions in brief §4
are verified sound:
- **E1** (stroke-shaped clip → `kurbo::stroke`): `kurbo::stroke` /
  `stroke_with` confirmed present (`$R/kurbo-0.13.1/src/stroke.rs:260,281`) and
  is what vello_cpu itself uses, so the geometry is shared.
- **E5** (image mask → synthesized premultiplied RGBA): correct on both, since
  both are premultiplied.
- **E6/E7** (`full_cover`): still no expression; emulated per B2.
- **E9**: covered by `snapshot`, verified.

---

## 4. Recommendation summary: engine-side vs trait change vs backend to avoid

| Finding | Engine-side workaround | Trait change | Backend feature to avoid |
|---|---|---|---|
| B1 `draw_image` alpha panics (vello) | ✅ wrap in `push_opacity_layer`/`pop_layer` | none | ❌ never set `ImageSampler::alpha != 1.0` on vello |
| B2 `full_cover` | ✅ scratch pixmap, cells at **alpha 1.0**, AA off; single alpha-applying blit | none | — |
| B3 tiny-skia drops thin fills | ✅ port §1.7 steps 2–3 faithfully; Tier-C hard_fail on "one backend painted nothing" | none | ❌ do not special-case in the tiny-skia backend |
| Asymmetric `/Extend` | ✅ `Pad` + an engine-computed clip path | none | — |
| Gradient stop normalisation differs | ✅ normalise stops engine-side | none | ❌ do not rely on either backend's degenerate fallback |
| Luminance mask coefficients | ✅ build bytes with `FXRGB2GRAY`, use `Mask::from_parts` / `from_vec` | none | ❌ `Mask::new_luminance`, `MaskType::Luminance` |
| `ImageQuality` Medium→Low downgrade | ✅ engine passes `Low` for integer translations | none | — |
| SIMD-level ±1 | ✅ pin `Level` and `RenderMode` in the vello backend | none | ❌ `Level::try_detect()` in conformance builds |
| vello `u16` dimensions | ✅ enforce the bound in `render_page` | documented invariant (§3 item 1) | — |
| `snapshot` needs layers popped | ✅ engine structure already satisfies it | documented precondition (§3 item 3) | — |
| `AlphaMask` sizing | ✅ engine pads/crops (already planned) | **assert** in backends (§3 item 4) | ❌ relying on vello's silent `None` or tiny-skia's `log::warn!` |
| tiny-skia has no layers | ✅ §5.2's emulation, probed working | none | — |
| Dash array legality | ✅ §5.5's normalisation ladder | none | ❌ `StrokeDash::new` on un-normalised arrays |
| Multithreaded non-determinism | — (refuted) | none | — |
| Filters panic in MT mode | — | none | ❌ `push_filter_layer` / `set_filter_effect` entirely |

**Nothing here blocks the SPEC §8 trait shape.** The six `RenderDevice` methods
plus `push_clip_rect`, and the four `RasterBackend` methods, are all
implementable on both backends. Every gap resolves engine-side, which is the
outcome D14 wants: the backends stay dumb and agree by construction.

---

## 5. Corrections the render brief itself needs

These are edits to `docs/design/pdfrum-render.md`, not to SPEC.md:

1. **§5's method note** — delete the "network is unavailable / must be verified"
   caveat; point at this document.
2. **§5.1** — replace the two "**verify (Q4)**" verdicts with **CONFIRMED**;
   correct the "Multiply a target's alpha by a scalar" row (vello has
   `Pixmap::multiply_alpha`); refine the image-brush row with the
   Medium→Low downgrade.
3. **§5.3** — state explicitly that patch cells are drawn at **alpha 1.0** into
   the scratch and that the shading's alpha is applied once, at the blit. As
   currently worded the emulation is wrong for translucent shadings (B2).
4. **§5.5** — demote gap #4 (determinism), close gap #3, and add the six new
   items (B1, B3, `/Extend`, quality downgrade, `u16`, luminance coefficients).
5. **§6.2** — add a row: "sub-1/4096-px path extents are out of contract"
   (B3), and note that `soft_diff`'s ±1 budget now has a measured witness in
   vello's SIMD-level flattening difference (§2.6).
6. **§6.4** — add a Tier-C assertion that the two backends' Coons scratch
   pixmaps are byte-identical (B2), and a "one backend painted nothing"
   hard_fail (B3).
7. **§8 Q4** — mark **RESOLVED**, with (a) yes, (b) no trait change needed,
   (c) threading bit-exact / SIMD ±1, fixed by pinning.
8. **§1.13** — note that both backends' luminance-mask helpers use BT.709 and
   must not be used.
