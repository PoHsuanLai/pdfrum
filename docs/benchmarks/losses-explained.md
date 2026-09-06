# Every loss in run 3, explained

> **Run 5 (2026-09-07, `258e24cc72e3`, idle `himmel`) re-measured every row in
> this file, and fourteen of the sixteen losses are closed.** The corpus-44
> loss list is now **two rows**, both against `pdfium-render` and both already
> classified below as tradeoffs we do not propose to close:
> `image_jpx_123.pdf` render (SSIM 0.9874 — upstream JPX decoder numerics) and
> `image_ccitt_3bigpreview.pdf` text (F1 0.999 — the duplicate `Clips` in the
> vertical CJK region, the residue this file already names as the last
> non-byte-exact row of the 44).
>
> The fourteen that closed are the ones the rows below already say were fixed
> on main after run 3 was taken, now confirmed by measurement rather than by
> pointer: the five text rows and `text_quick_start`'s F1 0.641 (M28's
> `run.advance` fix — corpus-44 text now **42 of 44 byte-exact**, was 21, and
> whitespace-normalized **43**, was 39), the four `vector_en_system` rows and
> `vector_tcpdf_009` (the snapped-reduction pass), `image_en_fqa` (the same
> pass wired to the colour path) and `shading_tcpdf_058`. Render `>= 0.99`
> went **35 -> 38 of 44** and median SSIM 0.9995 -> **0.9999**. On the 209-file
> PDFium sample the same passes give `>= 0.99` **180 -> 182**, text exact
> **165 -> 204** and normalized **198 -> 208**. **No row got worse, on either
> corpus.** The data is `data/2026-09-07-258e24cc72e3-*.json`; the machine
> reading and the delta table are under "Run 5" in `README.md`.

`docs/benchmarks/README.md` run 3 publishes, for each op, the files where
some peer lands closer to the oracle than pdfrum does, plus the speed and
memory rows where a peer is faster or smaller. This document is the
companion: **one row per loss, each with a measured cause, a citation on our
side and one on PDFium's, and a verdict.**

The four verdicts, as the user defined them:

| verdict | means |
|---|---|
| **tradeoff** | we chose the other side deliberately, and can say what we get for it |
| **more work** | we compute something the peer does not, and someone wants it |
| **inferior** | a defect or a missed technique, fixable |
| **oracle-bug** | PDFium is wrong, we implement the correct behaviour, both sides cited (PLAN.md:212-229) |

**Nothing here is classified that was not measured.** Where a row says *not
determined*, it says why.

**Run 3 was measured at `8a02d57`.** Four render rows in its loss list —
`vector_en_system`, `image_en_fqa`, `fx/image/1_image`, `fx/path/transparent1`
— have been **fixed on main since**; they appear below with a pointer to the
record rather than a re-derivation. The measurements in this document were
taken at `e08d7cc`.

> **One finding is not a loss but a measurement defect, and it changes how
> the board should be read.** All five text rows are inflated by the
> comparison harness emitting the wrong stream: `pdfium_test --txt` writes
> PDFium's unfiltered *character list*
> (`testing/pdfium_test/write.cc:364-370`), while
> `benches/compare/src/engines/pdfrum.rs:109` writes our `Display` impl,
> which `crates/pdfrum-text/src/lib.rs:417-419` documents as the
> `search_text` and explicitly **not** the `chars` stream. On
> `mixed_en_uicase.pdf` our `chars` stream is byte-identical to the oracle;
> the published F1 was 0.991. Every `--op text` F1 in run 3 understated the
> engine until that line was corrected. **It has been:** the adapter now
> writes the `chars` stream, and the corrected column is published as
> "Run 3, text column corrected" under run 3a in
> `docs/benchmarks/README.md` — pdfrum exact 21/44 -> **22/44**,
> whitespace-normalized 39/44 -> **41/44**, F1 >= 0.9 43/44 (unchanged),
> median F1 1.000 (unchanged). Details under
> [Text](#text--the-five-files).
>
> **Superseded for the engine question by M28 (2026-09-06).** The harness
> defect above was real and is fixed, but it was never the engine's loss.
> The engine's loss was a single conjunct in our own degenerate-object gate,
> found by instrumenting `ProcessInsertObject` in a private PDFium build:
> byte-exact went **22/44 -> 42/44**, whitespace-normalized **41/44 ->
> 43/44**, and the board's text pages **1785/2067 -> 2020/2067** with no row
> moving down. See
> [The generated space was our own rescue](#the-generated-space-was-our-own-rescue).

---

## The table

| what we lose | measured cause, both citations | verdict | what closing it would cost or change |
|---|---|---|---|
| **`vector_tcpdf_009.pdf` render** — SSIM 0.9593 vs pdfium-render 0.9995 | **A second resample.** The page is 22 draws of a 1181x1772 `DeviceRGB` JPEG reduced 2.67x-8.0x. PDFium box-filters straight onto the *integer* destination rect — `GetUnitRect().GetOuterRect()` at `core/fpdfapi/render/cpdf_imagerenderer.cpp:488-497` feeds the integer `dest_width`/`dest_height` to the area loop at `core/fxge/dib/cstretchengine.cpp:135-172` — one resample, ending on the device grid. We box-filter to the **ceiled fractional** footprint (`crates/pdfrum-render/src/stretch.rs:186` `reduced_len`, `:1103` `prescale` → `reduction_for`), which leaves a residual scale of 0.9990-0.9998 and a subpixel phase, and the backend's bilinear sampler then resamples a second time (quality is Bilinear here: `dh/8 = 83 < (1181x1772)/443 = 4724`, `crates/pdfrum-render/src/image.rs:37-56`). Measured: we carry **76.5% of the oracle's high-frequency energy** page-wide and 58.5% in the worst tile — a page-wide low-pass, no geometric drift. | **inferior — fixed 2026-09-06** | Small and already designed, and now landed. `stretch::snapped_reduction` (`stretch.rs:989`) is exactly this fix and already existed — it reduces to the integer rect so `placement_for` resolves the draw to `Placement::Exact` and the backend never samples again — but it was wired **only to soft-mask planes** (`crates/pdfrum-render/src/image.rs:493`, the `image_en_fqa` fix). The colour path now takes it too, through a `Reduction` type that carries the size and the placement as one value so the two cannot come apart, restricted away from type-3 char procs. **SSIM 0.959381 -> 0.999960**; see the note below the table. |
| **`shading_tcpdf_058.pdf` render** — SSIM 0.9908 vs pdfium-render 0.9962 | **Edge antialiasing coverage — not shading.** 3.72% of pixels differ. Splitting the total absolute difference by the oracle's own gradient magnitude: **95.3% of it sits on 5.3% of pixels, all of them edges**. The flat shaded interiors (137 399 px) are **81% bit-identical**, and where they differ it is 1-2 LSB on one channel (12 838 px at delta 1, 9 825 at delta 2) — function-sample quantisation, not banding and not dithering. The file's 37 shadings are types 2 and 3 only, and run 3e scores us 0.988 / 0.989 on those, *identical to pdfium-render*. The worst tiles are glyph edges (`www.tc…`, `TCPDF Ex…`), same outlines, different edge coverage. Our AA: `crates/pdfrum-raster-agg`; PDFium's: analytic AGG coverage, `core/fxge/agg/fx_agg_driver.cpp`. | **tradeoff** | So the answer to "shading dithering, edge antialiasing, or function sampling density" is **edge antialiasing**, measured. Note pdfium-render is 0.9962 and not 1.0 on the *same* file, which is the pypdfium2 `libpdfium.so` being a different build from `pdfium_test` — so part of the 0.005 gap is not ours to close. Matching PDFium's AA exactly means reproducing its scanline coverage arithmetic bit for bit; §3.1 of `docs/design/mupdf-comparison.md` is the cost side. |
| **`image_jpx_123.pdf` render** — SSIM 0.9874 vs pdfium-render 1.0000 | **Upstream JPX decoder numerics** — confirmed, previously ruled at the internal working notes "M21 losses". 96.99% of pixels differ but the *modal* delta is 6 (2 per channel) and the per-channel signed bias is R **-0.20**, G **+1.75**, B **-1.23** — a sub-2-LSB systematic offset with no structure. High-frequency energy matches the oracle to **0.06%** (104.29 vs 104.35), so there is no blur, no geometry error and no resample difference; only 1.92% of pixels exceed \|d\|>12 and those correlate with edges (r=0.70), i.e. where wavelet reconstruction rounding diverges most. That is the signature of the irreversible MCT and the 9/7 inverse DWT rounding differently, not of a decode defect. PDFium: `opj_mct_decode_real`, `third_party/libopenjpeg/mct.c:329-338` (f32 `y + v*1.402f`, `y - u*0.34413f - v*0.71414f`, `y + u*1.772f`) and `third_party/libopenjpeg/tcd.c:2350`. Ours: `hayro-jpeg2000`, a different but equally legal rounding of the same normative equations. | **tradeoff** (third-party) | Both roundings are legal under ISO/IEC 15444-1 for the irreversible path, which specifies real arithmetic without pinning a fixed-point schedule. Closing it means either vendoring OpenJPEG's exact f32 schedule into a Rust JPX decoder or taking a C dependency — the latter gives up `cargo add pdfrum` with no native build crate (run 3d), which is a headline property. Not proposed. |
| **`text_quick_start.pdf` text** — F1 0.641, unmoved by the harness fix | **The dot-leader hypothesis is refuted** — leaders are one line on both sides. Two causes, in order of size. (1) **The harness compares the wrong streams**: `pdfium_test --txt` writes the unfiltered char list (`testing/pdfium_test/write.cc:364-370`), our harness writes `Display` = `search_text` (`benches/compare/src/engines/pdfrum.rs:109`, documented as *not* the `chars` stream at `crates/pdfrum-text/src/lib.rs:417-419`). (2) A **spurious generated space** at object boundaries — leading, before the leader run, before the page number, trailing, plus ~14 whitespace-only lines. The rules are transcribed identically on both sides; the input is not — `GetCharWidth` returns `int` (`core/fpdftext/cpdf_textpage.cpp:185-208`), our `ladder_char_width` returns `f32` (`crates/pdfrum-text/src/object.rs:176-202`). | **inferior — fixed 2026-09-06 (M28)** | **Closed, and the other text rows with it.** The cause was ours, not a positional difference in the oracle. The degenerate-object gate in `crates/pdfrum-text/src/pipeline.rs` carried an extra `run.advance < SIZE_EPSILON` conjunct beside the box test, ruled an `[oracle-bug]` on the reasoning that PDFium's `fabs(GetRect().Width()) < kSizeEpsilon` (`core/fpdftext/cpdf_textpage.cpp:886`, `:1081`) loses a spaces-only object's `w0` word separator. Instrumenting `ProcessInsertObject` in a private PDFium build refuted that: the separator is **not** lost, because `GenerateSpace` already emits a space wherever the geometry calls for one, so keeping the object as well double-counted it. Replacing the conjunct with the plain box test — plus a page-level rescue for the one case the upstream report actually pins, so `whitespace.pdf` keeps its space — took the benchmark from **22 to 42 of 44 byte-exact with none lost**, whitespace-normalized 41 → 43, and the board's text pages **1785 → 2020 of 2067** (non-empty 760 → 990 of 1005) with **0 rows down**. See "The generated space was our own rescue" below, which also records the one loss left on the table (`bug_921.pdf`) and why. |
| **`text_tcpdf_055.pdf` text** — F1 0.953, **0.948** against the `chars` stream (4 peers closer) | Two residuals after correcting the stream. The same spurious leading space; and one row where we emit `^@ ^A ^B … ^_` and PDFium emits nothing — those C0 codes are legitimately in `char_list_` on both sides, so PDFium's empty row is an input-geometry or charcode-mapping difference, not a filtering rule. | **inferior — fixed 2026-09-06 (M28)** | **Closed, and the other text rows with it.** The cause was ours, not a positional difference in the oracle. The degenerate-object gate in `crates/pdfrum-text/src/pipeline.rs` carried an extra `run.advance < SIZE_EPSILON` conjunct beside the box test, ruled an `[oracle-bug]` on the reasoning that PDFium's `fabs(GetRect().Width()) < kSizeEpsilon` (`core/fpdftext/cpdf_textpage.cpp:886`, `:1081`) loses a spaces-only object's `w0` word separator. Instrumenting `ProcessInsertObject` in a private PDFium build refuted that: the separator is **not** lost, because `GenerateSpace` already emits a space wherever the geometry calls for one, so keeping the object as well double-counted it. Replacing the conjunct with the plain box test — plus a page-level rescue for the one case the upstream report actually pins, so `whitespace.pdf` keeps its space — took the benchmark from **22 to 42 of 44 byte-exact with none lost**, whitespace-normalized 41 → 43, and the board's text pages **1785 → 2020 of 2067** (non-empty 760 → 990 of 1005) with **0 rows down**. See "The generated space was our own rescue" below, which also records the one loss left on the table (`bug_921.pdf`) and why. |
| **`text_bug_1029.pdf` text** — F1 0.957, **1.000** against the `chars` stream | Almost entirely the harness stream defect (see the row above): via `Display` we lost the whole first line, via the `chars` stream it is present and the soft hyphen matches. The residual is one trailing space before each `\r`. | **harness defect** + **inferior** (small residual) | Closed: the harness fix took this row to 1.000; the trailing space did not survive into the char stream. |
| **`mixed_en_uicase.pdf` text** — F1 0.991, **1.000 and byte-exact** against the `chars` stream | **Pure harness artefact — the engine already matches.** Against the `chars` stream our output is byte-identical to the oracle modulo the BOM. Both engines write `0x2` to the record and `U+00AD` to the buffer (`crates/pdfrum-text/src/pipeline.rs:962-967` / `core/fpdftext/cpdf_textpage.cpp:1359-1361`) and both exempt `kHyphen` from `IsControlChar` (`cpdf_textpage.cpp:134-147` / `crates/pdfrum-text/src/charinfo.rs:93-108`). | **harness defect** — no engine loss | Closed: the harness fix took this row to 1.000, byte-exact, as predicted. |
| **`image_ccitt_3bigpreview.pdf` text** — F1 0.994, **0.999** against the `chars` stream | The same spurious generated space, in the vertical CJK region: `Clips Clips` vs `Clips` on line 4, and four blank lines that are a lone space each. | **inferior — mostly fixed 2026-09-06 (M28)** | The generated-space half is closed by the `run.advance` fix described below: F1 0.994 → **0.9988**, and the four blank lines that were a lone space each are gone. The **remaining** difference is not a space — we emit `Clips` twice where the oracle emits it once, in the vertical CJK region, which is a duplicate-object question rather than a spacing one. It is the only one of the 44 still not byte-exact. |
| **Warm open** — 0.14 ms median vs hayro 0.04, pdf-rs 0.12 | **The xref chain, and an eager page count.** Callgrind, `profile --op open --file text_quick_start.pdf`, 5 351 895 `Ir` whole process; `Document::from_bytes` (`crates/pdfrum/src/document.rs:148`) is 4 545 569 (84.9%). Split: **`xref::chain::load` 3 524 549 = 77.5% of open** (`crates/pdfrum-parser/src/xref/chain.rs`), of which `read_stream_section` 2 151 836 (47%) inflates the xref stream and `Xref::add_compressed` 1 398 422 (31%) inserts one `BTreeMap<u64, XEntry>` entry per compressed object — `BTreeMap::insert` alone is 1 271 375 `Ir` (28% of open) and `Xref::merge_up` (`crates/pdfrum-parser/src/xref/mod.rs:305-320`) a further 1 087 558 (24%), re-inserting every key of every section while walking `/Prev`. Then **`catalog_page_count` 645 606 = 14.2%** (`crates/pdfrum-parser/src/doc.rs:443-465`): `page_count_of` → `count_subtree` walks the page tree at open so `page_count()` is infallible and O(1) afterwards. hayro's open does not build a complete, merged, generation-checked object index or count pages eagerly. | **more work** (the page-tree walk) + **inferior** (the `BTreeMap`) | The 14.2% page walk is a deliberate API property — `Document::page_count()` returns `u32`, not `Result`, and run 3b shows we recover the whole `FRC` family of damaged trailers that lopdf, pdf-extract and pdf-rs cannot open (46, 46 and 72 files of 209). That is the "what we get". The **52% of open spent in `BTreeMap` insert plus `merge_up`** is not a tradeoff: a sorted `Vec<(u64, XEntry)>` built once and merged by a single descending-priority pass, or a `HashMap`, would remove most of it with no behavioural change. That is the fixable half and it is the larger one. **Fixed 2026-09-06** — the internal working notes. The table is now a slot vector indexed by object number (`EntryTable`, `crates/pdfrum-parser/src/xref/mod.rs`): `BTreeMap::insert` is gone from the profile, `merge_up` fell 1 087 558 → 127 989 and `add_compressed` 1 398 475 → 110 450. Open `Ir` on `text_quick_start` 5 350 120 → 2 713 090 (-49.3%), and on `forms_widgets_407` — the corpus's largest cross-reference at 9950 objects — 14 561 868 → 4 791 409 (-67.1%). Render and text `Ir` improved too, so lookups did not regress. Board: 0 changed rows of 1759, all 770 `FRC`/`bug_*` damaged-file rows unchanged. The 14.2% page walk stays, as argued above; with the container gone `catalog_page_count` is the largest item left in open at 23.6%. **A second pass, 2026-09-06, closed what the first one exposed:** with the per-entry cost gone, the largest single item in open became `__memcpy_avx_unaligned_erms` at **23.2%**, because a slot vector is copied in bulk where a `BTreeMap` was copied node by node. Two copies were redundant. `xref::chain::load` took the body as `&[u8]` and re-wrapped it with `Arc::from`, duplicating a document the caller already held reference-counted — 3.3 MB on `forms_widgets_407`. And `ObjectStore` owned its `Xref` by value, so each of the two stores an open builds deep-copied the whole table. Both now share: `chain::load` takes `&Arc<[u8]>`, `ObjectStore` holds `Arc<Xref>`. Open on `forms_widgets_407` 78 405 011 → **73 130 006** `Ir` for twenty iterations, of which `memcpy` 18 148 589 → **7 580 645** (23.2% → 10.4%). The public `read_xref` keeps its `&[u8]` and pays the one reference count itself. Board: 0 changed rows of 1759. **This is the case where an instruction count and a wall clock disagree, and both are right:** an AVX copy moves 32 bytes per instruction, so a megabyte-scale `memcpy` is cheap in `Ir` and expensive in time, and it evicts every cache line the parse wanted. The first pass cut open's instructions by 67.1% on this file and its wall clock went the other way. |
| **Warm text** — 0.36 ms median vs PDFium 0.04 | **The page build is real but is *not* skippable work.** the internal working notes recorded the build as 39.3% of `text_tcpdf_063` and called it "work no text consumer reads". Re-measured at `e08d7cc` (callgrind, `profile --op text --iterations 1`) that has moved and the reading has changed: build is **26.7% of `text_tcpdf_063`** (59.3 M of 222.2 M) and **44.8% of `text_quick_start`** (61.8 M of 137.8 M). But the build's own split says almost none of it is graphics. On `text_tcpdf_063`: `show_text` 24.7 M (41.6% of the build), `FontCache::get_or_load` via `find_font` 25.3 M (42.7%), `parse_content` + tokenize 23.4 M, `add_form` 4.1 M — **all four are prerequisites for text**. On `text_quick_start`: font load 46.2 M of the 61.8 M build (75%), `parse_content` 22.1 M; non-text graphics work is under 3% of the build. Ours: `crates/pdfrum-page/src/build.rs` `interpret_streams`. PDFium's cheaper answer: `CPDF_TextPage` reads a page object list PDFium builds once and caches on `CPDF_Page`, so a second text call pays nothing (`core/fpdftext/cpdf_textpage.cpp`). | **tradeoff**, not "more work we could skip" | This corrects `text-perf.md` §6 and answers the brief's question directly: **a text-only build mode is not worth writing.** Measured, it would save under 3% of the build on the two files profiled, because the build on a text page *is* text work — content lexing, font loading and `show_text`. The remaining gap to PDFium's 0.04 ms is font loading (33.6% of the whole `text_quick_start` run) and the per-run rebuild, and the lever there is caching the built page across calls, not a narrower build. The M13 §23.4 extreme case (`image_bug_583804`, 87% `image::unpack`) **did not reproduce**: page 1 of that file profiles at 1 114 966 `Ir` total and has no images, so that row belongs to a later page and is not this median's cause. |
| **Warm render** — 10.55 ms vs mupdf 4.18, pdfium-render 5.68, hayro 8.78 | Fully explained in `docs/design/mupdf-comparison.md` — §1.1 (`vello_cpu`'s `F32Kernel::pack`, a fixed cost per render), §1.2 (the backend, not the render crate, carries the gap), §4 (the seven things we compute that mupdf does not). **Do not re-derive here.** | **tradeoff** + **inferior**, split per §6 of that doc | §6 is the ranked plan: (a) portable with no pixel change, (b) changes pixels inside the PDFium floor, (c) mupdf's different answer, do not propose. |
| ~~**Render memory** — 1 539 MiB peak on `image_bug_583804.pdf` (every peer < 1 GiB); median 51.50 MiB~~ **Fixed at `56a01ac`: 916 MiB peak, 30.95 MiB median.** | The cost was never the image. The page is 4473 pt square, so at 150 DPI the target is 9318x9318 and **one** page-sized RGBA8 buffer is 331 MiB — and three existed at once. `VelloCpuDevice` allocated a `base` pixmap eagerly, read it once to seed the target and dropped it; `rasterize` then built a `vello_cpu::Pixmap`, rendered into it, and copied it back out. The seed is now a `Seed::{Solid, Backdrop}` enum, so a uniform clear colour is four bytes rather than a buffer, and `vello_cpu` rasterizes through a `PixmapMut` borrowed over the result buffer itself, so the second pixmap and the copy are both gone. Measured side by side under the same load with `benches/compare child`, peak `VmHWM` less the process floor: **1596 -> 916 MiB** on that file, corpus render median **49.73 -> 30.95 MiB**, and it is faster too (cold median 31.53 -> 22.03 ms, warm median 14.64 -> 12.14 ms). Board unchanged: zero rows whose pixel result moved across 1759 files. | **fixed** | Two of the three M27 items were not needed and are not done. **Decoding to the drawn size** buys nothing on this file — the image is 4473x4473 drawn into 9318x9318 device pixels, so it is already drawn above 1:1 and there is nothing to reduce; it remains the right change for a page whose images are larger than their destination. **Banding** was not needed to clear 1 GiB and was not attempted. **The cache budget rule** was tried and measured: dropping the rendered-pixmap cache's single-entry exemption saves 20 MiB and costs 14% warm and 42% cold on this very file — the regression `6af7a2a` was written to fix, reproduced. The exemption stays. |
| **Cold render** — 41.90 ms vs pdfium-render 10.75, hayro 13.13, mupdf 15.05 | **Diagnosed and largely fixed**. The gap was kernel time, not instructions: every benchmark `run` passes `--font-dir`, and the scan of the oracle's hermetic `test_fonts` read 33.8 MB of font files whole to reach two small tables per face. Since 2026-09-06 the scan reads only the `name` and `OS/2` byte ranges — **34.9 MB in 89 `read` calls becomes 1.14 MB in 182**, syscall time 22.0 → 10.1 ms, and the 44-file cold/warm ratio 3.83x → 2.69x, inside the peers' 1.5–3.6x band. 0 changed board rows. hayro bundles its fonts and scans nothing, so its cold column never included this work at all. | **largely fixed** | the internal working notes |
| **Warm regression on `image_bug_583804`** — reproducibly 0.76x across `cd1a511` | **In progress**, not diagnosed in this pass. `the internal working notesqueue.md:824` has the record. | **in progress** | Carried by the open work list (`docs/issues-to-file.md`). |
| `vector_en_system.pdf` render — SSIM 0.9600 (run 3) | **Already fixed on main.** the internal working notes, "The last two, fixed 2026-09-06 — and neither was the mask". Run 3's score is the pre-fix baseline. | fixed | — |
| `image_en_fqa.pdf` render — SSIM 0.9769 (run 3) | **Already fixed on main.** Same record; the fix is `stretch::SnappedReduction` wired at `crates/pdfrum-render/src/image.rs:493`. | fixed | — |
| `fx/image/1_image.pdf` render — SSIM 0.9857 (run 3b) | **Already fixed on main.** the internal working notes, "The two fixes, landed"; the diagnosis in the M21 table (a filter-selection defect) is corrected there. | fixed | — |
| `fx/path/transparent1.pdf` render — SSIM 0.9844 (run 3b) | **Already fixed on main.** Same record — the knockout buffer at `core/fxge/cfx_renderdevice.cpp:806`, `:873-875`. | fixed | — |
| Nine `fx/FRC_8.2.4_part1/*` text rows — F1 0.889 each (run 3b) | **Already ruled oracle-bug.** PDFium emits a U+0003 control character between "Foxit" and "PhantomPDF"; we do not. `docs/benchmarks/README.md`, "Reading run 3b". | **oracle-bug** | Nothing to close. A control character is not text. |
| `fx/FRC_3.5_part1/…SubFilter_s5.pdf` open — pdf_oxide opens it, we error | Public-key (`Adobe.PubSec`) encryption is not implemented. `mupdf` and `pdfium-render` also decline it (`docs/benchmarks/README.md`, "Reading run 3b"). | **more work** (unimplemented) | A PKCS#7 recipient-list decryptor and a certificate store. One file of 209, and the two C engines decline it too. |
| `third_party/tcpdf/example_025.pdf` render — SSIM 0.9913 vs pdfium-render 0.9989 | **Not determined.** Above the 0.99 conformance floor and outside this pass's six named files; not measured. | not determined | — |

---

## Text — the five files

**All five share one cause before any engine question is reached: the
benchmark harness compares the wrong two streams.**

`pdfium_test --txt` does not emit `CPDF_TextPage`'s assembled page text. It
loops the **character list**, unfiltered:
`testing/pdfium_test/write.cc:364-370` writes `FPDFText_GetUnicode(textpage, i)`
for every `i` in `FPDFText_CountChars(textpage)`. Our harness emits
`page.text_on(&mut session).to_string()`
(`benches/compare/src/engines/pdfrum.rs:109`) — the `Display` impl, which
`crates/pdfrum-text/src/lib.rs:417-419` documents in as many words as
"the `search_text` — **not** the `chars` stream, which holds the control
characters and placeholders this drops."

So the published F1s compare PDFium's char stream against our *search*
buffer. Re-diffing our `chars` stream against the oracle changes every one
of the five rows, and one of them to an exact match.

| file | published F1 | measured F1 vs `chars` | what the diff shows against the `chars` stream | verdict |
|---|---|---|---|---|
| `mixed_en_uicase.pdf` | 0.991 | **1.000** | **Byte-identical to the oracle** modulo the BOM. Via `Display` the diff was the soft hyphen: oracle `Cloud^BMenu`, ours `Cloud\u{ad}Menu`. Both engines write `0x2` to the record and `U+00AD` to the text buffer (ours `crates/pdfrum-text/src/pipeline.rs:962-967`, PDFium `core/fpdftext/cpdf_textpage.cpp:1359-1361`), and both exempt `kHyphen` from `IsControlChar` so the `0x2` survives in the char list (`cpdf_textpage.cpp:134-147`, ours `charinfo.rs:93-108`). The engine already agrees. | **harness defect** — no engine loss |
| `text_bug_1029.pdf` | 0.957 | **1.000** | Via `Display` we dropped the whole first line; via `chars` it is present and the soft hyphen matches. Residual: one trailing space before each `\r` (`…commits the ` vs `…commits the`). | **harness defect** + the generated-space bug below |
| `text_quick_start.pdf` | 0.641 | 0.641 | **The dot-leader hypothesis is refuted.** Leaders are one line on both sides — oracle `Different Views......2`, ours ` Different Views ...... 2 `. The whole 0.641 is spurious spaces: leading, before the leader run, before the page number, trailing, plus ~14 whitespace-only lines that are themselves made of those spaces. No line-merging difference exists. | **inferior** — the generated-space bug below |
| `image_ccitt_3bigpreview.pdf` | 0.994 | 0.999 | `Clips Clips` vs `Clips` on line 4, and four blank lines (a lone space each) in the vertical CJK region. Same spurious space. | **inferior** — same bug |
| `text_tcpdf_055.pdf` | 0.953 | 0.948 | Two residuals. The same leading space on two rows; and one row where we emit `^@ ^A ^B ^C … ^_` and PDFium emits nothing. Those C0 codes are legitimately in `char_list_` on both sides — PDFium's row is empty because its own extraction placed nothing there, which is an input-geometry or charcode-mapping difference, not a filtering rule. | **inferior** (space) + **not determined** (the C0 row — isolating it needs `GetCharWidth`/`GetPos` instrumented per item in a PDFium build, which this read-only pass could not do) |

**None of the five is an oracle-bug.** The nine-file U+0003 FRC 8.2.4 ruling
does not extend here: the `^B` / `^C` bytes in the oracle's output are
correct-by-design char-list contents under `IsControlChar`'s `kHyphen`
exemption, not PDFium misbehaving.

### The one engine bug: a generated space PDFium does not generate

Every residual above is the same rule firing on our side and not on
PDFium's, at text-object boundaries. The decision logic is a faithful
transcription — checked line for line, and identical on both sides:

| decision | ours | PDFium |
|---|---|---|
| whether a space is generated | `crates/pdfrum-text/src/pipeline.rs:1139-1158` | `GenerateSpace`, `cpdf_textpage.cpp:231-249` |
| the object-boundary dispatch | `pipeline.rs:696-788` | `ProcessInsertObject`, `cpdf_textpage.cpp:1194-1317` |
| the two float-equality threshold hacks | `pipeline.rs:779-783` | `cpdf_textpage.cpp:1310-1313` |
| finding the previous run | `pipeline.rs:682-692` | `FindPreviousTextObject` / `GetPrevCharInfo`, `cpdf_textpage.cpp:1046-1055`, `:1187-1191` |

So the divergence is in the **inputs to those rules, not the rules**. The
identified candidate was a type: PDFium's `GetCharWidth`
(`cpdf_textpage.cpp:185-208`) returns **`int`** at all three of its exits —
`font->GetCharWidth`, `GetStringWidth`, and `std::max(rect.Width(), 0)` —
and the value stays integral through `std::max(nLastWidth, nThisWidth)`
(`:1303`) and `nLastWidth * GetFontSize() / 1000` (`:1234-1238`). Our
`ladder_char_width` (`crates/pdfrum-text/src/object.rs`) is that function
transcribed exit for exit but returning **`f32`**, which was read as keeping
a fraction PDFium truncates.

#### That candidate is refuted: the width was never fractional

**Built and measured, and it is a no-op.** The ladder's output is now an
integer newtype, `GlyphWidth`, truncating toward zero exactly as
`FX_Number::GetSigned`'s `saturated_cast<int32_t>` does
(`core/fxcrt/fx_number.cpp:97-105`, reached from `CPDF_Array::GetIntegerAt`
at `core/fpdfapi/parser/cpdf_array.cpp:147-151`). It changed **nothing**:

- the 1 759-file conformance board is byte-identical, every row, both
  totals and every per-file entry;
- all 44 benchmark text rows are unchanged, `text_quick_start` included.

The reason is that **we already truncate where PDFium does**, one crate
earlier. `pdfrum-font` stores a simple font's `/Widths` as `raw: [u16; 256]`
filled through `Array::int_at` (`crates/pdfrum-font/src/widths.rs`), a CID
font's `/W` as `records: Vec<[i32; 3]>`, and the face fallback as
`f32::from(advance_tt(gid) as i16)`
(`crates/pdfrum-font/src/simple/mod.rs:111-130`). Every value reaching the
ladder is a whole number that happens to be carried in an `f32`. Instrumented
to report any fractional input, the truncation **fired zero times across
1 420 PDFs** — the 44-file benchmark corpus and the 1 376-file conformance
corpus.

The newtype is kept regardless, because it turns that agreement from a
coincidence of two crates into an invariant the compiler holds. But it is not
the cause of the generated space.

**`text_quick_start`'s 0.641 is diagnosed, and it was none of the three.**
The instrumentation the previous passes could not do was done (M28,
2026-09-06) and is described in "The generated space was our own rescue"
below. It was not `GetPos`, not the text-matrix composition and not
`FindPreviousTextObject`: every one of those inputs agreed with the oracle's
to six decimal places. The divergence was one conjunct in **our own**
degenerate-object gate, and removing it takes this file to 1.000 and
byte-exact on all eleven of its pages.

Note also that no corpus file here carries a fractional declared width at
all, so the oracle-bug question this raised — ISO 32000-1 §9.2.4 Table 111
gives `/Widths` as *numbers*, and PDFium's integer read would lose a
fractional one — stays hypothetical and is not ruled either way. It would
not be an oracle bug worth diverging on in any case: PDFium uses the same
integer to *position* the glyphs
(`core/fpdfapi/page/cpdf_textobject.cpp:185`, `:250-255`, `:333`), so it is
the geometry actually on the page that the extractor's heuristic asks about.

A second, unrelated input difference is documented at
`crates/pdfrum-text/src/object.rs:23-29`: PDFium threads `form_matrix`
separately where we fold a form's `/Matrix` into the CTM upstream. That is a
deliberate design choice and can only matter for text inside form XObjects,
which is not what these five files are.

### The generated space was our own rescue

*M28, 2026-09-06.* The three named candidates — `GetPos`, the text-matrix
composition, and `FindPreviousTextObject`'s choice of previous run — were
settled by instrumenting the oracle rather than by reasoning about it, and
all three were refuted.

**Method.** A private PDFium tree was made with `cp -al` from the read-only
oracle checkout, so the copy is hardlinked and costs no disk; `ninja` unlinks
before it writes, so the oracle's own files and `out/Release/pdfium_test`
were never touched (verified afterwards by inode and mtime, and again by the
conformance runner's own clean check). `ProcessInsertObject` in
`core/fpdftext/cpdf_textpage.cpp` was made to dump, per call and behind a
`PDFRUM_DUMP` environment guard, the object and previous-object pointers,
the writing mode, `GetPos()`, the text matrix, the previous text matrix, the
form matrix, the stored `prev_matrix_`, both rectangles, both font sizes,
and — at every one of the function's seven exits — which exit was taken,
with `pos`, `last_pos`, `this_width`, `last_width`, the integer
`GetCharWidth` results and both thresholds.

**What it showed.** On `text_quick_start.pdf` the positional inputs matched
ours everywhere. What did not match was which objects reached the function
at all. PDFium's loop opens with

```cpp
if (fabs(text_obj->GetRect().Width()) < kSizeEpsilon) {
  continue;
}
```

and ours read

```rust
if run.advance < SIZE_EPSILON && run.rect.width().abs() < SIZE_EPSILON {
```

That extra conjunct was a deliberate `[oracle-bug]` ruling: a text object
made only of spaces has empty glyph boxes but a non-zero `w0` (ISO 32000-1
§9.4.3 displacement, §9.2.2 bounding box — the standard keeps them
distinct), so PDFium was held to be dropping a legitimate word separator.

**The ruling was wrong, and the measurement is what says so.** The separator
is not lost when the object is dropped, because the inter-object rules
underneath — `decide` / `GenerateSpace` — already emit a space wherever the
geometry between two objects calls for one. Keeping the spaces-only object
*as well* emitted the separator twice. That is the entire "spurious
generated space": every difference on `text_quick_start`'s eleven pages was
a pure insertion by us of a space or a whitespace-only line, with no
deletion and no reordering anywhere.

**Scored across everything, per the rule that a fix which raises one file
and lowers another is not a fix:**

| | before | after |
|---|---|---|
| benchmark, byte-exact | 22 / 44 | **42 / 44** |
| benchmark, whitespace-normalized | 41 / 44 | **43 / 44** |
| `text_quick_start.pdf` F1 | 0.641 | **1.000** |
| `text_tcpdf_055.pdf` F1 | 0.948 | **1.000** |
| board text pages matched | 1785 / 2067 | **2020 / 2067** |
| board non-empty text pages | 760 / 1005 | **990 / 1005** |
| board files passing | 1551 / 1759 | **1675 / 1759** |

20 benchmark files gained and **none was lost**; on the board 125 files
gained text pages and **none lost any**, 124 files changed status and all
124 upward. The whitespace-normalized count *rising* is the direct evidence
that no word boundary was destroyed — had the dropped objects been carrying
separators, that column would have fallen.

### The ruling this reversed was not wholly wrong, and the bound matters

The conjunct was not a guess. It was ruled against a filed upstream report —
`crbug.com/40643656`, `crbug.com/444176962` — with `whitespace.pdf` committed
as its fixture, and three `pdfrum-text` embedder tests pinning it. A first
attempt at this change removed the conjunct outright and left those three
tests failing; that is corrected here.

**Both halves are true, and the split is by page rather than by object.**

- *With a neighbour*, the ruling is wrong. `GenerateSpace` already emits the
  separator from the gap the object sits in, so keeping the object emits it
  twice. `hello_world_with_invisible_spaces.pdf` is that shape, and so are
  all 20 corpus files: its assertion is back to the oracle's plain string.
- *Without a neighbour*, the ruling is right. When the page's single text
  object draws only spaces there is no gap for the heuristic to span, and
  PDFium's box gate discards the page's only content — `whitespace.pdf`
  extracts as empty where a space is plainly drawn. `[oracle-bug]` stands,
  and `pipeline::Builder::keep_spaces_only` keeps it.

That rescue is deliberately the tightest bound that covers the report. It
costs one benchmark file — `vector_en_system.pdf`, a corpus page of exactly
this shape where we emit the space and the oracle emits nothing — which is
the difference between 42 and 43 of 44, and it is a divergence we choose
rather than one we failed to notice. Two looser bounds were measured and
both lose more: rescuing *any* page with no width-occupying object also
rescues multi-object whitespace pages, costing `image_en_fqa.pdf`,
`vector_en_system.pdf` and `text_tcpdf_055.pdf`; rescuing any single-object
page also rescues `bug_651304.pdf`, whose one object draws `U+0001`, and
that moves a board row *down*.

### The letters PDFium drops, and how they are told apart

The same gate discards objects that draw **letters**, not only spaces. On
`bug_921.pdf` PDFium's `--txt` begins mid-sentence at "разве не выражает"
where the page draws "И разве не выражает"; five objects of this shape carry
an `И`, an em dash, a `в`, a `я` and a second `И`. `bug_665467.pdf` loses a
`Л` the same way and extracts as empty.

**This was first reported here as unfixable, and that was wrong.** The claim
was that these objects are identical to the C0 control runs the oracle
*rightly* drops in `text_tcpdf_055.pdf` and `bug_651304.pdf` — empty boxes,
no `ToUnicode` mapping, comparable `w0`. The mapping is indeed empty for
both. The **character codes** are not, and PDFium itself falls back to them
(`cpdf_textpage.cpp:1213-1215`, `unicode += static_cast<wchar_t>(char_code_)`):

| fixture | font | mapping | codes | verdict |
|---|---|---|---|---|
| `bug_921.pdf` | `FooFont` | empty | 1048 `И`, 8212 `—`, 1074 `в`, 1103 `я` | **kept** |
| `text_tcpdf_055.pdf` | `Courier` | empty | 0..=31 | dropped |
| `bug_651304.pdf` | (none) | empty | 1 | dropped |
| `bug_491516663.pdf` | `Test` | `U+200B` | 1 | dropped (`w0` 0) |
| `annots/annotation_*.pdf` | `ArialMT` | `U+00A0` | 3 | dropped (whitespace) |

So `object::gate` keeps an empty-box object when the pen moved *and* it shows
a scalar that is neither whitespace nor a C0/C1 control. `U+00A0` matters:
fifty annotation fixtures draw one, and a rule that tested only `U+0020`
put a trailing space on every one of them.

**Ruled by the user, 2026-09-06**, per the standing oracle-bug rule:
implement the correct behaviour, cite both sides, and bucket the golden as
not-achievable rather than match the defect. Four board files are now
deliberately not-achievable on their text artifact — `bug_921`,
`bug_665467`, `bug_1449` and two pages of `example_055` — and **all four
already failed on `main`**, so nothing regressed: the board shows 0 rows
down against the baseline. `example_055` in fact goes from 0 of 14 matched
pages to 12 of 14.

**`text_tcpdf_055`'s C0 row is closed by the same change**, and it was
neither a filtering rule nor a charcode mapping: the `^@ ^A … ^_` run lived
in an empty-box object that PDFium had already discarded before extraction,
so its row is empty for the same reason every other row differed. Its
character codes are 0..=31, which is what separates it from `bug_921.pdf`'s
recovered letters — see the section below.

**What remains.** `image_ccitt_3bigpreview.pdf` at 0.9988 is the only one of
the 44 still not byte-exact, and its residual is no longer a space: we emit
`Clips` twice in the vertical CJK region where the oracle emits it once.
That is a duplicate-object question and is not part of this defect.

`TextRun::advance` had no reader left once the conjunct went, and was
removed with it (a public field, so `docs/api-baseline/pdfrum-text.txt`
records the removal). The gate is now `object::gate` returning an
`ObjectGate`, with the page-level exception passed in, so the two reasons an
object survives cannot be confused.

Five tests moved. The two that asserted 45 words on `text_quick_start`'s
first page now assert **33**, the oracle's own count — the 45 was counting
the spurious spaces as separators. `invisible_spaces_are_extracted_as_the_
spaces_they_draw` is back to the oracle's plain string, and
`a_cyrillic_run_comes_out_in_order` back to the oracle's 268; both carry the
reasoning above in their own comments. `a_whitespace_only_page_yields_the_
one_space_it_draws` is unchanged and still passing, which is the point of
the page-level rescue.

### What this means for the board

**Done.** `benches/compare/src/engines/pdfrum.rs` now emits the `chars`
stream's unicodes rather than `Display`, by the same construction the
conformance runner's tier-A `--txt` dump uses
(`crates/pdfrum-tool/src/text.rs::to_utf32le`), so the harness and the board
compare the same bytes. Only our row moved — each other engine's adapter was
written against whatever that engine calls its text output and none was
touched. `mixed_en_uicase` went to 1.000 and byte-exact on the harness fix
alone, as predicted; `text_bug_1029` also went to 1.000. `text_tcpdf_055`
moved *down* 0.005, because the C0 run the search text was hiding is in the
char list and the oracle's row there is empty — that is the row's own "not
determined" residual, now visible rather than masked. `text_quick_start` did
not move at all, which localises its whole 0.641 to the generated space and
not to the stream. The corrected column is published in
`docs/benchmarks/README.md` under run 3a.


---

## Notes under the table

### `vector_tcpdf_009` — the fix already exists, on the wrong path

This is the clearest row in the table, and worth stating plainly: the
technique that closes it is already in the tree, already validated, and
already landed for a different caller.

`crates/pdfrum-render/src/stretch.rs` is a box-filter pre-pass whose module
doc (`:30-42`) is candid that it is a *two-stage approximation* — reduce to
whole pixels here, hand "the fractional scale, the rotation, the shear, the
subpixel placement" to the backend — and that the two engines therefore
"differ by at most the last two-tap interpolation". `vector_tcpdf_009` is
the measurement of what "at most" costs when the page is 22 photographs: a
23.5% loss of high-frequency energy and 0.036 of SSIM.

`SnappedReduction` (`stretch.rs:948-1017`) was written to remove exactly
that second interpolation, and its own doc comment says so — "one resample
too many, and each one spreads a mask edge by a pixel — `image_en_fqa`'s
whole loss". It reduces to the integer rect's extent so the reduced pixmap
lands one texel per pixel and `placement_for` classifies it as
`Placement::Exact`, drawn with the nearest sampler. There is no second
resample left to phase.

It was called from one place: `reduced_mask_pixmap`
(`crates/pdfrum-render/src/image.rs:493`), the soft-mask plane path. The
colour-image path used `reduction_for`'s ceiled fractional footprint and
still got the second sampler. So the `image_en_fqa` fix closed the mask
half of a defect whose colour half was this row.

The two guards `snapped_reduction` applies (`stretch.rs:1001`) — axis-aligned,
reducing in both axes, neither axis mirrored — are all satisfied by every
`/I2` draw on this page.

**Fixed 2026-09-06.** The wiring is a type rather than a call: `Reduction`
(`crates/pdfrum-render/src/stretch.rs`) makes the reduction size and the
placement transform one value, so a caller cannot pair the size of one rule
with the transform of the other — which is precisely how the two halves came
apart. `render_image` (`walk.rs`) now asks `stretch::reduction` for it and
reads both off the answer. The only restriction the colour path adds beyond
`snapped_reduction`'s own is a type-3 char proc, where the target is a
sub-bitmap on a grid of its own and snapping to the device grid would
quantise twice — the same restriction `image_placement` already carried for
`Placement::Snapped`, now applied one step earlier so the *reduction* is not
snapped either. SSIM on this file **0.959381 -> 0.999960**.

**What it costs, and what it buys.** Reducing onto upstream's integer
destination rect costs this file 42% to 55% on the **cold** render, measured
on `himmel` across the landing commit: agg 45.346 -> 64.520 ms, tiny-skia
41.475 -> 64.233 ms, vello_cpu 42.632 -> 65.091 ms. The cost is paid in
`convert_and_reduce`, which now box-filters to each draw's own snapped size
rather than to a shared ceiled footprint, so more distinct sizes are filtered
and the source is read 1.75x more often (`image/rows.rs:convert_row`,
44,627,116 -> 77,921,224 `Ir`). The JPEG decode count is unchanged —
`image::decode_dct` is byte-identical at 96,178,993 `Ir` — so this is filter
work, not repeated decoding, and it is inherent to reproducing upstream's
per-draw geometry.

What it buys is the second resample, which disappears: the reduced pixmap
lands one texel per device pixel, `placement_for` answers `Placement::Exact`,
and the backend's bilinear sampler falls from 13.4% of the page's instructions
to 1.5% (`AggDevice::draw_image` 118,953,754 -> 19,025,956 `Ir`). The **warm**
render — the state a caller who draws the same page twice is in, and the
convention the oracle's own column is taken in — therefore drops by 42% to
74%: agg 6.2435 -> 1.6376 ms, tiny-skia 2.3911 -> 1.3805 ms, vello_cpu 3.5211
-> 2.0615 ms. The three cold rows are raised in `benches/baseline.json` as a
deliberate trade, on that arithmetic and on the SSIM above.

### `shading_tcpdf_058` — the name of the file is not the cause

The brief asked which of three candidates the remaining 0.005 is. The
measurement rules out two of them:

- **Shading dithering**: no. The flat shaded interiors are 81% bit-identical
  and the differences are 1-2 LSB on a single channel, which is what
  quantising a continuous function to 8 bits does, not what dithering does.
  A dither would show a spatial pattern; there is none.
- **Function sampling density**: no, for the same reason — a coarse sampling
  shows as banding with runs of identical pixels meeting at a step, and the
  interiors have no such structure.
- **Edge antialiasing**: yes. 95.3% of the total absolute difference lies on
  the 5.3% of pixels the oracle's own gradient marks as edges, spread evenly
  down the page (20.4% / 19.0% / 23.5% / 37.0% across four bands), and the
  worst tiles are text glyphs.

The file is named for its shadings but its loss is the same antialiasing
difference every page in the corpus carries; it is visible here only because
the rest of the page is so nearly exact.

### Open — which half is fixable

The 0.14 ms is two different things and they deserve different verdicts.

The **page-tree walk** (14.2%) buys a property: `Document::page_count()` is
`u32` rather than `Result<u32>`, and the recovery it runs is why run 3b
opens 208 of 209 PDFium corpus files including the whole deliberately-damaged
`FRC` family, where pdf-rs fails on 72 and lopdf and pdf-extract on 46. hayro
is faster partly because it does less of this. That is a tradeoff with a
stated benefit.

The **`BTreeMap` traffic** is not. `Xref::add_compressed` and `Xref::merge_up`
together are 52% of open on this file, and nearly all of it is
`BTreeMap<u64, XEntry>` insertion and subtree cloning while walking `/Prev`.
Nothing about the semantics requires an ordered map or requires re-inserting
every key of every section: the xref is built once, read many times, and the
priority rule (newer sections win, `/Prev` and `/XRefStm` excepted) is a
single descending pass over sorted sections. A sorted `Vec` or a `HashMap`
would keep the behaviour and remove most of the cost. This is the row's
`inferior` half and it is the bigger one.

### Text speed — the 39% was a share, not a saving

The brief asked whether a text-only build mode is "more work we could skip"
(inferior) or a tradeoff, on the strength of the internal working notes's
"the page build is 39% of a text run because it builds graphics no text
consumer reads".

Re-measured, the second half of that sentence is wrong, and this document
corrects it. The build's *share* is real — 26.7% on `text_tcpdf_063`, 44.8%
on `text_quick_start` — but its *contents* are text work. Broken down, the
build on `text_tcpdf_063` is 42.7% font loading, 41.6% `show_text`, and the
remainder content lexing and form recursion. A text-only mode has under 3%
of the build to skip on either file profiled.

So: **tradeoff**, and more usefully, *not the lever*. The lever visible in
the same profile is font loading — 33.6% of the entire `text_quick_start`
text run sits in `FontCache::get_or_load`, on a page whose fonts are already
memoized within the build. Making the *parse* cheap (the `/ToUnicode`
storage item `text-perf.md` §6 leaves open) and caching the built page
across calls are both worth more than a narrower build mode, and neither is
this row.

---

## Method

Renders: `benches/compare`'s `child` subcommand for ours, so the process,
the PNG encode and the compositing over white are identical to the published
harness —

```sh
<target>/release/pdfrum-compare child --engine pdfrum --op render \
  --file <pdf> --out X.png --warm 0 --font-dir <oracle fonts>
```

— and `pdfium_test --png --md5 --scale=2.0833333333 --font-dir=<oracle fonts>`
for the oracle, both on a scratch copy of the fixture, never in the
read-only oracle checkout. Page 1 at 150 DPI, 1240x1753.

Pixel analysis is numpy over the two PNGs: per-channel signed difference,
total absolute difference partitioned by the oracle's own gradient magnitude
(the "edge" split), and mean absolute first difference in each axis as the
high-frequency energy measure.

Profiles: `benches/src/bin/profile` under callgrind, `--iterations 1`, whole
process, inclusive `Ir` from `callgrind_annotate --inclusive=yes`.

Text: `pdfium_test --txt` for the oracle (UTF-16LE), the harness child
`--op text` for ours.

Builds: a dedicated `CARGO_TARGET_DIR`;
the comparison harness from the warm `m21` target dir.

### A criterion interval is not a reproducibility bound

Established 2026-09-06, while attributing a ratchet run's regressions, and
worth stating because it invalidates the obvious reading of a benchmark
report. A criterion confidence interval bounds the spread of the samples
taken inside **one invocation**; it does not bound the spread across
invocations. On the render benchmarks the two differ by more than an order of
magnitude — a row can read 1.259 ms and 1.529 ms, minutes apart, from the
same binary at the same commit on an idle box, each with an interval under
0.05%.

The mechanism is heap residency carried across benchmark ids within one
process. All 44 documents in a group share one process
(`crates/pdfrum-render/benches/render.rs`), and the pathological image
documents leave a large resident set behind them: `image_bug_583804` alone
takes the bench binary's peak RSS from 27 MB to 521 MB, because the
decoded-image cache is deliberately allowed to retain up to its 100 MiB
budget rather than empty itself (`crates/pdfrum-page/src/image/cache.rs`).
Every row measured after them in the same process pays for that, by a margin
that varies from a few tenths of a percent to several percent with machine
state.

The margin is the same in relative terms on all three backends — measured at
+0.4% each on the pairing above — but only `render-warm-vello-cpu` has a
per-page cost large enough for it to clear the group's five percent band:
1.26 ms on the small shading files against agg's 92 us. That is why a
one-backend cluster is not by itself evidence of a backend-specific cause.

Two rules follow. **A row that moves with a hundredth-of-a-percent interval
and then returns to baseline on a re-run of the same binary at the same
commit has not regressed**; it was measured in a different process state.
And a re-baseline must gate on `pgrep` returning nothing rather than on load
average, which a job between iterations can satisfy while still perturbing
the box.

**The durable lesson is larger than either rule: while the pathological
documents shared a group, the ratchet's bands were tighter than the harness
could resolve.** The bands are 3% to 5%, measured and correct for what they
describe. The perturbation was reaching 20% and more. A threshold below a
harness's own reproducibility does not detect regressions — it manufactures
them, and it manufactures a fresh set each run. Two runs of the same binary at
the same commit on the idle reference box reported 34 and 27 regressions
sharing only **13** rows, and the warm cluster moved bodily from
`render-warm-vello-cpu` (14 rows, 0 on tiny-skia) to `render-warm-tinyskia`
(13 rows, 1 on vello-cpu). No code change can move between rasterizer
backends, so that migration is the measurement, not the engine.

The harness now orders the three heavy documents last within each render group
(`crates/pdfrum-render/benches/render.rs`), which removes the one perturbation
large enough to clear a band. **Rows in those six groups are not directly
comparable across that change**: they previously measured the renderer plus
whatever the allocator was still recovering from, and they now measure the
renderer. The benchmark ids and the bands are unchanged, so the baseline keeps
its history; the numbers in it were re-recorded in the same commit.

What this does not do is make the suite immune. It removes the largest known
source, and the test of whether a band is trustworthy remains empirical: two
consecutive runs at the same commit should agree row for row within it.

### What the 2026-09-06 re-baseline raised, and why

Nineteen rows were raised by `ratchet update --accept-regressions`. **None of
them is a regression between runs.** Seventeen are baseline entries that were
always wrong and that the harness fix made visible by removing the noise that
was masking them; the other three are the `vector_tcpdf_009` cold rows, the
deliberate trade recorded above.

The evidence is that every raised row reproduces across runs, including across
the harness change itself — so the new number is what the file costs and the
old one was not:

| row | old harness | new harness | against baseline |
|---|---|---|---|
| `render-cold-agg/forms_push_button` | 45.32 ms | 45.35 ms | +6.5% |
| `render-cold-tinyskia/forms_push_button` | 55.86 ms | 55.71 ms | +5.4% |
| `render-cold-vello-cpu/forms_push_button` | 50.49 ms | 50.58 ms | +6.1% |
| `render-cold-agg/mixed_tcpdf_006` | 20.61 ms | 20.56 ms | +16.0% |
| `render-cold-tinyskia/mixed_tcpdf_006` | 24.32 ms | 24.34 ms | +16.3% |
| `render-cold-vello-cpu/mixed_tcpdf_006` | 29.11 ms | 29.17 ms | +13.4% |

Two trios, each stable to within 0.3% across a change that reordered the group
they live in, each five to sixteen percent from its recorded baseline. A value
that does not move cannot be regressing. The sharpest case is
`text/vector_paths_1751`, flagged +16.3% against a baseline of 3.545 ms while
measuring 4.1202 ms and 4.1297 ms in two runs a harness change apart —
**p = 0.69**, statistically indistinguishable.

The same reading applies to the `open` group, where it was found first: those
entries were physically implausible for the work involved — an `open` median of
15.6 us against `mixed_formfield` at 3,470 us — and the group is now recorded
between 55% and 96% lower, partly from the `memcpy` fix above and partly
because the old numbers were never right.

The lesson for a reader of this file is the one in the subsection above: a
regression list produced by a harness that cannot reproduce itself is not a
list of regressions. Check that a row moves *between runs* before believing it
moved at all.
