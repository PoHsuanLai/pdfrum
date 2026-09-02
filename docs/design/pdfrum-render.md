# Design brief — `pdfrum-render` (+ `pdfrum-raster-vello-cpu`, `pdfrum-raster-tinyskia`)

**Contract:** SPEC.md §8. **Behavior source:** `core/fpdfapi/render/` (5 543 LOC)
and the device half of `core/fxge/` (`cfx_renderdevice.cpp`, `cfx_path.cpp`,
`dib/`, and `agg/` *as behavioural reference only* — we are not porting AGG).
**Predecessor:** `docs/design/pdfrum-page.md`, whose §1.16–1.19 end at a parsed
`Shading` / `Pattern` / `ImageData` / `SoftMask` record. This brief starts
there and ends at pixels.

**Scope boundary, stated once.** Everything the C++ does under
`BUILDFLAG(IS_WIN)` — `CPDF_ScaledRenderBuffer`, the printer device type,
`CFX_ImageTransformer`, `CGdi*Driver`, `CPSPrinterDriver`, `SetBitMask`'s
non-mask fallbacks, `StretchDIBits` — exists to serve GDI/PostScript printer
devices. Our only device is an in-memory RGBA8 bitmap, which is exactly the
configuration the oracle runs in (`pdfium_test` on Linux, `pdf_use_skia=false`,
so `CFX_AggDeviceDriver` with `DeviceType::kDisplay`). Those paths are
inventoried below where they *branch* (so a reader can confirm we take the
other arm) and then dropped. `IsPrint()` is a compile-time `false` for us;
`RenderCapGetBits()`, `RenderCapAlphaOutput()`, `RenderCapSoftClip()` and
`RenderCapBlendMode()` are all **true** for the AGG bitmap driver, and
`RenderCapShading()` is Skia-only, i.e. **false**. Every capability test in the
C++ therefore resolves to a constant for us, and this brief records which
constant and what it selects.

---

## 1. Behavior inventory

### 1.1 The render stack — five layers, and what each one owns

```
CPDF_ProgressiveRenderer      pausable driver over layers          (dropped, D1)
  CPDF_RenderContext          list of (PageObjectHolder, matrix) layers
    CPDF_RenderStatus         THE master walk: per-object dispatch, transparency,
                              patterns, type3, smask, compositing
      CPDF_ImageRenderer      image sub-state machine
      CPDF_RenderShading      7 shading rasterizers -> an ARGB bitmap
      CPDF_RenderTiling       tile cell -> repeated blit -> one ARGB bitmap
      CPDF_TextRenderer       charcode run -> device text calls
        CFX_RenderDevice      device-independent draw entry points + fast paths
          RenderDeviceDriverIface  (AGG / Skia / GDI)   -> our RasterBackend
```

`CPDF_RenderStatus` is a 1 000-line god class with 24 members
(cpdf_renderstatus.h:194-217). STYLE §1 forbids reproducing it. §3 decomposes
it into a small `RenderCtx` record plus free functions per object kind.

**Layer walk** (`CPDF_RenderContext::Render`, cpdf_rendercontext.cpp:65-92):
each layer gets a fresh `CPDF_RenderStatus` and a `StateRestorer` around it, so
a layer cannot leak clip state into the next. `final_matrix = layer.matrix *
last_matrix` when a last matrix is supplied. `pdfium_test` appends exactly one
page layer (`cpdfsdk_renderpage.cpp:68`) plus, under `FPDF_ANNOT`, one
`AppendLayer` per annotation appearance from `CPDF_AnnotList::DisplayAnnots`.
Layers stop early if `status.IsStopped()`.

**Object dispatch** (`ProcessObjectNoClip`, cpdf_renderstatus.cpp:307-330):

| `PageObject::Type` | handler | on `false` |
|---|---|---|
| `kText` | `ProcessText(obj, m, /*clipping_path=*/null)` | `DrawObjWithBackground` |
| `kPath` | `ProcessPath` | `DrawObjWithBackground` |
| `kImage` | `ProcessImage` | `DrawObjWithBackground` |
| `kShading` | `ProcessShading` | — (returns immediately, never retried) |
| `kForm` | `ProcessForm` | `DrawObjWithBackground` |

`DrawObjWithBackground` (:347-373) first checks `!device_->RenderCapGetBits()`;
for a bitmap device that is **false**, so it calls
`DrawObjWithBackgroundToDevice(obj, m, device_, CFX_Matrix())` — i.e. it
re-renders the object into the *same* device through a fresh `RenderStatus`
with an identity device matrix. The `CPDF_ScaledRenderBuffer` arm below it is
`#if BUILDFLAG(IS_WIN)` and `NOTREACHED()` elsewhere. **For us this fallback is
a no-op retry**: every `Process*` returning `false` on a bitmap device does so
for a reason that will recur. D2 drops it.

**Recursion guard** (`RenderSingleObject`, :242-245): a *process-global*
`g_CurrentRecursionDepth` with `kRenderMaxRecursionDepth = 64`, restored by
`AutoRestorer`. STYLE §1 bans globals; we thread a `depth: u32` field on the
render context. The guard counts **render** recursion (forms, type3 char procs,
patterns-through-forms, smask groups), which is *independent* of the page
layer's own parse-time form-recursion cap.

**Per-object culling.** Two spellings, deliberately different:
- `RenderObjectList` (:216, :227-231): computes
  `clip_rect = m.inverse().transform_rect(device.clip_box())` **once per list**,
  then skips an object when
  `obj.rect.left > clip.right || obj.rect.right < clip.left ||
   obj.rect.bottom > clip.top || obj.rect.top < clip.bottom`
  (strict `>`/`<`; note the C++ `FX_RECT`/`CFX_FloatRect` y-convention makes
  `bottom > top` the "above" test).
- `CPDF_ProgressiveRenderer::Continue` (cpdf_progressiverenderer.cpp:79-82)
  uses the **complement with `<=`/`>=`**, so an object exactly touching the
  clip edge is kept there and dropped here. Since we drop the progressive
  renderer (D1), only the `RenderObjectList` spelling survives; recorded so
  nobody "fixes" it later.
- `!pCurObj->IsActive()` skips deactivated objects (an edit-API concept; the
  page brief already models it).

**Stop object.** `stop_obj_` short-circuits the walk (`:219-222`) and is how
the C++ renders "the page background behind object X" for the printer-only
`GetBackdrop` path. Unused for us (D2).

### 1.2 Render options — the complete surface, and what `pdfium_test` sets

`CPDF_RenderOptions` (cpdf_renderoptions.h:19-84) is a colour mode + ten bools
+ a forced-colour scheme + an OC context.

```
enum Type : uint8_t { kNormal = 0, kGray, kAlpha, kForcedColor };
struct Options {
  bool bClearType          = false;   // ctor sets TRUE (cpdf_renderoptions.cpp:26)
  bool bNoNativeText       = false;
  bool bForceHalftone      = false;
  bool bRectAA             = false;
  bool bBreakForMasks      = false;
  bool bNoTextSmooth       = false;
  bool bNoPathSmooth       = false;
  bool bNoImageSmooth      = false;
  bool bLimitedImageCache  = false;
  bool bConvertFillToStroke= false;
};
struct ColorScheme { FX_ARGB path_fill_color, path_stroke_color,
                             text_fill_color, text_stroke_color; };
constexpr uint32_t kCacheSizeLimitBytes = 100 * 1024 * 1024;   // :11
```

**Flag mapping** (`cpdfsdk_renderpage.cpp:36-59`) — note that
`RenderPageImpl` *overwrites* `bClearType` from the flag, so the constructor's
`true` never survives a public render call:

| `FPDF_*` flag | effect |
|---|---|
| `FPDF_LCD_TEXT` | `bClearType = true` |
| `FPDF_NO_NATIVETEXT` | `bNoNativeText = true` |
| `FPDF_RENDER_LIMITEDIMAGECACHE` | `bLimitedImageCache = true` |
| `FPDF_RENDER_FORCEHALFTONE` | `bForceHalftone = true` |
| `FPDF_RENDER_NO_SMOOTHTEXT` | `bNoTextSmooth = true` |
| `FPDF_RENDER_NO_SMOOTHIMAGE` | `bNoImageSmooth = true` |
| `FPDF_RENDER_NO_SMOOTHPATH` | `bNoPathSmooth = true` |
| `FPDF_GRAYSCALE` | `SetColorMode(kGray)` |
| a non-null `FPDF_COLORSCHEME` | `SetColorMode(kForcedColor)` + scheme; only then is `FPDF_CONVERT_FILL_TO_STROKE` read |
| `FPDF_PRINTING` | selects `CPDF_OCContext::kPrint` instead of `kView` |
| `FPDF_ANNOT` | appends annotation layers |
| `FPDF_REVERSE_BYTE_ORDER` | device-level only: `CreateForBitmap(bitmap, rgb_byte_order=true)`. It swaps R/B *in the driver's compositor*, never in engine math. For us it is an output-encoding option on `Pixmap`, not a render option (D3). |

**The oracle's baseline is `flags = FPDF_ANNOT` alone**
(`testing/pdfium_test/pdfium_test.cc:221`). So for every golden PNG:
`bClearType = false`, `bNoTextSmooth = false`, colour mode `kNormal`, OC usage
`kView`, and the target bitmap is
- `FPDFBitmap_BGRA` cleared to `0x00000000` when `FPDFPage_HasTransparency(page)`,
- `FPDFBitmap_BGR` cleared to `0xFFFFFFFF` otherwise
(pdfium_test.cc:1070-1088). This is a **load-bearing conformance detail**: our
`render_page` must reproduce the same choice, because a transparent-backed page
composites differently at the edges than a white-backed one.

**Colour translation** (cpdf_renderoptions.cpp:33-76):
```
TranslateColor(argb):
    kNormal | kAlpha -> argb unchanged
    otherwise        -> gray = FXRGB2GRAY(r,g,b); ARGB(a, gray, gray, gray)
TranslateObjectFillColor(argb, type):
    !kForcedColor -> TranslateColor(argb)
    kPath -> scheme.path_fill_color ; kText -> scheme.text_fill_color ; else argb
TranslateObjectStrokeColor(argb, type):  same with the *_stroke_color fields
```
`FXRGB2GRAY(r,g,b) = (b*11 + g*59 + r*30) / 100` (fx_dib.h:210) — integer,
truncating, **NTSC weights on a 0..100 scale**, not Rec.709 and not
floating-point. `kForcedColor` replaces only path and text colours; images,
shadings and forms keep their own colour and are then *not* gray-translated
either (the `default:` arm returns `argb`).

Three colour modes exist beyond `kNormal`, and all three are reachable
internally regardless of the public flags:
- `kGray` — public `FPDF_GRAYSCALE`.
- `kAlpha` — set by `LoadSMask` for an **alpha**-type soft mask
  (cpdf_renderstatus.cpp:1480-1481) and by `DrawPatternBitmap` for an
  **uncoloured** tiling pattern (cpdf_rendertiling.cpp:61-63). In this mode
  `CPDF_ImageRenderer` diverts to `StartBitmapAlpha` (cpdf_imagerenderer.cpp:94-96)
  and `CPDF_RenderShading` ends with `pBitmap->SetRedFromAlpha()` (:1111-1112).
- `kForcedColor` — public colour scheme.

`bForceHalftone` is set unconditionally by the type-3 char-proc path
(cpdf_renderstatus.cpp:997) and by `DrawPatternBitmap` (cpdf_rendertiling.cpp:66);
`bRectAA` likewise by type-3 (:998). Both therefore reach us even with the
oracle's bare `FPDF_ANNOT`.

### 1.3 Fill and stroke colour resolution — the ARGB pipeline

`GetFillArgb` / `GetStrokeArgb` (cpdf_renderstatus.cpp:466-532) are where a
`ColorValue` becomes a 32-bit ARGB. Transcribed:

```
GetFillArgb(obj):
    if type3_char != null && (!type3_char.colored || fill_color_is_missing(obj)):
        return t3_fill_color                       // the caller-imposed colour
    return GetFillArgbForType3(obj)

GetFillArgbForType3(obj):
    cs = obj.color_state
    if !cs.has_ref() || cs.fill_color.is_null():   // "MissingFillColor"
        cs = initial_states.color_state            // inherited from the parent status
    colorref = cs.fill_color_ref()                 // FX_COLORREF, i.e. 0x00BBGGRR
    if colorref == 0xFFFFFFFF: return 0            // SENTINEL: fully transparent
    alpha = (int32) (obj.general_state.fill_alpha * 255)      // TRUNCATION, not round
    if obj.general_state.TR:
        tf = cached_transfer_func(TR)              // memoized on the object
        if tf: colorref = tf.translate_color(colorref)
    return options.TranslateObjectFillColor(ArgbEncode(alpha, colorref), obj.type)
```

Stroke is the same with `stroke_color`/`stroke_alpha`, and the comment
`// not rounded.` at :518 is upstream's own acknowledgement of the truncation.
Four details that are pixel-visible:

1. **`colorref == 0xFFFFFFFF` returns ARGB `0`, not white.** The page brief
   (§1.14.11) documents `0xFFFFFFFF` as the "no colour resolvable" sentinel for
   non-colored-tiling cases. Here it collapses to alpha 0 — the object is
   *invisible*, not white. Uncoloured tiling patterns use `0x00BFBFBF` instead
   precisely so they do not hit this.
2. **Alpha is truncated** from `alpha_float * 255`. `ca 0.5` → `127`, not 128.
3. **The transfer function applies to the colour, not the alpha**, and is
   applied *before* `TranslateObjectFillColor`, so `kGray` mode grays the
   already-transferred colour.
4. **Missing colour inherits from `initial_states_`**, which `Initialize`
   (:186-211) seeds from the parent status when a fill/stroke colour is absent
   — this is how a form XObject with no colour operators picks up the caller's
   colour.

**Transfer functions** (`CPDF_DocRenderData::CreateTransferFunc`,
cpdf_docrenderdata.cpp:80-150). `kMaxOutputs = 16`,
`CPDF_TransferFunc::kChannelSampleSize = 256`.

```
if obj is Array:
    if len < 3: return null                      // fewer than 3 -> whole TR dropped
    for i in 0..3: funcs[2 - i] = Function::load(array[i])   // NOTE the reversal
                   if !funcs[2-i]: return null
else:
    funcs[0] = Function::load(obj); if !funcs[0]: return null

output: [f32; 16] initialised to 0.0            // ONCE, outside the loops
for v in 0..256:
    input = v as f32 / 255.0
    array case:  for i in 0..3:
                     if funcs[i].output_count() > 16 { samples[i][v] = v; continue }
                     funcs[i].call(&[input], &mut output)
                     o = roundf(output[0] * 255)
                     if o != v { identity = false }
                     samples[i][v] = o           // truncating u8 store
    single case: if funcs[0].output_count() <= 16 { funcs[0].call(&[input], &mut output) }
                 o = roundf(output[0] * 255); if o != v { identity = false }
                 samples[0..3][v] = o
TransferFunc { identity, samples_r = samples[0], samples_g = samples[1], samples_b = samples[2] }

TranslateColor(colorref) = FXSYS_BGR(samples_b[B(colorref)],
                                     samples_g[G(colorref)],
                                     samples_r[R(colorref)])
    where FX_COLORREF packs (b << 16) | (g << 8) | r        (fx_dib.h:146-160)
```

Quirks to port verbatim:
- **`output` is not re-zeroed per iteration**, so a function whose
  `output_count() > 16` in the *single*-function case leaves the previous
  iteration's `output[0]` in place and the sample repeats the last good value
  (the array case explicitly writes `samples[i][v] = v` instead — identity).
- **The `funcs[2 - i]` reversal** in the array case. Whether this is a net
  no-op depends on how `samples[i]` maps back to R/G/B. The unittest
  `CPDFDocRenderDataTest.TransferFunctionArray`
  (cpdf_docrenderdata_unittest.cpp:197-225) is the authority: with the array
  `[Type0, Type2, Type4]` it asserts `GetSamplesR() == Type0`,
  `GetSamplesG() == Type2`, `GetSamplesB() == Type4`, i.e. **array order maps
  directly to R,G,B with no observable reversal**. Port the C++ literally and
  let that unittest pin it. **Q1** flags the discrepancy between the literal
  code reading and the asserted result for a resolver to settle before coding.
- **Any function in the array failing to load kills the whole TR**
  (`BadTransferFunctions`, :226-259, all three cases assert `EXPECT_FALSE`).
- The cache is per-`CPDF_Document`, keyed by the *object identity* of the TR
  object (`transfer_func_map_`, cpdf_docrenderdata.cpp:58-69).

### 1.4 Clip paths — an 8-bit coverage mask, not a path stack

`ProcessClipPath` (cpdf_renderstatus.cpp:534-596) is called *before* every
object:

```
if !clip_path.has_ref():
    if last_clip_path.has_ref(): device.restore_state(keep_saved = true)
                                 last_clip_path = null
    return
if clip_path == last_clip_path: return          // pointer-ish identity, cheap
last_clip_path = clip_path
device.restore_state(keep_saved = true)          // pop back to the layer baseline
for i in 0..clip_path.path_count():
    path = clip_path.path(i)
    if path is null: continue
    if path.points().is_empty():
        empty = Path::rect(-1, -1, 0, 0)         // a deliberately off-canvas rect
        device.set_clip_path_fill(empty, /*matrix=*/null, Winding)
    else:
        device.set_clip_path_fill(path, &obj2device, FillOptions(clip_path.clip_type(i)))
if clip_path.text_count() == 0: return
// text clip: RenderCapSoftClip() is TRUE for a bitmap device, so we do NOT bail
text_clipping_path = None
for i in 0..clip_path.text_count():
    text = clip_path.text(i)
    if text != null:
        if text_clipping_path.is_none(): text_clipping_path = Some(Path::new())
        ProcessText(text, obj2device, &mut text_clipping_path)   // accumulate outlines
        continue
    // a NULL entry is the flush sentinel
    if text_clipping_path.is_none(): continue
    opts = FillOptions(Winding); if options.bNoTextSmooth { opts.aliased_path = true }
    device.set_clip_path_fill(text_clipping_path, /*matrix=*/null, opts)
    text_clipping_path = None
```

Three contracts:
- **The empty-path clip is `rect(-1,-1,0,0)`**, a 1×1 rect entirely off the top
  left of the device — i.e. "clip everything out". Not an empty region, a
  degenerate rect. Reproduce literally: it interacts with the outer-rect
  rounding in §1.7.
- **Text clip accumulates glyph outlines across `ProcessText` calls and is
  flushed by a `null` entry in the text list**, with the accumulated path
  already in *device* space (matrix `null` at the `set_clip_path_fill` call —
  `ProcessText` transformed the outlines when it appended them).
- **`restore_state(keep_saved = true)` pops one clip level and immediately
  re-saves**, so the device-level clip stack never grows past depth 1 inside a
  layer. Our `RenderDevice::pop()` must have the same semantics: the engine
  maintains its own logical clip stack and pushes exactly one composite clip
  per object.

**What the clip actually is** (`CFX_AggClipRgn`, cfx_agg_cliprgn.h:15-33): a
`FX_RECT box_` plus an optional `k8bppMask` bitmap sized to `box_`. Two
operations:
- `IntersectRect` — rect ∩ rect, mask cropped.
- `IntersectMask(left, top, mask)` — `new[c] = old[c] * mask[c] / 255`
  (cfx_agg_cliprgn.cpp:62-63), **integer truncating**, box intersected.

`SetClip_PathFill` (cfx_agg_devicedriver.cpp:1112-1141) takes a **rect fast
path**: if `path.GetRect(matrix)` yields an axis-aligned rect, it is clamped to
the device and applied via `IntersectRect` on the **outer** integer rect — so an
axis-aligned clip is *hard-edged and snapped outward*, never antialiased.
Otherwise `SetClipMask` rasterizes the path into an 8bpp mask at
`fill_options_.aliased_path` and multiplies it in.

`SetClip_PathStroke` (:1143-1159) builds the mask from a stroke with
`scale = 1.0` and `filling_rule = non_zero`, and — a latent bug worth knowing —
**never assigns `fill_options_`**, so it inherits `aliased_path` from whatever
draw ran last.

This maps 1:1 onto tiny-skia's `Mask` + `Mask::intersect_path` (which is
literally `premultiply_u8(a, b)` = the same `a*b/255`), and onto vello_cpu's
clip layers. See §5.

### 1.5 Transparency — when an offscreen buffer is forced

`ProcessTransparency` (cpdf_renderstatus.cpp:628-753) is the single most
important function in the module. It returns `true` when it handled the object
(via an offscreen composite) and `false` when the caller should draw directly.

**Step 1 — gather.**
```
blend_type   = obj.general_state.blend_type
smask_dict   = obj.general_state.soft_mask               (mutable ref)
if smask_dict && obj is Image && obj.image.dict.has_key("SMask"):
        smask_dict = null            // an image's own /SMask WINS over the gs /SMask
group_alpha   = 1.0 ; initial_alpha = 1.0
transparency  = self.transparency                        // inherited from the layer
group_transparent = false
if obj is Form:
    group_alpha       = obj.general_state.fill_alpha     // the form's ca, as a GROUP alpha
    transparency      = obj.form.transparency            // the form's own /Group
    group_transparent = transparency.is_isolated()
    form_resource     = obj.form.dict["/Resources"]
    initial_alpha     = initial_states.general_state.fill_alpha
text_clip = !IsPrint() && obj.clip_path.has_ref() && obj.clip_path.text_count() > 0
                       && !device.RenderCapSoftClip()
        // RenderCapSoftClip() is TRUE for a bitmap device -> text_clip is ALWAYS FALSE
```

**Step 2 — the bail-out predicate** (:655-657), the exact condition under which
no offscreen buffer is created:
```
!smask_dict && group_alpha == 1.0 && blend_type == Normal
            && !text_clip && !group_transparent && initial_alpha == 1.0
```
Note what is **not** in it: a non-isolated group with `/Group` present but
`/I` absent and `ca == 1` and no smask draws **directly**, with no group
semantics whatsoever. And `bGroupTransparent` is `transparency.IsIsolated()`
of the *form's own* group, not the enclosing one.

**Step 3 — the offscreen buffer.**
```
rect = obj.transformed_bbox(obj2device) ∩ device.clip_box()
if rect.is_empty(): return true                     // handled: nothing to draw
w = rect.width(); h = rect.height()
backdrop = None
if !transparency.is_isolated() && device.RenderCapGetBits():     // TRUE for us
    backdrop = device.compatible_bitmap(w, h)
    device.get_dibits(backdrop, rect.left, rect.top)             // COPY THE BACKDROP IN
bitmap_device = RenderDevice::for_new_bitmap_with_backdrop(w, h, kBgra, backdrop)
new_matrix = obj2device translated by (-rect.left, -rect.top)
```
So: **isolated ⇒ the group starts on a transparent buffer; non-isolated ⇒ the
group starts on a copy of the existing page content.** That copy is passed as
the driver's `backdrop_bitmap_`, which is what turns on the AGG
`CompositeSpan` (backdrop) blend path rather than `CompositeSpanARGB` — i.e.
the non-isolated group's own drawing blends against the page content *inside*
the group buffer. This is the whole of PDFium's isolated/non-isolated handling;
there is no separate "remove backdrop after compositing" step, and therefore
**PDFium's non-isolated groups double-count the backdrop** when the group is
later composited back with a non-Normal blend. We reproduce it (D5).

`GetCompatibleArgbFormat()` (:1588-1595) returns `kBgra` for us
(`kBgraPremul` is Skia-only). `CreateCompatibleBitmap` returns
`CFX_DIBBase::kPlatformRGBFormat` (24bpp BGR on Linux) when
`RenderCapAlphaOutput()` is false, `kBgra` when true; for a `kBgra` page bitmap
it is `kBgra`, for a `kBgr` page bitmap it is `kBgr`
(cfx_renderdevice.cpp:502-517). **So the backdrop copy is 24bpp for an opaque
page and 32bpp for a transparent one** — an observable difference in how a
non-isolated group's own alpha behaves.

**Step 4 — render into it** (:718-727): a fresh status with
`SetStdCS(true)`, the form's resources, `SetInGroup(transparency.is_group())`,
`Initialize(null, null)` (so it does **not** inherit the parent's colour), and
`ProcessObjectNoClip(obj, new_matrix)` — note **`NoClip`**: the clip was
already applied to the outer device before the buffer was sized, so the buffer
is implicitly clipped by its own extent.

**Step 5 — masks and alphas, in this exact order** (:728-745):
```
if smask_dict:
    smask_matrix = obj.general_state.smask_matrix * obj2device
    m = LoadSMask(smask_dict, rect, smask_matrix)
    if m: bitmap_device.multiply_alpha_mask(m)
if text_mask_bitmap: bitmap_device.multiply_alpha_mask(text_mask)   // never, for us
if transparency.is_group(): bitmap_device.multiply_alpha(group_alpha)
if initial_alpha != 1.0 && !in_group: bitmap_device.multiply_alpha(initial_alpha)
```
The `!in_group` guard prevents a nested group from re-applying the enclosing
group's alpha. `initial_alpha` is only non-1 when a *form* object's own
`Initialize` inherited a fill alpha — in practice the type-3 and pattern paths.

**Step 6 — composite back** (:746-752):
```
transparency = self.transparency          // NOTE: reset to the ENCLOSING transparency
if obj is Form: transparency.set_group()  // and then forced to "group"
CompositeDIBitmap(buffer, rect.left, rect.top, mask_argb = 0,
                  alpha = 1.0, blend_type, transparency)
```
The `transparency` handed to the compositor is the **enclosing** one with
`group` forced on for forms — *not* the form's own group flags. The isolated
bit therefore comes from the page/parent, which (because
`CPDF_Page` sets `SetIsolated()` unconditionally, page brief §1.17) is almost
always **true** at the top level.

### 1.6 `CompositeDIBitmap` — the compositor decision tree

cpdf_renderstatus.cpp:1306-1432, with the Windows-only arms elided (they are
`NOTREACHED()` for us). Transcribed as a decision tree, with each device
capability replaced by its constant value for a bitmap device:

```
CompositeDIBitmap(bitmap, left, top, mask_argb, alpha, blend_mode, transparency):

  1.  if blend_mode == Normal:
          if bitmap.is_mask_format():        NOTREACHED for us (Windows-only arm)
          else:
              if alpha != 1.0: bitmap.multiply_alpha(alpha)
              if device.set_dibits(bitmap, left, top): return        // DONE
      // falls through only if SetDIBits failed, which it does not for AGG

  2.  isolated              = transparency.is_isolated()
      back_alpha_required   = blend_mode != Normal && isolated && !drop_objects
      get_background        = RenderCapAlphaOutput()                     // = TRUE
                              || (!RenderCapAlphaOutput() && RenderCapGetBits()
                                  && !back_alpha_required)
      // For a kBgra page bitmap RenderCapAlphaOutput() is TRUE  -> get_background = TRUE
      // For a kBgr  page bitmap it is FALSE and RenderCapGetBits() is TRUE
      //     -> get_background = !back_alpha_required

  3.  if get_background:
          if isolated || !transparency.is_group():
              if !bitmap.is_mask_format():
                  device.set_dibits_with_blend(bitmap, left, top, blend_mode)
              return                                                  // DONE
          // non-isolated GROUP with a non-Normal blend:
          rect = FX_RECT(left, top, left+w, top+h) ∩ device.clip_box()
          if device.backdrop() && device.bitmap():
              clone = device.backdrop().clip_to(rect)
              clone.composite_bitmap(0,0,w',h', device.bitmap(), rect.left, rect.top, Normal)
              left = min(left, 0); top = min(top, 0)          // (sic — clamps to 0)
              clone.composite_bitmap(0,0,w',h', bitmap, left, top, blend_mode)
          else:
              clone = bitmap
          if device.backdrop(): device.set_dibits(clone, rect.left, rect.top)
          else if !bitmap.is_mask_format():
                   device.set_dibits_with_blend(bitmap, rect.left, rect.top, blend_mode)
          return

  4.  // get_background == FALSE: only reachable on an OPAQUE page bitmap with a
      // non-Normal blend on an isolated group.
      bbox     = FX_RECT(left, top, left+w, top+h) ∩ device.clip_box()
      backdrop = GetBackdrop(cur_obj, bbox, blend_mode != Normal && isolated)
      if !backdrop: return
      backdrop.composite_bitmap(left - bbox.left, top - bbox.top, w, h,
                                bitmap, 0, 0, blend_mode)
      new_backdrop = Bitmap(bbox.w, bbox.h, kBgrx)
      new_backdrop.clear(0xffffffff)                       // WHITE
      new_backdrop.composite_bitmap(0,0,W,H, backdrop, 0, 0, Normal)
      device.set_dibits(new_backdrop, bbox.left, bbox.top)
```

`GetBackdrop` (:761-801): allocates `kBgra` when `back_alpha_required &&
!drop_objects`, else a device-compatible bitmap; then
`should_fetch_bits = backdrop.is_alpha_format() ? RenderCapAlphaOutput()
                                                : RenderCapGetBits()`.
For an opaque page bitmap and `back_alpha_required = true` the backdrop is
`kBgra` and `RenderCapAlphaOutput()` is `false`, so it falls to the **re-render**
branch: clear to white and re-run `context.Render(device, stop_obj = cur_obj,
options, final_matrix)` — i.e. **it replays every page object drawn before this
one into a fresh buffer** so the blend has a real backdrop. That is the only
use of `stop_obj_`, and it is what makes non-Normal blend modes on an opaque
page correct in the C++.

**We must reproduce step 4's semantics, but not its mechanism.** Our target is
always a premultiplied RGBA8 pixmap that *has* an alpha channel and *is*
readable, so `RenderCapAlphaOutput()` and `RenderCapGetBits()` are both true
and step 3's first arm — a plain blended blit — always fires. See D6: the
observable difference is confined to opaque-page + non-Normal-blend +
white-clear, which step 4 produces by *compositing over white*. Our engine
reproduces that by keeping the page's opaque white background as real pixels in
the pixmap, which is what `pdfium_test` asks for anyway
(`FPDFBitmap_FillRect(..., 0xFFFFFFFF)`), so the arithmetic coincides.
Conformance clusters `transparent*`, `smask_blend`, `alpha_composite`,
`composite-and-xnor`, `composite-or-xor-replace` are the proof obligation.

### 1.7 Path rendering — the device fast paths that change pixels

`ProcessPath` (cpdf_renderstatus.cpp:424-457):
```
fill_type = path_obj.fill_type ; stroke = path_obj.stroke
ProcessPathPattern(&mut fill_type, &mut stroke)      // §1.9: drains pattern colours
if fill_type == NoFill && !stroke: return true
if color_mode == kForcedColor && options.bConvertFillToStroke && fill_type != NoFill:
    stroke = true; fill_type = NoFill
fill_argb   = if fill_type != NoFill { GetFillArgb(obj) }  else { 0 }
stroke_argb = if stroke              { GetStrokeArgb(obj) } else { 0 }
path_matrix = path_obj.matrix * obj2device
if !IsAvailableMatrix(path_matrix): return true      // silently drop
return device.draw_path(path, &path_matrix, graph_state, fill_argb, stroke_argb,
                        fill_options)
```

`IsAvailableMatrix` (:148-158) — a degeneracy filter that is **not** a
determinant test:
```
if a == 0 || d == 0 { return b != 0 && c != 0 }
if b == 0 || c == 0 { return a != 0 && d != 0 }
return true
```
i.e. a matrix is rejected only when both diagonals contain a zero in a way that
collapses it. A genuinely singular matrix like `(1,1,1,1)` passes.

`GetFillOptionsForDrawPathWithBlend` (:85-110):
```
opts = FillOptions(fill_type)
if fill_type != NoFill && options.bRectAA        { opts.rect_aa      = true }
if options.bNoPathSmooth                          { opts.aliased_path = true }
if obj.general_state.stroke_adjust                { opts.adjust_stroke= true }
if is_stroke                                      { opts.stroke       = true }
if is_type3_char                                  { opts.text_mode    = true }
```

**`CFX_RenderDevice::DrawPath` (cfx_renderdevice.cpp:688-830) is where the
pixel-visible fast paths live.** In order:

1. **Cosmetic line** (:699-708). `stroke_alpha == 0 && points.len() == 2` →
   transform both points and call `DrawCosmeticLine(p1, p2, fill_color, opts)`.
   Note this fires on a *fill* with two points, and uses the **fill** colour as
   a stroke. `DrawCosmeticLine` (:927-942): if `color >= 0xff000000` (fully
   opaque) try the driver's own cosmetic-line routine, else build a 2-point
   path with a **default `CFX_GraphStateData`** (width 1.0) and stroke it. Since
   the AGG driver's `DrawCosmeticLine` is a real implementation, an opaque
   2-point "fill" becomes a 1-pixel line.

2. **Axis-aligned rect snapping** (:710-771), taken when
   `stroke_alpha == 0 && !fill_options.rect_aa` and `path.GetRect(matrix)`
   succeeds. Transcribed exactly:
   ```
   rect_i = rect_f.outer_rect()
   if !rect_i.valid(): return false
   width  = ceil(rect_f.right - rect_f.left) as int
   if width < 1 { width = 1; if rect_i.left == rect_i.right { rect_i.right += 1 } }
   height = ceil(rect_f.top - rect_f.bottom) as int
   if height < 1 { height = 1; if rect_i.bottom == rect_i.top { rect_i.bottom += 1 } }
   if rect_i.width() >= width + 1:
       if rect_f.left - rect_i.left as f32 > rect_i.right as f32 - rect_f.right
            { rect_i.left  += 1 }
       else { rect_i.right -= 1 }
   if rect_i.height() >= height + 1:
       if rect_f.top - rect_i.top as f32 > rect_i.bottom as f32 - rect_f.bottom
            { rect_i.top    += 1 }
       else { rect_i.bottom -= 1 }
   if FillRect(rect_i, fill_color): return true
   ```
   Every `+= 1` / `-= 1` is a checked add that returns `false` on overflow.
   **This is a hard-edged, integer-snapped fill with a "shrink the wider side"
   rule** — an axis-aligned rectangle in PDFium is *never* antialiased unless
   `rect_aa` is set. It is responsible for a large share of the corpus's
   pixel-exact rectangles and must be ported precisely, including which side
   gets trimmed on a tie (the `>` is strict, so a tie trims the **right/bottom**).
   `path.GetRect` itself (cfx_path.cpp:429-466) requires 4 or 5 points, all
   `kLine` after index 0, no coincident diagonal corners, and axis-alignment
   tested with **exact float equality** both pre- and post-transform (so 90°
   rotations qualify, 45° do not); paths of >5 points go through
   `GetNormalizedPoints`, which collapses zero-length segments and bails if
   more than 5 survive.

3. **Zero-area sub-path → thin line** (:773-804), taken when
   `fill && stroke_alpha == 0 && !opts.stroke && !opts.text_mode`. Each
   `kMove`-delimited sub-path is fed to `DrawZeroAreaPath`.
   `adjust = !!device_driver_->GetDriverType()`, and the AGG driver returns
   `1`, so **`adjust` is always true for us**.
   `GetZeroAreaPath` (:430-500) tries, in order:
   - `CheckSimpleLinePath` (:338-379): a 2-point `Move,Line` or a 3-point
     `Move,Line,Line` with `p[0] == p[2]`. If `p[0] == p[1]` it returns `true`
     with an **empty** new path (nothing drawn). Otherwise both points are
     transformed (when `adjust && matrix`) and snapped to
     `(int)coord + 0.5` — **truncation toward zero, then pixel centre** — and
     `set_identity = true` so the caller passes a null matrix downstream.
     `thin = true`.
   - `CheckPalindromicPath` (:383-409): odd point count > 3, every pair
     mirrored about the midpoint, no beziers. Emits `mid-i → left` segments.
     `thin = true`.
   - the folding-vertex scan (:451-499), which for each interior `kLine`
     vertex whose neighbours make the path double back emits the **shorter**
     of the two segments. Predicates, all exact-float:
     `IsFoldingVerticalLine(a,b,c) = a.x==b.x && b.x==c.x && (b.y-a.y)*(b.y-c.y) > 0`;
     horizontal is the y/x mirror; diagonal is
     `a.x!=b.x && c.x!=b.x && a.y!=b.y && c.y!=b.y &&
      (a.y-b.y)*(c.x-b.x) == (c.y-b.y)*(a.x-b.x)`.
     Vertical picks `use_prev` by comparing **y** distances, horizontal and
     diagonal by **x** distances. `thin = true` only if
     `points.len() > 3 && new_path_size > 0`.
   `DrawZeroAreaPath` (:944-982) then draws with `line_width = 0.0`,
   `fill_color = 0`, `stroke_color = fill_color` but — when `thin` —
   **`alpha >> 2`**:
   `stroke_color = ((fill_alpha >> 2) << 24) | (fill_color & 0x00ffffff)`,
   and `FillOptions { zero_area: true, aliased_path }`. The `zero_area` flag
   makes the driver skip the matrix decomposition and stroke at
   `scale = 1, matrix = null`, guaranteeing a 1-device-pixel hairline.
   Corpus witness: `single_point_paths`.

4. **Fill + translucent stroke** (:806-827): `fill && fill_alpha &&
   stroke_alpha < 0xff && opts.stroke` → `DrawFillStrokePath`, which renders
   fill and stroke into a **separate bitmap with `group_knockout = true`** and
   blits the result. That is the whole of "knockout" in PDFium: an fxge device
   flag that makes the driver blend against a saved backdrop copy rather than
   the live destination, so the stroke does not accumulate over the fill where
   they overlap. It is **not** PDF `/K` group knockout (which core/ never
   parses — page brief §1.17, SPEC §8). Corpus witness:
   `same_color_knockout_fill` (a `b` operator with `CA 0.5 ca 0.5`),
   which despite its name exercises this path, not `/K`.
   The bounding box is
   `path.GetBoundingBoxForStrokePath(line_width, miter_limit)` transformed by
   the matrix, then `outer_rect()`; the sub-device's backdrop is a **copy of
   the fill target**, and the result is blitted back with `SetDIBits`.

5. Otherwise → the driver's `DrawPath`.

**Stroke geometry contract** (`RasterizeStroke`,
cfx_agg_devicedriver.cpp:299-393, and the decomposition at :1229-1246). This is
behaviour we must reproduce with *any* rasterizer:
```
// matrix split, non-zero_area case:
matrix1.a = matrix1.d = max(|m.a|, |m.b|)
matrix2   = (m.a/matrix1.a, m.b/matrix1.a, m.c/matrix1.d, m.d/matrix1.d, 0, 0)
matrix1   = m * matrix2.inverse()
path pre-transformed by matrix1 ; matrix2 applied per-vertex ; scale = matrix1.a

// inside RasterizeStroke:
width = graph_state.line_width * scale
unit  = if matrix2 { 1.0 / ((matrix2.x_unit() + matrix2.y_unit()) / 2) } else { 1.0 }
width = max(width, unit)
```
with
`x_unit() = if b==0 {|a|} else if a==0 {|b|} else hypot(a,b)` and the c/d mirror
for `y_unit()` (fx_coordinates.cpp:443-461). **The minimum stroke width is one
device pixel**, expressed in pre-transform units. `line_width == 0` therefore
produces exactly a 1-pixel hairline, and there is no other threshold. When
`zero_area` is set the split is skipped entirely: the path is pre-transformed
by the full matrix, `matrix = null`, `scale = 1`, so `unit = 1.0` and the
hairline is exactly 1 device pixel.
`matrix1.a = max(|a|,|b|)` divides by zero when `a == b == 0`; no guard.

**Caps, joins, miter** (:305-328). `kButt→butt`, `kRound→round`,
`kSquare→square`; `kMiter→miter_join_revert`, `kRound→round`, `kBevel→bevel`.
**`miter_join_revert` falls back to a plain bevel when the limit is exceeded**
(agg_math_stroke.h:92-146) rather than truncating the miter — this is the
PDF-correct behaviour and differs from a naive "clamp the miter" rasterizer.
Miter limit is passed through **unclamped**; defaults are cap `kButt`, join
`kMiter`, miter limit `10.0`, width `1.0` (cfx_graphstatedata.h:49-55).

**Dashes** (:336-386, and third_party/agg23/agg_vcgen_dash.cpp). The page brief
(§1.4, D6) already moves the *normalization ladder* to `pdfrum-page`; what
belongs here is the device-scale-dependent half:
```
if dash_array is empty                       -> solid
if any entry is non-finite                   -> solid
cycle = Σ max(0.0, entry)                    // negatives clamped for the TEST only
if cycle * scale < 0.1                       -> solid          // kMinDashCycleThreshold
otherwise, per entry:
    if entry <= 0.000001 { entry = 0.1 }     // BEFORE scaling; catches 0 and negatives
    add_dash(|entry * scale|)                // the fabs is now dead
dash_start(dash_phase * scale)
```
plus AGG's own rules: **`max_dashes = 32`, silently truncating**; a negative
phase is incremented by `ceil(-ds / (2·Σ)) · 2·Σ`; and an **odd-length array
doubles the cycle** — `[5 2 1]` is `5 on, 2 off, 1 on, 5 off, 2 on, 1 off`
(agg_vcgen_dash.cpp:74-97). Dashes are measured in `matrix1` space, so an
anisotropic `matrix2` makes device-space dash lengths non-uniform. Corpus
witnesses: `dashed_lines`, `long_dashed_line`, `dashed_line_negative_scale`,
`7_dashed`, `annotation_*_dash`.

**Fill rule** (:395-400): `kWinding → non-zero`, **everything else including
`kNoFill` → even-odd**.

**Anti-aliasing** is always computed and then optionally binarized:
`aliased_path` thresholds coverage at `> 127 → 255, else 0`
(agg_rasterizer_scanline_aa.h:283-297). There is **no gamma table** on the path
side. Coordinates are clamped per-axis to **±32000** before rasterization
(`HardClip`, cfx_agg_devicedriver.cpp:53-58) — a clamp, not a clip, so geometry
beyond that is distorted. Subpixel resolution is 1/256 with **truncation**
(`poly_coord`), and beziers are flattened at a fixed tolerance
(`distance_tolerance_square = 0.25`, manhattan `4.0`, recursion limit 16,
agg_curves.cpp:29-39) that does **not** scale with zoom.

**A degenerate-dot rule inside the driver** (:950-957): an isolated 2-point
sub-path whose two points are identical, bracketed by open `kMove`s, has its
endpoint shifted `+1` in device x so it renders as a visible pixel.

### 1.8 Text rendering

`ProcessText` (cpdf_renderstatus.cpp:824-930):
```
if char_codes.is_empty(): return true
mode = text_state.text_mode
if mode == Invisible: return true                     // Tr 3
if font.is_type3(): return ProcessType3Text(...)      // §1.11
is_fill = is_stroke = is_clip = false
if clipping_path != null: is_clip = true
else match mode {
    Fill | FillClip                 => is_fill = true
    Stroke | StrokeClip             => if font.has_face() { is_stroke = true }
                                       else               { is_fill   = true }
    FillStroke | FillStrokeClip     => { is_fill = true
                                         if font.has_face() { is_stroke = true } }
    Invisible                       => unreachable
    Clip                            => return true     // Tr 7: clip-only, no paint
    Unknown                         => unreachable
}
stroke_argb = fill_argb = 0 ; pattern = false
if is_stroke { if stroke colour is a Pattern { pattern = true }
               else { stroke_argb = GetStrokeArgb(obj) } }
if is_fill   { if fill   colour is a Pattern { pattern = true }
               else { fill_argb   = GetFillArgb(obj) } }
text_matrix = obj.text_matrix()
if !IsAvailableMatrix(text_matrix): return true
if pattern: DrawTextPathWithPattern(...); return true              // §1.9
if is_clip || is_stroke:
    device_matrix = &obj2device
    if is_stroke:
        ctm = text_state.ctm()          // the 4-element Tz/Tr-independent CTM snapshot
        if ctm[0] != 1.0 || ctm[3] != 1.0:
            c = Matrix(ctm[0], ctm[1], ctm[2], ctm[3], 0, 0)
            text_matrix   *= c.inverse()
            device_matrix  = c * obj2device
    return TextRenderer::draw_text_path(device, codes, positions, font, size,
                                        text_matrix, device_matrix, graph_state,
                                        fill_argb, stroke_argb, clipping_path,
                                        GetFillOptionsForDrawTextPath(...))
text_matrix.concat(obj2device)
return TextRenderer::draw_normal_text(device, codes, positions, font, size,
                                      text_matrix, fill_argb, options)
```
`GetFillOptionsForDrawTextPath` (:112-130):
`if is_stroke && is_fill { opts.stroke = true; opts.stroke_text_mode = true }`;
`adjust_stroke` from the gs; `aliased_path` from `bNoTextSmooth`. Note
`fill_type` stays `kNoFill` here — `CFX_RenderDevice::DrawTextPath` sets
`fill_type = kWinding` per glyph when `fill_color != 0` (:1404-1406).

**The `ctm` un-transform for stroked text** is the subtle bit: stroke width
must be measured in *text* space, so when the text-state CTM has a non-unit
x/y scale the text matrix is pre-divided by it and the scale is folded into the
device matrix instead. `ctm` is a 4-float snapshot the page layer records
(page brief §1.9).

**`CPDF_TextRenderer::DrawTextPath`/`DrawNormalText`**
(cpdf_textrenderer.cpp:52-98, 138-179) both do exactly one thing beyond
forwarding: they **split the char-position list into runs of equal
`fallback_font_position_`** and issue one device call per run, choosing
`font.GetFont()` for `-1` and `font.GetFontFallback(pos)` otherwise. The
result is the AND of every run's result. Our `Font::decode` already carries the
fallback identity per `CharItem` (SPEC §6), so the run split is a `chunk_by`.

`GetTextRenderOptionsHelper` (:27-47):
```
if font.is_cid()          { opts.font_is_cid = true }
if options.bNoTextSmooth  { opts.aliasing_type = kAliasing }
else if options.bClearType{ opts.aliasing_type = kLcd }
if options.bNoNativeText  { opts.native_text = false }
```
Default `aliasing_type` is `kAntiAliasing` (cfx_textrenderoptions.h:39).
**With the oracle's flags (`bClearType == false`, `bNoTextSmooth == false`) the
aliasing type is `kAntiAliasing`**, so LCD/subpixel text never runs in
conformance. That is a large simplification: we render glyphs as filled paths
and the oracle renders them as grayscale glyph bitmaps, and the only
reconciliation needed is coverage-level, not layout-level.

**`CFX_RenderDevice::DrawNormalText` (cfx_renderdevice.cpp:1158-1376) — the
glyph-bitmap vs path decision.** For our configuration
(`DeviceType::kDisplay`, `bpp_ = 32`, `RenderCapAlphaOutput() = true`,
`is_text_smooth = true` since `kAntiAliasing`), the anti-alias selection
(:1171-1207) lands on `anti_alias = kLcd, normalize = true` via the
`render_cap_alpha_output_` arm — **even though `bClearType` is false** —
because `anti_alias` is an fxge-internal choice, not the PDF option. Then
:1240-1250:
```
char2device = text2device scaled by (font_size, -font_size)
if |char2device.a| + |char2device.b| > 50.0 || is_printer:
    if font.has_face():
        return DrawTextPath(..., aliased_path = !is_text_smooth)
// else: glyph bitmaps
```
**`50 * 1.0f` is the threshold at which the oracle switches from glyph bitmaps
to outline fills.** Below it the oracle rasterizes hinted FreeType glyph
bitmaps with LCD filtering, `AdjustGlyphSpace` integer-origin nudging
(:57-97, moves an origin by ±1 when the accumulated float/int spacing error
exceeds 0.5), a 256-entry `kTextGammaAdjust` table (:99-122), and
`AlphaMerge`-based compositing. **We do not reproduce any of that** — D7. Our
glyphs are always outline fills. This is the single largest deliberate
divergence in the crate and the reason text pixels are Tier-B, not Tier-A.

`DrawTextPath` (:1378-1418) per glyph:
```
path = font.load_glyph_path(gid, font_char_width)
matrix = charpos.effective_matrix(Matrix(size, 0, 0, size, origin.x, origin.y))
matrix.concat(text2user)
transformed = path.transformed(matrix)
if fill_color || stroke_color:
    opts = fill_options; if fill_color { opts.fill_type = Winding }
    opts.text_mode = true
    device.draw_path(transformed, user2device, graph_state, fill_color,
                     stroke_color, opts)
if clipping_path: clipping_path.append(transformed, user2device)
```
`opts.text_mode = true` is what excludes glyph outlines from the zero-area
thin-line conversion (§1.7 step 3). Note `if fill_color || stroke_color` tests
the whole ARGB word, so a colour of `0x00000000` (the transparent sentinel from
§1.3) skips the draw entirely but **still contributes to the clipping path**.

### 1.9 Patterns

`ProcessPathPattern` (cpdf_renderstatus.cpp:1273-1295) runs *before* the
ordinary path draw and drains pattern colours out of it:
```
if fill_type != NoFill && fill_colour.is_pattern():
    DrawPathWithPattern(obj, m, &fill_colour, stroke = false)
    fill_type = NoFill
if stroke && stroke_colour.is_pattern():
    DrawPathWithPattern(obj, m, &stroke_colour, stroke = true)
    stroke = false
```
so a path with both a pattern fill and a pattern stroke is drawn **twice**, and
the residual `ProcessPath` call then does nothing.

`DrawPathWithPattern` dispatches on `AsTilingPattern` / `AsShadingPattern`.

**Shading patterns** (`DrawShadingPattern`, :1185-1209):
```
if !pattern.load(): return
StateRestorer(device)
if !ClipPattern(obj, m, stroke): return
rect = obj.transformed_bbox(m) ∩ device.clip_box()
if rect.is_empty(): return
matrix = pattern.pattern_to_form * m
alpha  = roundf(255 * (stroke ? obj.stroke_alpha : obj.fill_alpha))    // ROUNDED here
RenderShading::draw(device, ctx, cur_obj, pattern, matrix, rect, alpha, options)
```
Note the alpha here is **rounded** (`FXSYS_roundf`), unlike §1.3's truncation.

`ClipPattern` (:598-609): a **path** object clips by its own geometry
(`SelectClipPath` → `SetClip_PathStroke` when stroking, else
`SetClip_PathFill` with the object's fill type and `bNoPathSmooth`); an
**image** object clips by its transformed bbox rect; anything else returns
`false` and the pattern is dropped.

**Tiling patterns** (`DrawTilingPattern`, :1225-1254 →
`CPDF_RenderTiling::Draw`, cpdf_rendertiling.cpp:85-299). The page brief §1.16.9
already transcribes the cell-sizing, step-validation, tile-range and
aligned-fast-path ladders; what belongs *here* is the rasterization:

- **Cell bitmap format** (`DrawPatternBitmap`, :29-73):
  `pPattern->colored() ? kBgra : k8bppMask`. The cell device is created with
  `CreateForBitmapWithBackdropAndGroupKnockout(bitmap, backdrop = null,
  group_knockout = true)` — knockout on, backdrop null, which for the AGG
  driver means `backdrop_bitmap_` is null and the knockout blend path is
  *not* taken. Effectively a no-op; recorded so nobody reads meaning into it.
  A separate `CPDF_RenderContext` is built over the pattern form with
  `mtPattern2Bitmap = mtObject2Device * mtAdjust`, where
  `mtAdjust.MatchRect(bitmap_rect(0,0,w,h), cell_bbox_in_device_space)`.
  Options: `kAlpha` colour mode when uncoloured, plus the caller's draw options
  with **`bForceHalftone = true`** forced on.
- **The 16-pixel enlargement rule** (:217-227): when
  `width * height < 16` the cell is rendered at **8×8** and then
  `StretchTo(width, height, default resample options)` down to its real size.
  This is an anti-aliasing trick for sub-4×4 tiles and is pixel-visible.
- **`kGray` colour mode** applies `ConvertColorScale(is_white_on_black = false)`
  to the cell bitmap (:232-234).
- **Screen buffer** (:239-242): one `kBgra` bitmap the size of the device clip
  box; every tile is blitted into it, and the whole thing is handed to
  `CompositeDIBitmap(screen, clip.left, clip.top, mask_argb = 0, alpha = 1.0,
  Normal, CPDF_Transparency())` — i.e. **the tile stack is composited as a
  plain normal-blend image, not as a transparency group**.
- **Per-tile blit** (:245-297):
  - aligned: `start = roundf(pattern2device.e) + col*width - clip.left` (and the
    row/height mirror).
  - unaligned: `original = pattern2device * (col*x_step, row*y_step)`, then
    `start_x = roundf(original.x + left_offset) - clip.left` where
    `left_offset = cell_bbox.left - pattern2device.e` (and `top_offset =
    cell_bbox.bottom - pattern2device.f`); both through checked i32, aborting
    the whole pattern on overflow.
  - **`width == 1 && height == 1`** is a special case that writes a single
    `u32` directly, bounds-checked against the clip box: coloured copies the
    cell's first pixel; uncoloured writes
    `(cell_byte << 24) | (fill_argb & 0xffffff)`.
  - otherwise `CompositeBitmap` (coloured) or `CompositeMask(fill_argb)`
    (uncoloured), both `BlendMode::Normal`.
- **The slow path** (:151-184), taken when
  `width > clip.width() || height > clip.height() ||
   width*height > clip.width()*clip.height()`: each tile is rendered *directly*
  to the device inside a `StateRestorer`, with `pStates` cloned via
  `CloneObjStates(obj.graphic_states, stroke)` for an uncoloured pattern, or a
  default state carrying only the object's fill alpha for a coloured pattern on
  a **path** object (and *nothing* for a coloured pattern on a non-path
  object). Returns `null`, so the caller composites nothing.
  `CloneObjStates` (:803-822) copies the source states and, when the chosen
  colour is non-null, sets **both** fill and stroke colour refs to the chosen
  one — so an uncoloured tile inherits a single colour for both operations.

Corpus witnesses: `FRC_4.5.5_Pattern_tiling`, `FRC_4.5.5_Pattern_shading`,
`path_5_pattern`, `2_color_type3_pattern_bbox`.

### 1.10 Shadings — the seven rasterizers

`CPDF_RenderShading::Draw` (cpdf_rendershading.cpp:1008-1118) is the common
entry. `kShadingSteps = 256` (:45).

```
cs = pattern.color_space(); if none: return
background = 0
if !pattern.is_shading_object() && dict.has_key("Background"):
    bc = dict["Background"]
    if bc && bc.len() >= cs.n_components():
        rgb = cs.to_rgb_or_zeros(bc[..n])
        background = ARGB(255, (i32)(r*255), (i32)(g*255), (i32)(b*255))   // TRUNCATED
clip_bbox = clip_rect
if dict.has_key("BBox"):
    clip_bbox ∩= (matrix * dict.rect("BBox")).outer_rect()
buffer = DeviceBuffer(ctx, device, clip_bbox, cur_obj, max_dpi = 150)
bitmap = buffer.initialize()               // kBgra, sized to the clipped bbox
if !bitmap: return
if background != 0: bitmap.clear(background)
final_matrix = matrix * buffer.matrix()
match shading_type { ... one of the seven below ... }
if color_mode == kAlpha: bitmap.set_red_from_alpha()
else if color_mode == kGray: bitmap.convert_color_scale(is_white_on_black = false)
buffer.output_to_device()
```

`CPDF_DeviceBuffer` (cpdf_devicebuffer.cpp:26-107): `CalculateMatrix` is a
**pure translation by `(-rect.left, -rect.top)` on non-Windows** — the
`max_dpi = 150` down-scaling is `#if BUILDFLAG(IS_WIN)`. `OutputToDevice`
therefore always takes the `matrix.a == 1 && matrix.d == 1` arm and calls
`SetDIBits(bitmap, rect.left, rect.top)`. **So on Linux the shading is
rasterized at exactly device resolution, one shading pixel per device pixel,
and blitted with a normal blend — no resampling, no DPI cap.** The `150` is
dead for us; recorded because it is the first thing a reader looks for.

`Draw` is called from two places with different alphas:
- `DrawShadingPattern` (§1.9): `roundf(255 * fill_or_stroke_alpha)`, clipped to
  the painting object's geometry.
- `ProcessShading` (:1211-1223) for a bare `sh` operator:
  `roundf(255 * fill_alpha)`, `matrix = shading_obj.matrix * obj2device`, and
  `rect = obj.transformed_bbox ∩ clip_box` — **no geometry clip**, the `sh`
  operator paints the whole clip region.

**The colour LUT** (`GetShadingSteps`, :65-100):
```
results_count = { let n = Σ funcs[i].output_count();
                  if n == 0 { return false } else { max(n, cs.n_components()) } }
result_array = vec![0.0; results_count]
diff = t_max - t_min
for i in 0..256:
    input = diff * i / 256 + t_min              // note: /256, NOT /255
    span = &mut result_array
    for f in funcs { if let Some(n) = f.call(&[input], span) { span = &mut span[n..] } }
    rgb = cs.to_rgb_or_zeros(&result_array)
    lut[i] = ARGB(alpha, roundf(r*255), roundf(g*255), roundf(b*255))     // ROUNDED
```
Two things: the divisor is `kShadingSteps` (256) not `kShadingSteps - 1`, so
the LUT never evaluates the function at `t_max` — the page brief calls this the
"shading LUT off-by-one" and SPEC §7 already accepted porting it verbatim. And
**multiple functions write into successive slices of one output buffer**,
`result_array` is *not* cleared between iterations, and a function returning
`None` does not advance the span.

`ComponentToShadingIndex(c, c_min, c_max)` (:102-107):
`if c_min == c_max { 0 } else { ((c - c_min) / (c_max - c_min)) * 255 }`
— scaled to `kShadingSteps - 1`, unlike the LUT build.

**Type 2 — axial** (`DrawAxialShading`, :109-175). Per destination pixel:
```
inv = object_to_bitmap.inverse()                          // computed once
pos = inv * (column as f32, row as f32)                   // PIXEL CORNER, not centre
scale = ((pos.x - x0)*x_span + (pos.y - y0)*y_span) / axis_len_square
index = (scale * 255) as i32                              // TRUNCATION toward zero
if index < 0     { if !start_extend { continue } ; index = 0   }
else if index >= 256 { if !end_extend { continue } ; index = 255 }
pix = lut[index]                                          // OPAQUE OVERWRITE of the u32
```
with `axis_len_square = x_span² + y_span²`. Note `axis_len_square == 0`
(coincident endpoints) yields ±inf/NaN and every pixel is skipped or clamped —
no guard. `t_min`/`t_max` come from `/Domain` (defaulting `0..1`), `/Extend`
booleans from `GetBooleanAt(i, false)`. The write is a **plain assignment to
the `u32` scanline**, so an unextended region keeps whatever `background` (or
zero) was there.

**Type 3 — radial** (`DrawRadialShading`, :177-275). The quadratic and its root
selection, verbatim:
```
dx = x1-x0 ; dy = y1-y0 ; dr = r1-r0
a  = dx² + dy² - dr²
a_is_zero = FXSYS_IsFloatZero(a)
decreasing = dr < 0 && (hypot(dx,dy) as i32) < -dr        // INTEGER truncation in the test
per pixel:
  pdx = pos.x - x0 ; pdy = pos.y - y0
  b = -2 * (pdx*dx + pdy*dy + r0*dr)
  c =  pdx² + pdy² - r0²
  if FXSYS_IsFloatZero(b):     s = sqrt(-c / a)            // f64 sqrt of an f32 quotient
  else if a_is_zero:           s = -c / b
  else:
      disc = b*b - 4*a*c
      if disc < 0 { continue }
      root = sqrt(disc)
      s1 = (-b - root) / (2*a) ; s2 = (-b + root) / (2*a)
      if a <= 0 { swap(s1, s2) }
      s = if decreasing { if s1 >= 0 || start_extend { s1 } else { s2 } }
          else          { if s2 <= 1.0 || end_extend { s2 } else { s1 } }
      if r0 + s*dr < 0 { continue }                        // negative radius -> skip
  index = (s * 255) as i32 ; clamp/extend exactly as axial
```
Subtleties that are pixel-visible: the `b == 0` branch takes
`sqrt(-c/a)` **without** checking the sign of `-c/a` (NaN → the `as i32` cast
is UB in C++ and in practice yields 0 or INT_MIN, which the `< 0` test then
catches); the `a <= 0` swap uses `<=` so `a == 0` reaching here is impossible
(guarded above) but the branch is written as if it could; the `r0 + s*dr < 0`
skip is **only** applied in the two-root branch, not in the `b == 0` or
`a == 0` branches. `FXSYS_IsFloatZero` is an epsilon test, not `== 0.0`.
Corpus witnesses: `radial_shading_point_at_center`,
`radial_shading_point_at_border`, `radial_shading_point_at_border_no_extend`,
`axial_shading_point_at_border_no_extend`.

**Type 1 — function-based** (`DrawFuncShading`, :277-336):
```
total = validated_outputs_count(funcs, cs); if 0 { return }
(xmin,xmax,ymin,ymax) = /Domain or (0,1,0,1)              // NOTE the order: x0 x1 y0 y1
matrix = object_to_bitmap.inverse() * dict.matrix("Matrix").inverse()
per pixel:
  pos = matrix * (column, row)
  if pos.x < xmin || pos.x > xmax || pos.y < ymin || pos.y > ymax { continue }
  span = &mut result_array; for f in funcs { ... same slicing as the LUT ... }
  rgb = cs.to_rgb_or_zeros(&result_array)
  pix = ARGB(alpha, (i32)(r*255), (i32)(g*255), (i32)(b*255))   // TRUNCATED, unlike the LUT
```
No LUT — the function is evaluated per pixel — and the RGB conversion
**truncates** where `GetShadingSteps` rounds. Corpus: `2_shading_type1`,
`2_shading_type1_sc_`.

**Types 4/5 — Gouraud triangles** (`DrawGouraud`, :357-456, driven by
`DrawFreeGouraudShading` :458-517 and `DrawLatticeGouraudShading` :519-586).
The triangle fill is a hand-rolled scanline rasterizer, **not** the path
rasterizer, and it is aliased:
```
min_y/max_y over the three vertices; if equal, skip
min_yi = max(floor(min_y) as i32, 0)
max_yi = ceil(max_y) as i32 ; if max_yi >= height { max_yi = height - 1 }
for y in min_yi..=max_yi:                                  // INCLUSIVE upper bound
    collect up to 3 edge intersections at INTEGER y via GetScanlineIntersect:
        skip an edge with first.y == second.y
        require y within [min(y0,y1), max(y0,y1)] INCLUSIVE on both ends
        x = x0 + (x1-x0)*(y-y0)/(y1-y0)
        rgb linearly interpolated by the same y_dist
    if intersections != 2 { continue }                     // 1 or 3 -> whole scanline dropped
    order by x; min_x = floor(lo); max_x = ceil(hi)
    start_x = clamp(min_x, 0, width) ; end_x = clamp(max_x, 0, width)
    range_x = clamped_sub(max_x, min_x)
    *_unit  = (c[end] - c[start]) / range_x
    diff_x  = clamped_sub(start_x, min_x)
    *_result = c[start] + diff_x * *_unit
    for x in start_x..end_x:
        r_result += r_unit                                 // INCREMENT BEFORE USE
        if lut.is_some():
            index = r_result as i32; clamp to 0..255
            pix = lut[index]
        else:
            g_result += g_unit ; b_result += b_unit
            pix = ARGB(alpha, (i32)(r*255), (i32)(g*255), (i32)(b*255))
```
Three quirks to port verbatim: the **`r_result += r_unit` before the first
write** (so the leftmost pixel is one step off), the **`end_x` exclusive against
a `clamp(_, 0, width)`** (so the rightmost column can be written at index
`width-1` at most, but `start_x == width` yields an empty loop rather than an
out-of-bounds), and the **`!= 2` intersection drop** which leaves seams at
exact-vertex scanlines. When functions are present only the red channel
carries the parametric `t` (already mapped through `ComponentToShadingIndex`
at vertex-read time) and the LUT is indexed by it.

Free-form triangle assembly (:484-516): `flag == 0` reads two more vertices
unconditionally and discards their flags; `flag == 1` shifts
`(t1,t2,new)`; `flag == 2` **and `flag == 3`** both shift `(t0,t2,new)` because
only `1` is special-cased.
Lattice (:560-585): two ping-pong rows, each adjacent column pair emitting
`(v[last][i], v[other][i-1], v[last][i-1])` then `(v[last][i], v[other][i-1],
v[other][i])`; an empty row ends the mesh. `VerticesPerRow < 2` aborts.
Corpus: `2_shading_type4_h`, `2_shading_type5_h`.

**Types 6/7 — Coons and tensor patches** (`DrawCoonPatchMeshes`, :848-1003,
with `PatchDrawer` :738-846 and `CubicBezierPatch` :588-684). Unlike the other
six, this one **draws through the path rasterizer**: it creates a
`CFX_RenderDevice` over the shading bitmap and calls `DrawPath` per
subdivided cell.
```
kBoundaryPathSize = 13        // 1 kMove + 12 kBezier points, reused across cells
kCoonColorThreshold = 4
IsSmall(patch) = bbox(all 16 control points).width < 2 && .height < 2

PatchDrawer::Draw(x_scale, y_scale, left, bottom, patch, lut):
  small = patch.is_small()
  div_colors[0] = bilinear(patch_colors, left,   bottom,   x_scale, y_scale)
  if !small:
      div_colors[1..4] = bilinear at (left,bottom+1), (left+1,bottom+1), (left+1,bottom)
      d_bottom = dist(c3,c0) ; d_left = dist(c1,c0)
      d_top    = dist(c1,c2) ; d_right= dist(c2,c3)
  if small || (all four < 4):
      boundary = the 12 outer control points, closed             // NOT the interior ones
      opts = FillOptions(Winding); opts.full_cover = true
      if bNoPathSmooth { opts.aliased_path = true }
      colour = if lut { lut[clamp(div_colors[0].comp[0], 0, 255)] }
               else   { ARGB(alpha, c0.comp[0], c0.comp[1], c0.comp[2]) }
      device.draw_path(boundary, null, null, colour, 0, opts)
  else if d_bottom < 4 && d_top < 4:      subdivide VERTICALLY,  y_scale*=2, bottom*=2
  else if d_left   < 4 && d_right < 4:    subdivide HORIZONTALLY,x_scale*=2, left  *=2
  else:                                   subdivide BOTH,        all four *=2
```
Bilinear colour interpolation is **integer** with overflow detection
(`Interpolate`, :686-696: `p = ((c1 - c0) * delta1) / delta2 + c0` in checked
i32; any overflow aborts the whole cell). `Distance` is the max per-component
absolute difference. Subdivision is de Casteljau at t=0.5 on all four rows or
columns (:622-681). **There is no recursion depth limit** — termination relies
solely on `IsSmall()` (bbox < 2×2 device units) and the colour threshold; a
patch with NaN control points would not terminate. **Q2.**

`full_cover = true` is the crucial rasterization flag: it makes the compositor
**ignore per-pixel coverage and write at the constant source alpha**
(cfx_agg_devicedriver.cpp:481-491, :691-693), so abutting cells overpaint each
other's antialiased edges to full opacity and the patch shows no seams. Any
backend we use must offer an equivalent, or we accept visible seams. See §5.

Per-patch setup (:895-1002): flags via `ReadFlag()`; the point/colour reuse
tables; a **bbox reject** (`bbox.right <= 0 || bbox.left >= width ||
bbox.top <= 0 || bbox.bottom >= height` → skip the patch) computed over the
first `point_count` coords *after* transformation; the Coons interior
derivation from ISO 32000-2 §8.7.4.5.8. The page brief §1.16.8 has the stream
side. Corpus: `2_shading_type_6_00`, `2_shading_type_6_001`, `shade-tensor`.

### 1.11 Type 3 fonts

`ProcessType3Text` (cpdf_renderstatus.cpp:933-1130).

```
if type3_font_cache contains this font: return true         // recursion guard, by identity
fill_argb = GetFillArgbForType3(textobj)                    // NOT GetFillArgb
fill_alpha = A(fill_argb)
text_matrix = obj.text_matrix()
char_matrix = font.font_matrix() scaled by (font_size, font_size)
glyphs = vec![default; char_codes.len()]                     // non-print only
for (i, code) in char_codes:
    if code == u32::MAX { continue }
    ch = font.load_char(code); if none { continue }
    matrix = char_matrix with e += char_positions[i], then * text_matrix * obj2device
    if !ch.load_bitmap_from_sole_image_of_form():
        // --- CHAR PROC PATH ---
        flush any glyphs accumulated so far via device.set_bit_mask(..., fill_argb)
        clear glyphs
        states  = CloneObjStates(&textobj.graphic_states, stroke = false)
        options = self.options with bForceHalftone = true, bRectAA = true
        if fill_alpha == 255:
            status = new RenderStatus(ctx, device)
                .options(options).transparency(form.transparency())
                .type3_char(ch).fill_color(fill_argb).form_resource(form.res)
                .initialize(parent = self, initial = states)
            status.type3_font_cache = self.type3_font_cache + [this font]
            StateRestorer(device); status.render_object_list(form, matrix)
        else:
            rect = (matrix * form.bounding_box()).outer_rect()
            if !rect.valid() { continue }
            bmp_device = RenderDevice::for_new_bitmap(rect.w, rect.h, kBgra)
            ... same status setup ...
            matrix translated by (-rect.left, -rect.top)
            status.render_object_list(form, matrix)
            device.set_dibits(bmp_device.bitmap(), rect.left, rect.top)
    else if ch.bitmap():
        // --- GLYPH BITMAP PATH ---
        cache = DocRenderData::cached_type3(font)
        bitmap = cache.load_glyph_bitmap(code, matrix)
        if none { continue }
        origin = (roundf(matrix.e), roundf(matrix.f))
        if glyphs.is_empty():
            device.set_bit_mask(bitmap, origin.x + bitmap.left,
                                        origin.y - bitmap.top, fill_argb)
        else: glyphs[i] = (bitmap, origin)
if glyphs.is_empty(): return true
rect = glyphs_bbox(glyphs, anti_alias_is_lcd = false)
mask = Bitmap(rect.w, rect.h, k8bppMask)
for glyph in glyphs where glyph.bitmap.is_mask_format():
    mask.composite_mask(pt.x, pt.y, w, h, glyph.bitmap, fill_argb, 0, 0, Normal)
device.set_bit_mask(mask, rect.left, rect.top, fill_argb)
```

Contracts:
- **The colour rule.** `GetFillArgbForType3` skips the `type3_char_` branch, so
  the *outer* text object's own colour is used to establish `fill_argb`; then
  inside the char proc, `SetFillColor(fill_argb)` + `SetType3Char(ch)` makes
  `GetFillArgb` return that colour for every uncoloured drawing operation
  (`Type3CharMissingFillColor(ch, cs) = ch != null && (!ch.colored ||
  fill_colour_missing)`, :168-176). A **coloured** (`d0`) char proc that *does*
  set its own colour keeps it; an **uncoloured** (`d1`) one, or one that never
  sets a colour, gets the text object's.
- **`bForceHalftone` and `bRectAA` are forced on** inside char procs. `bRectAA`
  disables the axis-aligned rect snapping (§1.7 step 2), so rectangles inside a
  type-3 glyph *are* antialiased where the same rectangle on the page would not
  be. `bForceHalftone` sets `resample_options.bHalftone` on images.
- **Translucent char procs go through an offscreen `kBgra` buffer** and are
  blitted with `SetDIBits` (normal blend, no group semantics). `fill_alpha`
  here is the truncated `alpha` from §1.3.
- **Recursion guard by font identity**, appended to a per-status vector that is
  copied into each child status. Not a depth counter — a *set*, so a font
  cannot appear twice anywhere in the ancestry.
- **The glyph-bitmap accumulation is a two-phase flush**: bitmaps accumulate
  into `glyphs[i]` until a char-proc char appears, at which point everything so
  far is blitted individually; at the end the remainder is merged into one
  8bpp mask and blitted once. `TextGlyphPos::GetOrigin(offset)` is a checked
  subtraction that drops a glyph on overflow.

**The type-3 glyph cache** (`CPDF_Type3Cache`, cpdf_type3cache.cpp:83-169;
`CPDF_Type3GlyphMap`, cpdf_type3glyphmap.cpp:20-51):
```
SizeKey = (roundf(m.a * 10000), roundf(m.b * 10000),
           roundf(m.c * 10000), roundf(m.d * 10000))       // translation-independent
per (font, SizeKey): a charcode -> CFX_GlyphBitmap map
RenderGlyph(size_cache, charcode, m):
    image_matrix = char.matrix * Matrix(m.a, m.b, m.c, m.d, 0, 0)
    if |image_matrix.b| < |image_matrix.a| / 100 && |image_matrix.c| < |image_matrix.d| / 100:
        top_line = first scanline with ink ; bottom_line = last
        if top_line == 0 && bottom_line == height - 1:              // ink touches both edges
            top_y = image_matrix.d + image_matrix.f ; bottom_y = image_matrix.f
            flipped = top_y > bottom_y ; if flipped { swap }
            (top_line, bottom_line) = size_cache.adjust_blue(top_y, bottom_y)
            h = if flipped { top_line - bottom_line } else { bottom_line - top_line }   // checked
            res = bitmap.stretch_to(image_matrix.a as i32, h, default options)
            top  = top_line
            left = if image_matrix.a < 0 { roundf(image_matrix.e + image_matrix.a) }
                   else                  { roundf(image_matrix.e) }
    if res.is_none(): res = bitmap.transform_to(image_matrix, &mut left, &mut top)
    GlyphBitmap { left, top: -top, bitmap: res }
```
with ink detection `IsScanLine1bpp` (any non-zero bit within the width, the
trailing partial byte masked `0xff << (8 - width % 8)`) and `IsScanLine8bpp`
(any byte `> 0x40`; for bpp > 8 the width is scaled by `bpp/8` and the same
`> 0x40` test is applied bytewise).

**"Blue" snapping** (`AdjustBlue`, cpdf_type3glyphmap.cpp:22-51):
```
kType3MaxBlues = 16
AdjustBlueHelper(pos, blues):
    best = -1 ; min_distance = 1_000_000.0
    for i, b in blues:
        d = |pos - b as f32|
        if d < min(0.8, min_distance) { min_distance = d ; best = i }
    if best >= 0 { return blues[best] }
    new = roundf(pos)
    if blues.len() < 16 { blues.push(new) }
    return new
```
Two independent lists (top and bottom), per `SizeKey`, capped at 16 entries,
snapping within **0.8 device units**. This aligns the horizontal edges of
type-3 glyphs across a run so a table of rules or a bitmap font does not shimmer.
It is **order-dependent** (the first glyph to establish a blue wins) and
therefore cache-state-dependent — a genuine hazard for a parallel renderer.
Corpus: `type3`, `type3_xobject`, `bitmap-symbol-*`.

### 1.12 Images

`CPDF_ImageRenderer` (cpdf_imagerenderer.cpp) is a small state machine
(`Mode { kNone, kDefault, kBlend, kTransform }`) whose progressive halves we
drop (D1). What matters is the **decision tree in `StartRenderDIBBase`**
(:86-186) and the resample selection.

```
if !loader.bitmap(): return false
alpha = obj.general_state.fill_alpha
dib   = loader.bitmap()
if color_mode == kAlpha && !loader.mask(): return StartBitmapAlpha()      // §1.12.3
if obj.general_state.TR:
    tf = cached_transfer_func(TR)
    if tf && !tf.identity(): dib = loader.translate_image(tf)             // per-pixel LUT
fill_argb = 0 ; pattern_color = false ; pattern = null
if dib.is_mask_format():
    if fill colour is a Pattern { pattern = it ; pattern_color = true }
    fill_argb = GetFillArgb(obj)
else if color_mode == kGray:
    dib = dib.realize().convert_color_scale(is_white_on_black = TRUE)     // NOTE: true here
resample = ResampleOptions::default()
if options.bForceHalftone  { resample.halftone = true }
if options.bNoImageSmooth  { resample.no_smoothing = true }
else if image.is_interpolate() { resample.interpolate_bilinear = true }   // the /Interpolate key
if loader.mask():   return DrawMaskedImage()                              // §1.12.1
if pattern_color:   return DrawPatternImage()                             // §1.12.2
if alpha != 1.0 || !state.has_ref() || !state.fill_op() || state.op_mode() != 0
   || state.blend_type() != Normal || state.stroke_alpha() != 1.0
   || state.fill_alpha() != 1.0:
       return StartDIBBase()
// --- the overprint-ish CMYK special case ---
cs = colorspace of the image's /ColorSpace, resolved against the PAGE resources
if cs.family in { DeviceCMYK, Separation, DeviceN }: blend_type = Darken
return StartDIBBase()
```

**The CMYK→Darken rule** (:160-184) is the one genuinely surprising branch: an
image in a subtractive colour space, drawn with *no* transparency of any kind
and with `/OP true` and `/OPM 0`, is composited with **`BlendMode::Darken`**
instead of Normal — PDFium's approximation of overprint. The gate requires
`state.HasRef() && state.GetFillOP() && state.GetOPMode() == 0` and all three
alphas at 1.0 and blend Normal. Corpus: `FRC_4.5.3_DeviceCMYK_k`,
`FRC_4.5.3_DeviceCMYK_K1`, `color_separation`, `FRC_4.5.4_Separation`.

**`StartDIBBase`** (:427-464):
```
if dib.bpp() > 1:
    image_size = (bpp/8) * width * height        // checked; overflow -> false
    if image_size > kHugeImageSize && !resample.halftone:
        resample.interpolate_bilinear = true
device.start_dibits_with_blend(dib, alpha, fill_argb, image_matrix, resample, blend_type)
```
`kHugeImageSize` is defined in `core/fxge/dib/fx_dib.h`; see the constants
table (§1.14). **Above that size, bilinear interpolation is forced on** unless
halftone was requested. This is a memory/quality heuristic that is nonetheless
pixel-visible.

`IsImageValueTooBig(v)` (:52-59): `|v| >= 256 * 1024 * 1024` → reject, used to
bound `dest_width`/`dest_height`/`dest_left`/`dest_top` in
`GetDimensionsFromUnitRect` (:667-699), which also encodes the **flip rule**:
`if image_matrix.a < 0 { dest_width = -dest_width }`,
`if image_matrix.d > 0 { dest_height = -dest_height }`, and then
`dest_left = if dest_width > 0 { rect.left } else { rect.right }` (mirror for
top). The `d > 0` (not `< 0`) is correct given PDF's y-down device space.

**`GetUnitRect`/`GetDrawRect`** (:251-261, :658-665): the image's device
footprint is `image_matrix.unit_rect().outer_rect()` (the unit square mapped
through the matrix), intersected with the device clip box. `GetDrawMatrix(rect)`
translates by `(-rect.left, -rect.top)`.

#### 1.12.1 `/SMask` and `/Mask` — `DrawMaskedImage` (:376-425)
```
rect = draw_rect(); if empty: return false
new_matrix = image_matrix translated by (-rect.left, -rect.top)
bmp_device = RenderDevice::for_new_bitmap(rect.w, rect.h, kBgrx)
bmp_device.clear(0xffffffff)                                  // WHITE, opaque
sub_status = RenderStatus(ctx, bmp_device).drop_objects(..).std_cs(true).initialize(null,null)
sub_renderer.start(dib, argb = 0, new_matrix, resample, std_cs = true); .continue()
mask_bitmap = CalculateDrawImage(bmp_device, loader.mask(), new_matrix, rect)
bmp_device.bitmap().multiply_alpha_mask(mask_bitmap)
bmp_device.bitmap().multiply_alpha(alpha)
device.set_dibits_with_blend(bmp_device.bitmap(), rect.left, rect.top, blend_type)
```
**The image is drawn onto opaque white, then the mask is applied as alpha.**
That white ground is observable wherever the image itself has alpha.

`CalculateDrawImage` (:263-319) renders the *mask* image into an `k8bppRgb`
bitmap through a nested `CPDF_ImageRenderer` with `argb = 0xffffffff`, then —
**the `/Matte` path** — if `loader.matte_color() != 0xffffffff`:
```
for each pixel with mask != 0:
    orig_b = (dest.b - matte_b) * 255 / mask + matte_b
    orig_g = (dest.g - matte_g) * 255 / mask + matte_g
    orig_r = (dest.r - matte_r) * 255 / mask + matte_r
    dest.* = clamp(orig_*, 0, 255)
```
i.e. **un-premultiply the image against the matte colour**, in integer
arithmetic, in place on the *destination* bitmap, before the mask is applied.
`mask == 0` pixels are skipped (avoiding the divide by zero). The mask bitmap
is then `ConvertFormat`ed to `k8bppMask`. Corpus witness: `matte`.

#### 1.12.2 Image masks painted with a pattern — `DrawPatternImage` (:325-374)
```
rect = draw_rect(); if empty: return false
bmp_device = RenderDevice::for_new_bitmap(rect.w, rect.h, kBgra)
sub_status over it with std_cs(true)
pattern_matrix = obj_to_device translated by (-rect.left, -rect.top)
sub_status.draw_tiling_pattern | draw_shading_pattern (pattern, image_obj,
                                                       pattern_matrix, stroke=false)
mask_bitmap = CalculateDrawImage(bmp_device, dib, new_matrix, rect)   // the STENCIL as mask
bmp_device.bitmap().multiply_alpha_mask(mask_bitmap)
device.set_dibits_with_blend(bmp_device.bitmap(), rect.left, rect.top, blend_type)
```
So a `/ImageMask true` XObject whose fill colour is a pattern paints the
*pattern* through the stencil. Note `alpha` is **not** applied here (unlike
`DrawMaskedImage`), because the pattern path already consumed the object's
alpha inside `DrawShadingPattern`/`DrawTilingPattern`.

#### 1.12.3 `kAlpha` colour mode — `StartBitmapAlpha` (:542-592)
Used when rendering an alpha-type soft mask or an uncoloured tile:
```
if dib.is_opaque_image():
    path = unit rect transformed by image_matrix
    a = roundf(alpha * 255)
    device.draw_path(path, null, null, ARGB(0xff, a, a, a), 0, Winding)
    return
mask = if dib.is_mask_format() { dib } else { dib.clone_alpha_mask() }
if |image_matrix.b| >= 0.5 || |image_matrix.c| >= 0.5:
    mask = mask.transform_to(image_matrix, &mut left, &mut top)
    device.set_bit_mask(mask, left, top, ARGB(0xff, a, a, a))
else:
    (left, top, w, h) = dimensions_from_unit_rect(unit_rect)
    device.stretch_bit_mask(mask, left, top, w, h, ARGB(0xff, a, a, a))
```
The **0.5 shear threshold** decides between a general transform and an
axis-aligned stretch — the same threshold appears in the Windows
`StartDIBBaseFallback` (:468-469) as `|b| >= 0.5 || a == 0` / `|c| >= 0.5 ||
d == 0`. In `kAlpha` mode the "colour" written is a gray whose value *is* the
alpha, which `SetRedFromAlpha`/the mask readback later interprets.

### 1.13 Soft masks — `LoadSMask`

cpdf_renderstatus.cpp:1434-1542. The *parsing* half (which keys, the `/S`
default, the `/TR` acceptance rule, the `/BC` backdrop colour) is in the page
brief §1.17; the *rendering* half is here.

```
group = smask_dict["/G"] as Stream; if none: return null
func  = smask_dict["/TR"] if Dict|Stream else None
matrix = smask_matrix translated by (-clip_rect.left, -clip_rect.top)
form = Form(doc, PAGE resources, group); form.parse_content()
luminosity = smask_dict["/S"] != "Alpha"
w = clip_rect.width() ; h = clip_rect.height()
format = if luminosity { kBgr } else { k8bppMask }          // non-Apple, non-Skia
bmp_device = RenderDevice::for_new_bitmap(w, h, format)
cs_family = Unknown
background = if luminosity { GetBackgroundColor(smask, group.dict, &mut cs_family) }
             else { 0 }
bmp_device.clear(background)
options = RenderOptions::default().color_mode(if luminosity { kNormal } else { kAlpha })
status = RenderStatus(ctx, bmp_device).options(options).group_family(cs_family)
             .load_mask(luminosity).std_cs(true).form_resource(form.res)
             .initialize(null, null)
status.render_object_list(&form, matrix)

result = Bitmap(w, h, k8bppMask)
transfers: [u8; 256] = if let Some(f) = func {
        for i in 0..256 { transfers[i] = roundf(f.call(&[i as f32 / 255.0])[0] * 255) }
    } else { 0, 1, 2, ..., 255 }                            // std::iota — identity
if luminosity:
    bpp = bmp.bpp() / 8
    for row, col: result[row][col] = transfers[FXRGB2GRAY(src[2], src[1], src[0])]
    // src is B,G,R -> FXRGB2GRAY(r=src[2], g=src[1], b=src[0])
else:
    src is already 8bpp alpha; result = transfers[src]  (or a straight copy if no func)
```
Contracts:
- **The mask is rendered at exactly the clip rect's device resolution**, in the
  device's own pixel grid, so no resampling is ever needed when it is applied.
- **The luminosity buffer is `kBgr` (24bpp, no alpha)** on our platform, cleared
  to the `/BC` backdrop (default opaque black). So an area the group does not
  paint contributes the backdrop's luminosity, not zero.
- **The alpha buffer is `k8bppMask`** cleared to `0`, and the group renders in
  `kAlpha` colour mode so every drawing operation writes *alpha as gray*.
- `SetGroupFamily(cs_family)` and `SetLoadMask(true)` are threaded down into the
  image loader (`cpdf_imagerenderer.cpp:76-79`) so an image inside a luminosity
  mask is converted through the group's colour space. The page brief's image
  path already takes `group_family` and `load_mask`.
- **The `/TR` LUT uses output component 0 only** and is built at 256 entries
  regardless of the function's domain.

`MultiplyAlphaMask` then multiplies the group buffer's alpha channel by this
mask (§1.14 for the exact math).

### 1.14 Constants and limits

| Constant | Value | Where | Meaning |
|---|---|---|---|
| `kRenderMaxRecursionDepth` | 64 | cpdf_renderstatus.cpp:82 | render-recursion cap (global counter) |
| `kShadingSteps` | 256 | cpdf_rendershading.cpp:45 | shading colour LUT size |
| `kCoonColorThreshold` | 4 | cpdf_rendershading.cpp:739 | max per-component colour delta before a patch cell is flat-filled |
| `kBoundaryPathSize` | 13 | cpdf_rendershading.cpp:589 | 1 move + 12 bezier points of a patch boundary |
| patch "is small" | bbox `w < 2 && h < 2` | cpdf_rendershading.cpp:591-595 | subdivision terminator |
| `CPDF_DeviceBuffer` max_dpi (shading) | 150 | cpdf_rendershading.cpp:1047 | **Windows-only**; identity on Linux |
| `CPDF_ScaledRenderBuffer` max_dpi | 300, or 0 for printed images | cpdf_renderstatus.cpp:362 | Windows-only |
| `kImageSizeLimitBytes` | 30 · 1024 · 1024 | cpdf_scaledrenderbuffer.cpp:18 | Windows-only; halve the matrix until under it |
| `kCacheSizeLimitBytes` | 100 · 1024 · 1024 | cpdf_renderoptions.cpp:11 | image cache cap, honoured only with `bLimitedImageCache` |
| `kHugeImageSize` | **60 000 000** | cpdf_dib.h:39; used cpdf_imagerenderer.cpp:437 and cpdf_pageimagecache.cpp:322 | above it, force bilinear; also the image-cache admission threshold |
| `kMaxProgressiveStretchPixels` | 1 000 000 | cfx_imagestretcher.cpp:23 | below it the stretch runs synchronously (D1: always, for us) |
| `kFixedPointOne` | 65536 (`1 << 16`) | cstretchengine.h:28-29 | resampling weight fixed point |
| `kMaxTableBytesAllowed` | 512 · 1024 · 1024 | cstretchengine.cpp:74 | weight-table allocation cap |
| `kMaxImageDimension` | 1024 · 1024 | cstretchengine.h:51 | per-axis destination size cap |
| `kPaletteSize` | 256 | cfx_dibbase.h | 1bpp palette expansion target |
| `kBase` / `kFix16` (transformer) | 256 / 0.05 | cfx_imagetransformer.cpp:26-30 | Windows-only (D4) |
| `IsImageValueTooBig` limit | 256 · 1024 · 1024 | cpdf_imagerenderer.cpp:55 | reject a dest dimension/offset |
| image shear threshold | 0.5 | cpdf_imagerenderer.cpp:468-469, 558 | transform vs stretch |
| `kType3MaxBlues` | 16 | cpdf_type3glyphmap.cpp:20 | per-size blue-zone list cap |
| type-3 blue snap distance | 0.8 | cpdf_type3glyphmap.cpp:27 | device units |
| type-3 size-key scale | 10000 | cpdf_type3cache.cpp:87-90 | matrix quantization for the cache key |
| type-3 near-axis test | `/100` | cpdf_type3cache.cpp:133-134 | `\|b\| < \|a\|/100 && \|c\| < \|d\|/100` |
| type-3 ink threshold (8bpp) | `> 0x40` | cpdf_type3cache.cpp:37 | scanline "has ink" |
| glyph-bitmap vs path threshold | 50.0 | cfx_renderdevice.cpp:1243 | `\|char2device.a\| + \|char2device.b\| > 50` |
| `AdjustGlyphSpace` error tolerance | 0.5 | cfx_renderdevice.cpp:85 | nudge a glyph origin by ±1 |
| `kTextGammaAdjust` | 256-entry table | cfx_renderdevice.cpp:99-118 | glyph-bitmap gamma (not used by us, D7) |
| thin-line alpha reduction | `>> 2` | cfx_renderdevice.cpp:968 | zero-area fills draw at ¼ alpha |
| pixel snap | `(int)c + 0.5` | cfx_renderdevice.cpp:368-369 | truncation, then pixel centre |
| `kMaxPos` (HardClip) | 32000.0 | cfx_agg_devicedriver.cpp:53 | per-axis coordinate clamp |
| minimum stroke width | 1 device px | cfx_agg_devicedriver.cpp:335 | `max(width·scale, 1/mean_unit)` |
| `kMinDashCycleThreshold` | 0.1 | cfx_agg_devicedriver.cpp:358 | device px; below it, solid |
| dash zero substitution | `<= 0.000001 → 0.1` | cfx_agg_devicedriver.cpp:372-374 | before scaling |
| `max_dashes` | 32 | agg_vcgen_dash.h:31 | silently truncating |
| default line width / miter | 1.0 / 10.0 | cfx_graphstatedata.h:52-53 | |
| aliased-path coverage threshold | 127 | agg_rasterizer_scanline_aa.h:293 | `> 127 → 255 else 0` |
| bezier flatten tolerance | 0.25 (sq), 4.0 (manhattan), depth 16 | agg_curves.cpp:29-39 | **does not scale with zoom** |
| AGG subpixel | 1/256, truncating | agg_rasterizer_scanline_aa.h:52 | |
| tile enlargement rule | `w*h < 16` → render 8×8 then stretch | cpdf_rendertiling.cpp:217-227 | |
| `FXRGB2GRAY` | `(b·11 + g·59 + r·30) / 100` | fx_dib.h:210 | integer, truncating |
| `AlphaMerge(d, s, a)` | `(d·(255-a) + s·a) / 255` | fx_dib.h:212-214 | integer, truncating |
| clip mask intersection | `old · new / 255` | cfx_agg_cliprgn.cpp:62-63 | integer, truncating |

### 1.15 Blend modes — exact per-channel math

`BlendMode` (fx_dib.h:124-144) is exactly ISO 32000-1 table 136/137 order,
values 0..15: `Normal, Multiply, Screen, Overlay, Darken, Lighten, ColorDodge,
ColorBurn, HardLight, SoftLight, Difference, Exclusion, Hue, Saturation, Color,
Luminosity`.

**Separable modes** (`fxge::Blend`, blend.cpp:47-103) — **all-integer,
truncating**, inputs and outputs in `0..=255`, **no clamping on output**:
```
Normal      -> s
Multiply    -> s * b / 255
Screen      -> s + b - s * b / 255
Overlay     -> HardLight(back = s, src = b)              // ARGUMENTS SWAPPED
Darken      -> min(s, b)
Lighten     -> max(s, b)
ColorDodge  -> if s == 255 { 255 } else { min(b * 255 / (255 - s), 255) }
ColorBurn   -> if s == 0   { 0 }   else { 255 - min((255 - b) * 255 / s, 255) }
HardLight   -> if s < 128 { (s * b * 2) / 255 }
               else       { Screen(back = b, src = 2*s - 255) }
SoftLight   -> if s < 128 { b - (255 - 2*s) * b * (255 - b) / 255 / 255 }
               else       { b + (2*s - 255) * (kColorSqrt[b] - b) / 255 }
Difference  -> |b - s|
Exclusion   -> b + s - 2 * b * s / 255
```
Three details that will bite a re-derivation:
- **`Overlay` is literally `HardLight` with the arguments swapped**, not an
  independently written formula.
- **`HardLight`'s threshold is `s < 128`**, not `2*s <= 255`.
- **`SoftLight` uses a 256-entry `kColorSqrt` LUT** (blend.cpp:21-43). Despite
  the name it is **not** `sqrt` — it is ISO 32000-1 §11.3.5.2's piecewise
  auxiliary `D(x)`, tabulated:
  ```
  D(x) = if x <= 0.25 { ((16x - 12)x + 4)x } else { sqrt(x) }
  kColorSqrt[i] == round(255 * D(i / 255))          // verified: exact for all 256 entries
  ```
  At `i = 1` the table holds `3`, where `round(255·sqrt(1/255))` would be `16`
  — a 17-count divergence, so a rewrite that "simplifies" to a square root is
  badly wrong, not off by rounding. Transcribe the table (Q7) or generate it
  from `D(x)`; both are correct, the table is safer.
  Note also that the low branch divides **twice** (`/255/255`), not once by
  65025 — the truncation differs.

**Non-separable modes** (cfx_scanlinecompositor.cpp:41-121) — **integer
`FX_RGB_STRUCT<int>` arithmetic, no floats**:
```
Lum(c)            = (c.r * 30 + c.g * 59 + c.b * 11) / 100
Sat(c)            = max(r,g,b) - min(r,g,b)
SetLum(c, l)      = ClipColor(c + (l - Lum(c)) on all three channels)
SetSat(c, s)      = { let mn = min(r,g,b); let mx = max(r,g,b)
                      if mn == mx { return (0,0,0) }
                      each channel: (ch - mn) * s / (mx - mn) }
ClipColor(c)      = { let l = Lum(c); let n = min(r,g,b); let x = max(r,g,b)
                      if n < 0 { each: l + (ch - l) * l       / (l - n) }
                      if x > 255 { each: l + (ch - l) * (255-l) / (x - l) }
                      c }

Hue        -> SetLum(SetSat(src,  Sat(back)), Lum(back))
Saturation -> SetLum(SetSat(back, Sat(src)),  Lum(back))
Color      -> SetLum(src,  Lum(back))
Luminosity -> SetLum(back, Lum(src))
```
**`ClipColor` computes `l`, `n`, `x` once from the *pre-clip* colour, and the
`x > 255` branch then operates on channels the `n < 0` branch may already have
modified while still using the original `l` and `x`.** That ordering is
load-bearing.

**On a grayscale destination** the four non-separable modes collapse
(`GetGrayWithBlend`, :226-235): `Luminosity` → the source gray;
`Hue`/`Saturation`/`Color` → **keep the backdrop unchanged**.

**Composition, straight-alpha BGRA over BGRA** (the canonical path,
cfx_scanlinecompositor.cpp:553-608):
```
src_alpha = src.a * clip / 255                       // clip = 255 when unclipped
if dest.a == 0:  dest = { src.rgb, alpha: src_alpha }; DONE — NO BLEND APPLIED
if src_alpha == 0: skip
dest_a      = AlphaUnion(dest.a, src_alpha) = dest.a + src_alpha - dest.a*src_alpha/255
alpha_ratio = src_alpha * 255 / dest_a
blended.C   = Blend(mode, dest.C, src.C)             // == src.C for Normal
blended.C   = AlphaMerge(src.C, blended.C, dest.a)   // the (1-αb)·Cs term
dest.C      = AlphaMerge(dest.C, blended.C, alpha_ratio)
dest.a      = dest_a
```
**`dest.a == 0` short-circuits before any blending** — the biggest structural
difference from a textbook Porter-Duff implementation, and it means a blend
mode is a no-op wherever the backdrop is fully transparent (which is exactly
what ISO 32000 §11.3.6 requires, arrived at by a different route).
`AlphaMerge(d, s, a) = (d*(255-a) + s*a) / 255`, truncating, **unclamped**.
For an **opaque** destination (`kBgr`/`kBgrx`) the `AlphaMergeToSource` step and
the ratio vanish: `dest.C = AlphaMerge(dest.C, Blend(dest.C, src.C), src_alpha)`.

Two asymmetries in upstream that a faithful port must reproduce:
- **the non-separable mask-over-BGRA path omits the `AlphaMergeToSource` step**
  (:1155-1157, :1287-1289) while the separable one keeps it;
- `GetAlphaWithSrc` (:146-154) divides by 255 **twice in sequence**
  (`mask_alpha * src_byte / 255 * clip / 255`), not once by 65025.

**`MultiplyAlpha(f)`** (cfx_dibitmap.cpp:382-410): `bitmap_alpha =
(int)(f * 255.0)` — **truncating**, so `0.5 → 127` (note this is *not*
`FXSYS_GetUnsignedAlpha`, which rounds); then `pixel.a = pixel.a * bitmap_alpha
/ 255`, truncating. `f == 1.0` is an exact-compare early return. The format is
force-converted to `kBgra` first.
**`MultiplyAlphaMask(m)`** (:341-380): `m` must be exactly `k8bppMask` and
dimension-identical (hard `CHECK`s — this is E8's invariant). `kBgrx` dest →
converted to `kBgra` and `alpha := m` (the simplification of `255*m/255`);
`kBgra` dest → `alpha := a * m / 255`. **Colour channels are never touched** —
consistent with straight alpha, and the one place §5.4's premultiplied
conversion is not a no-op.

### 1.16 Image resampling — the actual decision tree

`FXDIB_ResampleOptions` has exactly **four** fields (fx_dib.h:113-122):
`bInterpolateBilinear`, `bHalftone`, `bNoSmoothing`, `bLossy`. Only the first
three reach us (`bLossy` is Windows/PostScript-only, D4). Set by:
`bHalftone ← bForceHalftone`; `bNoSmoothing ← bNoImageSmooth`;
`bInterpolateBilinear ← the image's /Interpolate`, **or** forced by the
`kHugeImageSize` rule (§1.12), **or** by the auto-enable heuristic below.
Note the `else if` at cpdf_imagerenderer.cpp:141-145: **`bNoSmoothing` wins over
`/Interpolate`**.

**Source-format promotion before stretching** (`GetStretchedFormat`,
cfx_imagestretcher.cpp:29-41):
`k1bppMask → k8bppMask`, `k1bppRgb → k8bppRgb`,
`k8bppRgb with palette → kBgr`, everything else unchanged. A 1bpp *indexed*
source additionally has its 2-entry palette **linearly expanded to 256 entries**
(:45-65), `p[i] = lerp(p0, p1, i/255)` per channel, with both endpoints required
opaque.

**The bilinear auto-enable heuristic** (`CStretchEngine::UseInterpolateBilinear`,
cstretchengine.cpp:48-59) — transcribed exactly:
```
!options.bInterpolateBilinear && !options.bNoSmoothing && abs(dest_width) != 0
  && abs(dest_height) / 8  <  (src_width as i64 * src_height) / abs(dest_width)
```
Both divisions are **integer and truncating**; only the right-hand product is
widened to `i64`. Read plainly: bilinear turns itself on unless the image is
being enlarged by more than roughly 8× in area.

**Applying it** (:240-249) — note that each branch *replaces* the whole options
struct rather than setting one field:
```
if options.bNoSmoothing            { resample = { bNoSmoothing: true } }
else if use_interpolate_bilinear() { resample = { bInterpolateBilinear: true } }
else                               { resample = options }
```
So `bHalftone` is **silently dropped** whenever either of the first two arms
fires.

**Per-axis method selection** (:93-106), `scale = src_len / dest_len` as `f64`,
`base = if dest_len < 0 { src_len } else { 0 }`:
```
if bNoSmoothing || |scale| < 1.0:              // enlarging, or smoothing off
    src_pos = dest_pixel * scale + scale/2 + base
    if bInterpolateBilinear:                    // TWO taps
        start = floor(src_pos - 0.5) clamped to >= src_min
        end   = floor(src_pos + 0.5) clamped to <= src_max - 1
        if start >= end { w = [1.0] }
        else { second = fixed(src_pos - start - 0.5);
               w[start] = ONE - second; w[start+1] = second }
    else:                                       // NEAREST NEIGHBOUR
        p = floor(src_pos); start = max(p, src_min); end = min(p, src_max-1)
        w = [1.0]
else:                                          // |scale| >= 1: BOX / AREA downsample
    span = [dest_pixel*scale + base, that + scale]
    for each source pixel j in the span:
        weight = overlap of j's dest-space footprint with [dest_pixel, dest_pixel+1]
        fixed  = fixed(weight + rounding_error)          // ERROR FEEDBACK
        rounding_error = weight - fixed / 65536
    the last tap absorbs the remainder so the weights sum to exactly 65536
```
Fixed point is `1 << 16 = 65536`; `FixedFromDouble` **rounds**
(`FXSYS_round`, half-away-from-zero) while `PixelFromFixed` **truncates**
(`>> 16`) and the narrowing to `u8` **does not clamp**.

**Per-source-format accumulation** (:271-284, :352-534):
- **1bpp**: each set bit contributes `weight * 255` — the 0/255 expansion
  happens *inside* the weighted sum, not as a pre-pass.
- **8bpp mask**: a plain weighted sum of the mask bytes; **no
  alpha-weighting**.
- **`kBgra`**: alpha-weighted accumulation
  (`pixel_weight = weight * src.a / 255`, colour channels multiplied by it,
  alpha accumulated separately), then the vertical pass **un-premultiplies**
  with the only `clamp(·, 0, 255)` in the whole pipeline (:681-691). When the
  accumulated alpha is 0 the colour channels are **left at whatever the
  previous row wrote**.
- The horizontal and vertical passes each compute their **own** weight table
  from their **own** axis `scale`, using the same `resample_options_`, so the
  method can differ per axis.

**Our mapping** (§5.1): this whole engine is the backend's job. What the engine
owns is the *selection* — `ImageQuality::{Nearest, Bilinear}` — computed by
porting `UseInterpolateBilinear` plus the three flag sources plus the
`kHugeImageSize` rule, and the degenerate-size guards
(`kMaxImageDimension`, zero-length source/dest). The kernels themselves are
Tier B. See §7.1 for the `CStretchEngine` assertions that port.

### 1.17 Alpha compositing at the device — the AGG span functions

**Alpha compositing, exactly.** The destination is **straight (non-premultiplied)
BGRA**, and the ARGB blend (cfx_agg_devicedriver.cpp:715-745) is:
```
src_alpha = if full_cover { alpha·clip/255 or alpha }
            else          { alpha·cover/255, or alpha·cover·clip/255/255 }   // TWO truncating divides
if src_alpha == 0:   skip
if src_alpha == 255: overwrite the whole u32 with the source colour
if dest.a == 0:      dest.rgb = src.rgb ; dest.a = src_alpha
else:
    dest_a      = dest.a + src_alpha - dest.a·src_alpha/255
    alpha_ratio = src_alpha·255 / dest_a
    dest.c      = AlphaMerge(dest.c, src.c, alpha_ratio)   for c in {b, g, r}
    dest.a      = dest_a
```
For an opaque (`kBgr`/`kBgrx`) destination it is a plain
`AlphaMerge(dest.c, src.c, src_alpha)` per channel. **vello_cpu and tiny-skia
are premultiplied RGBA8**; the conversion the engine owns is documented in §5.4.

---

## 2. Divergences

**D1 — No progressive rendering.** `CPDF_ProgressiveRenderer`,
`ContinueSingleObject`, `CPDF_ImageRenderer::Continue`, the `Mode` state
machine and `PauseIndicatorIface` exist to let a UI interleave rendering with
input. `render_page` runs to completion. The `kStepLimit` budget, the
"forms and shadings force a pause check" rule
(cpdf_progressiverenderer.cpp:105-109) and `bBreakForMasks` control *when*
control returns, never what is produced. **One behavioural residue is kept**:
the progressive path's `<=`/`>=` cull test differs from `RenderObjectList`'s
strict one (§1.1); we keep the non-progressive spelling, which is the one the
oracle's own `--png` path exercises for every object after the first pause
boundary anyway. Consequence: none observable.

**D2 — `DrawObjWithBackground` and `stop_obj_` erased.** On a bitmap device
`DrawObjWithBackground` degenerates to re-invoking the identical render on the
identical device (§1.1), and `stop_obj_` exists only to feed `GetBackdrop`'s
Windows/opaque-page replay (§1.6 step 4). Our device always supports both
`GetBits` and `AlphaOutput`, so neither is reachable. A `Process*` failure
becomes a diagnostic and a skipped object.

**D3 — `FPDF_REVERSE_BYTE_ORDER` is an output encoding, not a render option.**
The C++ threads `rgb_byte_order_` into the driver's compositor, duplicating
every span function. We render into one canonical premultiplied RGBA8 `Pixmap`
and swap channels once at the boundary if asked. Byte-identical output; one
fewer axis in every backend.

**D4 — Windows-only machinery dropped wholesale.** `CPDF_ScaledRenderBuffer`
(and its `kImageSizeLimitBytes = 30 MiB` halving loop), `CFX_ImageTransformer`,
`StretchDIBits`, the `CPDF_DeviceBuffer` DPI cap, `HandleFilters`'s
`bLossy` flag, `SetBitMask`'s use in `CompositeDIBitmap`, and every
`DeviceType::kPrinter` branch. Inventoried in §1 at each branch point so a
reader can verify we take the other arm. No observable consequence for the
oracle's configuration; if a printer backend is ever wanted it is a new brief.

**D5 — Non-isolated groups reproduce PDFium's backdrop double-count.** The C++
seeds a non-isolated group's buffer with a *copy* of the page content
(§1.5 step 3) and never removes it before compositing the group back. That is
not what ISO 32000 §11.4.6 specifies (the backdrop should be removed after the
group is composited), and it makes a non-isolated group with a non-Normal blend
apply the backdrop twice. It is what the oracle does, parity wins, we port it.
Documented here so it is not "fixed" during a burn-down.

**D6 — One compositing path instead of five.** §1.6's tree has four arms
selected by device capabilities that are *constants* for us: with a
premultiplied RGBA8 target, `RenderCapAlphaOutput()` and `RenderCapGetBits()`
are both true, so arm 3's first branch (a plain blended blit) always fires and
arms 1(mask), 3(non-isolated-group clone) and 4(white-backdrop replay) are
unreachable. We implement: *normal blend → blit; other blend → blended blit;
non-isolated group → the backdrop was already copied into the buffer.* The one
place this could diverge is an **opaque** page bitmap with a non-Normal blend,
where the C++ takes arm 4 and composites over an explicitly white-cleared
`kBgrx` buffer. We reproduce that arithmetic by keeping the page's white
background as real opaque pixels — which is what `pdfium_test` requests
(`FPDFBitmap_FillRect(0xFFFFFFFF)`) — so the results coincide. Proof
obligation: the `transparent*`, `alpha_composite`, `composite-*`, `smask_blend`
clusters.

**D7 — Text is always outline fills; no glyph-bitmap pipeline.** The oracle
below the `|char2device.a| + |char2device.b| > 50` threshold rasterizes hinted
FreeType glyph bitmaps with LCD filtering, `AdjustGlyphSpace` integer-origin
nudging, and the `kTextGammaAdjust` table (§1.8). Reproducing that would mean
porting FreeType's hinter and autofitter — explicitly out of scope (PLAN §1,
Fontations, no FreeType). We render every glyph as a filled `BezPath` through
the glyph cache, at every size. **Consequence: text pixels are Tier B, never
Tier A**, and small text will differ in coverage on stem edges. The
*geometry* (glyph origins, advances, matrices) is unchanged and must match
exactly. `SPEC §8`'s "text renders as filled glyph BezPaths" already states
this; the brief records the size of the divergence.

**D8 — Glyph-origin rounding not reproduced.** With outline fills there are no
integer glyph origins to snap, so `AdjustGlyphSpace`'s ±1 nudge
(cfx_renderdevice.cpp:57-97) and the LCD `floor` vs `round` split for
`origin.x` (:1255-1258) have no analogue. Glyph positions stay in float device
space. Follows from D7.

**D9 — Type-3 blue snapping is kept but made deterministic.** `AdjustBlue`
(§1.11) mutates a per-`(font, size-key)` list as glyphs are rendered, so the
result depends on render order. We keep the algorithm exactly (it is pixel-
visible on the `bitmap-symbol-*` cluster) but scope the map to the render
session's `Type3Cache`, which is owned by the single-threaded per-page render.
Cross-page parallelism (`rayon` in the facade, PLAN §3) therefore cannot make
it non-deterministic. **No divergence in output; a divergence in ownership.**

**D10 — The AGG `HardClip` ±32000 coordinate clamp is kept.** It distorts
rather than clips geometry beyond ±32000 device units, which is observable on
pathological corpus files. Both our backends clip properly instead, so we
apply the clamp **in the engine**, before handing geometry to a backend, so
both backends agree and both match the oracle. (This is one of the few places
where matching the oracle means deliberately introducing an artefact.)

**D11 — Bezier flattening tolerance is the backend's, not AGG's.** AGG
flattens at a fixed `0.25` squared-distance tolerance that does not scale with
zoom (agg_curves.cpp:29-39); kurbo/vello_cpu/tiny-skia all flatten adaptively
in device space. Ours is *better* and the difference is sub-pixel, so it lands
in Tier B's AA budget. Recorded because a curve-heavy corpus file that
currently fails a tight SSIM threshold may be failing for this reason and not a
bug.

**D12 — `full_cover` is emulated, not native.** The Coons/tensor rasterizer
depends on `full_cover` to suppress seams between abutting patch cells
(§1.10). Neither backend exposes "write at constant alpha, ignore coverage".
§5.3 specifies the emulation (draw the patch cells into a scratch pixmap with
`BlendMode::Source` and no AA, then blit once) and §6/Q3 flags the residual
risk.

**D13 — Shading rasterizers are pure functions producing a `Pixmap`.** The C++
writes directly into a `CFX_DIBitmap` scanline (`GetWritableScanlineAs<u32>`)
and, for Coons, constructs a whole `CFX_RenderDevice` over that bitmap. We
keep the same *pixel* semantics but express types 1–5 as
`fn(…) -> Pixmap` with no device involvement, and type 6/7 as a small
`RasterBackend` target. Structural only; the per-pixel math is ported verbatim
including the truncating `as i32` casts and the `/256` LUT divisor.

**D14 — `IsAvailableMatrix`, `GetZeroAreaPath`, and the rect-snapping rule move
into the engine.** In the C++ they live in `cfx_renderdevice.cpp`, above the
driver. They are pure functions of path + matrix + colour, and both backends
must see the same result, so `pdfrum-render` applies them before calling
`RenderDevice`. Consequence: `RenderDevice` implementations stay dumb, and the
Tier-C cross-backend contract (§6) can require these to be *identical*.

**D16 — Blend arithmetic: the backends' native modes, not ported integer math.**
Every blend, alpha merge and clip intersection in the oracle is **truncating
integer** (`/255`, `/100`, `>>8`, `>>16`) with **no rounding anywhere** and no
output clamping; `SoftLight` uses a hand-tuned 256-entry `kColorSqrt` table
whose entries deviate by up to ±1 from any closed form; `GetAlphaWithSrc`
divides by 255 twice in sequence rather than once by 65025 (§1.15). Both our
backends composite in their own pipelines — tiny-skia in fixed-point `u16`
lanes, vello_cpu in `f32` — and neither is bit-identical to PDFium's.
**Decision:** use each backend's native blend modes for on-device compositing
and accept a ±1-per-channel difference, which is precisely what Tier B's
threshold and §6.2's rounding budget exist for. **But** the *engine's own*
arithmetic — the soft-mask luminosity readback, the `/Matte`
un-premultiplication, the shading colour LUTs, the Gouraud interpolation, the
Coons integer bilinear, the clip-mask intersection, and every `× alpha`
— is ported **verbatim as truncating integer**, because those are decisions,
not rasterization, and §6.1 requires them identical across backends. `blend.rs`
therefore exists for the engine's own offscreen compositing (e.g. merging a
type-3 glyph mask) even though the device path never calls it.
Consequence: a `Multiply` blend on the device may differ from the golden by ±1
per channel; a soft-mask luminosity value may not.

**D17 — `ConvertColorScale`'s `is_white_on_black` asymmetry is ported.**
(cfx_dibitmap.cpp:460-495.) At ≤8bpp it recolours the *palette* and honours the
flag (inverting when `false`); at >8bpp it rewrites pixels in place, **ignores
the flag entirely** (never inverts), and **leaves the alpha byte untouched**.
It also early-returns doing nothing for `is_white_on_black == false` on a
palette-less ≤8bpp bitmap. Its two callers pass `true` from the image path
(cpdf_imagerenderer.cpp:126) and `false` from the shading and tiling paths
(cpdf_rendershading.cpp:1114, cpdf_rendertiling.cpp:233), so **the `false`
callers get no inversion on a 32bpp buffer** — grayscale shadings and tiles are
grayscaled, not inverted, while grayscale images are grayscaled without
inversion too. The flag is effectively dead. Ported as written; recorded so the
`FPDF_GRAYSCALE` cluster is not "fixed" into symmetry.

**D15 — No global recursion counter.** `g_CurrentRecursionDepth` becomes a
`depth: u32` on the render context (STYLE §1). The cap stays 64. Consequence:
the C++'s counter is shared across *concurrent* renders in the same process and
ours is not — ours is strictly more correct and never less permissive.

---

## 3. Module plan

### 3.1 `pdfrum-render`

```
crates/pdfrum-render/src/
  lib.rs              // re-exports: RenderDevice, RasterBackend, Pixmap, Brush,
                      // AntiAlias, ImageQuality, AlphaMask, RenderOptions,
                      // render_page, Error

  options.rs          // RenderOptions, ColorMode { Normal, Gray, Alpha, Forced(ColorScheme) },
                      // TextAa, the FPDF-flag equivalence table (§1.2)
  device.rs           // the RenderDevice + RasterBackend traits (SPEC §8) and the
                      // vocabulary types they speak: Brush, Stroke, FillRule,
                      // AntiAlias, ImageQuality, AlphaMask, RasterImage, Pixmap
  ctx.rs              // RenderCtx: the small record that replaces CPDF_RenderStatus —
                      // options, depth, initial_color, type3 state, transparency flags,
                      // caches. Data only; every verb is a free function elsewhere.

  walk.rs             // render_page, render_layer, render_object_list,
                      // render_object: the dispatch table (§1.1), the cull test,
                      // the depth guard
  clip.rs             // ClipState: the logical clip stack, the empty-path rect(-1,-1,0,0)
                      // rule, text-clip accumulation and flush (§1.4)
  color.rs            // fill_argb / stroke_argb (§1.3), the 0xFFFFFFFF sentinel,
                      // TranslateColor / forced-colour scheme, FXRGB2GRAY
  transfer.rs         // TransferFunc: the 3x256 LUT build + translate_color (§1.3).
                      // (The page crate owns the /TR *parse*; the LUT build is a
                      //  render concept — it is cached per document by the renderer.)

  path.rs             // draw_path: IsAvailableMatrix, the rect-snapping fast path,
                      // the cosmetic-line case, zero-area detection, the fill+stroke
                      // knockout buffer (§1.7). All of D14 lives here.
  zero_area.rs        // GetZeroAreaPath: CheckSimpleLinePath, CheckPalindromicPath,
                      // the folding-vertex scan, the (int)c+0.5 snap, the >>2 alpha
  stroke.rs           // StrokeGeometry: the matrix1/matrix2 split, the 1-device-pixel
                      // minimum width, cap/join/miter mapping, the dash device-scale
                      // ladder + the 32-entry cap + odd-array cycle doubling (§1.7)

  text.rs             // render_text: the Tr mode table, the pattern diversion, the
                      // stroked-text CTM un-transform, the fallback-run split (§1.8)
  glyphs.rs           // GlyphCache: (font-id, gid) -> BezPath, owned by the session
  type3.rs            // render_type3_text: the char-proc vs bitmap split, the colour
                      // rule, the recursion set, the offscreen path for alpha<255 (§1.11)
  type3_cache.rs      // Type3Cache: SizeKey quantization, the blue-zone lists,
                      // the near-axis stretch heuristic, ink detection (§1.11)

  shading/
    mod.rs            // draw_shading: the common entry, /Background, /BBox,
                      // the kAlpha/kGray post-passes (§1.10)
    steps.rs          // the 256-entry LUT, component_to_shading_index
    axial.rs          // type 2 (§1.10)
    radial.rs         // type 3, incl. the root-selection ladder
    function.rs       // type 1
    gouraud.rs        // types 4/5: the scanline triangle rasterizer
    patch.rs          // types 6/7: CubicBezierPatch, subdivision, PatchDrawer,
                      // the integer bilinear colour interpolation
  pattern.rs          // draw_tiling_pattern / draw_shading_pattern: clip_pattern,
                      // the cell bitmap, the <16px enlargement, the screen buffer,
                      // the slow path, CloneObjStates (§1.9)

  image.rs            // draw_image: the StartRenderDIBBase decision tree, the
                      // CMYK->Darken rule, the kHugeImageSize bilinear forcing,
                      // GetDimensionsFromUnitRect's flip rule (§1.12)
  image_mask.rs       // draw_masked_image (incl. /Matte un-premultiply),
                      // draw_pattern_image, alpha-mode bitmaps (§1.12.1-.3)

  group.rs            // needs_offscreen (§1.5 step 2), render_group: the buffer,
                      // the isolated/non-isolated backdrop rule, the mask/alpha
                      // ordering, the composite-back call
  softmask.rs         // load_soft_mask: the luminosity/alpha buffers, the /BC
                      // backdrop clear, the /TR LUT, the gray/alpha readback (§1.13)
  composite.rs        // composite_layer: §1.6 collapsed to D6's single path;
                      // multiply_alpha, multiply_alpha_mask
  blend.rs            // the 12 separable formulas (§1.15) + the kColorSqrt table
                      // verbatim + Lum/Sat/SetLum/SetSat/ClipColor, all integer.
                      // Used by the ENGINE's own offscreen compositing; the
                      // backends' native blend modes cover the on-device case (D16).
  pixmap.rs           // Pixmap (premultiplied RGBA8), AlphaMask (8bpp), the
                      // straight<->premultiplied conversions the engine owns (§5.4)

  error.rs            // Error (thiserror)
```

**Types beyond SPEC §8:**

```rust
/// The render session's mutable context. A record, not an object: every field
/// is read by some free function, none of them by all.
pub(crate) struct RenderCtx<'a, B: RasterBackend> {
    pub opts: &'a RenderOptions,
    pub backend: &'a B,
    pub depth: u32,                       // replaces g_CurrentRecursionDepth (D15)
    pub initial_color: ColorPair,         // CPDF_RenderStatus::initial_states_ colour half
    pub type3: Option<Type3Frame>,        // char + imposed fill colour, when inside a proc
    pub type3_fonts: SmallVec<[FontId; 4]>, // the recursion SET, not a depth
    pub transparency: Transparency,       // group/isolated flags of the enclosing holder
    pub in_group: bool,
    pub std_cs: bool,
    pub load_mask: bool,
    pub group_family: Option<ColorSpaceFamily>,
    pub caches: &'a RenderCaches,
}

/// Session-scoped caches (STYLE §1: no globals; the owner passes them down).
pub struct RenderCaches {
    glyphs: GlyphCache,                   // (FontId, Gid) -> BezPath
    type3: Type3Cache,                    // (FontId, SizeKey) -> glyph bitmaps + blues
    transfer: TransferCache,              // ObjRef -> Arc<TransferFunc>
    images: ImageCache,                   // (ObjRef, RequestedSize) -> Arc<RasterImage>
}

pub struct Pixmap { w: u32, h: u32, data: Vec<u8> }   // premultiplied RGBA8
pub struct AlphaMask { w: u32, h: u32, data: Vec<u8> } // 8bpp coverage

pub enum Brush<'a> { Solid(peniko::Color), Image(&'a RasterImage) }
pub enum AntiAlias { On, Off }                        // = !fill_options.aliased_path
pub enum ImageQuality { Nearest, Bilinear }           // §1.12's resample selection
```

**Data flow.** `render_page(page, opts, backend)`:
```
build the page-space -> device-space matrix from opts.transform
create the target device via backend.new_target(w, h)
for each layer (page, then annotation appearances):
    render_object_list(ctx, device, holder, matrix)
        for each object, culled by the transformed clip box:
            apply_clip(device, obj.clip)                   // clip.rs
            if needs_offscreen(obj, ctx): render_group(..) // group.rs
            else: dispatch by PageObject variant
backend.finish(device) -> Pixmap
```
Every `render_*` is a free function taking `&RenderCtx` and `&mut dyn
RenderDevice`; the only `dyn` in the crate, as STYLE §2b permits.

### 3.2 `pdfrum-raster-vello-cpu`

```
crates/pdfrum-raster-vello-cpu/src/
  lib.rs        // VelloCpuBackend (RasterBackend), VelloCpuDevice (RenderDevice)
  convert.rs    // kurbo/peniko -> vello_cpu vocabulary; BlendMode mapping table
  layer.rs      // push_layer/pop over vello_cpu's own layer stack
```

### 3.3 `pdfrum-raster-tinyskia`

```
crates/pdfrum-raster-tinyskia/src/
  lib.rs        // TinySkiaBackend (RasterBackend), TinySkiaDevice (RenderDevice)
  convert.rs    // kurbo::BezPath -> tiny_skia::Path, Affine -> Transform,
                // peniko::BlendMode -> tiny_skia::BlendMode, FillRule, Stroke
  clip.rs       // the Mask stack: intersect_path, save/restore by cloning
  layer.rs      // offscreen Pixmap layers: push_layer allocates, pop composites
```
tiny-skia has neither layers nor a clip stack, so both are emulated here rather
than in the engine — the engine must not know which backend it has.

---

## 4. Validating SPEC §8's trait surfaces

SPEC §8 declares:
```rust
pub trait RenderDevice {
    fn fill_path(&mut self, path: &BezPath, t: Affine, brush: &Brush, rule: FillRule, aa: AntiAlias);
    fn stroke_path(&mut self, path: &BezPath, t: Affine, brush: &Brush, stroke: &Stroke, aa: AntiAlias);
    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32);
    fn push_clip(&mut self, path: &BezPath, rule: FillRule);
    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>);
    fn pop(&mut self);
}
pub trait RasterBackend { type Device: RenderDevice;
    fn new_target(&self, w: u32, h: u32) -> Self::Device;
    fn finish(&self, d: Self::Device) -> Pixmap; }
```

I inventoried every device call the engine makes (`DrawPath`, `SetClip_PathFill`,
`SetClip_PathStroke`, `SetClip_Rect`, `FillRect`, `SetDIBits`,
`SetDIBitsWithBlend`, `StretchDIBits*`, `SetBitMask`, `StretchBitMask*`,
`StartDIBits*`, `DrawNormalText`, `DrawTextPath`, `MultiplyAlpha`,
`MultiplyAlphaMask`, `GetDIBits`, `GetBackDrop`, `Clear`, `SaveState`,
`RestoreState`, `SetBaseClip`, `CreateCompatibleBitmap`) and mapped each onto
the six trait methods. Most map cleanly. The following **cannot be expressed**
and are escalations.

**E1 — `push_clip` cannot express a stroke-shaped clip.** `SelectClipPath`
(cpdf_renderstatus.cpp:611-626) calls `SetClip_PathStroke` when a pattern is
painted through a stroked path (`W`-less; the pattern's clip *is* the stroke
outline), and `SetClip_PathStroke` rasterizes the *stroke* into the clip mask
with `scale = 1, fill_rule = non_zero`. The trait has only
`push_clip(path, rule)`.
*Resolution proposed:* the engine converts the stroke to an outline itself and
pushes the outline as a winding clip. kurbo has `stroke()` (`kurbo::stroke`,
producing a filled `BezPath` from a stroke) which is exactly this. That keeps
the trait at six methods and makes both backends agree by construction (D14's
principle). **No spec change needed** if we accept that our stroke-outline
geometry differs from AGG's `conv_stroke` at joins by sub-pixel amounts — which
is already inside Tier B's budget. Recommend accepting; flagged so the
orchestrator can rule.

**E2 — `push_clip` cannot express a *rect* clip cheaply, and rect clips are
hard-edged in the oracle.** §1.4: `SetClip_PathFill` takes an
`IntersectRect` fast path on any axis-aligned rect, snapping to the **outer**
integer rect with **no antialiasing**, while a non-rect clip is an antialiased
8-bit mask. Pushing a rect as a `BezPath` through `push_clip` would give an
antialiased edge on both backends and diverge from the oracle on every
`re W n` — which is the single most common clip in the corpus.
*Resolution proposed:* add
```rust
fn push_clip_rect(&mut self, rect: Rect);   // integer-snapped, hard-edged
```
to `RenderDevice`. Both backends implement it trivially (tiny-skia: a
`Mask::fill_path` of the rect with `anti_alias = false`, or better, an
`IntRect`-based mask fill; vello_cpu: a non-AA rect clip). The engine detects
the rect case with the ported `CFX_Path::GetRect` + the outer-rect rule.
**This is a `[spec]` change to SPEC §8.** Corpus witness: `rectangles_clipped`.
Recommend adopting — the alternative (rasterizing rect clips as AA paths) will
cost a visible pixel on every clipped edge in a large fraction of the corpus.

**E3 — `push_layer` cannot express a non-isolated group's backdrop seeding.**
§1.5 step 3: a non-isolated group's offscreen buffer is initialised with a
*copy of the current device contents*, and the group's own drawing then blends
against it inside the buffer. `push_layer(blend, alpha, mask)` starts from
transparent.
*Resolution proposed:* keep the trait as is and give `RasterBackend` a
read-back, which the engine already needs for D5:
```rust
pub trait RasterBackend {
    type Device: RenderDevice;
    fn new_target(&self, w: u32, h: u32) -> Self::Device;
    fn finish(&self, d: Self::Device) -> Pixmap;
    /// Snapshot a sub-rectangle of a device's current pixels. Used for
    /// non-isolated transparency groups (ISO 32000 §11.4.6 backdrop).
    fn snapshot(&self, d: &Self::Device, rect: Rect) -> Pixmap;
    /// Seed a fresh target with existing pixels (the inverse of `snapshot`).
    fn new_target_with_backdrop(&self, w: u32, h: u32, backdrop: &Pixmap) -> Self::Device;
}
```
The engine then renders the group into its own target rather than into a
`push_layer` on the parent, and composites the result with `draw_image`. That
is exactly what the C++ does. **This is a `[spec]` change to SPEC §8.**
Recommend adopting; without it non-isolated groups are unimplementable and
`transparent.pdf` / `transparent1.pdf` cannot pass.

**E4 — the fill+stroke knockout buffer needs the same read-back.**
§1.7 step 4: `fill && fill_alpha && stroke_alpha < 0xff && opts.stroke` renders
into a bitmap whose **backdrop is a copy of the destination**, with knockout
on, so the stroke does not accumulate over the fill. E3's `snapshot` +
`new_target_with_backdrop` cover it; no additional method. Corpus witness:
`same_color_knockout_fill`.

**E5 — `draw_image` cannot express an image *mask* (stencil) with a fill
colour.** `SetBitMask(bitmap, left, top, argb)` paints a 1bpp/8bpp mask in a
single colour, and it is used for: `/ImageMask true` XObjects (via the
`dib.is_mask_format()` branch of §1.12), type-3 glyph bitmaps (§1.11), and the
`kAlpha`-mode bitmaps (§1.12.3). Expressing it as `draw_image` of a synthesized
RGBA image works but forces the engine to materialize a full 4-channel buffer
for what is an 8-bit coverage blit.
*Resolution proposed:* no spec change. The engine synthesizes
`RasterImage { premultiplied RGBA where rgb = colour·cov, a = cov }` and calls
`draw_image`. Premultiplication makes this exactly correct for a `SourceOver`
blend, and image masks in the corpus are small. Recorded as accepted cost.

**E6 — `full_cover` has no expression.** §1.10's Coons rasterizer and D12.
*Resolution proposed:* no spec change; emulate in the engine (§5.3).

**E7 — `AntiAlias` is per-call in the trait but the oracle's AA decision is
per-*primitive-kind*.** `aliased_path` comes from `bNoPathSmooth` /
`bNoTextSmooth`, but the oracle *additionally* forces hard edges for
axis-aligned rect fills (§1.7 step 2) and rect clips (§1.4) regardless of the
option, and forces `full_cover` for patch cells. The trait's per-call
`AntiAlias` is sufficient for the first two once E2 exists; the third is E6.
**No spec change.**

**E8 — `push_layer`'s `mask: Option<&AlphaMask>` needs a defined origin.** The
soft mask produced by §1.13 is sized to the *clip rect*, not to the layer, and
positioned at `(clip.left, clip.top)` in device space. The signature carries no
offset. *Resolution proposed:* define `AlphaMask` as device-sized-and-aligned
(the engine pads/crops), which is what `MultiplyAlphaMask` effectively assumes
(`cfx_renderdevice.cpp:1631-1642` requires identical dimensions, as does
tiny-skia's `apply_mask`). **No spec change; a documented invariant.**

**E9 — `RasterBackend::finish` consumes the device, but the engine needs the
pixels of a *live* device** for E3/E4. Covered by E3's `snapshot`.

**E10 — no `clear`.** The page background (white or transparent, §1.2) and the
soft-mask backdrop clear (§1.13, `/BC`) both need it. Expressible as
`fill_path` of the full-device rect with `BlendMode::Source`… except the trait
has no blend mode on `fill_path`. *Resolution proposed:* have
`new_target(w, h)` take a clear colour:
`fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device`.
**This is a `[spec]` change to SPEC §8**, small and unambiguous. Recommend
adopting.

**Summary of proposed `[spec]` changes to SPEC §8** (E2, E3, E10):
```rust
pub trait RenderDevice {
    fn fill_path(&mut self, path: &BezPath, t: Affine, brush: &Brush, rule: FillRule, aa: AntiAlias);
    fn stroke_path(&mut self, path: &BezPath, t: Affine, brush: &Brush, stroke: &Stroke, aa: AntiAlias);
    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32);
    fn push_clip(&mut self, path: &BezPath, rule: FillRule);
    fn push_clip_rect(&mut self, rect: Rect);                     // E2 (hard-edged)
    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>);
    fn pop(&mut self);
}
pub trait RasterBackend {
    type Device: RenderDevice;
    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device;   // E10
    fn new_target_with_backdrop(&self, w: u32, h: u32, backdrop: &Pixmap) -> Self::Device; // E3
    fn snapshot(&self, d: &Self::Device, rect: Rect) -> Pixmap;   // E3/E4
    fn finish(&self, d: Self::Device) -> Pixmap;
}
```
Everything else in §1 maps onto the existing six methods. The trait stays
object-safe, stays small, and gains nothing PDF-specific.

---

## 5. Backend capability mapping and gaps

**Method note.** `tiny-skia 0.12.0` was read directly from the vendored source
at `~/.cargo/registry/src/*/tiny-skia-0.12.0` and `tiny-skia-path-0.12.0`;
every claim below carries a file reference. **`vello_cpu 0.2.0` and `peniko`
are not present in the local registry and the network is unavailable in this
environment**, so the vello_cpu column is reasoned from the crate's documented
API surface and must be **verified against the real crate before implementation
begins** — that verification is **Q4**, and it is the highest-risk open item in
this brief.

### 5.1 Capability matrix

| Engine need (§) | tiny-skia 0.12 | vello_cpu 0.2 | verdict |
|---|---|---|---|
| Fill a path, winding/even-odd, AA on/off (§1.7) | `Pixmap::fill_path(path, &Paint, FillRule, Transform, Option<&Mask>)`, `Paint::anti_alias` (painter.rs:217) | `RenderContext::fill_path` + `set_fill_rule`/`set_anti_alias` | **exact** |
| Stroke a path with cap/join/miter/dash (§1.7) | `Pixmap::stroke_path(path, &Paint, &Stroke, Transform, Option<&Mask>)` (painter.rs:330); `Stroke { width, miter_limit, line_cap, line_join, dash }` | `RenderContext::stroke_path` + `set_stroke(Stroke)` (kurbo `Stroke`) | **exact**, with the dash caveat below |
| Solid colour brush | `Paint::set_color(Color)` | `set_paint(peniko::Color)` | **exact** |
| Image/pattern brush, nearest vs bilinear (§1.12) | `shaders::Pattern::new(pixmap, SpreadMode, FilterQuality, opacity, Transform)`; `FilterQuality { Nearest, Bilinear, Bicubic }` | `set_paint(peniko::Image)` with a quality/extend on the image | **exact** |
| Clip by a path, AA, intersecting (§1.4) | `Mask::intersect_path(path, FillRule, anti_alias, Transform)` (mask.rs:352); the intersection is literally `premultiply_u8(a, b)` = `a·b/255` — **identical to `CFX_AggClipRgn`** | native clip layers (`push_clip_layer`/`pop_layer`) | **exact** on tiny-skia; vello native |
| Clip by an integer rect, **hard-edged** (E2) | `Mask::fill_path(rect_path, Winding, anti_alias=false, …)`; or build the mask by hand | a non-AA rect clip layer | **exact** |
| Layer with blend + alpha + soft mask (§1.5, §1.6) | **absent** — must be emulated with an offscreen `Pixmap` + `draw_pixmap` (§5.2) | `push_layer(clip, blend_mode, opacity, mask)` — **native** | **gap on tiny-skia** |
| The 16 PDF blend modes (§1.6) | `BlendMode` has all 12 separable + `Hue`/`Saturation`/`Color`/`Luminosity` (blend_mode.rs:5-60) | `peniko::Mix` has the same 16 | **exact on both** |
| Blend-mode *on a blit* | `PixmapPaint::blend_mode` via `draw_pixmap` | `push_layer` + `draw_image` | **exact** |
| Multiply a target's alpha by a scalar (§1.5) | none; iterate the `Vec<u8>` (`Pixmap::pixels_mut`) | `push_layer(opacity)` covers the common case | **engine-side** |
| Multiply a target's alpha by a mask (§1.5, §1.13) | `Pixmap::apply_mask(&Mask)` (painter.rs:518) — exactly `DestinationIn` with an 8-bit mask; requires identical size (E8) | `push_layer(mask)` | **exact on both** |
| Read back pixels (E3, E4) | `Pixmap::data()` / `PixmapRef` — trivially | `RenderContext::render_to_pixmap` then read; a *mid-render* snapshot may need a flush | **verify (Q4)** |
| Seed a target with existing pixels (E3) | `Pixmap::from_vec` / `draw_pixmap` with `BlendMode::Source` | `draw_image` at identity with `Src` | **exact** |
| Write at constant alpha ignoring coverage (`full_cover`, E6/D12) | **absent** | **absent** | **gap on both** — §5.3 |
| Premultiplied RGBA8 output | `Pixmap` is premultiplied RGBA8 (`PremultipliedColorU8`) | `Pixmap` is premultiplied RGBA8 | **exact** |
| Determinism across runs | single-threaded, integer/`f32` pipelines, no SIMD dispatch that changes results | **multithreaded and SIMD-dispatched** — results must be verified stable across CPU feature levels | **verify (Q4)** |

### 5.2 tiny-skia: emulating layers

`Pixmap` has no layer stack. `push_layer(blend, alpha, mask)` becomes:
```
allocate a Pixmap of the current target's size (or of the layer's device bbox)
push it on a Vec<LayerFrame> along with (blend, alpha, mask, the clip Mask in force)
all subsequent draws target the top frame
pop():
    let frame = stack.pop()
    if alpha != 1.0 { multiply the frame's premultiplied RGBA by alpha }
    if let Some(m) = mask { frame.apply_mask(m) }
    parent.draw_pixmap(0, 0, frame.as_ref(),
                       &PixmapPaint { blend_mode: blend, opacity: 1.0,
                                      quality: Nearest },
                       Transform::identity(), current_clip_mask)
```
Cost: one full-size allocation per layer. The C++ pays the same cost (§1.5
allocates a `w × h` bitmap per group), so this is not a regression. The clip
`Mask` must be **cloned into the frame** because tiny-skia takes it per draw
call, not as state.

`Mask` is also the clip stack: `push_clip` clones the current mask and
`intersect_path`es into the clone; `pop` restores the previous. A `Vec<Mask>`
with copy-on-write (`Arc<Mask>` for the unmodified case) keeps this cheap for
the common "many objects, one clip" pattern.

### 5.3 The `full_cover` gap (D12, E6)

The Coons/tensor rasterizer subdivides a patch into cells and fills each cell
with a flat colour; abutting cells share an edge. With ordinary source-over AA,
each cell writes ~50% coverage on the shared edge and the two writes compose to
~75%, leaving a visible lighter seam. AGG's `full_cover` writes both at 100%.

Emulation, in `shading/patch.rs`:
1. Allocate one scratch `Pixmap` for the patch's device bbox via
   `RasterBackend::new_target(w, h, TRANSPARENT)`.
2. Draw every cell into it with **anti-aliasing off** (`AntiAlias::Off`). A
   hard-edged fill writes each pixel at full coverage exactly once, which is
   what `full_cover` achieves, and the cells tile the patch exactly.
3. `draw_image` the scratch onto the target with the shading's alpha.
The patch's *outer* boundary then has a hard edge rather than an antialiased
one — a 1-pixel difference along the silhouette of the whole shading, not at
every internal cell edge. That is strictly closer to the oracle than the
alternative. **Q3** records the residual risk and names
`2_shading_type_6_00`, `2_shading_type_6_001`, `shade-tensor` as the test.

### 5.4 Premultiplication — what the engine owns

The oracle's buffers are **straight** (non-premultiplied) BGRA or BGR; both our
backends are **premultiplied** RGBA8. The engine owns exactly these
conversions, all in `pixmap.rs`:

| Operation | oracle (straight) | ours (premultiplied) |
|---|---|---|
| source-over blend | §1.14's `dest_a` + `alpha_ratio` + three `AlphaMerge`s | the backend's native `SourceOver`; algebraically the same result, different rounding |
| `MultiplyAlpha(f)` | scale the alpha byte only | scale **all four** channels |
| `MultiplyAlphaMask(m)` | scale the alpha byte by `m/255` | scale all four by `m/255` (= tiny-skia's `apply_mask`) |
| soft-mask luminosity readback (§1.13) | `FXRGB2GRAY(r, g, b)` on a `kBgr` (opaque) buffer | the buffer is opaque, so premultiplied == straight; the same formula applies unchanged |
| soft-mask alpha readback (§1.13) | read the 8bpp mask directly | read the alpha channel |
| `/Matte` un-premultiply (§1.12.1) | integer, against the matte colour, on a straight buffer | **must un-premultiply first**, apply the matte formula, re-premultiply |
| final output | BGRA or BGR, straight | un-premultiply to straight RGBA/BGRA at `finish` if the caller wants straight |

The rounding difference in the first row is the main source of ±1 channel
diffs against the goldens and is exactly what Tier B's threshold exists to
absorb. `pdfium_test --render-premultiplied-alpha` (pdfium_test.cc:587) exists
and selects `FPDFBitmap_BGRA_Premul`; the golden store is generated **without**
it, so our `finish` must un-premultiply for conformance output.

### 5.5 Genuine gaps, ranked

1. **`full_cover`** — both backends. Emulated (§5.3). Residual risk: the patch
   silhouette. **Q3.**
2. **tiny-skia has no layers** — fully emulable at a known cost (§5.2). Not a
   blocker.
3. **vello_cpu API unverified** — the entire vello column above is inference.
   **Q4**, must be closed before M3 code.
4. **vello_cpu determinism** — it is multithreaded and SIMD-dispatched. Tier C
   compares vello against tiny-skia; if vello is not bit-stable across CPU
   feature levels, the Tier-C baseline must be tiny-skia-vs-tiny-skia for
   regression and vello-vs-tiny-skia only for the §6 contract. **Q4.**
5. **Dash arrays** — tiny-skia's `StrokeDash::new` rejects odd-length arrays,
   negative entries, and a zero total (dash.rs:48-60). PDF permits all three
   and §1.7's ladder gives them meaning (odd → doubled cycle, negative → `0.1`,
   zero total → solid). The engine must normalize to an even-length,
   all-positive array **before** calling either backend: duplicate an odd array
   to double its length (which is exactly what the doubled cycle means), and
   apply the `<= 0.000001 → 0.1` substitution and the 32-entry truncation. Then
   both backends see a legal array. Not a gap once normalization is engine-side
   (which D6-of-the-page-brief already decided).
6. **Stroke geometry at joins** — AGG's `miter_join_revert` bevels on limit
   exceedance. kurbo's `Join::Miter` and tiny-skia's `LineJoin::Miter` both
   **bevel** on limit exceedance (Skia semantics), so this matches. `MiterClip`
   would not; do not use it.
7. **Hairlines** — tiny-skia treats `width == 0` as a hairline
   (stroker.rs:44-46) and vello likewise. The engine computes the
   1-device-pixel minimum itself (§1.7, D14) and never passes 0, so the
   backends' own hairline paths are dead code for us. Deliberate: it keeps the
   two backends identical here.

---

## 6. Tier-C cross-backend comparison contract

PLAN §5 defines Tier C as "vello_cpu vs tiny-skia output of *our own* engine;
large divergence = backend bug, not engine bug", with an M3 exit criterion of
**Tier-C divergence < 1%**. This section makes "divergence" precise, because
without it the metric is unfalsifiable.

**The principle.** Everything that is a *decision* belongs to the engine and
must be bit-identical across backends. Everything that is *edge coverage
arithmetic* belongs to the rasterizer and may differ within a bounded envelope.

### 6.1 MUST be identical (a difference is an engine bug)

| Property | Why it is the engine's |
|---|---|
| **Geometry.** The set of `fill_path`/`stroke_path`/`draw_image`/`push_clip` calls, their `BezPath` contents, and their `Affine`s. | D14: `IsAvailableMatrix`, `GetZeroAreaPath`, the rect-snapping rule, the matrix1/matrix2 stroke split, the `HardClip` ±32000 clamp (D10), stroke-to-outline for clips (E1), and dash normalization (§5.5) all run in the engine. |
| **Interior pixel colour**, to within the ±1 (±3 for non-separable blends) rounding budget of §6.2. Any pixel strictly inside a filled region, ≥1px from every edge of every primitive that touched it. | Colour resolution (§1.3), the transfer-function LUT, colour-space conversion, the shading LUT and every per-pixel shading formula (§1.10) are all engine code. Backends only *composite*, and only compositing may round differently (D16). |
| **Alpha at interior pixels**, same budget. | Same. |
| **Every value the engine computes itself**, exactly: shading LUT entries, Gouraud vertex/scanline interpolation, Coons integer bilinear colours, soft-mask luminosity and `/TR` LUT output, `/Matte` un-premultiplication, clip-mask intersection products, `× alpha` results. **Zero tolerance.** | These are the truncating-integer ports of D16's "decisions" half; they run once, in the engine, before either backend sees them. |
| **Object visibility.** Whether an object was drawn at all: the cull test, the OC predicate, `IsAvailableMatrix`, the empty-rect early-outs, the `0xFFFFFFFF → alpha 0` sentinel. | All engine. |
| **Layer structure.** The number, order, extent, blend mode, alpha and mask of every group and soft mask. | §1.5/§1.6/§1.13 are engine. |
| **Image sample values.** Decoded image pixels before resampling. | `pdfrum-page`. |
| **Shading pixels for types 1–5.** These are computed into a `Pixmap` by pure engine code (D13) and blitted; the backends never rasterize them. | By construction. |
| **The Coons/tensor cell decomposition.** Which cells exist and what colour each is. | Engine (§1.10). Only the *fill* of each cell is the backend's. |

**Enforcement:** the engine emits an optional call trace
(`RenderOptions::trace: Option<&mut Vec<DeviceCall>>`, a debug-only sink) and
the Tier-C harness asserts the two backends received **byte-identical traces**
before it ever compares pixels. A trace mismatch is an engine bug, reported as
such, and pixel comparison is skipped. This is cheap and turns most Tier-C
failures into precise diagnoses.

### 6.2 MAY differ (a difference is not a bug, within budget)

| Property | Budget |
|---|---|
| **Anti-aliased edge coverage.** A pixel whose centre is within 1px of a primitive edge. | Any value; the two backends use different coverage integration (tiny-skia: Skia's supersampled scan converter; vello_cpu: sparse strips with analytic area). |
| **Anti-aliased clip edges.** Pixels within 1px of a non-rect clip boundary. | Same. |
| **Curve flattening.** Sub-pixel differences in where a flattened bezier lands. | Bounded by each backend's flattening tolerance; both are ≤ 0.25px. |
| **Stroke join/cap outlines.** Sub-pixel differences in the outline of a round join or cap. | ≤ 1px in the outline position. |
| **Image resampling.** Bilinear filter kernels and edge handling differ (tiny-skia clamps via `SpreadMode::Pad`). | Pixels within 1px of the image's device-space boundary. |
| **Rounding of the composite.** ±1 per channel from premultiplied-integer rounding and from each backend's own blend pipeline (D16: tiny-skia fixed-point `u16`, vello_cpu `f32`, PDFium truncating integer). | ±1, everywhere. |
| **Non-separable blend modes** (`Hue`/`Saturation`/`Color`/`Luminosity`). Their `ClipColor` clamping amplifies rounding: the two backends may differ by more than ±1 where a channel is driven to the 0 or 255 boundary. | ±3 per channel, and only on pixels where a non-separable blend was in force. |

### 6.3 The metric

For each corpus file, render with both backends and compute:
```
edge_mask   = dilate(union of all primitive edges from the call trace, 1px)
interior    = !edge_mask
tol(px)     = if a non-separable blend was in force at px { 3 } else { 1 }   // §6.2
hard_fail   = any pixel in `interior` differing by > tol(px) in any channel
soft_diff   = fraction of pixels in `edge_mask` differing by > 8 in any channel
tierC_score = if hard_fail { FAIL } else { soft_diff }
```
- **`hard_fail` is the real gate.** A single interior pixel differing by more
  than its rounding allowance is a defect: an engine bug if the traces
  differed, a backend bug if they matched.
- The engine-computed values in §6.1's third row are compared **separately and
  exactly**, not through the pixel metric: the harness diffs the two backends'
  device-call traces (which carry the shading `Pixmap`s, the `AlphaMask`s and
  every solid `Brush` colour as data), so a divergence there is caught before
  any rasterization difference can mask it.
- **`soft_diff < 1%` of *edge* pixels** is PLAN §5's "< 1% divergence",
  now measured against the right denominator.
- The dilation radius is 1px because every listed soft difference is bounded by
  1px of geometric displacement. A backend needing more than that is a backend
  bug.

Deriving `edge_mask` from the call trace (rather than by diffing) is what makes
this contract non-circular: it is computed from the *inputs*, so it cannot
absorb a real disagreement by declaring it an edge.

### 6.4 Tier-C-specific test corpus

Beyond the whole corpus, these clusters are the ones where the contract
actually bites and where a Tier-C-only fixture set is worth maintaining:
`shading_2_*` and `2_shading_type*` (edge-dense), `2_shading_type_6_*` and
`shade-tensor` (the `full_cover` emulation, §5.3), `dashed_*` and `7_dashed`
(join/cap outlines), `single_point_paths` (the hairline/thin-line rules —
these must be **identical**, since D14 puts them in the engine), `type3` and
`bitmap-symbol-*` (blue snapping, must be identical),
`rectangles_clipped` (E2's hard-edged rect clip — must be identical),
`transparent*` and `alpha_composite` (layer emulation on tiny-skia vs native on
vello — the composite must be identical at interior pixels).

---

## 7. Test plan

### 7.1 Ported C++ unittest assertions

| C++ test | file:line | ports to |
|---|---|---|
| `CPDFDocRenderDataTest.TransferFunctionOne` | cpdf_docrenderdata_unittest.cpp:172 | `transfer::tests::type2_lut` — the 256 sample bytes and all ten `TranslateColor` pairs |
| `CPDFDocRenderDataTest.TransferFunctionArray` | :197 | `transfer::tests::array_lut` — **the R/G/B ordering authority for Q1** |
| `CPDFDocRenderDataTest.BadTransferFunctions` | :226 | `transfer::tests::bad_tr_drops_whole_function` (3 cases) |
| `Blend.{Normal,Multiply,Screen,Overlay,Darken,Lighten,ColorDodge,ColorBurn,HardLight,SoftLight,Difference,Exclusion}` | core/fxge/dib/blend_unittest.cpp:12-170 | `composite::tests::blend_*` — 12 tests, per-channel integer expectations |
| `ScanlineCompositorTest.CompositeRgbBitmapLineBgra{Normal,Screen,Darken,SoftLight,Hue,Saturation,Color,Luminosity}` | cfx_scanlinecompositor_unittest.cpp:117-468 | `composite::tests::span_*` — the non-separable four are the ones worth having; they pin `Lum`/`SetLum`/`Sat`/`SetSat`/`ClipColor` |
| `ScanlineCompositorTest.…BgraPremul*` | :470-858 | the premultiplied variants — **directly applicable**, since our buffers are premultiplied |
| `CFXPath.{BasicTest,ShearTransform,Hexagon,ClosePath,FivePointRect,SixPlusPointRect,NotRect,EmptyRect,Append,GetBoundingBoxForStrokePath}` | cfx_path_unittest.cpp:10-410 | `path::tests::*` — `FivePointRect`/`SixPlusPointRect`/`NotRect`/`EmptyRect` pin `GetRect`'s normalization, which drives §1.7's rect fast path |
| `CFXRenderDeviceTest.{GetClipBoxDefault,GetClipBoxPathFill,GetClipBoxPathStroke,GetClipBoxRect,GetClipBoxEmpty}` | cfx_renderdevice_unittest.cpp:22-88 | `clip::tests::clip_box_*` |
| `CFXDIBitmapTest.{UnPreMultiplyFromPreMultiplied,UnPreMultiplyFromUnPreMultiplied,PreMultiplyFromUnPreMultiplied,PreMultiplyFromPreMultiplied}` | cfx_dibitmap_unittest.cpp:137-181 | `pixmap::tests::premul_roundtrip` — §5.4's conversions |
| `CStretchEngine.{WeightRounding,WeightRoundingNoSmoothing,WeightRoundingBilinear,WeightRoundingNoSmoothingBilinear,ZeroLengthSrc*,EmptySourceRange*,ZeroLengthDest,TooManyWeights,MirroredDestinationEmptySourceClip,EmptySourceClipAtRightEdge}` | cstretchengine_unittest.cpp:107-280 | `image::tests::resample_*` — the resampling-selection and degenerate-size assertions; the *kernel* is the backend's, the *selection and degenerate handling* is ours |
| `FPDFRenderPatternEmbeddertest.LoadError_555` | fpdf_render_pattern_embeddertest.cpp | conformance fixture, not a unit test |
| `FPDFProgressiveRenderEmbeddertest.*` | fpdf_progressive_render_embeddertest.cpp | **not ported** (D1), except the *final images*, which the conformance harness already covers |

### 7.2 Hand-written tests pinning heuristics with no C++ unittest

These are behaviours §1 transcribes that upstream has no unit test for; each
gets a Rust test built from a synthetic input.

1. `path::tests::rect_snap_*` — §1.7 step 2's full ladder: sub-1px width/height
   promotion, the `>= width + 1` shrink, the tie going to right/bottom, the
   checked-overflow rejections.
2. `zero_area::tests::simple_line_snap` — `(int)c + 0.5`, including negative
   coordinates (truncation toward zero, **not** floor).
3. `zero_area::tests::palindromic` — odd counts only, bezier rejection.
4. `zero_area::tests::folding_{vertical,horizontal,diagonal}` — the exact
   predicates, and that vertical picks by y-distance while the others pick by x.
5. `zero_area::tests::thin_alpha_quarter` — `alpha >> 2`.
6. `stroke::tests::min_width_one_device_pixel` — `line_width = 0` and
   `line_width = 0.01` both yield 1 device px, under identity, scale-2, and
   anisotropic matrices.
7. `stroke::tests::matrix_split` — `matrix1.a = max(|a|,|b|)` and
   `matrix1 * matrix2 == m` for a rotation, a shear, and a flip.
8. `stroke::tests::dash_{tiny_cycle_solid,nonfinite_solid,zero_entry_becomes_0_1,
   odd_array_doubles_cycle,over_32_entries_truncated,negative_phase}` — §1.7.
9. `color::tests::sentinel_white_is_transparent` — `0xFFFFFFFF → ARGB 0`.
10. `color::tests::alpha_truncates` — `ca 0.5 → 127`.
11. `shading::tests::lut_divisor_256` — the LUT's last entry is *not* `f(t_max)`.
12. `shading::tests::axial_index_truncates` and `..::extend_skips`.
13. `shading::tests::radial_root_selection` — a table over
    `(a sign, decreasing, extends)` covering all eight combinations, plus the
    `b == 0` and `a == 0` branches, plus the `r0 + s·dr < 0` skip being absent
    from those two branches.
14. `shading::tests::gouraud_{increment_before_use,two_intersections_only,
    scanline_inclusive}`.
15. `shading::tests::patch_{is_small,color_threshold,subdivision_axis_choice,
    integer_interpolate_overflow}`.
16. `pattern::tests::tile_under_16px_renders_8x8`.
17. `pattern::tests::single_pixel_tile_writes_u32`.
18. `type3::tests::{colour_rule_colored_vs_uncolored,recursion_set_not_depth,
    translucent_uses_offscreen,force_halftone_and_rect_aa}`.
19. `type3_cache::tests::{size_key_quantization,blue_snap_within_0_8,
    blue_list_caps_at_16,near_axis_stretch_condition,ink_threshold_0x40}`.
20. `image::tests::cmyk_overprint_becomes_darken` — the full seven-clause gate.
21. `image::tests::huge_image_forces_bilinear`.
22. `image::tests::dimensions_flip_rule` — `a < 0` and `d > 0`.
23. `image_mask::tests::matte_unpremultiply` — the integer formula and the
    `mask == 0` skip.
24. `image_mask::tests::masked_image_drawn_on_white`.
25. `group::tests::needs_offscreen_predicate` — all six clauses, each
    independently flipped.
26. `group::tests::{isolated_starts_transparent,non_isolated_copies_backdrop,
    mask_then_group_alpha_then_initial_alpha_order}`.
27. `softmask::tests::{luminosity_default,alpha_only_on_exact_string,
    bc_clears_backdrop,tr_lut_output_zero_only}`.
28. `walk::tests::{cull_strict_inequality,depth_cap_64,
    shading_failure_not_retried}`.
29. `clip::tests::{empty_path_is_offscreen_rect,text_clip_flush_on_null,
    restore_keeps_one_level}`.
30. `composite::tests::normal_blend_is_plain_blit` — D6's collapse.
31. `blend::tests::overlay_is_hardlight_swapped` and
    `blend::tests::hardlight_threshold_128` — §1.15's two re-derivation traps.
32. `blend::tests::color_sqrt_is_piecewise_d_not_sqrt` — assert all 256 entries
    equal `round(255·D(i/255))` for the piecewise `D`, **and** that `[1] == 3`
    rather than `16`, so a "simplification" to a plain square root fails loudly.
33. `blend::tests::softlight_divides_twice` — a case where `/255/255` and
    `/65025` differ.
34. `blend::tests::clip_color_ordering` — a colour that trips both the `n < 0`
    and `x > 255` branches, pinning that `l` and `x` come from the pre-clip
    colour while the channels do not.
35. `blend::tests::nonseparable_on_gray_collapses` — Luminosity takes the
    source, the other three keep the backdrop.
36. `composite::tests::transparent_backdrop_skips_blend` — `dest.a == 0`
    copies the source verbatim regardless of blend mode.
37. `composite::tests::multiply_alpha_truncates` — `0.5 → 127`.
38. `image::tests::use_interpolate_bilinear_heuristic` — a table over
    `(src_w, src_h, dest_w, dest_h)` straddling the `/8` boundary, including
    the integer-truncation edges and `dest_width == 0`.
39. `image::tests::resample_options_replaced_not_merged` — `bHalftone` is
    dropped when `bNoSmoothing` or the heuristic fires (§1.16).
40. `image::tests::stretched_format_promotion` — the five-way
    `GetStretchedFormat` table, plus the 1bpp palette lerp to 256 entries.
41. `pixmap::tests::convert_color_scale_asymmetry` — D17: the flag is honoured
    at ≤8bpp and ignored at >8bpp, and alpha survives.

### 7.3 Snapshot tests (`insta`)

- `insta` snapshots of the **device call trace** (§6.1) for a dozen hand-written
  content streams covering: a filled rect, a clipped rect, a dashed stroke, a
  glyph run, a type-3 run, an axial shading, a tiling pattern, an isolated
  group, a non-isolated group, a soft-masked image, an image mask with a
  pattern, and a Coons patch. These pin the *engine's decisions* independently
  of any rasterizer and are the fastest signal on a regression.
- Snapshots of `TransferFunc` LUTs and shading LUTs (256 bytes each).

### 7.4 Fuzz targets

`pdfrum-render` consumes a `Page`, not bytes, so its fuzz surface is the
composed pipeline. One target: `fuzz_render` = `load → page(0) → build_page →
render_page` at a fixed small size against both backends, asserting no panic
and no unbounded allocation. Seeded from the corpus and from
`pdfium-c++/testing/fuzzers/`'s render corpora. The specific hazards it must
cover, all named in §1: the Coons subdivision's missing depth limit (Q2), the
`matrix1.a` division by zero, the radial `sqrt` of a negative, the tile-count
ladder, the `±32000` clamp, and the recursion cap.

### 7.5 Conformance clusters

Named so `--triage` can group them:

| Cluster | Corpus / resource files |
|---|---|
| `render-path` | `rectangles_*`, `single_point_paths`, `dashed_lines`, `long_dashed_line`, `dashed_line_negative_scale`, `7_dashed`, `annotation_*_dash` |
| `render-clip` | `rectangles_clipped`, `clipping_text` |
| `render-text` | the whole text-bearing corpus; expected Tier-B-only (D7) |
| `render-type3` | `type3`, `type3_xobject`, `2_color_type3_pattern_bbox`, `bitmap-symbol-*` |
| `shading-axial` | `shading`, `shading1`, `shading2`, `shading_2_*`, `axial_shading_point_at_border_no_extend` |
| `shading-radial` | `radial_shading_point_at_center`, `radial_shading_point_at_border`, `radial_shading_point_at_border_no_extend`, `2_shading_type3` |
| `shading-function` | `2_shading_type1`, `2_shading_type1_sc_` |
| `shading-mesh` | `2_shading_type4_h`, `2_shading_type5_h` |
| `shading-patch` | `2_shading_type_6_00`, `2_shading_type_6_001`, `shade`, `shade-tensor` |
| `pattern-tiling` | `FRC_4.5.5_Pattern_tiling`, `path_5_pattern` |
| `pattern-shading` | `FRC_4.5.5_Pattern_shading` |
| `transparency` | `transparent`, `transparent1`, `alpha_composite`, `composite-and-xnor`, `composite-or-xor-replace`, `same_color_knockout_fill` |
| `softmask` | `smask_blend`, `matte`, `SMaskInData2/*` |
| `image-render` | `jpxdecode*`, `image_transformer_other`, `lzw1` |
| `color-render` | `FRC_4.5.3_DeviceCMYK_k`, `FRC_4.5.3_DeviceCMYK_K1`, `color_separation`, `FRC_4.5.4_Separation`, `icc_profile_bad_*` |
| `transfer-function` | `transfer_function` |

Upstream `SUPPRESSIONS` marks `path_5_pattern`, `transparent`, `transparent1`,
`long_dashed_line`, `smask_blend`, `2_shading_type_6_00`, `2_shading_type_6_001`,
`FRC_4.5.5_Pattern_shading`, `FRC_4.5.5_Pattern_tiling` and
`annotation_square_fill_opacity_dash` as platform-divergent (mac/gdi). Our
oracle is Linux/AGG, so all of them **do** have valid goldens for us; the
suppression list is a hint that these are the fragile ones, not a licence to
skip them.

---

## 8. Open questions

**Q1 — the transfer-function array reversal.**
`CreateTransferFunc` loads `pFuncs[2 - i] = Load(array[i])`
(cpdf_docrenderdata.cpp:89) and then fills `samples[i]` from `pFuncs[i]`
(:118-129), where `samples = {samples_r, samples_g, samples_b}` (:113-114). Read
literally that maps `array[2] → R`. But `CPDFDocRenderDataTest.TransferFunctionArray`
(:197-225) asserts `GetSamplesR() == kExpectedType0FunctionSamples` for the
array `[Type0, Type2, Type4]`, i.e. `array[0] → R`, and the `TranslateColor`
expectations are consistent with that reading. Either I have misread one of the
two, or `kExpectedType*FunctionSamples` are named by array position rather than
by function type. **Resolution:** implement whatever makes the ported unittest
pass, and record the finding in a code comment. Low risk (the unittest is
unambiguous about the observable), but it must be settled *before* someone
writes the code from the C++ rather than from the test. **Escalated for a
resolver with a build of the oracle to settle in one run.**

**Q2 — the Coons/tensor subdivision has no depth limit.**
`PatchDrawer::Draw` (cpdf_rendershading.cpp:741-838) recurses until
`IsSmall()` (bbox < 2×2 device units) or the colour threshold is met. With NaN
or infinite control points — reachable from a crafted mesh stream, since
`ReadCoords` interpolates raw bits into floats and `/Decode` values are
unvalidated — neither terminator fires and the recursion is unbounded.
Upstream is protected only by `Interpolate`'s overflow check on *colours*, which
does not run when the geometry is degenerate but the colours are flat.
**Proposed:** add a depth cap of **32** (each level halves the patch, so 32
levels covers any patch up to 2^32 device units — unreachable in practice) plus
a non-finite control-point check that drops the patch with a diagnostic. This
is a safety net over a crash, not a fidelity change; it is the same shape as the
page brief's Q1 caps that the orchestrator already accepted. **Needs an
orchestrator ruling** because it is an added limit, not a ported one.

**Q3 — is the `full_cover` emulation good enough?**
§5.3 emulates `full_cover` by drawing patch cells with AA off into a scratch
pixmap. Internal seams disappear (correct), but the patch's outer silhouette
becomes hard-edged where the oracle's is antialiased. On a shading that fills
its whole clip region the silhouette is the clip edge and invisible; on a small
free-standing patch it is a 1-pixel jaggy along the boundary. **Proposed:**
accept it, measure it on `2_shading_type_6_00` / `shade-tensor`, and if the
SSIM cost is material, refine by drawing the cells AA-off into the scratch and
then *additionally* filling the patch's outer boundary path AA-on into the
scratch's alpha channel only. **Needs a ruling only if the measurement is bad;
recorded so the burn-down agent knows the fallback.**

**Q4 — vello_cpu 0.2.0's real API and determinism. BLOCKING.**
`vello_cpu` and `peniko` are absent from this machine's cargo registry and the
network is unavailable, so §5.1's vello column and §4's assumption that
vello_cpu offers native layers with blend+opacity+mask are **inferred, not
verified**. Three things must be confirmed before `pdfrum-raster-vello-cpu` is
written:
(a) that `push_layer`/`pop_layer` accept a blend mode, an opacity **and** an
8-bit mask (E8/§1.5 depend on all three);
(b) that a mid-render read-back of the target's pixels is possible (E3/E4's
`snapshot`), or that the engine must instead render every group into a separate
target — which is fine, but changes `RasterBackend`;
(c) that output is **bit-stable across CPU feature levels and thread counts**;
if not, §6.3's `hard_fail` gate must be relaxed for vello only, and the Tier-C
baseline must be documented as asymmetric.
**Escalated:** this is the one item that can invalidate part of §4's proposed
spec change, and it needs an environment with network access.

**Q5 — should `RenderOptions` carry the page's transparency decision?**
`pdfium_test` chooses BGRA+transparent vs BGR+white from
`FPDFPage_HasTransparency(page)` (§1.2), and D6's claim that our single
compositing path matches the oracle depends on the white background being real
pixels. That makes the choice **load-bearing for fidelity**, not a caller
convenience. **Proposed:** `render_page` decides it internally from
`page.has_transparency()` and `RenderOptions` gains only an override
(`background: Option<peniko::Color>`, default `None` = follow the oracle).
Low risk; recorded because it is a public-API shape decision that SPEC §8's
`RenderOptions` sketch does not cover.

**Q7 — transcribe or generate `kColorSqrt`?**
`SoftLight` needs the 256-entry table at `core/fxge/dib/blend.cpp:21-43`.
I verified it is **exactly** `round(255 · D(i/255))` for ISO 32000-1
§11.3.5.2's piecewise `D(x) = if x <= 0.25 { ((16x−12)x+4)x } else { sqrt(x) }`
— all 256 entries agree, so either a verbatim `const [u8; 256]` or a
`const fn`-generated table is correct. **Proposed:** transcribe verbatim with a
comment naming the closed form and the file it came from, matching how the page
brief handles the sRGB gamma and 6561-entry Adobe CMYK tables, and let test 32
assert both the table *and* the closed form so the two can never drift.
No licence question (BSD PDFium source, same provenance DEPS.md already
sanctions for the Foxit fallback fonts). Recorded for consistency, not as a
real decision.

**Q6 — where does the `TransferFunc` cache live?**
The C++ caches it on `CPDF_Document` (`CPDF_DocRenderData`), keyed by the TR
object's identity, so it survives across pages. SPEC §7 puts the *image* cache
on the render session. A TR LUT is 768 bytes and rebuilding it costs 256
function evaluations, so a per-session cache is cheap enough. **Proposed:**
per-session (`RenderCaches`, §3.1), consistent with STYLE §1's "caches live
inside the owning value". No behavioural difference. Recorded for consistency
with the page brief's cache decisions rather than as a real question.
