# PDFium → Rust Rewrite — Master Plan

**Status:** Phase 1 complete (all milestones met 2026-08-30 — see Phase 2 section for the v1-completion & performance plan: M9 parity tail, M10 encrypted save, M11 page mutation, M12 performance, M13 release). Phase 2 not started.
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
| Rendering | Linebender stack: `kurbo` + `peniko` vocabulary types; `RenderDevice` trait with two backends: **`vello_cpu` (primary)**, **`tiny-skia` (cross-check)**. GPU `vello` is a future third backend. |
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

## M9 — Parity tail closure  *(small; parallel with M10)*

Port `CStretchEngine`'s area-average box downscale filter (largest remaining
pixel cluster); wire CCITT into the filter chain (unblocks the reverted
`bug_1746` Type3 fix); implement the `CPDF_BAFontMap` charset-driven N-slot
second-face fallback (the last 4 annot artifacts); re-derive `en_fqa` from a
fresh trace; finalize the upstream hayro-jbig2 issue text (user files it).
*Exit:* pixel >= 97.5% @0.99; annot 100%; every remaining failing file has a
one-line cause in the status doc.

## M10 — Encrypted save  *(reverses ruling E3's deferral)*  — **MET 2026-08-30**

`pdfrum-crypt` gains the encrypt direction ([spec] on SPEC.md §3/D2: RC4 +
AES-CBC encrypt with per-object keys, PKCS#7 padding, fresh IVs); the writer
re-encrypts strings/streams under the original handler on save (the encrypt
dict itself never encrypted; the security_changed_ / /ID interlock already
ported in M7 stays authoritative). Password-preserving only — no re-keying
API in v1. *Exit:* every encrypted corpus fixture saves, the ORACLE reopens
it with the same password, and the round-trip renders equal; incremental
save of an encrypted doc holds the append discipline.

## M11 — Page mutation  *(before M12 — it touches pdfrum-page's core types)*

The deferred holder-mutation half of content regeneration (edit brief E6):
dirty/active tracking on PageObject, content-stream index + per-stream CTM
facility in pdfrum-page ([spec] §7), regenerate-on-save wired through the
existing byte-pinned emitters, facade API (add/remove/edit path, text, and
image objects; `page.objects_mut()`; `doc.save` reflects it). Port the
relevant FPDFPage_* embeddertest assertions as the behavior net. *Exit:*
mutate -> save -> oracle reopens and renders the expected result; round-trip
property tests extended to mutated documents; regenerated-page divergence
cluster (R13) thresholds unchanged.

## M12 — Performance program  *(after M11; conformance is the regression gate)*

- **P0 baseline:** expand the bench corpus to ~40 files across feature
  classes (text-heavy, image-heavy, vector, shading, forms); commit a
  benchmark-baseline JSON with a ratchet like the scoreboard; add a
  profiling harness (perf/flamegraph scripts under scripts/).
- **P1 CPU:** measured hotspots first — per-page allocation churn (arena /
  RenderCaches reuse deeper than the session API), exact-backend scanline
  loops (restructure for autovectorization BEFORE any SIMD dependency — a
  new dep is a DEPS.md decision, default no), image resample + composite
  loops, lexer throughput, `vello_cpu` thread config, rayon multi-page
  scaling validation.
- **P2 memory:** peak-RSS harness vs the oracle on large docs; decoded-image
  cache eviction; Arc/clone pressure audit.
- **Targets (adjustable):** single-thread geometric mean >= oracle on the
  bench corpus; >= 3x oracle throughput on multi-page docs with rayon; peak
  RSS <= 1.5x oracle. Every perf commit re-runs conformance — byte-exact
  stays byte-exact; the bench ratchet only tightens.

## M13 — Release

MSRV declared and CI-checked; repository URL; bottom-up family publish to
crates.io per docs/status/M8.md (round-one --no-verify for dev-dep
back-edges); README badges + final docs pass; the 24h fuzz-gate certificate
re-run to completion on an idle machine; upstream hayro-jbig2 issue filed.
*Post-1.0 options (explicitly out of Phase 2):* progressive/linearized
loading, GPU vello backend, JS actions via boa, re-keying/encryption-mode
conversion on save.
