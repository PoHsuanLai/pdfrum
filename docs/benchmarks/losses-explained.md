# Every loss in run 3, explained

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

---

## The table

| what we lose | measured cause, both citations | verdict | what closing it would cost or change |
|---|---|---|---|
| **`vector_tcpdf_009.pdf` render** — SSIM 0.9593 vs pdfium-render 0.9995 | **A second resample.** The page is 22 draws of a 1181x1772 `DeviceRGB` JPEG reduced 2.67x-8.0x. PDFium box-filters straight onto the *integer* destination rect — `GetUnitRect().GetOuterRect()` at `core/fpdfapi/render/cpdf_imagerenderer.cpp:488-497` feeds the integer `dest_width`/`dest_height` to the area loop at `core/fxge/dib/cstretchengine.cpp:135-172` — one resample, ending on the device grid. We box-filter to the **ceiled fractional** footprint (`crates/pdfrum-render/src/stretch.rs:186` `reduced_len`, `:1103` `prescale` → `reduction_for`), which leaves a residual scale of 0.9990-0.9998 and a subpixel phase, and the backend's bilinear sampler then resamples a second time (quality is Bilinear here: `dh/8 = 83 < (1181x1772)/443 = 4724`, `crates/pdfrum-render/src/image.rs:37-56`). Measured: we carry **76.5% of the oracle's high-frequency energy** page-wide and 58.5% in the worst tile — a page-wide low-pass, no geometric drift. | **inferior** | Small and already designed. `stretch::snapped_reduction` (`stretch.rs:989`) is exactly this fix and already exists — it reduces to the integer rect so `placement_for` resolves the draw to `Placement::Exact` and the backend never samples again. It is wired **only to soft-mask planes** (`crates/pdfrum-render/src/image.rs:493`, the `image_en_fqa` fix); the colour path at `stretch.rs:1092` `prescale` does not call it. Extending it to colour images is a wiring change, not a design one. It moves pixels on every downscaled photograph in the corpus, so it needs a board pass. |
| **`shading_tcpdf_058.pdf` render** — SSIM 0.9908 vs pdfium-render 0.9962 | **Edge antialiasing coverage — not shading.** 3.72% of pixels differ. Splitting the total absolute difference by the oracle's own gradient magnitude: **95.3% of it sits on 5.3% of pixels, all of them edges**. The flat shaded interiors (137 399 px) are **81% bit-identical**, and where they differ it is 1-2 LSB on one channel (12 838 px at delta 1, 9 825 at delta 2) — function-sample quantisation, not banding and not dithering. The file's 37 shadings are types 2 and 3 only, and run 3e scores us 0.988 / 0.989 on those, *identical to pdfium-render*. The worst tiles are glyph edges (`www.tc…`, `TCPDF Ex…`), same outlines, different edge coverage. Our AA: `crates/pdfrum-raster-agg`; PDFium's: analytic AGG coverage, `core/fxge/agg/fx_agg_driver.cpp`. | **tradeoff** | So the answer to "shading dithering, edge antialiasing, or function sampling density" is **edge antialiasing**, measured. Note pdfium-render is 0.9962 and not 1.0 on the *same* file, which is the pypdfium2 `libpdfium.so` being a different build from `pdfium_test` — so part of the 0.005 gap is not ours to close. Matching PDFium's AA exactly means reproducing its scanline coverage arithmetic bit for bit; §3.1 of `docs/design/mupdf-comparison.md` is the cost side. |
| **`image_jpx_123.pdf` render** — SSIM 0.9874 vs pdfium-render 1.0000 | **Upstream JPX decoder numerics** — confirmed, previously ruled at `docs/status/pdfrum-render.md` "M21 losses". 96.99% of pixels differ but the *modal* delta is 6 (2 per channel) and the per-channel signed bias is R **-0.20**, G **+1.75**, B **-1.23** — a sub-2-LSB systematic offset with no structure. High-frequency energy matches the oracle to **0.06%** (104.29 vs 104.35), so there is no blur, no geometry error and no resample difference; only 1.92% of pixels exceed \|d\|>12 and those correlate with edges (r=0.70), i.e. where wavelet reconstruction rounding diverges most. That is the signature of the irreversible MCT and the 9/7 inverse DWT rounding differently, not of a decode defect. PDFium: `opj_mct_decode_real`, `third_party/libopenjpeg/mct.c:329-338` (f32 `y + v*1.402f`, `y - u*0.34413f - v*0.71414f`, `y + u*1.772f`) and `third_party/libopenjpeg/tcd.c:2350`. Ours: `hayro-jpeg2000`, a different but equally legal rounding of the same normative equations. | **tradeoff** (third-party) | Both roundings are legal under ISO/IEC 15444-1 for the irreversible path, which specifies real arithmetic without pinning a fixed-point schedule. Closing it means either vendoring OpenJPEG's exact f32 schedule into a Rust JPX decoder or taking a C dependency — the latter gives up `cargo add pdfrum` with no native build crate (run 3d), which is a headline property. Not proposed. |
| **`text_quick_start.pdf` text** — F1 0.641, unmoved by the harness fix | **The dot-leader hypothesis is refuted** — leaders are one line on both sides. Two causes, in order of size. (1) **The harness compares the wrong streams**: `pdfium_test --txt` writes the unfiltered char list (`testing/pdfium_test/write.cc:364-370`), our harness writes `Display` = `search_text` (`benches/compare/src/engines/pdfrum.rs:109`, documented as *not* the `chars` stream at `crates/pdfrum-text/src/lib.rs:417-419`). (2) A **spurious generated space** at object boundaries — leading, before the leader run, before the page number, trailing, plus ~14 whitespace-only lines. The rules are transcribed identically on both sides; the input is not — `GetCharWidth` returns `int` (`core/fpdftext/cpdf_textpage.cpp:185-208`), our `ladder_char_width` returns `f32` (`crates/pdfrum-text/src/object.rs:176-202`). | **inferior**, cause **not determined** | The harness fix landed and did **not** move this row. The `ladder_char_width` truncation also landed, as the `GlyphWidth` newtype, and is a measured **no-op** — `pdfrum-font` already truncates widths where PDFium does, and zero fractional widths exist across 1 420 corpus PDFs. Both named causes are therefore eliminated and the 0.641 is undiagnosed; the remaining candidates are positional, not width-based. See below. |
| **`text_tcpdf_055.pdf` text** — F1 0.953, **0.948** against the `chars` stream (4 peers closer) | Two residuals after correcting the stream. The same spurious leading space; and one row where we emit `^@ ^A ^B … ^_` and PDFium emits nothing — those C0 codes are legitimately in `char_list_` on both sides, so PDFium's empty row is an input-geometry or charcode-mapping difference, not a filtering rule. | **inferior** (the space) + **not determined** (the C0 row) | The width truncation is a measured no-op (see below), so the space is not shared with a fix that works. The C0 row needs `GetCharWidth`/`GetPos` instrumented per item in a PDFium build, which a read-only pass cannot do. |
| **`text_bug_1029.pdf` text** — F1 0.957, **1.000** against the `chars` stream | Almost entirely the harness stream defect (see the row above): via `Display` we lost the whole first line, via the `chars` stream it is present and the soft hyphen matches. The residual is one trailing space before each `\r`. | **harness defect** + **inferior** (small residual) | Closed: the harness fix took this row to 1.000; the trailing space did not survive into the char stream. |
| **`mixed_en_uicase.pdf` text** — F1 0.991, **1.000 and byte-exact** against the `chars` stream | **Pure harness artefact — the engine already matches.** Against the `chars` stream our output is byte-identical to the oracle modulo the BOM. Both engines write `0x2` to the record and `U+00AD` to the buffer (`crates/pdfrum-text/src/pipeline.rs:962-967` / `core/fpdftext/cpdf_textpage.cpp:1359-1361`) and both exempt `kHyphen` from `IsControlChar` (`cpdf_textpage.cpp:134-147` / `crates/pdfrum-text/src/charinfo.rs:93-108`). | **harness defect** — no engine loss | Closed: the harness fix took this row to 1.000, byte-exact, as predicted. |
| **`image_ccitt_3bigpreview.pdf` text** — F1 0.994, **0.999** against the `chars` stream | The same spurious generated space, in the vertical CJK region: `Clips Clips` vs `Clips` on line 4, and four blank lines that are a lone space each. | **inferior** | The harness fix took it to 0.999. The `ladder_char_width` truncation is a measured no-op and does not close the rest. |
| **Warm open** — 0.14 ms median vs hayro 0.04, pdf-rs 0.12 | **The xref chain, and an eager page count.** Callgrind, `profile --op open --file text_quick_start.pdf`, 5 351 895 `Ir` whole process; `Document::from_bytes` (`crates/pdfrum/src/document.rs:148`) is 4 545 569 (84.9%). Split: **`xref::chain::load` 3 524 549 = 77.5% of open** (`crates/pdfrum-parser/src/xref/chain.rs`), of which `read_stream_section` 2 151 836 (47%) inflates the xref stream and `Xref::add_compressed` 1 398 422 (31%) inserts one `BTreeMap<u64, XEntry>` entry per compressed object — `BTreeMap::insert` alone is 1 271 375 `Ir` (28% of open) and `Xref::merge_up` (`crates/pdfrum-parser/src/xref/mod.rs:305-320`) a further 1 087 558 (24%), re-inserting every key of every section while walking `/Prev`. Then **`catalog_page_count` 645 606 = 14.2%** (`crates/pdfrum-parser/src/doc.rs:443-465`): `page_count_of` → `count_subtree` walks the page tree at open so `page_count()` is infallible and O(1) afterwards. hayro's open does not build a complete, merged, generation-checked object index or count pages eagerly. | **more work** (the page-tree walk) + **inferior** (the `BTreeMap`) | The 14.2% page walk is a deliberate API property — `Document::page_count()` returns `u32`, not `Result`, and run 3b shows we recover the whole `FRC` family of damaged trailers that lopdf, pdf-extract and pdf-rs cannot open (46, 46 and 72 files of 209). That is the "what we get". The **52% of open spent in `BTreeMap` insert plus `merge_up`** is not a tradeoff: a sorted `Vec<(u64, XEntry)>` built once and merged by a single descending-priority pass, or a `HashMap`, would remove most of it with no behavioural change. That is the fixable half and it is the larger one. |
| **Warm text** — 0.36 ms median vs PDFium 0.04 | **The page build is real but is *not* skippable work.** `docs/status/text-perf.md` §6 recorded the build as 39.3% of `text_tcpdf_063` and called it "work no text consumer reads". Re-measured at `e08d7cc` (callgrind, `profile --op text --iterations 1`) that has moved and the reading has changed: build is **26.7% of `text_tcpdf_063`** (59.3 M of 222.2 M) and **44.8% of `text_quick_start`** (61.8 M of 137.8 M). But the build's own split says almost none of it is graphics. On `text_tcpdf_063`: `show_text` 24.7 M (41.6% of the build), `FontCache::get_or_load` via `find_font` 25.3 M (42.7%), `parse_content` + tokenize 23.4 M, `add_form` 4.1 M — **all four are prerequisites for text**. On `text_quick_start`: font load 46.2 M of the 61.8 M build (75%), `parse_content` 22.1 M; non-text graphics work is under 3% of the build. Ours: `crates/pdfrum-page/src/build.rs` `interpret_streams`. PDFium's cheaper answer: `CPDF_TextPage` reads a page object list PDFium builds once and caches on `CPDF_Page`, so a second text call pays nothing (`core/fpdftext/cpdf_textpage.cpp`). | **tradeoff**, not "more work we could skip" | This corrects `text-perf.md` §6 and answers the brief's question directly: **a text-only build mode is not worth writing.** Measured, it would save under 3% of the build on the two files profiled, because the build on a text page *is* text work — content lexing, font loading and `show_text`. The remaining gap to PDFium's 0.04 ms is font loading (33.6% of the whole `text_quick_start` run) and the per-run rebuild, and the lever there is caching the built page across calls, not a narrower build. The M13 §23.4 extreme case (`image_bug_583804`, 87% `image::unpack`) **did not reproduce**: page 1 of that file profiles at 1 114 966 `Ir` total and has no images, so that row belongs to a later page and is not this median's cause. |
| **Warm render** — 10.55 ms vs mupdf 4.18, pdfium-render 5.68, hayro 8.78 | Fully explained in `docs/design/mupdf-comparison.md` — §1.1 (`vello_cpu`'s `F32Kernel::pack`, a fixed cost per render), §1.2 (the backend, not the render crate, carries the gap), §4 (the seven things we compute that mupdf does not). **Do not re-derive here.** | **tradeoff** + **inferior**, split per §6 of that doc | §6 is the ranked plan: (a) portable with no pixel change, (b) changes pixels inside the PDFium floor, (c) mupdf's different answer, do not propose. |
| **Render memory** — 1 539 MiB peak on `image_bug_583804.pdf` (every peer < 1 GiB); median 51.50 MiB | Recorded in `docs/status/queue.md` (§ the `image_bug_583804` rows — 176 MB retained for one page with no eviction, 59-75% of that document's render in the image path). Not re-measured here. | **in progress** | Carried by `docs/status/queue.md`. |
| **Cold render** — 41.90 ms vs pdfium-render 10.75, hayro 13.13, mupdf 15.05 | **In progress**, not diagnosed in this pass. `docs/status/queue.md` carries it (the cold-render rows at `:10`, `:13`, `:18`, `:345`). | **in progress** | Carried by `docs/status/queue.md`. |
| **Warm regression on `image_bug_583804`** — reproducibly 0.76x across `cd1a511` | **In progress**, not diagnosed in this pass. `docs/status/queue.md:824` has the record. | **in progress** | Carried by `docs/status/queue.md`. |
| `vector_en_system.pdf` render — SSIM 0.9600 (run 3) | **Already fixed on main.** `docs/status/pdfrum-render.md`, "The last two, fixed 2026-09-06 — and neither was the mask". Run 3's score is the pre-fix baseline. | fixed | — |
| `image_en_fqa.pdf` render — SSIM 0.9769 (run 3) | **Already fixed on main.** Same record; the fix is `stretch::SnappedReduction` wired at `crates/pdfrum-render/src/image.rs:493`. | fixed | — |
| `fx/image/1_image.pdf` render — SSIM 0.9857 (run 3b) | **Already fixed on main.** `docs/status/pdfrum-render.md`, "The two fixes, landed"; the diagnosis in the M21 table (a filter-selection defect) is corrected there. | fixed | — |
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

**So `text_quick_start`'s 0.641 is still undiagnosed.** Two hypotheses are
now eliminated — the dot leaders (refuted by the earlier pass) and the
fractional width (refuted here) — and the harness stream is excluded, since
that file did not move on the stream fix either. The remaining candidates are
in the *positions* rather than the widths: `GetPos`, the text-matrix
composition, or `FindPreviousTextObject`'s choice of the previous run.
Separating them needs the per-item instrumentation in a PDFium build that
the earlier pass also could not do.

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

It is called from one place: `reduced_mask_pixmap`
(`crates/pdfrum-render/src/image.rs:493`), the soft-mask plane path. The
colour-image path — `walk.rs:1904` and `walk.rs:2182`, both into
`stretch::prescale` (`stretch.rs:1092`) — still uses `reduction_for`'s
ceiled fractional footprint and still gets the second sampler. So the
`image_en_fqa` fix closed the mask half of a defect whose colour half is
this row.

The two guards `snapped_reduction` applies (`stretch.rs:1001`) — axis-aligned,
reducing in both axes, neither axis mirrored — are all satisfied by every
`/I2` draw on this page.

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
(inferior) or a tradeoff, on the strength of `docs/status/text-perf.md` §6's
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

Builds: `CARGO_TARGET_DIR=/mnt/data2/r13921098/cargo-target/losses2`;
the comparison harness from the warm `m21` target dir.
