# PDFium → Rust Rewrite — Master Plan

**Status:** M1 code-complete (2026-08-29): 1468/1468 loads agree with the oracle, metadata+pageinfo 100% Tier-A byte-exact; 24h fuzz gate running. M2 (font/text) and M3 briefs done; font implementation in flight.
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
| `pdfrum-raster-vello` | Skia backend | `vello_cpu` implementation of `RenderDevice`. Primary. |
| `pdfrum-raster-tinyskia` | AGG backend | `tiny-skia` implementation. Cross-check + determinism baseline. |
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
- **M5 — Full rendering.** Shadings 1–7, tiling patterns, transparency groups/soft masks/blend modes, Type3 fonts. *Exit:* ≥ 95% pixel corpus at SSIM ≥ 0.99, ≥ 80% exact-or-near-exact; burn-down loop owns the tail.
- **M6 — Document features.** `pdfrum-doc`: annots + appearance generation, AcroForm fill (no JS), bookmarks/links/actions, struct tree. *Exit:* `--annot`/`--show-structure` Tier-A ≥ 98%; form-cluster pixel tests green (forms rendered from generated appearances).
- **M7 — Edit & save.** `pdfrum-edit`: full save, incremental save, page import, subsetting. *Exit:* round-trip property tests green over full corpus (save→reload→render == original within Tier-B); oracle re-opens our saved files cleanly.
- **M8 — API & release polish.** Facade API review, rustdoc + examples, criterion benchmarks vs oracle timings, MSRV, publishability audit. *Later options:* `vello` GPU backend, `boa` JS actions, `pdfrum-capi`.

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
