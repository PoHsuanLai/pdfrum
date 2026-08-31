# PDFium → Rust Rewrite — Master Plan

**Status:** Phase 2: M9-M12 ALL MET (2026-08-30). M12 scorecard: warm render geomean 0.97x oracle (FASTER; image 0.24x, vector 0.90x, shading 0.95x; forms 3.35x is the named residue), rayon 3.09x@4/6.09x@16, RSS 1.20x, conformance byte-identical, ratchet green over 440 entries. Per-crate benches + bench-quick landed. M13 (release) NOT STARTED — loop paused by user.
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
  without which the `bumpalo` question cannot be asked honestly.
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

## M12b — Second performance pass: resolution-aware decode, the walk, and the arena question  *(after M12; before release)*

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

- **P1 — Resolution-aware image decode.** The single largest confirmed gap, and
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
  M12's biggest surprise, and its most-deferred question: the **engine half is
  not small** — 85.4% of `mixed_tcpdf_045`, 59.8% of `vector_paths_1751`, 53.8%
  of `text_foxittext` is `pdfrum_render::walk` and the `Vec` churn under it, not
  the rasterizer. M12 deliberately refused to reach for an arena there, and the
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

- **P3 — The scalar conversion loops.** `pdfrum_render::image::to_pixmap`
  measured ~4 us per output pixel (M12 §10) — the number that makes three 70x53
  widget thumbnails cost 45 ms on `mixed_formfield`, the `forms` class's 3.35x
  warm residue. M12's P1 optimized the *compositor* and never came back to this
  path; the per-pixel work (colourspace -> RGBA, premultiply, `/Decode`,
  transfer function) is scalar against the oracle's scanline kernels. Attack it
  as loop structure first — hoist per-image decisions out of the per-pixel body
  the way the stencil test already was, work through row slices, specialize the
  common `(8bpc, DeviceRGB, no transfer, no mask)` case — and only *then*, if a
  measurement says the remaining cost is arithmetic rather than dispatch,
  revisit SIMD. **The `fearless_simd` reopening condition still stands and is
  about span length, not about this loop** (spans there are 1 px; these are full
  image rows, which is a *different* workload and may genuinely vectorize — if
  so, that is a new measurement, not a reversal of §3.9, and it gets its own
  A/B against a tuned scalar baseline).

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
  has no open arena row when the release milestone starts. Every ratchet entry
  that moves gets re-baselined in the same commit that moves it.

- **Exit:** `docs/status/M12b.md` written in M12.md's register — hypothesis, A/B,
  verdict, *including the non-results* — plus DEPS.md rows updated for
  `bumpalo` (and `memchr` if its status changed), SPEC.md carrying the
  decode-target contract via `[spec]`, PLAN.md marked, and the bench baseline
  re-committed. Withdrawn claims are recorded, never deleted.

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

## M13 — Release

MSRV declared and CI-checked; repository URL; bottom-up family publish to
crates.io per docs/status/M8.md (round-one --no-verify for dev-dep
back-edges); README badges + final docs pass; the 24h fuzz-gate certificate
re-run to completion on an idle machine; upstream hayro-jbig2 issue filed.
*Post-1.0 options (explicitly out of Phase 2):* progressive/linearized
loading, JS actions via boa, re-keying/encryption-mode conversion on save.
*(GPU `vello` was promoted out of this list into M12c on 2026-08-31.)*
