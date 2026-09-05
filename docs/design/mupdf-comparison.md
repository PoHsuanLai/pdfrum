# Where the 2.5× goes — pdfrum beside MuPDF, read from the source

**Taken 2026-09-06** on `himmel`, tree at `fe7096b`. The question from the
user: on an idle box mupdf renders the 44-file corpus with a warm median of
4.2 ms against our 10.6 ms, and the 209-file PDFium sample at 2.1 ms against
7.4 ms (`docs/benchmarks/README.md` is the method). Correctness is not the
question — we lead there, 0.9995 median SSIM against mupdf's 0.975. The
question is *where the 2.5× goes and what mupdf does structurally that we do
not*, read from mupdf's C source rather than guessed.

This is a study. Nothing here is implemented.

**Read §1 and §6 if you read nothing else.** §1 is where the gap actually
is, and it is not where the brief assumed. §6 is the three lists the user
asked for: what is portable with no pixel change, what changes pixels but
stays inside the PDFium floor, and what is mupdf's different answer rather
than a faster one.

---

## 0. Method, and the one correction it forced

Both engines run through the same harness child, `benches/compare`'s
`child` subcommand, so the process, the PNG encode and the compositing over
white are identical on both sides and sit *outside* the engine measurement.
The anchor is the inclusive count on
`benches/compare/src/engines/mod.rs:run`, not `PROGRAM TOTALS`.

```sh
cd benches/compare
CARGO_TARGET_DIR=/mnt/data2/r13921098/cargo-target/m21 \
  cargo build --release --features c-engines
valgrind --tool=callgrind --callgrind-out-file=<scratch>/cg.<file>.<engine> \
  <target>/release/pdfrum-compare child --engine pdfrum|mupdf --op render \
  --file <pdf> --out <scratch>/x.png --warm N --font-dir <oracle fonts>
callgrind_annotate --inclusive=yes
```

**The correction.** The brief asked for `--warm 0`. At `--warm 0` the two
engines are almost level:

| page | pdfrum `Ir` | mupdf `Ir` | ratio |
|---|---:|---:|---:|
| `text_tcpdf_063` | 296,964,435 | 258,823,246 | **1.15×** |
| `vector_en_tem` | 224,930,602 | 199,237,808 | **1.13×** |
| `shading_tcpdf_058` | 376,133,131 | 371,067,452 | **1.01×** |
| `text_quick_start` | 350,459,447 | 305,594,224 | **1.15×** |
| `mixed_tcpdf_045` | 225,105,622 | 235,325,344 | **0.96×** |

A 1.15× cold ratio cannot produce a 2.5× warm median. The published
benchmark reports a **warm** median (`warm_runs: 3`), and `Ctx::measure`
(`benches/compare/src/model.rs:127-147`) runs the closure N+1 times in one
process. So the honest measurement is the **marginal cost of one more
render**, taken by differencing a `--warm 4` run (5 renders) against a
`--warm 0` run (1 render); the shared one-time setup cancels exactly.

Both loops are fair: mupdf's closure is `page.to_pixmap(...)`, which reaches
`fz_new_pixmap_from_page` → `fz_run_page` → `pdf_run_page_contents`
(`mupdf-sys-0.8.0/wrapper/page.c:9-19`), i.e. it re-runs the content stream
every iteration exactly as `Page::render_on` re-runs `prepare` → `build`
every iteration (`crates/pdfrum/src/page.rs:199-209`). Neither side hoists
the interpretation.

**Marginal `Ir` per render, and the wall clock beside it** (wall from the
same harness outside valgrind, `--warm 12`, median of 12):

| page | pdfrum marginal `Ir` | mupdf marginal `Ir` | `Ir` ratio | pdfrum warm ms | mupdf warm ms | wall ratio |
|---|---:|---:|---:|---:|---:|---:|
| `text_tcpdf_063` | 250,034,321 | 111,110,370 | **2.25×** | 15.72 | 5.45 | 2.88× |
| `vector_en_tem` | 155,465,693 | 48,506,053 | **3.21×** | 12.64 | 1.91 | 6.61× |
| `shading_tcpdf_058` | 258,377,830 | 208,862,060 | **1.24×** | 22.66 | 9.98 | 2.27× |
| `text_quick_start` | 211,194,212 | 118,655,003 | **1.78×** | 11.07 | 5.30 | 2.09× |
| `mixed_tcpdf_045` | 165,074,001 | 47,727,304 | **3.46×** | 8.62 | 1.91 | 4.51× |

The marginal `Ir` ratios track the wall ratios. **This table is the
subject of the document.** Where wall exceeds `Ir` the difference is cache
and allocator behaviour that `Ir` does not count, which is itself a finding
(§2.1).

All five corpus pages are A4 at 150 DPI = 1240×1753 = 2,173,720 px. That
coincidence matters in §2.1 and is controlled for there.

---

## 1. The answer, stated first

**The gap is not in the interpreter, not in the page graph, and not in the
font layer. It is overwhelmingly in the rasterizer backend, and most of that
is one upstream function.**

Three measurements, in increasing order of how much they narrow the claim.

### 1.1 `vello_cpu`'s `F32Kernel::pack` is a fixed cost per render

In the warm profiles, one figure is **byte-for-byte identical on four of the
five pages** — 478,367,460 `Ir` in
`vello_cpu-0.2.0/src/fine/highp/mod.rs`, called exactly 2,195 times, on
`text_tcpdf_063`, `text_quick_start`, `mixed_tcpdf_045` and `vector_en_tem`.
A cost that does not vary with page content is not a rendering cost; it is a
per-pixel tax on the canvas.

Demangled, the symbol is `F32Kernel::pack`. It is the f32→u8 conversion on
flush, and upstream has marked it:

```rust
fn pack(_simd: S, scratch: &[Self::Numeric], width: usize, region: &mut Region<'_>) {
    for y in 0..region.height {
        let row = &mut region.row_mut(y)[..width * COLOR_COMPONENTS];
        // TODO: SIMDify
        for (dx, pixel) in row.chunks_exact_mut(COLOR_COMPONENTS).enumerate() {
            ...
            pixel[0] = (src[0] * 255.0 + 0.5) as u8;
            pixel[1] = (src[1] * 255.0 + 0.5) as u8;
            pixel[2] = (src[2] * 255.0 + 0.5) as u8;
            pixel[3] = (src[3] * 255.0 + 0.5) as u8;
        }
    }
}
```

`vello_cpu-0.2.0/src/fine/highp/mod.rs:272-285`. Scalar, per channel, four
float multiplies and four casts per pixel. Its sibling `unpack`
(`:287-297`, 120,926,385 `Ir`, same 2,195 calls) carries
`// TODO: SIMDify + multiply by 1.0/255.0 instead` — it divides.

Proportionality confirmed against a differently-sized page:
`text_bug_1029` at 1275×1650 = 2,103,750 px gives `pack` = 462,965,280 `Ir`.
462,965,280 / 2,103,750 = **220.07 `Ir`/px**; 478,367,460 / 2,173,720 =
**220.07 `Ir`/px**. Identical to five significant figures over 5 renders, so
**44.0 `Ir` per pixel per render for `pack`, plus 11.1 for `unpack`** —
about **55 `Ir` per pixel per render** spent converting between f32 and u8,
before any drawing.

For scale: 44.0 `Ir`/px × 2,173,720 px = 95.6 M `Ir` per render. On
`mixed_tcpdf_045`, mupdf's *entire* marginal render is 47.7 M. **We spend
twice mupdf's whole page budget on a format conversion upstream has not
optimized yet.**

### 1.2 The backend, not the render crate, carries the gap

Same page, same page graph, same walk, only the backend swapped
(`benches/src/bin/profile --op render --iterations 3`, callgrind,
`PROGRAM TOTALS`):

| page | vello-cpu `Ir` | agg `Ir` | vello/agg |
|---|---:|---:|---:|
| `text_tcpdf_063` | 2,925,811,530 | 1,444,973,111 | **2.02×** |
| `vector_en_tem` | 1,186,927,989 | 629,622,834 | **1.89×** |
| `mixed_tcpdf_045` | 1,897,766,471 | 433,455,396 | **4.38×** |
| `shading_tcpdf_058` | 400,789,176 | 562,993,052 | **0.71×** |

On `vector_en_tem` the backend's own inclusive cost is **609,972,833 `Ir`
for vello-cpu against 57,397,829 for agg — 10.6×** for the identical
sequence of device calls. Everything above the backend seam
(`crates/pdfrum-render/src/device.rs:194-231`) is unchanged between the two
columns.

Note the sign flip on shading: vello **wins** there by 1.4×. The backend
question is not "vello is slow"; it is "vello is slow at the fill-and-blit
shapes that dominate text, vector and mixed pages, and fast at large smooth
paints".

### 1.3 What is *not* the problem

Measured shares of pdfrum's warm engine-inclusive cost:

| page | `pdfrum-page::interpret_streams` (graph build) | `walk::draw_glyph_bitmap` |
|---|---:|---:|
| `text_tcpdf_063` | 34,343,853 (2.00%) | 114,450,551 (6.65%) |
| `vector_en_tem` | 48,339,558 (4.32%) | 19,262,033 (1.72%) |
| `shading_tcpdf_058` | 48,217,668 (2.74%) | 13,395,906 (0.76%) |
| `text_quick_start` | 61,963,038 (4.16%) | 67,200,323 (4.52%) |
| `mixed_tcpdf_045` | 49,287,363 (4.22%) | 45,835,033 (3.92%) |

**The typed page graph costs 2.0–4.3%.** The brief asked whether building an
intermediate graph, where mupdf runs straight into the device, is a
structural loss. Measured, it is a 2–4% tax — real, worth knowing, and not
the 2.5×. It also buys things mupdf's architecture cannot offer (a
re-drawable page, `Page::objects`, the editor's flattened view). Removing it
entirely would leave us at ~2.4× instead of 2.5×.

**The glyph path costs 0.8–6.7%,** and our `BitmapCache` is doing its job:
on a hit nothing is re-flattened or re-rasterized
(`crates/pdfrum-render/src/glyph.rs:633-691`, blit at
`crates/pdfrum-render/src/walk.rs:1493-1557`).

---

## 2. Per-page side by side

Engine-inclusive warm profiles, top cost centres. pdfrum symbols are
demangled and shortened; mupdf paths are relative to
`mupdf-sys-0.8.0/mupdf/source/`.

### 2.1 `text_tcpdf_063` — text-heavy — 2.25× `Ir`, 2.88× wall

| pdfrum | `Ir` | mupdf | `Ir` |
|---|---:|---|---:|
| `walk::render_page_with` | 1,251,821,701 | `fz_new_pixmap_from_page` | 661,983,005 |
| vello `fine/*` | 876,341,899 | `pdf_run_page_contents` | 629,202,271 |
| **vello `F32Kernel::pack`** | **478,367,460** | `pdf_process_keyword` | 443,367,816 |
| vello `fine/highp` (other) | 268,213,069 | `pdf_show_path` / `pdf_run_S` | 247,266,792 |
| `__memset_avx2_unaligned_erms` | 204,248,812 | `fz_draw_stroke_path` | 246,572,219 |
| `walk::draw_glyph_bitmap` | 114,450,551 | `fz_convert_gel` (rasterize) | 242,982,604 |
| vello_common `strip` | 123,420,158 | **`paint_span_with_color_3_da_solid`** | **240,388,200** |
| `paint::draw_path` | 125,764,431 | `fz_paint_pixmap` (group end) | 135,596,800 |
| `pdfrum-page::interpret_streams` | 34,343,853 | `fz_draw_fill_text` | 58,975,279 |

Read the two rasterizer rows against each other. mupdf's entire scan
conversion *and* composite is 243 M + 240 M; our `pack` alone — which draws
nothing — is 478 M. Note also `__memset` at 204 M on our side: vello's fine
stage clears scratch tiles; mupdf's `init_stack[STACK_SIZE]` of 96 device
states is inline and never heap-allocated
(`fitz/draw-device.c:36,85,3191-3192`), and its per-scanline `alphas`/`deltas`
buffers are allocated once per `fz_scan_convert_aa` and grown only when
needed (`fitz/draw-edge.c:584-597`). That 204 M of memset is most of why the
wall ratio (2.88×) exceeds the `Ir` ratio (2.25×).

### 2.2 `vector_en_tem` — vector-heavy — 3.21× `Ir`, 6.61× wall — the worst page

| pdfrum | `Ir` | mupdf | `Ir` |
|---|---:|---|---:|
| `walk::render_page_with` | 790,970,835 | `fz_new_pixmap_from_page` | 352,562,495 |
| **vello backend `finish`** | **609,972,833** | `fz_run_page` | 319,789,021 |
| vello `fine/mod` | 653,512,936 | `pdf_process_keyword` | 161,299,903 |
| **vello `F32Kernel::pack`** | **478,367,460** | `fz_paint_pixmap` | 112,072,470 |
| `__memset_avx2_unaligned_erms` | 203,724,800 | **`paint_span_3_sa`** | **111,913,535** |
| vello `coarse/depth` | 121,453,060 | `fz_draw_fill_text` | 58,975,279 |
| `pdfrum-page::interpret_streams` | 48,339,558 | `pdf_run_Do_image` | 54,146,085 |
| `pdfrum-font::get_or_load` | 43,426,423 | — | — |

The sharpest row in the study: **our backend's `finish` (610 M) is nearly
twice mupdf's whole page (353 M)**, and `pack` alone (478 M) exceeds it. The
same page through agg costs 57 M in the backend. This page is the 10.6×
backend measurement of §1.2.

### 2.3 `shading_tcpdf_058` — shading — 1.24× `Ir`, 2.27× wall — our best page

| pdfrum | `Ir` | mupdf | `Ir` |
|---|---:|---|---:|
| `walk::render_page_with` | 1,341,491,235 | `fz_new_pixmap_from_page` | 1,163,274,599 |
| vello `fine/mod` | 819,477,960 | `pdf_process_keyword` | 947,039,465 |
| **vello `F32Kernel::pack`** | **432,770,880** | `pdf_show_path` | 512,112,123 |
| `__memset_avx2_unaligned_erms` | 310,171,945 | **`fz_paint_shade`** | **379,550,745** |
| `render::shading/mod.rs` | 280,845,183 | `fz_convert_gel` | 279,521,533 |
| `render::shading/axial.rs` | 154,892,240 | `fz_draw_stroke_path` | 248,372,266 |
| `pdfrum-page::interpret_streams` | 48,217,668 | `pdf_run_sh` / `pdf_show_shade` | 425,264,884 |

We are nearly level here, and our shading code is *cheaper* than mupdf's:
280 M + 155 M against 380 M. The reason is architectural and in our favour —
see §3.5. This is also the one page where vello beats agg (§1.2).

### 2.4 `text_quick_start` — image page — 1.78× `Ir`, 2.09× wall

| pdfrum | `Ir` | mupdf | `Ir` |
|---|---:|---|---:|
| `walk::render_page_with` | 1,105,041,925 | `fz_new_pixmap_from_page` | 728,264,221 |
| vello `fine/*` | 858,853,672 | `pdf_process_keyword` | 517,829,742 |
| **vello `F32Kernel::pack`** | **478,367,460** | **`pdf_run_Do_image` / `pdf_show_image`** | **373,965,291** |
| `__memset_avx2_unaligned_erms` | 208,107,533 | `fz_transform_pixmap` | 271,562,336 |
| vello `coarse/depth` | 121,453,060 | **`fz_scale_pixmap_cached`** | **271,549,216** |
| `walk::draw_glyph_bitmap` | 67,200,323 | `fz_draw_fill_image` | 265,596,798 |
| `pdfrum-page::interpret_streams` | 61,963,038 | `scale_row_from_temp` | 136,410,720 |
| `render::imagecache.rs` | 55,235,480 | `fz_end_group` | 115,661,395 |

Per the brief this page's cold-start image cost is already recorded in
`docs/status/queue.md` §"Cold start, measured again" and is not re-measured:
7.43 G for us against mupdf's 3.92 G over eleven pages, our image path 3.6 G
against mupdf's 2.62 G. What the *warm* profile adds is that once the image
is in `RenderedImageCache` (`crates/pdfrum-render/src/imagecache.rs:56-100`)
the image path largely leaves the profile — 55 M — while mupdf still pays
272 M in `fz_scale_pixmap_cached` on every render, because **mupdf caches the
decoded pixmap but not the scaled one** (`fitz/draw-scale-simple.c:456-484`
caches only the per-axis filter *weights*). This is the one stage where our
caching is structurally better than mupdf's, and it is why this page has the
second-smallest gap.

### 2.5 `mixed_tcpdf_045` — mixed/form — 3.46× `Ir`, 4.51× wall

| pdfrum | `Ir` | mupdf | `Ir` |
|---|---:|---|---:|
| `walk::render_page_with` | 822,404,801 | `fz_new_pixmap_from_page` | 381,984,133 |
| vello `fine/mod` | 657,623,518 | `fz_run_page` | 349,207,374 |
| **vello `F32Kernel::pack`** | **478,367,460** | `pdf_process_keyword` | 187,489,961 |
| `__memset_avx2_unaligned_erms` | 205,216,600 | `fz_paint_pixmap` | 113,850,825 |
| vello `coarse/depth` | 121,453,060 | **`paint_span_3_sa`** | **113,691,890** |
| `pdfrum-page::interpret_streams` | 49,287,363 | `pdf_load_font` | 76,780,933 |
| `pdfrum-font::get_or_load` | 39,848,155 | `fz_draw_fill_text` | 76,510,442 |

Our worst `Ir` ratio. `pack` (478 M) is **ten times** mupdf's entire
marginal render (47.7 M). The page is cheap; the fixed per-pixel tax is not.

---

## 3. The mechanisms, both sides cited

### 3.1 The antialiased rasterizer and compositor

**mupdf.** An active-edge-table "gel" supersamples each pixel into
`hscale` × `vscale` subsamples — by default **17 × 15 = 255**, giving exactly
8-bit coverage (`fitz/draw-imp.h:48-53`, runtime values from
`ctx->aa.hscale`/`vscale` at `:105-111`, declared
`include/mupdf/fitz/context.h:828-829`). Crucially, coverage is accumulated
**not per subsample but as `int` deltas, one per pixel column**:
`add_span_aa` adds `h*(subpixel extent)` into `list[x0pix]` and
`list[x1pix]` (`fitz/draw-edge.c:461,485-498`), a running-difference
encoding that `undelta_aa` prefix-sums into one `unsigned char` alpha per
pixel (`:532-543`). So 255 subsamples cost roughly two integer adds per
edge crossing, not 255 samples. Per-scanline buffers are one
`unsigned char alphas[bcap]` and one `int deltas[bcap]`, allocated once per
`fz_scan_convert_aa` and grown only when needed (`:584-597`). Each finished
row goes straight to a span painter (`:544-553`).

The compositor is **8-bit integer, premultiplied**, dispatched once per fill
by `fz_get_span_color_painter(n, da, color, eop)` — which switches on
component count *and* on whether alpha is 255, giving separate solid and
alpha variants (`fitz/draw-paint.c:1116-1165`). The n=3 solid path packs
RGBA into one `unsigned int` and blends two channels at a time with mask
`0xFF00FF00` (`:701-731`), short-circuiting `ma==256` to a plain 32-bit
store. No SIMD intrinsics — SWAR on plain integers.

**Ours.** `vello_cpu` flattens to lines, bins them into 4×4 tiles
(`vello_common-0.2.0/src/tile.rs:14-41,263-266`), sorts tiles by a packed
`u64` key (`:379`), then accumulates winding/area per subpixel column in
**f32** and converts to u8 alpha at the end of each strip
(`vello_common-0.2.0/src/strip.rs:490`). Compositing then runs in a
selectable-precision kernel; we pin the **f32** one
(`RenderMode::OptimizeQuality`,
`crates/pdfrum-raster-vello-cpu/src/lib.rs:68-81`), deliberately, for
cross-machine determinism (rationale at `:32-38`). The f32 scratch is
converted back to u8 in `F32Kernel::pack`
(`vello_cpu-0.2.0/src/fine/highp/mod.rs:272-285`).

**The mechanism, stated plainly.** mupdf composites once per span, in
integers, at the precision it will store. We composite in f32 across a
whole tile row and then pay a scalar 4-multiply-4-cast conversion for every
pixel of the canvas whether or not anything was drawn there — measured at
44.0 `Ir`/px for `pack` plus 11.1 for `unpack` (§1.1). A `U8Kernel` exists
upstream (`vello_cpu-0.2.0/src/fine/mod.rs:51-52,69-85`) whose `pack` is
already SIMD block/tail (`fine/lowp/mod.rs:308-353`).

### 3.2 Glyphs

**mupdf** caches a **rendered 8-bit alpha bitmap**, keyed by
`{font pointer, matrix a/b/c/d in 16.16 fixed point, gid, quantised
subpixel translation e/f, aa level}` (`fitz/draw-glyph.c:36-44,319-322`).
Subpixel quantisation is size-adaptive: 8 positions under size 24, 4 for
24–48, 1 at ≥48, with the perpendicular axis coarser and one axis suppressed
for axis-aligned matrices (`fz_subpixel_adjust`, `:166-224`). Storage is a
private hash plus a **doubly linked LRU list**, 1 MiB cap, strict eviction
from the tail (`:32,47-69,404-415`), and small single-component glyphs are
stored **RLE-compressed** rather than as pixmaps (`fitz/glyph.c:30,164-176`).
Colour is resolved **once before the span loop**, not per glyph
(`fitz/draw-device.c:1094`).

**Ours** caches an LCD-stripe coverage bitmap keyed by
`{GlyphKey, matrix quantised to 1e-4}` with subpixel phase deliberately
*excluded* from the key and applied as a window shift at blit time
(`crates/pdfrum-render/src/glyph.rs:611-663`). That is a *better* key than
mupdf's — one entry serves all phases where mupdf stores up to 8. Budget is
16 MiB with **no eviction**: past budget we stop inserting and return
`Cached::Uncached` (`:664-691`), reasoned as "a page's repertoire is small
and hot, so LRU would evict what is about to be wanted".

**Verdict: this stage is not our problem** (0.8–6.7%, §1.3). The one real
difference is that on a hit we still call `RenderDevice::draw_image` with a
translate and `ImageQuality::Nearest` (`walk.rs:1548-1556`), routing a
glyph blit through the general image path, where mupdf calls a dedicated
`draw_glyph` that blits rows directly through a span painter
(`fitz/draw-device.c:990-1030`).

### 3.3 The store

**mupdf** has one `fz_store` per `fz_context`, **shared across every
document opened on that context**, refcounted and inherited by cloned
contexts (`fitz/store.c:707-720`). Default budget **256 MiB**
(`FZ_STORE_DEFAULT`, `include/mupdf/fitz/context.h:316-317`). It holds
decoded images (keyed by image pointer + `l2factor` + subarea,
`fitz/image.c:111-141`), draw-device tiles (`fitz/draw-device.c:2709`), ICC
colorspace links (`fitz/colorspace.c:843`), PDF resources
(`fitz/../pdf/pdf-store.c:75`) and more, behind a polymorphic
`fz_store_type` vtable (`include/mupdf/fitz/store.h:265-274`). Reclaim is a
scavenger that scans from the LRU tail, frees the **largest** evictable item,
and restarts (`fitz/store.c:795-877`) — deliberately freeing as few blocks as
possible.

**Ours** splits the same duty across `BuildContext` (per document:
colorspaces, functions, decoded images at 100 MiB, fonts in an `Arc<FontCache>`
— `crates/pdfrum-page/src/build.rs:61-142`) and `RenderCaches` (per session:
glyph outlines, glyph bitmaps at 16 MiB, rendered images at 64 MiB —
`crates/pdfrum-render/src/ctx.rs:127-176`). **Nothing is shared across
documents, and nothing evicts.**

For the single-page warm benchmark this costs us nothing — both engines are
warm. It matters for a server rendering many documents, which is the shape
`compare throughput` measures.

One concrete reuse gap: `Resources::new()` is constructed fresh on every
`rasterize` (`crates/pdfrum-raster-vello-cpu/src/lib.rs:174`), so vello's own
image cache and atlas (`vello_common-0.2.0/src/image_cache.rs`,
`multi_atlas.rs`) never survive a render.

### 3.4 Colour

**mupdf** dispatches DeviceGray/RGB/CMYK/Lab conversions through a pure
type-pair switch with no lcms involvement at all —
`fz_lookup_fast_color_converter` (`fitz/color-fast.c:179-220`) over
`gray_to_rgb` (`:36`), `rgb_to_gray` (`:43`), `cmyk_to_rgb` (`:117`) and the
rest, plus whole-pixmap variants (`fast_cmyk_to_rgb` `:942`, dispatched
`:1678-1690`). lcms is reached only under `FZ_ENABLE_ICC` *and*
`ctx->icc_enabled`, and even then identical profiles and DeviceGray→CMYK
escape first (`fitz/colorspace.c:979-1017`); links are cached in the store.

Decisively, **a solid fill colour is converted once per fill operation, not
per pixel**: `resolve_color` converts and immediately quantises to
`colorbv[i] = colorfv[i] * 255` (`fitz/draw-device.c:577-641`), and the byte
vector is what the span painters consume. It is called once per
fill/stroke/text (`:740,810,1094,1198`).

**Ours** resolves `Argb` once per object via `resolve_argb`
(`crates/pdfrum-render/src/walk.rs:1024-1044`) — equivalent — but then calls
`Argb::to_peniko()` **once per device call**, at
`crates/pdfrum-render/src/paint.rs:110`, `:138`, `:193` and again in the
ordinary fill/stroke arms. A fill-and-stroke object converts twice, and the
degenerate zero-area path converts once per sub-path (`:172-193`). This is
small — a handful of instructions against 44/px — but it is free to fix.

### 3.5 Shading

**mupdf** decomposes *everything* to Gouraud triangles and paints them with
a scanline triangle filler; there is no closed-form axial or radial shader.
`fz_paint_shade` builds a 256-entry colour LUT (`fitz/draw-mesh.c:276-277`,
cached in `fz_shade_color_cache` `:242-257`), then `fz_process_shade` walks
the mesh into `fz_paint_triangle` (`:125,228,355`), which interpolates
position and all n colour components linearly per scanline (`:152-184`).
Axial and radial get special *tessellation*, not a special pixel path: a
linear shading becomes a pair of quads extended to a scissor-derived radius
(`fitz/shade.c:152,174-205`), a radial becomes annular bands (`:286`).
Triangles are painted **without antialiasing**.

**Ours** evaluates axial and radial analytically per pixel
(`crates/pdfrum-render/src/shading/axial.rs`, `mod.rs`). Measured, ours is
*cheaper*: 280 M + 155 M against mupdf's 380 M (§2.3), and this is the one
page where vello beats agg. **Do not port mupdf's mesh approach** — it is
the reason mupdf's shading pages score worse against the oracle, and it is
slower for us as well.

### 3.6 The content interpreter and per-page setup

**mupdf** runs the content stream straight into the device: `pdf_process_stream`
is a `pdf_lex` token loop (`pdf/pdf-interpret.c:1527,1567`) dispatching to
`pdf_process_keyword`, whose vtable entries are the `pdf_run_*` functions
(`pdf/pdf-op-run.c:3230-3300`) that call `fz_device` methods directly. There
is **no retained graph** unless a caller interposes `fz_new_list_device`.
Per-page allocation is one `pdf_run_processor` plus a growable gstate array
doubling to a 4096 cap (`pdf/pdf-op-run.c:133-134,546-557`). The draw device
allocates an **inline 96-entry state stack** used with no heap allocation
(`fitz/draw-device.c:36,85,3191-3192`), one rasterizer and two scale caches
(`:3243-3245`) — and no page-sized buffers beyond the caller's pixmap.

**Ours** builds `Page { objects: Vec<PageObject>, .. }` where every drawing
variant is **boxed** (`crates/pdfrum-page/src/page.rs:254-265`), each
carrying `GraphicsState` and `ContentMarks` (`:269-321`), with forms nesting
as sub-`Vec`s (`:205-208`) — a tree of boxed enum nodes with owned geometry
(`BezPath` per path, `Box<[TextSegment]>`, `Box<[u8]>` per segment).

**Measured at 2.0–4.3%** (§1.3). Worth one line in the ranked plan, not a
rewrite.

### 3.7 Images

**mupdf** chooses between pre-scaling and per-pixel sampling via a tuning
hook (`fitz/draw-device.c:1922`, default `:1758`). Pre-scale goes through
`fz_transform_pixmap` with explicit axis-aligned and 90°-rotated fast paths
(`:1697-1755`); the final blit inverse-maps per destination pixel in 64-bit
fixed point with `PREC 14` (`fitz/draw-affine.c:30-37,3975-3990,4094-4102`),
specialising on `fa == 0` / `fb == 0`, on gray→RGB, and on `da`/`sa`/`dn`
(`:632,701,1014,4040-4083`). It orders colour conversion around the scale to
convert the fewer components (`:1889-1918`). Notably it caches **only the
per-axis filter weights** (`fitz/draw-scale-simple.c:214,456-484`), not the
scaled pixmap — so a repeated blit re-runs the resampling.

**Ours** decodes in `pdfrum-page` (`ImageCache` keyed on reference + requested
size, `build.rs:70-73`), fuses conversion and box-filter reduction into one
pull pipeline so no full-size RGBA is materialised
(`crates/pdfrum-render/src/stretch.rs:46-90`, `walk.rs:2058-2067`), and
memoizes the **rendered** result keyed by `(ObjRef, PixmapRequest)`
(`imagecache.rs:56-100,182-205`). Warm, this beats mupdf (§2.4). Cold, the
`image-rows` pass (`docs/design/image-rows.md`) is the answer and is already
scoped.

---

## 4. What we do that mupdf does not — the honest column

So the comparison is not read as a list of our failings:

1. **We render at f32 precision into straight-alpha RGBA; mupdf composites
   in 8-bit premultiplied integers.** That is the direct cause of §1.1 and it
   is also why we score 0.9995 median SSIM against mupdf's 0.975. The
   precision is not free and it is not wasted.
2. **We keep `Option` and bounds checks the user chose to keep.** They are
   visible in the profiles as a few percent and are not proposed for removal.
3. **mupdf's antialiasing and stroke adjustment differ from PDFium's.** Its
   AA is a 17×15 supersample; PDFium's AGG path is analytic. Its shading
   triangles are painted with no antialiasing at all (§3.5). This is most of
   its 0.975 against our 0.9995 — it is a different answer, not a faster one.
4. **We cache the scaled image; mupdf re-scales every render** (§3.7, §2.4).
5. **Our glyph key excludes subpixel phase** and serves all phases from one
   entry, where mupdf stores up to 8 (§3.2).
6. **Our shading is analytic and cheaper than mupdf's mesh** (§3.5).
7. **We produce a re-drawable typed page graph**, which mupdf's architecture
   does not offer without a display list; it costs 2–4% (§1.3) and is what
   `Page::objects` and the editor are built on.

---

## 5. The backend table, as numbers

Requested as its own table without a recommendation. Same page graph, same
walk, only `RasterBackend` swapped.

**Wall clock**, `benches/src/bin/profile --op render --iterations 30`,
ms/iteration (this binary rebuilds the graph per iteration and holds no warm
caches, so absolutes are far above §0's harness figures; only the columns
compare):

| page | vello-cpu | agg | tinyskia |
|---|---:|---:|---:|
| `text_tcpdf_063` | 78.55 | **26.94** | 57.07 |
| `vector_en_tem` | 34.73 | **13.16** | 27.02 |
| `shading_tcpdf_058` | **8.97** | 9.02 | 27.16 |
| `text_quick_start` | 153.64 | 159.68 | **149.65** |
| `mixed_tcpdf_045` | 59.28 | **7.22** | 28.30 |

**`Ir`**, same binary at `--iterations 3` under callgrind, `PROGRAM TOTALS`:

| page | vello-cpu | agg | vello/agg |
|---|---:|---:|---:|
| `text_tcpdf_063` | 2,925,811,530 | 1,444,973,111 | 2.02× |
| `vector_en_tem` | 1,186,927,989 | 629,622,834 | 1.89× |
| `mixed_tcpdf_045` | 1,897,766,471 | 433,455,396 | 4.38× |
| `shading_tcpdf_058` | 400,789,176 | 562,993,052 | 0.71× |

Backend-only inclusive on `vector_en_tem`: vello-cpu **609,972,833**, agg
**57,397,829**.

---

## 6. The ranked plan

Ranked by (`Ir` saved) / (cost and correctness risk), in the three lists the
user asked for. Every number is from §1–§3, not intuition. Nothing here is
implemented.

### (a) Portable with no change to the pixels we produce — board byte-identical

**a1. SIMD `F32Kernel::pack`/`unpack` upstream in `vello_cpu`.**
*Saves:* 44.0 `Ir`/px for `pack` + 11.1 for `unpack` per render (§1.1) — at
A4/150 DPI, **~120 M `Ir` per render**, i.e. 48% of our marginal cost on
`text_tcpdf_063`, 77% on `vector_en_tem`, 72% on `mixed_tcpdf_045`. Even a
conservative 3× from SIMD saves ~80 M/render.
*Pixels:* none. Rounding is `(x * 255.0 + 0.5) as u8`, exactly reproducible
in SIMD; the `U8Kernel`'s `pack` is already SIMD
(`fine/lowp/mod.rs:308-353`) and shows the shape.
*Kind:* **backend change, vello_cpu upstream.** The `// TODO: SIMDify`
(`fine/highp/mod.rs:274`) and `// TODO: SIMDify + multiply by 1.0/255.0
instead` (`:289`) are upstream's own. The `unpack` divide-to-multiply change
alone is a one-line, bit-exact win.
*Risk:* low, but it is someone else's crate — the work is a PR plus a version
bump, or a vendored patch.
**This is the single highest-value item in the study and it is not our code.**

**a2. Reuse `Resources` across renders in the vello backend.**
*Saves:* not separately measured — `Resources::new()` per `rasterize`
(`crates/pdfrum-raster-vello-cpu/src/lib.rs:174`) discards vello's image
cache and atlas every render. Bounded above by the image-paint share.
*Pixels:* none, if the cache is keyed correctly.
*Kind:* render-crate/backend change. Type-driven: hang it off a
`RasterBackend`-owned handle so a `VelloCpuDevice` cannot be built without
one, rather than an `Option` re-checked per call.
*Risk:* low. Needs a board run to confirm no cross-render bleed.

**a3. Stop converting the colour per device call.**
*Saves:* small — a handful of instructions per fill; two conversions per
fill-and-stroke object, one per degenerate sub-path
(`crates/pdfrum-render/src/paint.rs:110,138,172-193`).
*Pixels:* none. mupdf's `resolve_color` does exactly this once per operation
(`fitz/draw-device.c:577-641`).
*Kind:* render-crate change. Type-driven: carry a converted
`PenikoColor` newtype produced once by `resolve_argb`, so an unconverted
`Argb` cannot reach a device call.
*Risk:* very low. Cheap, obviously correct, worth doing while nearby.

**a4. A shared, budgeted, evicting store across documents.**
*Saves:* nothing on the single-page warm benchmark (both engines warm);
matters for `compare throughput` and any multi-document server.
*Pixels:* none.
*Kind:* cache change. mupdf's model is one store per context, 256 MiB,
scavenge-largest-from-LRU-tail (`fitz/store.c:707-720,795-877`,
`include/mupdf/fitz/context.h:316-317`). Ours is per-document +
per-session with **no eviction anywhere**
(`glyph.rs:664-691`, `imagecache.rs:195-201`).
*Risk:* medium — it is an API-shape question (who owns the store, what its
lifetime is), not a hot-loop question. Worth scoping only if multi-document
throughput becomes a goal.

**a5. Blit a cached glyph through a dedicated path, not `draw_image`.**
*Saves:* a fraction of the 0.8–6.7% glyph share (§1.3) — the general image
path's setup per glyph. Small.
*Pixels:* none if the placement stays whole-pixel `Nearest`, which it already
is (`walk.rs:1548-1556`).
*Kind:* render-crate + backend-trait change; mupdf's `draw_glyph` blits rows
straight through a span painter (`fitz/draw-device.c:990-1030`).
*Risk:* low, but it widens the `RenderDevice` seam — STYLE.md §2b makes that
a `[spec]` decision. Not worth it for the size.

### (b) Changes pixels, but stays within the PDFium floor

**b1. Render in `U8Kernel` (`RenderMode::OptimizeSpeed`) instead of f32.**
*Saves:* the whole of a1's 120 M `Ir`/render *and* the f32 compositing above
it — the largest single lever available without touching upstream, since
`U8Kernel` already exists (`vello_cpu-0.2.0/src/fine/mod.rs:51-52,69-85`)
with a SIMD `pack`.
*Pixels:* **yes.** 8-bit compositing loses precision against our current f32
path. This is exactly the trade in §4 item 1, and it is the one that put us
at 0.9995 SSIM. It must be measured, not assumed: the question is whether
the board stays above the 0.99 conformance floor
(`conformance/thresholds.toml`) and how many rows move.
*Kind:* backend change, one line
(`crates/pdfrum-raster-vello-cpu/src/lib.rs:68-81`) — but that line is
pinned deliberately for cross-machine determinism (rationale at `:32-38`), so
flipping it is a documented-decision reversal, not a tweak.
*Which rows move, how far:* **not determined** — this needs a full board run
under `OptimizeSpeed`, which I did not take. That measurement is the
precondition for even proposing it.

**b2. Choose the backend per page shape.**
*Saves:* from §5 — agg is 2.02× / 1.89× / 4.38× cheaper on text, vector and
mixed; vello is 1.41× cheaper on shading. A perfect oracle over these five
pages saves roughly half our `Ir` on three of them.
*Pixels:* **yes** — the three backends do not agree pixel-for-pixel; that is
why `docs/design/backend-verification.md` exists and why the board is run per
backend.
*Kind:* a policy question above the `RasterBackend` seam, and a bad one to
answer with a heuristic — a page-shape sniffer that picks a rasterizer is a
correctness surface with no oracle. Recorded because the numbers are
striking, **not** recommended as a heuristic. The defensible version is the
existing one: the user picks the backend, and we publish this table so the
choice is informed.

### (c) mupdf's different answer, not a faster one — do not propose these

**c1. 17×15 supersampled AA with delta-encoded integer coverage**
(`fitz/draw-imp.h:48-53`, `fitz/draw-edge.c:461,485-498,532-543`). Elegant —
255 subsamples for about two integer adds per crossing — but it is a
*different antialiasing model* from PDFium's analytic AGG coverage, which is
what our board is scored against. Adopting it would move essentially every
antialiased edge in the corpus. **SSIM cost: this is a large part of mupdf's
0.975 median against our 0.9995.**

**c2. Stroke adjustment.** mupdf's differs from PDFium's. Same argument.

**c3. 8-bit premultiplied compositing as the engine's only mode**
(`fitz/draw-paint.c:658-934,1116-1165`). As a *backend option* this is b1; as
the engine's single precision it forecloses the f32 path that earns our SSIM.

**c4. Gouraud-triangle shading for axial/radial**
(`fitz/draw-mesh.c:125-355`, `fitz/shade.c:152-286`). Not only a pixel
change — mupdf paints shading triangles **without antialiasing** — but also
**slower for us**: our analytic axial/radial is 280 M + 155 M against mupdf's
380 M on `shading_tcpdf_058` (§2.3, §3.5).

**c5. Dropping the typed page graph to run straight into the device**
(`pdf/pdf-interpret.c:1527-1567`). Measured at 2.0–4.3% (§1.3) and it would
cost `Page::objects`, the re-drawable page and the editor's flattened view.
The cost is real and small; the loss is structural.

---

## 7. What I could not determine

- **How far b1 moves the board.** The `U8Kernel` swap is the biggest lever
  that does not require an upstream PR, and its correctness cost is exactly
  the thing this study did not measure. A board run under
  `RenderMode::OptimizeSpeed` is the missing number.
- **The `Ir` a1 would actually save**, as opposed to the `Ir` it would
  target. 120 M/render is the current cost of `pack` + `unpack`; what a SIMD
  implementation costs instead is unmeasured. A 3–4× improvement is typical
  for this shape but it is an estimate, flagged as one.
- **a2's size.** `Resources::new()` per rasterize is visibly wasteful but I
  did not isolate its cost from the surrounding image paint.
- **Why the wall ratio exceeds the `Ir` ratio on every page** (e.g. 6.61×
  against 3.21× on `vector_en_tem`). The 204 M of
  `__memset_avx2_unaligned_erms` in every one of our profiles is the leading
  suspect — cache and allocator traffic that `Ir` does not count — but I did
  not take cachegrind or `perf` counters to confirm it. If that is right, the
  wall-clock win from a1/b1 would exceed their `Ir` win.
- **Whether mupdf's 0.975 SSIM is dominated by AA or by font substitution.**
  The harness gives mupdf no font directory (`docs/benchmarks/README.md`),
  so some of that gap is substitution, not rasterization. Untangling them
  would need a run with matched faces.
