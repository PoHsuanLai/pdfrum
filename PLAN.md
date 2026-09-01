# PDFium → Rust Rewrite — Master Plan

**Status:** Phase 2: M9-M12 ALL MET (2026-08-30); **M12c (GPU backend) MET 2026-08-31**; **M12b MET 2026-09-01 with two targets missed and named** (docs/status/M12b.md). M12 scorecard: warm render geomean 0.97x oracle (FASTER; image 0.24x, vector 0.90x, shading 0.95x; forms 3.35x is the named residue), rayon 3.09x@4/6.09x@16, RSS 1.20x, conformance byte-identical, ratchet green over 440 entries. Per-crate benches + bench-quick landed. M12b: three of its four items had their premise corrected by measurement — P1's scaled-decode diagnosis was wrong on three of its four cited documents and its target is MISSED at -33.4% against >=40%; P2 closed the arena question AGAINST bumpalo (+265%/+121%/+97% slower than a tuned no-dep baseline) and produced the profile M12 asked for (colour conversion <=7.2%, allocation ~0, interpretation 60-90% of the engine half); P3 was retargeted mid-milestone and delivered -42.6% on the engine half of vector_paths_1751 and -27.9% on the whole render, agreed by three independent rasterizer backends. Conformance byte-identical after every commit. Four near-false findings were caught rather than published (M12b.md §7). The bench ratchet re-baseline is an UNPAID DEBT, deliberately: warm is 132 entries / 32 improved / ZERO regressed, every regression is in cold, and five blocked with no pre-argued case. forms warm was never re-measured and stands at M12's 3.35x. M12c: GPU vello backend landed isolated (zero Tier-C interior differences on 44/44; 4.8x on the heaviest vector page, 2.64x slower overall on rasterization; shading target missed and named) — but its exemption cannot yet be spent, because vello 0.10 pins wgpu 29 while egui is on 30 and iced on 27, so no released frontend can inject a device. **M12d MET 2026-09-01, with M1 and M2 restated as owed** (docs/status/M12d.md): it paid M12b's three engineering debts and, like M12b, had its premise corrected by measurement on two of three items. D1 gave shading/patch.rs its owner — one BezPath buffer per patch instead of one per cell, netted −22.7% / −25.2% / −18.5% on the three shading documents, control unmoved, byte-identical across 202 page hashes — and refuted cell-merging by counting (the longest same-colour run is 2, usually 1), so M12 §3.9's SIMD reopening condition stays unmet *with a reason*. D2 was owed one third of its brief: the glyph-spacing heuristic and the croscore fix were on main since 2026-08-29 (ba8662f, 10905fe) and never reverted — the scoreboard's timestamp merely predated them — so that story is withdrawn in place (d81ac68); its real work was the serif bit plus a fontdb name-ID divergence it found itself, both moving 0 of 1675 files and paid on the oracle comparison rather than on a number. D3's inherited digest was wrong about what image_en_fqa is (301 draws of a 2x2 RGB image carrying an /SMask, not 552 minified 1-bit masks), and correcting it is what located the cost: cold 679.7 → 158.7 ms (−76.7%), build −83.7%, image class cold geomean −14.5% on top of P1's −33.4%; on image_bug_718762 the brief's ordering hypothesis is refuted, to_pixmap is −10.4%, and the remaining ~360 ms is the upstream scaled-decode gap. Two cross-vendor Grok reviews (both MERGEABLE-WITH-NITS) landed three GPU should-fixes — a device leaked before the check that would refuse it, a documented TargetTooLarge never constructed with a 17 GiB allocation reachable behind it, and a device-loss panic hook — and the walk review's NaN finding was real but ran the opposite direction from its own reasoning. Conformance byte-identical after every commit; no dependency fact moved, verified mechanically. **M1 (ratchet re-baseline) and M2 (oracle side-by-side) are STILL OWED**, unpaid for a second milestone: load never dropped below ~7 and stood at 42 at close, with two unrelated python3 jobs at 450-490% CPU. Until M2 runs, every "versus PDFium" figure in this line dates from M12 — forms warm included, still 3.35x unmeasured. M13 (release) NOT STARTED — loop paused by user; it inherits M1, M2, and the filings and pins that were always the user's or upstream's.
**Oracle:** `/mnt/data2/pdfium/pdfium-c++` (read-only C++ PDFium checkout @ `6f2272e`)
**Workspace:** `/mnt/data2/pdfium/pdfrum` (this repository)

This is the operative coordination document. Every agent working on this project
reads this file first, then **STYLE.md** (binding Rust style constitution:
functional, data/logic separation, small structs, error discipline) and
**SPEC.md** (concrete type/API contracts per crate + the `[spec]` change
protocol). Per-module design docs live in `docs/design/`, live
progress in `docs/status/`, conformance results in
`conformance/scoreboard.json`.

---

## 1. Locked decisions

| Decision | Choice |
|---|---|
| Scope | Core PDF: parse, render, text extraction, doc features (annots/forms data + appearances), edit/save. **No V8/JS, no XFA** (only `fpdfsdk` referenced them; `core/` is clean of both). JS-in-PDF = AcroForm script actions; executing them is optional forever, and the future slot is a pure-Rust engine (`boa`) behind a trait, never V8. |
| API | **Pure idiomatic Rust.** No C ABI. C++ is used only as the differential-test oracle. |
| Rendering | Linebender stack: `kurbo` + `peniko` vocabulary types; `RenderDevice` trait with **`vello_cpu` (facade default)**, **`tiny-skia` (cross-check)** and **`raster-exact` (conformance default)**. GPU `vello` landed in M12c as a fourth, deliberately isolated backend — Tier C only, never the facade's default, and nothing in the core ring depends on it. |
| Fonts | **Fontations** (`skrifa` + `read-fonts`) for outlines/metrics/charmaps — no FreeType. Upstream PDFium already ships a `cxx`-bridged skrifa backend (`core/fxge/skrifa/`), validating coverage. Type1 needs our own small parser (Fontations doesn't read PFA/PFB). |
| Fidelity | **Tiered.** Byte-exact where cheap (text extraction, metadata/structure/annot dumps, decoded image bytes); perceptual pixel diff (per-test thresholds, tightened over time) for rendering. |
| Toolchain | Stable Rust, Cargo workspace, `cargo nextest` for tests, clippy `-D warnings`, rustfmt. `unsafe_code = "forbid"` everywhere except (if ever needed) an isolated SIMD/interop crate. |

**Prime directive: this is a rewrite, not a transliteration.** Each module gets a
design pass that maps *what the C++ does and why* onto idiomatic Rust (enums +
pattern matching instead of type-tag switches, iterators, `Result`, ownership
instead of `RetainPtr`/observer patterns). Port *behavior and test assertions*,
never C++ structure. The style bar — functional, type-everything, small structs
with data/logic separation, no OOP emulation, no panics in libraries — is
codified in STYLE.md; the concrete contracts agents implement against are in
SPEC.md.

## 1b. The greenfield question

Would this be dramatically simpler if PDF-engine-in-Rust had been the design
from day one? **Yes for infrastructure, no for semantics — and this plan *is*
the greenfield design.** Of PDFium's ~40 `third_party/` dirs, only ~13 are
functional dependencies of the core; the rest are Chromium build machinery.
Of the functional ones, everything except JBIG2/JPX dissolves into either
`std` (fxcrt's 36k-line runtime predates usable STL in that codebase) or a
small modern crate. The convergent evidence is strong: independent modern-Rust
PDF efforts (hayro, the typst ecosystem's krilla, pdf-rs) all land on the same
stack we chose — kurbo/peniko rasterization, Fontations fonts, RustCrypto,
miniz_oxide — and Google itself is retrofitting skrifa into PDFium. What does
NOT simplify: the PDF spec's inherent surface (CMaps, Type1, colorspaces,
functions, shadings, the transparency model) and two decades of broken-file
recovery folklore — that behavioral knowledge is PDFium's actual value and the
one thing we port rather than redesign. Net: ~2.5M LOC of C++-plus-deps
becomes roughly 80–120k LOC of Rust over ~15 focused pure-Rust crates.

## 2. What the C++ tells us (exploration summary)

In-scope C++ ≈ 240k LOC. Module → dependency order (leaf → root), from BUILD.gn analysis:

```
constants → fxcrt → {fdrm, cmaps, contentstream_write_utils} → fxge → fxcodec
  → fpdfapi/parser → fpdfapi/font → fpdfapi/page → {fpdfapi/render, fpdfapi/edit, fpdftext}
  → fpdfdoc → fpdfsdk(public API layer)
```

Key facts:
- `fxcrt` (36k LOC) is strings/spans/smart-pointers/streams/safe-ints — **mostly dissolves** into `std`, `Arc`, `bytes`. Its ICU use is only bidi → `unicode-bidi` crate. Its `css/` subdir is XFA-only → dropped.
- `fxge` (72k) = device abstraction + DIB/bitmap + font loading/mapping/glyph cache + AGG/Skia backends + per-OS font discovery. Backends are runtime-swappable behind `RenderDeviceDriverIface` — our `RenderDevice` trait mirrors this.
- `fxcodec` (20k): flate/LZW/fax/RLE/AHx/JBIG2/JPX/JPEG/ICC. bmp/gif/png/tiff are XFA-only → dropped.
- `fpdfapi` (84k) splits cleanly: parser / cmaps (static CJK tables) / font / page / render / edit.
- `fpdfsdk` (63k) is mostly the C-ABI shim + interactive widget UI (`pwl/`, `formfiller/`) that only matters with JS event handling → **not ported**. Our facade crate replaces it with an idiomatic API.
- Third-party deps that vanish: Skia, AGG, FreeType, HarfBuzz (subsetting → Rust `subsetter`), lcms2 (→ `moxcms`), ICU (→ `unicode-bidi`), zlib (→ `miniz_oxide`), libjpeg_turbo (→ `zune-jpeg`), dragonbox (→ `ryu`), abseil/PartitionAlloc (→ std). OpenJPEG and the custom JBIG2 impl are covered by hayro's pure-Rust `hayro-jpeg2000`/`hayro-jbig2`/`hayro-ccitt` (Apache-2.0/MIT) — first-party ports remain only as a *fallback* if conformance exposes gaps. The full locked manifest with rationale is **DEPS.md**.

## 3. Crate architecture

Workspace `pdfrum/`, crates under `crates/`. Name **pdfrum**: facade crate `pdfrum`, prefix `pdfrum-` (verified free on crates.io, 2026-08-29).

| Crate | Replaces | Contents |
|---|---|---|
| `pdfrum-common` | fxcrt (residue) | Error types, shared small types; re-export `kurbo` geometry. Keep minimal — resist a "utils" dumping ground. |
| `pdfrum-object` | fpdfapi/parser objects + constants/ | PDF object model: `Object` enum (Null/Bool/Int/Real/String/Name/Array/Dict/Stream/Ref), dict-key constants, indirect-ref resolution traits. |
| `pdfrum-crypt` | fdrm + parser security handlers | RC4/AES-CBC, MD5/SHA-1/2 via RustCrypto; standard security handler, revision 2–6. |
| `pdfrum-filters` | fxcodec (simple) | Flate (`miniz_oxide`) + predictors, LZW, RunLength, ASCIIHex/85, CCITT fax. |
| *(deps)* JBIG2 / JPX / CCITT | fxcodec/jbig2, libopenjpeg, fax | `hayro-jbig2`, `hayro-jpeg2000`, `hayro-ccitt` wrapped behind our own entry points (SPEC §12); first-party `pdfrum-jbig2`/`pdfrum-jpx` ports only as fallback if conformance exposes gaps. No FFI anywhere: the oracle's own `--save-rendered-images` output is the C-reference for decode diffing. |
| `pdfrum-cmap` | fpdfapi/cmaps | Build-script-generated compact static CJK CMap tables + predefined CMap parser. |
| `pdfrum-font` | fpdfapi/font + fxge font half | Font dicts (Type1/TrueType/Type0/Type3/CID), encodings, ToUnicode, glyph mapping; outlines via `skrifa`; `pdfrum-type1` sub-crate (PFA/PFB/charstrings); substitution/fallback via `fontdb` + embedded Foxit fallback fonts (`core/fxge/fontdata`, PDFium BSD license); glyph cache. |
| `pdfrum-page` | fpdfapi/page | Content-stream interpreter → typed page-object graph; graphics state; colorspaces (device/ICC via `moxcms`/Indexed/Separation/Lab); PDF functions (types 0/2/3/4 incl. PostScript calc); patterns & shadings (types 1–7); transparency model (groups, soft masks, blend modes). |
| `pdfrum-render` | fpdfapi/render + fxge device half | `RenderDevice` trait (kurbo/peniko types); engine walking the page graph; layer compositor for isolated/knockout groups and soft masks; image resampling. |
| `pdfrum-raster-vello` | Skia backend | `vello_cpu` implementation of `RenderDevice`. The facade's default, for API users who want a production rasterizer. |
| `pdfrum-raster-tinyskia` | AGG backend | `tiny-skia` implementation. Cross-check + determinism baseline; Tier C's gating partner. |
| `pdfrum-raster-vello-gpu` | *(new capability)* | GPU `vello` on `wgpu`, M12c. **Outside the core ring by construction**: nothing depends on it, the facade cannot name it, and `scripts/check-no-wgpu.sh` asserts a headless tree resolves with zero `wgpu`. Takes a caller-supplied `wgpu::Device`. Tier C only — GPU rasterization is not bit-reproducible across drivers, so it never joins the scoreboard. `publish = false`. |
| `pdfrum-raster-exact` | AGG parity | Analytic scanline rasterizer of our own, no rasterizer dependency. Integrates coverage exactly on the oracle's subpixel grid; the conformance default, so a golden diff measures the engine rather than a sampling policy. Reachable from the facade as `Backend::Exact`, but not its default — no SIMD. |
| `pdfrum-text` | fpdftext | Text extraction, reading order/whitespace heuristics, search, link detection. Depends only on parser/font/page — parallelizable with render. |
| `pdfrum-doc` | fpdfdoc | Bookmarks, named dests, links/actions, annotations + appearance-stream generation (variable text), AcroForm data model (fill/read, no JS), struct tree, metadata. |
| `pdfrum-edit` | fpdfapi/edit | Serializer, incremental update writer, page import/reorganize, font subsetting (`subsetter`), content-stream generation (`ryu` floats). |
| `pdfrum` | fpdfsdk/public | Facade: `Document`, `Page`, `TextPage`, `Form`, render options — the idiomatic API. Ownership model designed here (interior `Arc`, lazy page cache, `Send` story). |
| `pdfrum-tool` | testing/pdfium_test | CLI mirroring the oracle's flags/outputs (`--png --md5 --txt --annot --show-metadata --show-pageinfo --show-structure --save-images`, same filename conventions) so the harness diffs like-for-like. |
| `conformance/` | testing/tools | Harness (see §5). Not published; workspace member. |

Dependency rule: mirrors §2's DAG; enforce with `cargo-deny`/workspace lints. No crate below `pdfrum-page` may depend on rendering.

## 4. Oracle setup (M0, one-time)

The checkout is **not built** (no `out/`, `buildtools/linux64` missing → needs
`gclient sync` using bundled `third_party/depot_tools`, or hand-rolled gn args
with system clang). Build the *small* oracle — no V8/XFA/Skia:

```bash
cd /mnt/data2/pdfium/pdfium-c++
gn gen out/Release --args='is_debug=false pdf_enable_v8=false pdf_enable_xfa=false pdf_use_skia=false pdf_is_standalone=true'
ninja -C out/Release pdfium_test pdfium_diff
```

Oracle invocation recipe (determinism: frozen clock + hermetic fonts + AGG):

```bash
out/Release/pdfium_test --time=1399672130 --png --md5 \
  --croscore-font-names --font-dir="$(readlink -f third_party/test_fonts)" \
  <file.pdf>                       # also: --txt (UTF-32LE), --annot, --show-*
```

Corpus available locally: `testing/corpus/` (836 PDFs, 4022 expected PNGs),
`testing/resources/` (287 PDFs + 223 `.in` templates via
`testing/tools/fixup_pdf_template.py`), suppressions in `testing/SUPPRESSIONS*`,
comparator `pdfium_diff`, hermetic fonts in `third_party/test_fonts/`.
**Generate the full golden store once** (all corpus+resource PDFs → PNG + txt +
annot + metadata/pageinfo/structure + saved-image dumps) into
`conformance/goldens/` keyed by content hash; the oracle binary is
then only re-run on demand.

## 5. Conformance harness (the project's heartbeat)

`conformance/` crate + `cargo xtask conformance`:

1. Runs `pdfrum-tool` over every corpus/resource PDF (panic = automatic failure, caught per-file).
2. Compares against golden store, tiered:
   - **Tier A — byte-exact:** text extraction (transcode oracle UTF-32LE → UTF-8), metadata/pageinfo/structure dumps, annot dumps, decoded embedded image bytes (`--save-images` md5s), page counts.
   - **Tier B — perceptual:** rendered PNGs vs goldens; metrics: exact-match flag, per-channel max diff, SSIM. Global default threshold, per-test overrides in `conformance/thresholds.toml` (ratcheted down over time; a passing test's threshold may never loosen).
   - **Tier C — cross-backend:** vello_cpu vs tiny-skia output of *our own* engine; large divergence = backend bug, not engine bug.
3. Emits `conformance/scoreboard.json`: per-file status, failure tag (crash / parse / tierA-mismatch / pixel-fail+SSIM / unsupported-feature), aggregates per feature cluster. **This file is the fitness function every loop optimizes.**
4. `--triage` mode: clusters failures by tag/feature (encryption, jpx, shading-type-N, type3, cjk, …) and prints the top clusters — the unit of work for burn-down agents.

Also: `cargo-fuzz` targets seeded from `pdfium-c++/testing/fuzzers/` corpora
(parser, filters, jbig2, jpx, cmap, type1) from M1 onward; property tests
(edit round-trip: save → reparse → re-render == original) from M7.

## 6. Milestones & exit criteria

Every milestone exits only with: green `cargo nextest run`, clippy clean, scoreboard criteria met, status doc updated, committed.

- **M0 — Infrastructure.** Oracle built; golden store generated; workspace scaffolded (all crates stubbed, CI script: fmt+clippy+nextest+conformance-smoke); `git init` pdfrum; harness v1 runs end-to-end (everything failing is fine — the scoreboard exists).
- **M1 — Object model & parser.** `pdfrum-{common,object,crypt,filters,parser… }`: lexer, xref (tables + streams), object streams, incremental updates, encryption, lazy object loading, damaged-file recovery (port `fpdfapi/parser` heuristics — PDFium's superpower is broken-PDF tolerance). *Exit:* 100% of corpus+resources load without crash; page counts + metadata dumps Tier-A across the board; parser fuzzers running clean for 24h.
- **M2 — Fonts & text extraction.** `pdfrum-{cmap,font,type1,text}`. *Exit:* `--txt` Tier-A byte-exact ≥ 98% of corpus (documented waivers for the rest); ToUnicode/CJK clusters green.
- **M3 — First pixels.** `pdfrum-{page,render,raster-*}` for paths, fills/strokes, clips, DeviceRGB/Gray/CMYK, text rendering via glyph outlines. *Exit:* ≥ 60% of pixel corpus at SSIM ≥ 0.99; Tier-C divergence < 1%.
- **M4 — Codecs & images.** JPEG (`zune-jpeg` integration), fax, JBIG2, JPX ports; ICC via `moxcms`; image page-objects + resampling. *Exit:* decoded-image Tier-A ≥ 95%; image-cluster pixel tests green.
- **M5 — Full rendering.** Shadings 1–7, tiling patterns, transparency groups/soft masks/blend modes, Type3 fonts. *Exit:* ≥ 95% pixel corpus at SSIM ≥ 0.99, ≥ 80% exact-or-near-exact; burn-down loop owns the tail. **Met 2026-08-29 (burn-down wave 11): 96.2% at SSIM ≥ 0.99 (1566/1628), and 87.0% at ≥ 0.999 against the ≥ 80% near-exact clause** — read as SSIM ≥ 0.999, the band where a difference is at most a thin antialiased rim, with byte-exact at 516/1628 (31.7%) inside it. No threshold was relaxed to get there; `conformance/thresholds.toml` is untouched and the monotone rule held on every commit. **The tail is 62 files / 51 unique documents**, and it is shallow — 43 of the 51 sit above 0.95 and 30 of those above 0.98. Four items account for most of it, each a feature rather than a defect, and all four are inventoried with their measurements in `docs/status/pdfrum-render.md` (wave 11): the **image downscale kernel** (`CStretchEngine`'s area-average box filter, which we answer with a 2-tap bilinear — the largest single item, and 74% of the 14-file `FRC_8.2.4` cluster plus all of `example_009`); **CCITT having no caller** in `pdfrum-filters`' chain, which is `bug_1746` and also blocks a verified Type3 fix; **`bug_725389`**'s `CPDF_BAFontMap` charset fallback to a second face; and the two standing external items — **`bug_867501`** (0.646), an upstream `hayro-jbig2` segment-type gap behind our own wrapper (SPEC §12), and **`en_fqa`** (0.885), whose earlier instrumentation was proven unreliable and needs re-deriving from scratch.
- **M6 — Document features.** `pdfrum-doc`: annots + appearance generation, AcroForm fill (no JS), bookmarks/links/actions, struct tree. *Exit:* `--annot`/`--show-structure` Tier-A ≥ 98%; form-cluster pixel tests green (forms rendered from generated appearances). **Met outright 2026-08-29 (wave 10): 99.4%, and the widget-text-body waiver is retired.** The exit criterion used to carry an exclusion for that cluster — 76 `--annot` artifacts whose object counts the field body produces — on SPEC §10 E1's ruling against a second variable-text engine. E1 was revised once it was established that there is no second engine (`CPWL_EditImpl` is a shell over the same `CPVT_VariableText` this crate already ports), the three `SetAs*` producers were implemented over it, and the cluster went **76 → 13**. The 13 that remain are not the text body. **Wave 11 took those to 4**, landing the `/AP`-presence rule, the invalid-appearance outline and the `/Hide` open action together (each is a net loss alone), the push-button caption, `/NeedAppearances` with its sub-state rule, and shared-`/T` field merging. The 4 left are `bug_725389`'s `CPDF_BAFontMap` fallback to a second face, and `example_014`/`example_054`.
- **M7 — Edit & save.** `pdfrum-edit`: full save, incremental save, page import, subsetting. *Exit:* round-trip property tests green over full corpus (save→reload→render == original within Tier-B); oracle re-opens our saved files cleanly. Met 2026-08-29: 100% oracle reopen over 1638 saved files, 100% render fidelity over a 233-file sample, 100% incremental append discipline. The exit's Tier-B clause is read as *relative* — a saved file must render as well as its original, not better than the renderer manages on the original — because the absolute figure reports the renderer's standing gap rather than the writer's; `conformance save-round-trip` prints both.
- **M8 — API & release polish.** Facade API review, rustdoc + examples, criterion benchmarks vs oracle timings, MSRV, publishability audit. Benchmarks (`benches/`), cache reuse (`RenderSession`), the document-wide diagnostics view and the publishability audit are done — `docs/status/M8.md`. **MSRV remains undeclared**, and the version-locked family release process is written up but unexercised. *Later options:* `vello` GPU backend, `boa` JS actions, `pdfrum-capi`.

Parallelism: after M1, {M2-text} ∥ {M3-render} ∥ {M4-codec ports}; `pdfrum-jbig2`/`pdfrum-jpx` are self-contained ports that can start any time (their oracle = decoded bytes, not pixels).

## 7. Agent orchestration

**Per-module cycle** (the standard unit of work):
1. **Design brief** — Explore agent reads the C++ module + its unittests → `docs/design/<module>.md`: behavior inventory, quirks/heuristics that MUST survive (esp. broken-file tolerance), proposed Rust design (types/traits), test plan. Human-reviewable before code.
2. **Implement** — agent(s) build the crate against the brief; port unittest *assertions* (851 unittest files upstream) as nextest tests.
3. **Conformance loop** — run harness on the module's clusters; fix; repeat until exit criteria; commit per green step.

**Fan-out workloads** (Workflow tool, ≤15 agents/run): cmap table generation, per-codec ports, unittest-assertion porting, and above all **corpus triage** — one agent per failure cluster from `--triage`, each producing either a fix or a tagged diagnosis into the scoreboard.

**Burn-down loop** (`/loop`, long interval): run harness → pick top cluster → dispatch fix → verify no regressions (scoreboard diff must be monotone: no previously-passing test may fail) → commit → repeat.

**Guardrails for every agent:**
- STYLE.md and SPEC.md are binding. Deviating from a SPEC.md contract requires
  a `[spec]` commit editing SPEC.md with rationale in the same commit (SPEC.md §0)
  — never silent divergence.
- Oracle checkout is **read-only**. Never edit, never rebuild without being asked.
- Definition of done: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run` + relevant conformance cluster. (Doctests: `cargo test --doc` — nextest skips them silently.)
- Scoreboard regressions block commit.
- No new dependencies without adding them to the table in §3 (deps are a design decision here, not a convenience).
- Rewrite-not-transliteration: if a Rust file reads like the C++ file, it's wrong.

## 8. Risks

| Risk | Mitigation |
|---|---|
| JPX/JBIG2 now ride on young hayro codec crates | Wrapped behind our own entry points; oracle = decoded bytes (strong signal) validates them across the full corpus; first-party port stays the drop-in fallback; the oracle's own decoded-image dumps are the C reference, so no FFI is needed in our tree. |
| `vello_cpu` still maturing | Trait boundary + tiny-skia cross-check backend from day one. |
| Type1 fonts unsupported by Fontations | Small dedicated `pdfrum-type1`; corpus has fixtures; charstring interpreter is well-trodden ground. |
| Transparency/soft-mask/blend correctness (hardest area) | Isolated compositor above the rasterizer; dedicated fixture subset; SSIM ratchet. |
| Oracle build friction (gclient/network for buildtools) | M0 task with fallback: hand-written gn args + system clang; worst case, prebuilt pdfium_test from pdfium.googlesource CI. |
| Broken-PDF tolerance is undocumented folklore | Design briefs must inventory recovery heuristics from C++ before parser work; corpus `bug_*` files (~151) are the regression net. |
| Pixel-diff noise (AA, hinting) burning agent cycles | Tiered fidelity; SSIM thresholds per test; never chase bit-exactness on Tier B. |

## 9. Immediate next actions (M0)

1. `git init` + scaffold `pdfrum` workspace (all crates stubbed, workspace lints, CI script).
2. Build the oracle (§4); verify the determinism recipe on 3 sample PDFs.
3. Golden-store generator (drives `pdfium_test` over corpus+resources incl. `.in` fixup, writes `conformance/goldens/`).
4. Harness v1 + `scoreboard.json` + `--triage`.
5. Design briefs for `pdfrum-object` + `pdfrum-parser` (first real code of M1).

---

# Phase 2 — v1 completion & performance (planned 2026-08-30)

Phase 1 delivered the feature-complete engine. Phase 2 closes the three real
feature gaps, grinds the documented parity tail, and runs a measured
optimization program. Same rules: briefs where behavior is nontrivial,
`[spec]` protocol, monotone scoreboard, oracle-evidence over reasoning.

## M9 — Parity tail closure  *(small; parallel with M10)*  — **MET 2026-08-30**

Port `CStretchEngine`'s area-average box downscale filter (largest remaining
pixel cluster); wire CCITT into the filter chain (unblocks the reverted
`bug_1746` Type3 fix); implement the `CPDF_BAFontMap` charset-driven N-slot
second-face fallback (the last 4 annot artifacts); re-derive `en_fqa` from a
fresh trace; finalize the upstream hayro-jbig2 issue text (user files it).
*Exit:* pixel >= 97.5% @0.99; annot 100%; every remaining failing file has a
one-line cause in the status doc. **Met 2026-08-30 at 97.4%** — accepted 0.1
under target: the sole crossing change (outer-integer image snapping) measured
+4/-4 with regressions and was reverted under the monotone rule; the 42-file
tail is four independent explained mechanisms (wave 12). Annot: 100%.

**Delivered** (wave 12, `docs/status/pdfrum-render.md`): the box filter as an
engine pre-pass shared by both backends, with its destination size rounded
**up** — worth as much again as the filter itself, because a fractional
footprint covers one more device pixel than its width names. CCITT wired on
both the sample and the stencil rung, repacked from the decoder's four-byte
rows to the image's own pitch. `bug_1746`'s Type 3 fix re-landed: its "second
factor of two" was one factor applied in the wrong place, and the alpha belongs
on the procedure's fill colour with an opaque blit (.835 → .952). `en_fqa`
re-derived from scratch and found to be an *image* file — 87 masks minified
8.33x, one font that can render only a space (.885 → .921). The upstream
`hayro-jbig2` issue drafted at `docs/upstream/hayro-jbig2-issue.md`.

**Annot 100%** — met, and by two separate fixes, neither the expected one: a
form field carrying `/Kids` is not a control and must not generate an
appearance (`example_014`, `example_054`), and `bug_725389` needs only
`GetPDFWordString`'s raw-code-point fallthrough, not the N-slot map, because on
a hermetic font set the map finds no second face and the fallthrough is its
whole observable contribution.

**Pixel 97.4%, 0.1 short of the 97.5% target.** The change that would clear it
— snapping an axis-aligned image to its outer integer rect, which is what
`CPDF_ImageRenderer` does — measures +4 at 0.99 and +14 byte-exact while
regressing four files, and was reverted rather than tuned under the monotone
rule. The tail's four remaining mechanisms are measured spatially and recorded
per-file; the shortfall is documented rather than carried as unknown.

## M10 — Encrypted save  *(reverses ruling E3's deferral)*  — **MET 2026-08-30**

`pdfrum-crypt` gains the encrypt direction ([spec] on SPEC.md §3/D2: RC4 +
AES-CBC encrypt with per-object keys, PKCS#7 padding, fresh IVs); the writer
re-encrypts strings/streams under the original handler on save (the encrypt
dict itself never encrypted; the security_changed_ / /ID interlock already
ported in M7 stays authoritative). Password-preserving only — no re-keying
API in v1. *Exit:* every encrypted corpus fixture saves, the ORACLE reopens
it with the same password, and the round-trip renders equal; incremental
save of an encrypted doc holds the append discipline.

## M11 — Page mutation  — **MET 2026-08-30**

The deferred holder-mutation half of content regeneration (edit brief E6):
dirty/active tracking on PageObject, content-stream index + per-stream CTM
facility in pdfrum-page ([spec] §7), regenerate-on-save wired through the
existing byte-pinned emitters, facade API (add/remove/edit path, text, and
image objects; `page.edit()`; `doc.save_pages` reflects it). E6, E8 and E9 are
all resolved in the brief.

*Exit met.* The tool grows `--mutate=` and the harness grows
`mutate-round-trip`: pdfrum mutates page 0 and saves, the **oracle** reopens
and renders the result, and its render is compared against ours of that same
file. Over a 280-file sample x 3 mutations: **697 applied, 697 reopened by the
oracle, 697 losing nothing** (685 at SSIM >= 0.99 outright; the other 12 fall
short only where the two implementations already disagree about a plain save
of the same file). Scoreboard unmoved at 1624/1675, so R13's thresholds are
untouched — and R13 turned out not to need a cluster of its own, because both
sides read the same regenerated bytes and the emitter's documented losses
cancel. 23 embeddertest-derived assertions are ported in
`crates/pdfrum/tests/mutation.rs`, each three-phase — edit, save, reload —
because each phase can be right while the next is wrong.

The sweep earned its keep twice. Its oracle comparison found that the content
tokenizer's *real* parser was two ulps out — an obsolete `FX_atof` the C++
replaced with a correctly-rounded parse, invisible to every existing test
(all compared to `1e-4`) and worth a whole row of pixels once a rectangle's
edge went through `floor`/`ceil`. And it took two wrong diagnoses to learn
what the sweep is allowed to charge to a mutation: SSIM is local, so adding
ink changes what the metric weighs, and a plain save is not neutral either.
Both lessons are written down where the next person will meet them.

## M12 — Performance program  *(after M11; conformance is the regression gate)*

- **P0 baseline — DONE** (docs/status/M12.md §1): `benches/corpus/` holds 44
  files across six classes, selected by *measured* operator counts and render
  time over all 1375 files in the oracle's two directories rather than by size
  (a 25 MB file renders in 90 ms; a 1 KB one takes 1.2 s). Eleven criterion
  groups (`open`, `build`, `render-{cold,warm}-{exact,tinyskia,vello}`, `text`,
  `save`), **split per crate** in P3 so `cargo bench -p pdfrum-render` is a
  command with a meaning; `benches/baseline.json` +
  `benches/src/bin/ratchet.rs` with empirically
  measured per-group noise bands; `scripts/profile.sh`. **`perf` was
  unavailable** (`perf_event_paranoid = 4`, no root): the harness detects it,
  prints the remedy, and falls back to exact instrumentation at the
  engine/rasterizer seam rather than to a sampler that could only report
  elapsed time.
- **P1 CPU — DONE for what the profile named** (docs/status/M12.md §3–4).
  Landed with numbers: the opaque-destination composite fast path (**-58% on
  the glyph blit, -33% on a text page, -17% on a shading page**, proved
  byte-identical three ways) and the span blitter's loop-invariant hoist
  (which also removed a latent narrow-clip read past a mask row). Landed and
  measured at *no effect*, reported as such rather than rounded up: the image
  resample loop, the `to_pixmap` stencil hoist, and `FxHash` on both hot maps.
  **rayon scaling measured: 3.09x at 4 threads, 6.09x at 16, flat past the
  16 physical cores** — the >= 3x target is met. Two findings redirect later
  work: the *engine* half (walk + interpretation) is 54–85% of a render and is
  untouched, and the image class's real cost is `to_pixmap`+`prescale`
  **re-running on every render of an unchanged image** — a cache, which is P2,
  not a loop.
- **P2 memory — DONE** (docs/status/M12.md §9). The image cache §3.6 demanded
  is built and is the largest single win in the milestone: **-90% on five
  image documents** (`image_bug_718762` 837 → 84 ms), keyed `(ObjRef,
  request)` per SPEC §7, with a **byte-identical** conformance scoreboard —
  not merely regression-free. `scripts/bench-rss.sh` measures peak RSS against
  the oracle: **geometric mean 1.20x** against the ≤1.5x target, met on 39 of
  44 files; the five that miss are image documents where the oracle scales its
  decode and we materialize, named rather than averaged away. On eviction the
  C++ answer turned out to be the opposite of what it looks like —
  `CacheOptimization`'s 15-entry-then-100-MiB policy runs only under
  `bLimitedImageCache`, **off by default**, so upstream's default is an
  unbounded *per-page* cache; ours matches that lifetime with a 64 MiB budget
  on top. An opt-in one-entry mode was built, measured at nothing, and
  removed — the documents with a memory problem draw one image each. The
  Arc/clone pass found the walk's per-object path clone and removing it
  measured inside the noise band, which is the first data point against
  allocation being the engine half's cost. Still outstanding: the prerequisite
  M12 could not satisfy — a real `perf` profile of the page-graph walk,
  without which the `bumpalo` question cannot be asked honestly. **(Satisfied by
  M12b P2, not by `perf` — which is still blocked — but by in-walk
  instrumentation. This §9.5 datum held: allocation is 0.6% of the walk and the
  arena is declined. docs/status/M12b-P2.md.)**
- **P3 the benchmark convention, and the two outliers — DONE**
  (docs/status/M12.md §1.8–1.10, §11). The suite measured **cold** renders
  against a **warm** oracle: criterion built a fresh `RenderSession` per
  iteration while `pdfium_test --render-repeats` renders in one process with
  `CPDF_PageImageCache` on by default. §1.7 named the defect and §9.1 had
  already measured it at fourteen times; P3 fixes it. There are now **two
  measured conventions** — `render-cold` (first-render latency, ratcheted, no
  oracle target) and `render-warm` (steady state, and **the convention the
  exit target is judged on**, because it is the one the oracle column was
  always taken in). Both re-baselined. The suite is also **split per crate**
  — `pdfrum-parser` owns `open`, `pdfrum-page` the new `build` group, and so
  on — with the corpus, the baseline and the ratchet staying in `benches/` as
  the certificate suite, plus `scripts/bench-quick.sh` (18 files, criterion's
  floor, ~3 min) as the dev loop. The two outliers §1.7 flagged are
  **diagnosed but not fixed** (§10), and both diagnoses cost two wrong
  hypotheses each: `mixed_formfield` is three genuine image decodes behind
  three widgets, producing 70x53 thumbnails at **15 ms each** — not appearance
  regeneration (0.19 ms) and not the font re-parsing an earlier draft blamed
  (measured at 0.38 ms, once); `image_ccitt_transfer` is one JBIG2 decode of a
  3562x851 codestream page behind a 400x400 dictionary, and the 15.4 ms is
  inside `hayro-jbig2`'s arithmetic decoder, which no change to our sink can
  reach. Both are the same gap in different clothes: **M12 made repeated image
  work free and left single image work exactly where it was**, which should
  set the next milestone's agenda. Two changes did land and are kept —
  memoizing the built-in Multiple-Master and base-14 faces (~5 ms on any
  document with a non-embedded font), and a **correctness** fix where a JBIG2
  codestream wider than its dictionary bled set pixels into the next row,
  silently, on every row — but the A/B says neither moves the row it was
  written for, and §10 reports both non-results rather than rounding them up.
  Conformance after both: the scoreboard is **byte-identical** to the
  committed one.
- **Targets (adjustable): all three MET** (docs/status/M12.md §11 has the
  scorecard). Single-thread geometric mean >= oracle on the bench corpus —
  **met: 0.97x**, judged on the **warm** convention P3 established, because
  that is the one the oracle column was always taken in (§1.8); the sum ratio
  is 1.00x, 1644.2 ms against 1639.2 ms, and `image` `vector` and `shading`
  all come in under 1.0. The same corpus measured **cold** is 3.46x and is
  tracked with its own ratchet entries and no target. **Almost none of that
  3.58x → 0.97x is a code change**: it is the measurement correction §1.8
  argues for, and §9.1's cache had already done the work a milestone earlier
  where the suite could not see it. >= 3x oracle throughput on multi-page docs
  with rayon (**met**: 3.09x at 4 threads, 6.09x at 16); peak RSS <= 1.5x
  oracle (**met on the geometric mean at 1.20x**, and on 39 of 44 files — the
  five image documents that miss are recorded with their cause in M12.md §9.2
  rather than averaged away). Every perf commit re-runs conformance —
  byte-exact stays byte-exact; the bench ratchet only tightens. **Named
  residue, not averaged away:** `forms` at 3.35x warm is the largest remaining
  gap and the clearest M13 target; `image_en_fqa` at 11.24x warm is the proof
  that single-image documents are where the engine is genuinely behind, since
  M12 made *repeated* image work free (§9.1's cache) and left *single* image
  work exactly where it was (§10.3).

## M12b — Second performance pass: resolution-aware decode, the walk, and the arena question  *(after M12; before release)*  — **MET 2026-09-01, with two targets missed and named**

**Milestone record: docs/status/M12b.md** (§9 is the scorecard, §10 what is owed
to M13). All four items are done. **Three of the four had their premise
corrected by measurement**, and the corrections are the milestone's real
output: P1's scaled-decode diagnosis was wrong on three of its four cited
documents, P2 closed the arena question *against* `bumpalo`, and P3 was
retargeted mid-milestone after P1 and P2 jointly retired its original premise —
then delivered the largest result, **−42.6%** on the engine half of
`vector_paths_1751` and **−27.9%** on the whole render, confirmed on three
independent rasterizer backends. Conformance was **byte-identical after every
commit**. **Missed and recorded as missed:** the `image` class at −33.4%
against ≥40%, and the ratchet re-baseline, which is an unpaid debt rather than a
decision. **Superseded and never measured:** the `forms` warm target — no item
in this milestone ran an oracle side-by-side, so `forms` stands where M12 left
it at 3.35x. The measurement-integrity thread is the milestone's character:
**four near-false findings were caught rather than published** (M12b.md §7),
including a reproducible +5% regression that survived a bisection and did not
exist.

M12 closed the warm number (0.97x geomean) and named its own residue. This
milestone spends that residue list, in the order the *evidence* ranks it — not
the order the classes appear in the bench table. Every rule from M12 still
binds: **the perf-dep bar** (a dep is admitted only on a committed A/B showing
>= 10% on one class or >= 5% geomean *against a tuned no-dep baseline*),
**conformance is the regression gate** (`scripts/ci.sh` conformance cluster on
every perf commit; the scoreboard's monotone rule holds), and the **bench
ratchet** only tightens. A hypothesis that measures to zero is written down as
a non-result, not deleted and not rounded up — §10 of M12.md is the model, and
that section exists because two claims were withdrawn after a proper A/B
refuted them.

**The framing that must not be lost:** the cold number is 3.46x and the warm
number is 0.97x *on the same engine*. Part of that spread is a measurement
asymmetry (the oracle column was always its warm marginal pass — its per-page
image cache is on by default and unbounded, so `pdfium_test`'s repeat pass
skips every decode, and its true cold was never measurable). Part of it is real
missing work. **P1 is the part that is real.** Do not "fix" the asymmetry by
changing the cold convention; it is documented and ratcheted deliberately.

- **P1 — Resolution-aware image decode — DONE, and its premise was wrong**
  (docs/status/M12b-P1.md). The seam landed via `[spec]` and the JPX half is
  wired, byte-identically; the **speed came from somewhere else entirely**, and
  that is the item's real finding. Only **one** of the four documents this
  bullet names as evidence is a JPEG at all — `image_bug_898443`,
  `image_bug_583804` and `image_en_fqa` are Flate, which the oracle does not
  resolution-reduce either (`cpdf_dib.cpp` passes the level count to exactly two
  branches, JPX and DCT). And on the one file that *is* a JPEG, instrumenting
  the decode found `zune_jpeg` at 17% of the build against `apply_codec_decode`
  at 83% — a `/Decode` array evaluated per byte over a hundred million bytes,
  which is a kibibyte of lookup table (**-67.9%** on that build). The larger win
  was downstream of every codec: `to_pixmap` reached bytes through a float round
  trip that is provably the identity on all 256 values, and deleting it is
  **-35% on the `image` class's cold render geomean** — helping the JBIG2 and
  Flate documents as much as the JPEG. Both changes are *proved* pixel-identical
  by exhaustive test, and conformance was **byte-identical** after each of the
  three commits. **Target missed: -33.4% against ≥40%**, the shortfall being
  `image_en_fqa` (552 small images; `to_pixmap` totals 0.1 ms there, so its cost
  is per-image and out of scope) and `image_ccitt_3bigpreview` (band edge). The
  zune-jpeg `scale_denom` request is written up at
  `docs/upstream/zune-jpeg-scaled-decode.md` with our numbers *including* the
  one that argues against its urgency — it is worth ~59 ms of decode plus most
  of a remaining 464 ms of buffer-sized downstream work on one file, against the
  658 ms option (b) already took off it. Still owed: `ratchet update` on an idle
  machine (§9.2 — the machine carried load 9–15 all session and a baseline
  written under that permanently loosens a one-way ratchet).

  *Original brief:* The single largest confirmed gap, and
  the one that is a *missing feature* rather than a slow loop. The oracle
  computes a libjpeg `scale_denom` from the **destination** size
  (`core/fpdfapi/page/cpdf_dib.cpp:531-566` — "its dimensions are
  authoritative") and passes it to
  `core/fxcodec/jpeg/libjpeg_scanline_decoder.cc`, so a 5000x5000 JPEG headed
  for a 64x64 device rectangle is decoded at reduced resolution *inside the
  inverse DCT* — 1/64th of the sample work, before a single output pixel
  exists. We decode full resolution in `crates/pdfrum-page/src/image/dct.rs`
  (`decoder.decode()`, no scale hint) and then throw 99.98% of it away in
  `pdfrum_render::stretch::prescale`. This is why `build/image_image_en_fqa` is
  516 ms and `build/image_image_bug_718762` is 326 ms — **the cost is in
  `build`, not in the rasterizer**, which is also why M12's compositor work
  could not touch it.

  The two codecs are *not* the same problem and must not be batched:

  - **JPX is already available and is the cheap half.** `hayro-jpeg2000 0.4.0`
    exposes `DecodeSettings::target_resolution: Option<(u32, u32)>` — "a hint
    for the target resolution that the image should be decoded at" — and
    `crates/pdfrum-page/src/image/jpx.rs` passes `None`. JPEG 2000 carries
    reduced-resolution levels in the codestream, so this is the format's own
    mechanism, not an approximation. No new dependency, no upstream work. Wire
    the device footprint through and A/B it. **Do this first**; it is the
    lowest-risk proof that the plumbing (see the next bullet) is right.
  - **JPEG has no such API, and the obvious shortcut is a trap.** zune-jpeg
    0.5.15 has `idct_1x1_func` and `idct_4x4_func`, which *look* like libjpeg's
    scaled IDCTs but are not: `src/mcu.rs:585-591` selects them by **coefficient
    sparsity** (`len <= 1`, `len <= 10`) and they still write a full 8x8 block.
    They are a sparse-block shortcut, not a scaled-output path, and cannot be
    repurposed. `DecoderOptions` has `set_max_width`/`set_max_height`, but those
    are rejection guards, not scale hints. So the JPEG options are, in
    preference order: **(a)** upstream a real `scale_denom` to zune-jpeg (new
    block-writing code plus MCU geometry — genuine work, and the *right* home
    for it); **(b)** decode full-resolution but make the reduction cheaper and
    fused, so the win is in `prescale`/`to_pixmap` rather than in the decoder;
    **(c)** a scanline-windowed decode if zune's API permits consuming rows
    without materializing the whole surface. **Measure (b) before committing to
    (a)** — (b) is bounded, in-tree, and if it captures most of the win then the
    upstream patch becomes an optional follow-up rather than a blocker. Filing
    the zune-jpeg feature request with our numbers attached is worthwhile
    regardless, alongside the hayro-jbig2 issue M13 already owes.

  **The plumbing is the actual deliverable, and it is shared.** A decoder can
  only decode to a target if the target is *known at decode time*, and today the
  device footprint is a render-side fact while decode is a page-side one. That
  seam — how `pdfrum-render` tells `pdfrum-page` "this image lands on 64x64
  device pixels" without `pdfrum-page` growing a dependency on the renderer or a
  transform type leaking into a public image API — is a **SPEC.md contract
  change** and goes through the `[spec]` protocol before the code lands, not
  after. The image cache key must include the requested resolution (M12's cache
  is keyed on `(ObjRef, PixmapRequest)`; a scaled decode makes resolution part
  of identity, and getting this wrong returns a 64x64 decode for a later
  full-page draw — a *correctness* bug, so it needs a test that draws the same
  image at two sizes on one page). And decoding at reduced resolution changes
  pixels: expect SSIM movement, hold the monotone rule, and if a file regresses,
  say so and keep the reduction *only* where it is at worst neutral. The oracle
  does this too, so its own goldens are the evidence for what the tolerance
  should be.

- **P2 — Profile the walk before touching it, and settle `bumpalo` on data.**
  **DONE — the arena question is closed, against** (docs/status/M12b-P2.md;
  DEPS.md's `bumpalo` row is now `NOT ADMITTED` and the evidence queue's second
  entry is struck through, so **no open arena row remains**). `perf` stayed
  blocked (`perf_event_paranoid` is still 4; the one-line request is recorded in
  the status doc and nothing depends on it), so path (b) is what landed: in-walk
  phase timers and per-site allocation counters behind a default-off feature,
  kept as permanent introspection. **The three-way split the milestone asked
  for:** colour conversion **≤ 7.2%** and under 0.5% on three of four documents;
  allocation churn **0.6%**; interpretation **60–90% of the engine half** and
  the majority on three of four. The no-dep path went first and took the traffic
  away — `RenderCaches` buffer reuse for the zero-area scan and the glyph
  placement, plus an `Arc` in the glyph cache to stop copying an outline the
  bitmap path never reads — cutting the heaviest document from **3696
  allocations per render to 42** for **-9.7%** on `vector_paths_1751` and
  **-6.0%/-5.5%/-4.3%** on the text documents, every one byte-identical on the
  scoreboard. Against *that* baseline the arena measures **+265%/+121%/+97% —
  slower**, and it would also have put a lifetime in a public type
  (`RenderCaches` is `pub caches` on the facade's `RenderSession`), which
  disqualifies the design on its own. **What is left, named and deliberately not
  attempted:** interpretation is 794 ns per object of per-object constant
  overhead — a question about the walk's shape, not a loop or a buffer. Two
  numbers handed to other items rather than acted on: the `image` phase is
  **1.2%** of a *warm* engine half, so P3's case must be made cold; and
  `shading_axial_radial`'s engine half is 61% `pattern.rs` residue, which has no
  owner.

  M12's biggest surprise, and its most-deferred question: the **engine half is
  not small** — 85.4% of `mixed_tcpdf_045`, 59.8% of `vector_paths_1751`, 53.8%
  of `text_foxittext` is `pdfrum_render::walk` and the `Vec` churn under it, not
  the rasterizer. (P2 re-measured these before changing anything and found
  `mixed_tcpdf_045` at **50.2%**, not 85.4% — M12's own P2 image cache landed
  after §3.1 was taken and moved that document's balance. The other two
  reproduce. See docs/status/M12b-P2.md §2; the "engine half is not small"
  premise survives, the single largest figure does not.) M12 deliberately refused to reach for an arena there, and the
  refusal is the point: the profile attributed cost to the *seam*, not to
  symbols, so "85% is engine" does not distinguish allocation churn from colour
  conversion from interpretation. **Guessing here is exactly what the dep
  protocol exists to prevent.**

  So the prerequisite is a real symbol-level profile. `perf` is installed on
  this machine but `kernel.perf_event_paranoid` is **4**, which blocks it;
  `scripts/profile.sh` already detects this and degrades rather than failing.
  Two ways forward, and the agent takes whichever is available without
  blocking: (a) ask for `sudo sysctl kernel.perf_event_paranoid=1` — record it
  as a *request to the user in the status doc*, do not attempt privilege
  escalation; (b) the fallback M12 already specified — finer-grained
  instrumentation inside the walk, which is dep-free, works under any paranoid
  setting, and is useful afterwards regardless. **(b) is the default; (a) is a
  bonus if granted.**

  Only once the profile names symbols does `bumpalo` get decided, and the order
  is fixed by DEPS.md: **the no-dep path first** — extend `RenderCaches` buffer
  reuse to the walk's per-object temporaries — then A/B the arena against *that*
  tuned baseline, not against today's code. Arena lifetimes must never appear in
  a public type. If allocation turns out not to be the cost, the honest outcome
  is a **NOT ADMITTED** row in DEPS.md with the profile as evidence, exactly as
  `rustc-hash` and `fearless_simd` got, and the walk's real cost gets optimized
  on its own terms instead.

- **P3 — ~~The scalar conversion loops~~ → the walk's per-object overhead.**
  **DONE** (docs/status/M12b-P3.md). The engine half of `vector_paths_1751`
  falls **2.526 → 1.451 ms (−42.6%)** and its per-object constant **504 → 290
  ns**, for **−27.9%** on the whole render, from two changes that are neither a
  loop nor an arena: the cull test's exact box is now **bracketed** between a
  subset and a superset that decide it without solving a cubic (`b7fe49e`,
  −14.8%), and the five heap buffers a path object took to hand the device one
  geometry are now one (`d51ef60`, −13.3%). Both byte-identical on the
  conformance scoreboard over all 1675 files. **The five named suspects
  measured**, and two of them were nothing: the `RenderCtx` threading is one
  field-set per render on a flat object list, and **no measurement found a cost
  in the dispatch `match`** — a five-variant jump table was never the expense;
  what the arms *call* was.

  **Two corrections came out of it, and neither reverses a verdict.** P2's
  "allocation in the walk is 0.6%" was taken on a `Site` list covering `walk.rs`
  and `clip.rs`; with `paint.rs` and `path.rs` counted the same render was making
  **21421 allocations and 5.6 MiB**, not one. And `shading_axial_radial`'s
  "61% `pattern.rs` residue" is **not `pattern.rs`** — it is the Coons/tensor
  mesh rasterizer in `shading/patch.rs`, which `Phase::Shading` never wrapped
  because the mesh kinds do not reach `draw_to_pixmap`. **That item should be
  re-aimed rather than closed**: it is 28.7 ms of `shading_axial_radial` and
  47.1 ms of `shading_tcpdf_030`, it is the largest unattributed engine cost
  left in the corpus, and `shading_coons` — a Coons document with *zero* patch
  calls — is the control to start from. The arena verdict and the colour-
  conversion closure are untouched by both.

  **Three non-results are recorded** (§6), one of them a *reverted* change that
  measured zero because LLVM was already doing it. **And a harness trap is
  documented at length** (§4): `profile`'s plain loop rebuilds the page graph
  every iteration and is 82% `pdfrum-page` on a content-heavy document, so it
  produced a reproducible **+5% "regression"** on `vector_en_tem` that survived
  a bisection and does not exist — `--sample` shows the same document at −1.1%
  with every phase agreeing to three decimals. That is the third false finding
  this milestone; read §4 before trusting a number from that binary.
  **A full 264-benchmark run was taken** and the warm family — the convention
  M12's exit target is judged on — is **clean: 132 entries, 32 improved, zero
  regressed**, with `vector_paths_1751` at **−14.5% / −10.4% / −9.4%** across
  the exact, vello and tinyskia backends. Three independent rasterizers agreeing
  is what places the saving in the engine code they share.
  **The `ratchet update` is nevertheless still owed**, deliberately: all eight
  regressions are in `render-cold`, which rebuilds the page graph per iteration,
  and five of them have no case argued in advance — so they block the update
  under a rule written down *before* the numbers
  (`scripts/ratchet-decide.py`, committed at benchmark 57 of 266). Each is
  regressed in exactly one of its four group/backend cells while improving in
  its own warm counterpart, and a self-A/B of byte-identical binaries moves the
  two largest by −4.4% and +7.0% — but that evidence was gathered *after* they
  blocked, and retrofitting a case is the drift the ratchet exists to prevent.
  **A ratchet that declines to write a file is not a verdict on the
  engineering.** Whoever pays the debt should know the cold family will move
  enormously for reasons that are **not P3's**: `image_bug_718762` (−56%),
  `vector_tcpdf_009` (−67%) and `text_quick_start` (−47%) are **P1's image work
  landing in a baseline that has not been rewritten since before P1**.

  *Original brief:* **Retargeted 2026-08-31, after P1 and P2 took the original target away.** The
  item as first written aimed at `to_pixmap`'s ~4 us per output pixel (M12 §10).
  Two independent measurements retired that premise before this item started,
  which is the system working rather than a plan failing:
  - **P1 already took the largest piece.** `to_pixmap`'s float round trip was
    *proved* to be the identity on all 256 byte values and deleted (`bfb6f42`),
    and the `/Decode` array became a 1 KiB lookup table instead of an
    evaluation per byte over 100 M bytes (`33bcc3f`). Those are the loop-
    structure wins this item was going to go looking for.
  - **P2 measured what was left and it is small.** The `image` phase is **1.2%
    of a warm engine half**, because M12 §9.1's cache already pays it, and
    colour conversion is **≤ 7.2%** of the engine half on every profiled
    document. On `image_en_fqa` — the document the original P3 text leaned on —
    `to_pixmap` totals **0.1 ms**. The residue is per-image overhead across 552
    small images, which is a different problem from a slow per-pixel loop.

  What is left there is genuinely arithmetic and genuinely small: the Adobe
  CMYK 4-D interpolated table, ~403 ms on `image_bug_718762` (P1 §, left
  deliberately). It stays a candidate but it is **not** the biggest one.

  **The biggest one, and this item's new remit: interpretation.** P2's profile
  (docs/status/M12b-P2.md §9) leaves exactly one bucket standing —
  **interpretation is 60–90% of the engine half, and after P2's three landed
  changes it is essentially all of it.** It is not one function and not a loop:
  on `vector_paths_1751` the engine half is 3.98 ms across 5010 objects,
  **794 ns per object**, spent in the dispatch `match`, the cull test,
  `GraphicsState` reads, the `RenderCtx` threading, and `draw_path`'s five-case
  decision tree. **No P-item owned this**, which is why it is being given one.

  This is per-object *constant* overhead, so the only way to move it is to **do
  less per object**. Rank by measurement before touching anything: the cull test
  (an object rejected early costs nothing else), the dispatch shape, and the
  `GraphicsState`/`RenderCtx` access pattern are the three named suspects, and
  P2's default-off instrument (`0ebc7e9`) is already in the tree to attribute
  between them. **Vectorization is not the tool here** and neither is an arena —
  P2 closed that. `shading_axial_radial`'s engine half is **61% `pattern.rs`
  residue**, which P2 flagged as unowned; fold it into this item's ranking.

  Two constraints carried from the retired text, because they still bind: the
  `fearless_simd` reopening condition is about **span length** and nothing in
  this item reopens it; and if a measurement ever does justify SIMD on full
  image rows, that is a *new* measurement against a tuned scalar baseline, not a
  reversal of M12 §3.9.

- **P4 — `memchr` stays closed, and the reason is written down.** It needs an
  *input*, not a bar: `open` is tens of microseconds on all 44 corpus documents
  against renders of milliseconds, because the oracle's checkout contains no
  large linearized file. If P1-P3 produce no such input, leave the row as-is.
  Do **not** synthesize a giant PDF purely to justify a dependency — that is
  benchmarking the benchmark.

- **Targets (adjustable).** Judged on the **cold** convention, since that is the
  one this milestone is about, with warm held as a **no-regression** gate:
  cold geomean on the `image` class **>= 40% faster** than the committed
  baseline (P1 is the mechanism; if the JPEG half lands only as option (b),
  >= 20% and the JPX half proven separately); `forms` warm **below 2.5x** oracle
  (P3); warm geomean **no worse than 0.97x** and the conformance scoreboard **no
  worse than committed** on every commit; and the `bumpalo` question **closed
  either way** — admitted with an A/B, or declined with a profile — so DEPS.md
  has no open arena row when the release milestone starts (**met: declined with
  both, 2026-08-31**). Every ratchet entry that moves gets re-baselined in the
  same commit that moves it.

  **Scored 2026-09-01** (docs/status/M12b.md §9 is the scorecard): `image` cold
  geomean **MISSED at −33.4%** against ≥40%, though the fallback clause
  (≥20% if the JPEG half lands only as option (b), which is what happened) is
  met and the JPX half is proven as plumbing rather than as speed. `forms` warm
  below 2.5x is **SUPERSEDED and not measured** — P3 was retargeted away from it
  and no item ran an oracle side-by-side, so `forms` stands at M12's 3.35x and
  is the largest single item handed to M13. Warm geomean no worse than 0.97x is
  **met as the no-regression gate it exists to enforce** (132 warm entries, zero
  regressed) but **not re-measured against the oracle**, and is stated as an
  inference rather than a measurement. Conformance **met and stronger**:
  byte-identical, not merely regression-free. `bumpalo` **met**. `memchr`
  **met — untouched**, no large lexing input was produced and none was
  synthesized. The re-baseline clause is **missed and owed** (see Exit).

- **Exit:** `docs/status/M12b.md` written in M12.md's register — hypothesis, A/B,
  verdict, *including the non-results* — plus DEPS.md rows updated for
  `bumpalo` (and `memchr` if its status changed), SPEC.md carrying the
  decode-target contract via `[spec]`, PLAN.md marked, and the bench baseline
  re-committed. Withdrawn claims are recorded, never deleted.

  **Met 2026-09-01, with one clause outstanding and named.** `docs/status/M12b.md`
  is written as the milestone record above the four item docs — §7 collects the
  four near-false findings with their mechanisms, §9 scores every target
  met/missed/superseded, §10 lists what is owed. DEPS.md's `bumpalo` row is
  `NOT ADMITTED` with the A/B and the profile, `memchr` is unchanged (it needs an
  input, not a bar), and `wgpu`/`vello` carry M12c's scoped exemption with the CI
  check that bounds it. SPEC.md §7 carries the decode-target contract via
  `[spec]` (P1). **The bench baseline is NOT re-committed**, and that clause is
  the milestone's declared debt: `scripts/ratchet-decide.py` returns exit 2 over
  the committed run at `docs/status/data/M12b-P3-bench.txt` because five entries
  regressed with no case argued in advance, and building a case after an entry
  blocks is the drift rule R1 exists to prevent. Every regression in the
  milestone is in `render-cold`, which rebuilds the page graph per iteration;
  the **warm family — the convention M12's exit target is judged on — is 132
  entries, 32 improved, ZERO regressed.** A ratchet that declines to write a
  file is not a verdict on the engineering. Verified for the exit rather than
  assumed: `scripts/ci.sh` green, `cargo nextest run` 3049/3049, and the
  conformance scoreboard per-file identical to the committed one.

## M12c — GPU backend: `vello` on `wgpu`  *(parallel with M12b; independent crate)*

Promoted out of "post-1.0 options" on 2026-08-31 by an argument that resolves
the objection which put it there. **The reasoning is worth recording, because it
is the only place the pure-Rust guarantee is deliberately relaxed.**

DEPS.md left GPU `vello` as "a post-M8 decision" with a careful hedge: `wgpu`
reaches OS graphics drivers, which it called "system API calls, not vendored C"
— an *argued exemption* from the pure-Rust rule rather than a clean pass. The
resolving argument: **pdfrum's most likely consumer is a Rust GUI frontend**
(egui, iced, dioxus — all wgpu-backed). In that process `wgpu` is *already in
the dependency tree and a device is already open*. Refusing it preserves purity
for nobody; it merely forces a CPU rasterize-then-upload path in an application
that is holding a GPU device the whole time. So the exemption buys a real
capability and costs a guarantee that, for that consumer, was already spent.

Two constraints keep this from relaxing the guarantee for everyone else, and
they are not negotiable:

- **Isolation.** The backend lives in its own crate, `pdfrum-raster-vello-gpu`,
  that **nothing in the core ring depends on** — not the `pdfrum` facade's
  default, not `pdfrum-tool`'s default features. A headless or embedded build
  must still resolve a tree with **zero `wgpu`**, and `cargo-deny`'s bans on
  `cc`/`bindgen`/`cmake`/`pkg-config` stay enforced on every other crate. Prove
  this with a committed check, not an assertion: a `cargo tree` assertion in CI
  that the facade's default feature set contains no `wgpu`.
- **Device injection, not device creation.** The API takes a caller-supplied
  `wgpu::Device` and `Queue`. A frontend already has them, and standing up a
  second device inside a PDF library is waste the embedder cannot opt out of.
  Owning-the-device may exist as a convenience constructor behind a feature for
  headless use, but the *primary* constructor borrows. This is the difference
  between a backend an application can adopt and a demo.

**Versions** — ~~(checked 2026-08-31): `vello` 0.10 targets `wgpu` 30, which is
current, so the shared-device story with today's frontends is real rather than a
version-skew fight.~~ **WRONG, and withdrawn on measurement during
implementation (docs/status/M12c.md §1). `vello` 0.10.0 — the newest published
vello — depends on `wgpu` 29, and no vello release targets 30.** Two `wgpu`
majors in one tree are unrelated types, proved by compiling it: `expected
vello::wgpu::Device, found wgpu::Device`, with no conversion possible. And the
gap is not a lag the ecosystem is closing — `egui-wgpu` 0.36 requires `^30.0`
and `iced_wgpu` 0.14 requires `^27.0`, so vello sits *between* two of the three
frontends this section names and matches neither.

**That is a finding about this milestone's own premise, not a pin detail.** The
exemption above is bought entirely with the claim that a frontend "already has
a device open" — so it is worth stating plainly what ships: device injection is
implemented and is the primary constructor, and today it works only for a
caller on `wgpu` 29. For an egui or iced application right now the shared-device
story does **not** work; such a caller must hold its graphics stack back to
vello's `wgpu` or let this backend open its own device, which is the cost the
exemption was granted to avoid. The block is upstream and the fix is a version
bump rather than a redesign — when vello publishes against `wgpu` 30 the pin is
the only line in this workspace that changes — but until then the exemption's
cost is paid here and its benefit arrives on someone else's release schedule.
Pin through `vello` exactly (never a direct `wgpu` dependency, which could
disagree) and re-export vello's own `wgpu` so an embedder learns which one to
hand us from the docs rather than from a type error.

- **G1 — the backend.** Implement `RenderDevice` (`crates/pdfrum-render/src/device.rs:91`)
  against `vello` 0.10. The seam is already proven by three backends and the
  vocabulary types are `kurbo`/`peniko`, which vello consumes natively, so this
  is mostly a mapping exercise. The parts that are *not* mechanical, and which
  the status doc must address explicitly:
  - **Layers and clips.** The trait's contract requires every `push_layer`
    matched by a `pop`, and vello's scene model is retained rather than
    immediate. Blend modes, soft masks, and transparency groups are where a
    naive mapping silently diverges.
  - **`draw_image`'s prescale contract is CPU-shaped, and on GPU it is
    backwards.** The docs on that method say a reduced image "has already been
    box-filtered down to roughly its device size by `prescale`" so the two-tap
    kernel runs near 1:1. On a GPU you would instead upload the full texture and
    let the sampler and its mipmaps do the reduction. **Do not unilaterally
    change that contract** — M12b P1 is concurrently making decode itself
    resolution-aware, and these two interact. Measure it, write down what the
    right GPU answer is, and route any contract change through `[spec]` with P1's
    outcome in hand.
  - **Pattern cells and shadings**, which the CPU path decomposes into thousands
    of tiny fills — the same decomposition that made SIMD useless in M12 §3.9.
    A GPU may want the opposite structure entirely. Note it; do not rewrite the
    engine for it in this milestone.

- **G2 — what "correct" can even mean here, decided before measuring.** GPU
  rasterization is **not bit-reproducible across vendors and drivers**, so this
  backend **cannot join the conformance scoreboard on the CPU backends' terms**
  and must never be allowed to weaken it. It is a **Tier C** participant only
  (cross-backend divergence, per PLAN.md's fidelity tiers): the CPU backends
  remain the oracle-compared ones, and the GPU backend is compared against
  *them*, with a divergence budget stated up front and justified. Large
  divergence is a backend bug. **If making the GPU backend agree required
  changing the CPU output, that is the wrong direction and is forbidden** — the
  CPU output is what the oracle validates.

- **G3 — measurement, on real hardware.** This machine has two NVIDIA
  RTX 4090-class cards plus AMD integrated graphics with render nodes present
  (`/dev/dri/renderD128-130`), so the backend can be *measured*, not written
  blind. `vulkaninfo` is absent but is only a diagnostic; `wgpu` talks to the
  loader directly. Benchmark honestly: **include upload and readback**, because
  an embedder rendering a page to a texture pays them, and a GPU number that
  excludes them is marketing rather than measurement. Report the crossover —
  the page complexity below which the CPU backend wins, which for small simple
  pages it certainly will. A backend that loses on the corpus but wins on
  heavy vector pages is a **success with a documented envelope**, not a failure;
  say which documents fall on which side.

- **Targets (adjustable).** Correctness before speed: every corpus document
  renders without panic or device loss, and Tier-C divergence against
  `raster-exact` is inside a stated, justified budget on all 44 bench documents.
  Then: a measured speedup on at least the `vector` and `shading` classes at
  a stated resolution, upload and readback included, with the CPU-wins
  crossover documented. **The facade's default backend does not change**, and
  the conformance scoreboard stays byte-identical throughout — this milestone
  adds a capability and must not perturb an existing number.

- **Exit:** `docs/status/M12c.md` in M12.md's register (hypothesis, method,
  measurement, verdict, non-results included); a DEPS.md row for `wgpu`/`vello`
  that states the exemption, **its blast radius, and the CI check that bounds
  it**; the isolation check committed; and the crate excluded from the M13
  publish set unless it is genuinely ready.

**MET 2026-08-31, with one target missed and named** (docs/status/M12c.md §10
is the scorecard). `pdfrum-raster-vello-gpu` renders all 44 bench documents on
an RTX 4090 with **zero Tier-C interior differences** and no page where one
backend painted and the other did not; the edge residue is traced to a single
engine call site (`shading/patch.rs:480`'s `FullCover`, which vello cannot
express per-primitive) rather than averaged away. Isolation is
`scripts/check-no-wgpu.sh`, run by `ci.sh` and **verified by negative control**.
`cargo-deny` needed no relaxation of `cc`/`cmake`/`bindgen` and one `wrappers`-
scoped `pkg-config` exception whose safety rests on a measured feature
resolution. The facade's default is unchanged — it cannot name this backend —
and the scoreboard is byte-identical by construction.

**The `shading` speedup target is missed, and it is the same finding as the
divergence.** Upload and readback included, the backend is 2.10x slower than
`vello_cpu` on the corpus, or **2.64x on rasterization alone** once a null
backend shows four of its apparent wins are pure decode common to both columns.
But `vector` delivers: **4.8x faster on `vector_en_system`**, the heaviest
vector page. `shading` comes in at 2.96x, because the engine decomposes patches
into thousands of tiny fills (M12 §3.9) — the structure a GPU is worst at, and
the one this milestone forbade restructuring. The crossover is **not a
threshold**: the cheapest page the GPU wins is 1.40 ms of CPU work and the
dearest the CPU wins is 180.51 ms. Page *shape* predicts the winner, not size —
the GPU wins one big scene and loses many small offscreen targets, each of
which is a texture, a dispatch and a `map_async` host stall.

Two findings outlive the numbers. **The exemption's benefit cannot currently be
spent** (§1): device injection works and is primary, but vello 0.10 pins `wgpu`
29 while `egui-wgpu` requires `^30.0` and `iced_wgpu` `^27.0`, so no released
frontend can hand us its device — the cost is paid now, the benefit waits on an
upstream release. And **the 2.64x is an interface property, not a shader
problem** (§8.6): `RasterBackend` returns a `Pixmap`, so every group, soft mask
and pattern cell is a host round trip. That is where future GPU work starts.
The crate stays `publish = false` and out of the M13 publish set.

## M12d — Paying M12b's debts  *(before release; the release itself is the user's)*  — **MET 2026-09-01, with M1 and M2 restated as owed** (docs/status/M12d.md)

M12b closed with nine items stated as owed (docs/status/M12b.md §10). The user
is doing M13 themselves, so this milestone exists to hand them a tree with the
engineering debts paid and the measurement debts either paid or honestly
re-stated. Every M12 rule binds: **the perf-dep bar**, **conformance as the
regression gate** (pure-perf changes leave the scoreboard byte-identical;
parity changes may only move it *up*), the **monotone ratchet**, and the
register of M12.md §10 — a refuted claim is withdrawn in place, never deleted.

The items sort into two kinds, and the split is the plan.

### Engineering debts — start now, disjoint file ownership

- **D1 — `shading/patch.rs` gets its owner** (M12b §10 item 3) — **DONE
  2026-09-01, record `docs/status/M12d-D1.md`.** The netted patch figure falls
  **−22.7% / −25.2% / −18.5%** on `shading_axial_radial` / `shading_tcpdf_030`
  / `shading_tensor`, and the engine half **−13.7% / −16.0% / −16.5%**,
  measured in both orders against a ±2% floor bought by a self-A/B.
  `shading_coons`, the control, is unmoved. Conformance byte-identical on the
  scoreboard and on all 202 page hashes of a direct two-binary comparison.
  **One change carried it** — a single path buffer per patch in place of a
  `BezPath` per cell, 28 368 of them on one document. **Four directions
  measured to zero and three of those are structurally zero**: merging adjacent
  cells has nothing to merge (the longest run of same-coloured neighbours is 2,
  usually 1, because the split test and the merge test are the same test with
  opposite signs), no cell in the corpus is outside the buffer it draws into,
  and the compiler had already seen through the `Option` scaffolding. **M12
  §3.9's reopening condition is therefore still unmet and now has a reason:**
  making these spans long is a different rasterizer, not an optimization of
  this one. `pattern.rs` stays exonerated and was not revisited. The ratchet
  was **not** touched — see M1.

- **D2 — the §1.15 glyph-spacing heuristic** (M12b §10 item 8). **Premise
  withdrawn 2026-09-01:** steps 1 and 2 below were already on `main`
  (`ba8662f`, `10905fe`, 2026-08-29, never reverted — the scoreboard's timestamp
  predates them, which is what made them look absent). D2's only owed work was
  step 3; it also found and fixed a second divergence (fontdb naming faces by
  name ID 16 where the oracle's enumerator uses ID 1). Both fixes move
  **0 of 1675** files — paid on the oracle comparison, not on a number. The
  text below is kept as written. A parity
  feature, not a perf item. `CPDF_Font::LoadCharPositions`
  (cpdf_font.cpp:449-467) gated by `ShouldApplyGlyphSpacingHeuristic`
  (cpdf_font.cpp:240-264): when the PDF's declared width is narrower than the
  substituted face's, the glyph is scaled horizontally (`adjust_matrix_ =
  [pdf/font, 0, 0, 1]`), with a second branch that shifts the origin. On
  `bug_601362`, `/MissingWidth 506` vs Arimo's 667 = 0.759 — the oracle's glyph
  is 20 px wide where ours is 28 px at equal height. `SubstFont::is_actual_font_loaded`
  exists as dead code and is the gate. **The evidence came from a reverted
  branch (patch at `scratchpad/croscore-fix/`): re-derive before building.**
  Order matters: land the heuristic first, *then* re-apply the croscore-boundary
  fix from that patch (which was correct and was reverted only because this
  heuristic was missing — it made `bug_601362` regress from 0.99078 to
  0.98788 on the *right* face). Also close the serif-bit divergence the same
  investigation found (`SystemFontDb::describe` from PANOSE vs the oracle's
  face-name-contains-"Serif"). Owns `crates/pdfrum-render/src/text.rs` and
  `crates/pdfrum-font/src/subst/`. Parity: scoreboard may only improve;
  monotone rule absolute.

- **D3 — single-image work** (M12b §10 item 5, M12 §10.3) — **DONE
  2026-09-01, record `docs/status/M12d-D3.md`.** `image_en_fqa` falls
  **−76.7% cold** (679.7 → 158.7 ms), its page-graph build **−83.7%** and its
  warm walk **−40.4%**; the `image` class cold geomean is **−14.5%** on top of
  P1's −33.4%. Conformance byte-identical after every one of four commits.
  **The brief's premise was wrong about what the document is**, and correcting
  it is what located the cost: not 552 minified 1-bit masks but **301 draws of
  a 2x2 RGB image carrying an `/SMask`**, the 8.33x minification being the
  *mask's*. Those draws take `render_masked_image`, which sits outside
  `Phase::Image` **and** outside M12 §9.1's render cache — so 73.5% of the
  render was in the profile's unattributed residue. Three costs, all the
  mask's: `separate_mask` copied a coverage plane `ImageMask::Alpha` already
  holds (32.0% → 0.008 ms, now a borrow); the reduction box-filtered four
  identical copies of one channel (41.7% → 25.4%, now `reduce_gray_to` with
  the expansion after the reduction); and — the larger half, in the *build* —
  `load_mask_image` ran the float round trip **P1 §7 deleted from `to_pixmap`
  and could not reach**, because it lives in `pdfrum-page`. Every change is
  proved byte-identical by test, not by the corpus. **Caching measured to
  8%** — 276 distinct masks in 301 draws — so the work had to get cheaper, not
  be skipped, which is M12 §10.3's residue in its exact form.
  **`image_bug_718762`: the brief's ordering hypothesis is refuted.** The box
  filter taps all 5000 source columns, so no pixel `to_pixmap` converts is one
  `prescale` discards; and P1 §8.3's oracle evidence against averaging-before-
  converting stands. What was reachable — the row walk's index, in front of the
  4-D table — is **−10.4% of `to_pixmap`** (409.9 → 367.3 ms, non-overlapping
  sets). The remaining ~360 ms is the `scale_denom` decode, still upstream's.
  Owned `crates/pdfrum-render/src/image.rs`, `stretch.rs`, and
  `crates/pdfrum-page/src/image/`.

- **Deferred, on purpose:** interpretation (item 4). P3 took it 504 → 290 ns per
  object and named the two suspects that measured at nothing. Reopen only if an
  oracle side-by-side (below) shows it still matters; do not spend a third pass
  on it blind.

### Measurement debts — gated on an idle machine, not on work

- **M1 — the ratchet re-baseline** (item 1). The one hard debt. Cannot be paid
  while unrelated jobs hold load at 8+; five cold-group entries block and the
  rule is that they are re-judged on a *fresh* run. `scripts/ratchet-decide.py`
  runs first, then `ratchet update`. The cold family will move enormously for
  **P1's** reasons — say so in the commit.
- **M2 — the oracle side-by-side** (item 2). `scripts/bench-oracle.sh`, never
  run in M12b. Until it runs, every "versus PDFium" claim dates from M12.
  `forms` warm stands at 3.35x unmeasured. Run it in the same idle window as
  M1, and re-score M12b's `forms` target against the result.

### Not this milestone

Items 6 (two upstream filings — drafts written, filing is the user's), 7 (the
GPU exemption waits on a vello release) and 9 (`perf_event_paranoid`, the
user's machine) are the user's or upstream's. They are listed so no one
re-derives them.

- **Targets**, scored in docs/status/M12d.md §8. D1: a measured reduction on
  the netted patch figure with the control unchanged — **MET**, −22.7% /
  −25.2% / −18.5% netted, `shading_coons` unmoved, byte-identical on all 202
  page hashes. D2: `bug_601362` back above 0.99 *on the oracle's face* —
  **SUPERSEDED**, it stood at 0.999117 *before the item opened*, earned by
  `ba8662f`/`10905fe`, which the brief wrongly called reverted; the croscore
  fix landed — **SUPERSEDED**, same two commits; no file regressed — **MET**,
  0 of 1675 moved. D3: a measured reduction on `image_en_fqa` and/or
  `image_bug_718762` cold — **MET**, −76.7% cold on the first and −10.4% of
  `to_pixmap` on the second. M1/M2: paid, or re-stated as owed with the load
  that prevented it — **OWED, re-stated**: load never below ~7 and 42 at close,
  two unrelated `python3` jobs at 450–490% CPU (§7).
- **Exit:** `docs/status/M12d.md` in M12.md's register, PLAN.md marked, and the
  tree handed to the user for M13 with no engineering debt left unaddressed —
  **MET.** The three engineering debts are paid or accounted for, two of them
  by first correcting a false premise in the brief that named them; what
  remains is M1, M2, and the items that were always the user's or upstream's.
  DEPS.md needed no change and that was verified mechanically, not assumed:
  no `Cargo.toml`, `Cargo.lock` or DEPS.md line moved in the whole milestone.

---

# Phase 3 — The functionality the charter excluded, where the oracle has a corpus  *(planned 2026-09-01)*

Phase 1 drew the scope line at "no JS, no XFA, no interactive widget UI, whole
file in memory". That line was right for getting to a conformance-proven core.
This phase revisits it with one criterion: **a feature is planned here only if
the oracle checkout carries fixtures to measure it against.** The survey that
produced this plan (2026-09-01, `testing/` in the READ-ONLY checkout):

| Feature | Fixtures | Where |
|---|---|---|
| JavaScript | 146 files, **46 `_expected.txt` goldens**, 4 `.evt` | `testing/resources/javascript/`, `run_javascript_tests.py` |
| Form interaction (FORM_On* events) | **139 embeddertests**, only 2 gated on V8 | `fpdfsdk/fpdf_formfill_embeddertest.cpp`, `pdfium_test --send-events` |
| Linearized / progressive load | **103** corpus PDFs with `/Linearized`; 14 tests | `fpdf_dataavail_embeddertest.cpp` |
| Flatten | 13 tests | `fpdf_flatten_embeddertest.cpp` |
| Thumbnails | 14 tests | `fpdf_thumbnail_embeddertest.cpp` |
| Signatures (read) / attachments / searchex | 10 / 14 / 6 tests | respective embeddertests |
| XFA | 35 corpus PDFs | `testing/corpus/xfa_specific/` |

Every rule from Phases 1–2 binds: design brief before code, `[spec]` protocol,
the closed dependency manifest, conformance as the regression gate with the
monotone rule, M12.md §10's register for status docs, path-scoped commits.
**The existing scoreboard must not move except upward** — none of this touches
the rendering core's numbers, and any milestone that does has a bug.

## M14 — Form interaction  *(first: no new dependency, and it unblocks M15's event cascade)*

The CPWL/formfiller layer Phase 1 excluded on the grounds that it "only matters
with JS". The survey refutes that premise: **137 of 139** form-interaction
tests run with V8 off. What it is: hit-testing, focus, mouse and keyboard
events on widgets (`FORM_OnLButtonDown/Up`, `OnChar`, `OnKeyDown`, focus
in/out), text-field editing with selection/undo/redo, checkbox and radio
toggling, combo/list selection — and appearance regeneration after each.
**The text-layout half already exists**: `pdfrum-doc/src/vt/` is the variable-
text engine M6 built and M10-era wave 10 wired to widget bodies; what is
missing is the *editing* state machine on top of it and the event model.

- **Brief first** (`docs/design/pdfrum-form.md`): behaviour inventory of
  `fpdfsdk/formfiller/` + `fpdfsdk/pwl/` (the state machines, not the C++
  class shapes — STYLE §1), the exact event ordering, what each `.evt` verb
  means (`pdfium_test`'s `--send-events` parser is the spec), and every
  assertion in the 137 non-V8 embeddertests as the port target.
- **Design constraint**: no widget object hierarchy. A form session is a data
  record (focused field, selection, undo stack, dirty set) plus functions that
  take an event and return the appearance updates — the same data/logic split
  every crate here has. Lives in `pdfrum-doc` (or a `pdfrum-form` crate if the
  brief argues the boundary; `[spec]` either way).
- **Oracle**: the existing V8-off build already runs `--send-events`. Add the
  `.evt`-driven pixel goldens to the golden store and a `form-events` cluster
  to the harness; port the 137 embeddertest assertions as nextest tests.
- **Exit**: 137/137 non-V8 embeddertest assertions pass; `.evt` corpus pixel
  goldens pass at the standard threshold; facade gains an event API
  (`Form::on_mouse_down(...)`, `on_char(...)`, …) documented with an example;
  existing scoreboard unmoved.

**Rulings 2026-09-01 (orchestrator, on the brief's §5 escalations; the user
may overrule any of these — each is a doc edit and a crate boundary, nothing
irreversible).** The brief is `docs/design/pdfrum-form.md` (`dc2e53f`).

- **E2 accepted — the counts above were wrong both ways.** 11 of the 139 are
  V8-gated (not 2), 7 are XFA, and four `pwl/`+`formfiller/` embeddertest
  files hold **70 uncounted V8-free tests**. Exit criterion restated:
  **191/191** assertions ported and passing (121 + 70); the 11 V8-gated names
  are listed in the brief §4.1 for M15. Of the 59 `.evt` files, 32 are XFA
  (excluded), 4 are JavaScript (suppressed naming M15), **27** are M14's.
  The harness baseline (`6f33c82`) scored 38 event rows — the reviewer of the
  harness slice must reconcile 38 with 27 before goldens are treated as the
  target.
- **E3 accepted — new crate `pdfrum-form`**, depending on `pdfrum-doc`,
  no new external dependency (DEPS.md unchanged, verified mechanically at
  close). Three additive `pdfrum-doc` changes authorized (brief §2b table).
  SPEC §15 is written by the implementation agent from brief §3.2 as its
  first `[spec]` commit.
- **E1 accepted — clarifying clause, not reversal**, applied to SPEC §10 in
  the same commit as this ruling.
- **E4 accepted — `Cascade` is the third seam.** STYLE §2b's list is amended
  under the `[spec]` protocol that clause itself names. The enum alternative
  would put a feature-gated `boa` type into `pdfrum-form`'s public surface;
  the generic-parameter fallback would put a type parameter on the facade.
  One `&mut dyn Cascade` at one call site.
- **E5 accepted** — `Limits.max_undo_items: u32 = 10_000`, minimum 4,
  recorded in SPEC §10's Limits line.
- **U1 answered by the baseline**: 15 of the 38 event rows already pass with
  the unfocused appearance; the 23 that fail are where the work is.
- **U2 (`SearchWordPlace` tie-break) is assigned to the `pdfrum-doc` slice**,
  which must read `cpvt_variabletext.cpp` and record the rule in its status
  doc before `place_at_point` is written.
- **OQ6 (caret phase)**: settle by inspecting one golden PNG at the start of
  the edit-control work; record in `docs/status/M14.md`.

**Work split (cross-vendor rule: each vendor reviews the other's slice).**
Grok: `.evt` harness (landed `0eff591`..`6f33c82`, Claude reviews), then the
`pdfrum-doc` additive slice. Claude/Opus: `pdfrum-form` + facade + SPEC §15,
Grok reviews.

*Amended 2026-09-01 17:10.* Grok completed the harness slice and part 1 of
the `pdfrum-doc` slice (`567f0b9`), then its runner died at turn start three
times in a row (16:20, 17:03, 17:04) with zero output, including the session
holding the `pdfrum-form` review. The rest of the `pdfrum-doc` slice (the
uncommitted `field_body` caret/selection overlay, then the four `vt` place
queries with the strict half-width tie-break from U2) is reassigned to a
Claude/Opus agent under the same `pdfrum-doc`-only ownership; the
`pdfrum-form` review is retried on Grok once, else a Claude/Opus reviewer
does it. Grok keeps the cross-review of the `--send-events` wiring and of
the `vt` slice if it can hold a session. Recorded so the cross-vendor rule's
gaps are visible, not silent.

## M15 — JavaScript via `boa`  *(after M14, whose event cascade the field scripts hang off)*

Settled 2026-09-01: **the engine is boa**, pinned exactly, behind a cargo
feature, admitted to DEPS.md on the pure-Rust bar (verify its tree with
`cargo-deny` — no C, no `-sys`). Boa is at 95.5% of test262, register VM,
runtime limits for loop/recursion/stack — and PDF JavaScript is ES3-era
"core JavaScript", so the *language* is not the risk. The work is the
**Acrobat object model**, which PDFium implements in `fxjs/` (~20k LOC C++):
`app`, `Doc`, `Field`, `event`, `util`, `color`, `global`, and the `AF*`
library (`AFNumber_Format`, `AFDate_*`, `AFSimple_Calculate`, …). Alternatives
(`rquickjs`, `deno_core`) bind C/V8 and fail the purity rule outright.

- **Scoring needs no V8 build**: the 46 `_expected.txt` goldens are checked in
  — they are `Alert:` transcripts of `app.alert` output, byte-exact Tier-A.
  `testing/tools/run_javascript_tests.py` is the comparison contract. A V8-
  enabled oracle build (`pdf_enable_v8=true`) is needed only for *pixel*
  goldens of JS-driven forms; treat it as an optional M15b, since V8 is heavy
  to build and the checked-in goldens carry the milestone.
- **Order inside the milestone, by value**: (1) engine binding + `util` +
  `app.alert` (scores the transcripts); (2) the `AF*` format/calculate/
  validate functions and the document-open action — this is where a JS-off
  renderer *visibly* differs (a field with `AFNumber_Format` shows raw text
  where Acrobat shows `$1,234.00`), and it needs no user interaction; (3) the
  full field-event cascade (keystroke → validate → calculate → format) on top
  of M14's event model; (4) `Doc`/`Field` mutation surface.
- **Sandbox is a first-class requirement**: boa's runtime limits (iteration,
  recursion, stack) are configured from `Limits`, the DOM exposes no I/O, and
  a script that exhausts a limit is a `Diagnostic`, never a hang or a panic.
  This is a *stronger* property than the C++ has; say so in the brief.
- **Exit**: 46/46 `_expected.txt` byte-exact; `AF*` formatting reproduces
  the oracle's field appearances on the forms corpus (V8 build) or, without
  it, on hand-verified fixtures; 2/2 V8-gated formfill tests pass; a script
  with `while(true)` terminates via the limit; boa admitted in DEPS.md with
  its tree audit.

## M16 — Linearized and progressive loading

The v1 decision was whole-file-in-memory. 103 corpus files are linearized and
`fpdf_dataavail` has 14 tests, so the feature has an oracle. What it is: the
linearization dictionary + hint tables, first-page-first availability, and an
availability API (`is_doc_avail`, `is_page_avail`, `is_form_avail`) over a
byte-range source. The parser brief already inventoried the C++'s linearized
path (and flagged its metadata-decryption inconsistency, parser OQ). Design
constraint: `Document` keeps its current whole-file constructor unchanged;
progressive load is an *additional* source abstraction, not a rewrite of
`ObjectStore`. **Exit**: 14/14 dataavail assertions; every linearized corpus
file renders page 1 with only its first-page byte range supplied; full-file
load numbers unchanged (bench ratchet).

## M17 — The small APIs with tests

- **Flatten** (13 tests): bake annotation appearances into page content. Both
  halves exist — appearance streams (M6) and page mutation + regeneration
  (M11); this is composition plus the C++'s ordering/`/Rect` rules.
- **Thumbnails** (14 tests): `/Thumb` decode via the existing image ladder,
  plus the raw-stream accessor.
- **Audit, then close**: signatures (read: 10 tests), attachments (14),
  searchex (6). `pdfrum-doc` already touches all three; port the assertions,
  fix what fails, and record what was already covered.
- **Exit**: every listed embeddertest assertion ported and passing.

## XFA — declined, with the count written down

35 corpus files exist, so it *qualifies* under this phase's criterion, and the
decision to decline is therefore stated rather than implied: XFA is
~140k LOC of C++ (`xfa/` + `fxjs/xfa`), is deprecated by its own vendor, is
disabled in Chrome, and its 35 files are 2% of the corpus. It stays out.
Reopen only with a consumer who needs it, not a corpus that has it.

## M13 — Release

MSRV declared and CI-checked; repository URL; bottom-up family publish to
crates.io per docs/status/M8.md (round-one --no-verify for dev-dep
back-edges); README badges + final docs pass; the 24h fuzz-gate certificate
re-run to completion on an idle machine; upstream hayro-jbig2 issue filed.
*Post-1.0 options (explicitly out of Phase 2):* progressive/linearized
loading, JS actions via boa, re-keying/encryption-mode conversion on save.
*(GPU `vello` was promoted out of this list into M12c on 2026-08-31.)*
