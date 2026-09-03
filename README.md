# pdfrum

**A pure-Rust PDF engine: parse, render, extract text, edit.**

`pdfrum` is a rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/) —
the PDF engine inside Chrome — as idiomatic Rust. Not a binding and not a
transliteration: a rewrite, with PDFium kept alongside as a differential test
oracle so that the behaviour that matters survives even where the code shares
nothing.

```rust
use pdfrum::{Document, RenderOptions};

let doc = Document::open("report.pdf")?;
for page in doc.pages() {
    let pixmap = page.render(&RenderOptions::scaled(2.0))?;
    let text = page.text().to_string();
    println!("page {}: {}x{}, {} chars",
        page.index(), pixmap.width(), pixmap.height(), text.len());
}
# Ok::<(), pdfrum::Error>(())
```

## Why rewrite an engine that already works

The PDF specification is 1000 pages, and implementing it gets you a reader for
files that are *correct*. Almost none are. Two decades of a browser opening
whatever the web threw at it left PDFium with a large body of undocumented
recovery folklore — how to find objects when the cross-reference table lies,
what to do with a stream whose `/Length` is wrong, which of a damaged page
tree's contradictions to believe. That folklore is the engine's actual value,
and it is the one thing here that is ported rather than redesigned.

Everything else is designed fresh for Rust: enums with exhaustive matching
where the C++ used type-tag switches and virtual dispatch, plain data records
where it used 1000-line god classes, ownership where it used `RetainPtr` and
observer webs, `Result` and an explicit diagnostics channel where it used
out-parameters and silence.

**Pure Rust, all the way down.** No C or C++ is compiled into any library
build and no `-sys` crate appears anywhere in the dependency tree — checked
mechanically in CI by `cargo-deny` plus a dependency-graph grep, not promised
in prose. `unsafe_code = "forbid"` in every crate.

## JavaScript is off by default

A PDF may carry scripts for form validation, calculation and formatting. With
default features this engine reads them as data and never runs them, and
`scripts/check-no-boa.nu` asserts that no JavaScript engine is anywhere in the
dependency tree of a default `cargo add pdfrum` — a checked property, not a
promise. That is the *default* because an engine which executes untrusted
script out of a document is a different security proposition from a renderer,
and most PDF work does not need one.

The `script` feature turns it on. This section used to say "the future slot,
if it is ever filled, is a pure-Rust interpreter behind a trait; never V8" —
that came true: the slot is filled, the interpreter is
[boa](https://boajs.dev/), and it sits behind the `Cascade` trait as its
second implementation. `FormSession::with_scripts` builds one and installs the
document's own `/AA` scripts into it.

```toml
pdfrum = { version = "0.1", features = ["script"] }
```

What a script reaches today is the `AF*` library, `util`, `app.alert` and the
`event` object; the `Doc`/`Field` object model is not built yet, so 11 of the
oracle's 47 JavaScript fixtures reproduce byte-exactly (PLAN.md §M15). Nothing
a script asks for is performed by the library — `app.alert`, `Doc.submitForm`
and `app.launchURL` come back as values the host decides about, and no socket,
file or process is reachable from a script at all.

## What it deliberately is not

- **No XFA.** Adobe's XML Forms Architecture is a second, largely disjoint
  document format that travels inside a PDF wrapper. PDFium implements it in
  a subsystem the size of the rest of the engine. Out of scope, permanently.
- **Not a viewer.** No window, no scrolling, no text caret, no interactive
  widget behaviour. This turns pages into pixels and text into strings.

## Crate map

The facade is the API; everything under it is a public library in its own
right, and none of it is hidden. Reach past `pdfrum` whenever you need to.

| Crate | Replaces (C++) | Contents |
|---|---|---|
| **`pdfrum`** | `fpdfsdk/` public API | **The facade.** `Document`, `Page`, `TextPage`, `Form`, render and save options. Composition only — no logic. |
| `pdfrum-common` | `fxcrt` residue | `Diagnostics`, `Limits`; re-exports `kurbo` geometry. |
| `pdfrum-object` | parser object classes | The `Object` enum, `Dict`/`Array`/`Stream`, `ObjRef`, the `Resolve` trait, dict-key constants. |
| `pdfrum-crypt` | `fdrm` + security handlers | RC4, AES-CBC, the standard security handler revisions 2–6. |
| `pdfrum-filters` | `fxcodec` (basic) | Flate, LZW, RunLength, ASCIIHex/85, CCITT fax, predictors. |
| `pdfrum-parser` | `fpdfapi/parser` | Lexer, cross-reference tables and streams, object streams, incremental updates, encryption, lazy loading, **damaged-file recovery**. |
| `pdfrum-cmap` | `fpdfapi/cmaps` | Compact static CJK CMap tables and the predefined-CMap parser. |
| `pdfrum-type1` | (FreeType's Type1) | PFA/PFB parsing and charstring interpretation. |
| `pdfrum-font` | `fpdfapi/font` + `fxge` | Font dictionaries, encodings, ToUnicode, glyph mapping; outlines via `skrifa`; substitution via `fontdb`; the glyph cache. |
| `pdfrum-page` | `fpdfapi/page` | Content-stream interpreter → typed page-object graph; graphics state; colorspaces; PDF functions; patterns and shadings 1–7; the transparency model. |
| `pdfrum-render` | `fpdfapi/render` + `fxge` | The `RenderDevice`/`RasterBackend` seam, the engine that walks the page graph, the layer compositor, image resampling. |
| `pdfrum-raster-vello-cpu` | Skia backend | `vello_cpu` rasterizer. **Primary.** *(renamed 2026-09-02, was `pdfrum-raster-vello`)* |
| `pdfrum-raster-tinyskia` | AGG backend | `tiny-skia` rasterizer. Cross-check and determinism baseline. |
| `pdfrum-text` | `fpdftext` | Extraction, reading order, whitespace heuristics, search, selection, link detection. |
| `pdfrum-doc` | `fpdfdoc` | Bookmarks, named destinations, links and actions, annotations with appearance generation, the AcroForm data model, the structure tree. |
| `pdfrum-edit` | `fpdfapi/edit` | Serializer, incremental-update writer, page import, font subsetting. |
| `pdfrum-tool` | `testing/pdfium_test` | CLI mirroring the oracle's flags and output formats byte for byte, so the harness diffs like for like. |
| `conformance/` | `testing/tools` | The differential-test harness. Not published. |

Dependencies flow strictly leaf-to-root; nothing below `pdfrum-page` may
depend on rendering. The full closed dependency set, with the rationale for
every crate, is [`DEPS.md`](DEPS.md).

## Conformance: how correctness is measured

Correctness here is not "the tests pass" — it is **"we agree with PDFium"**,
measured continuously over a corpus of **1675** real and deliberately-broken
PDFs (PDFium's own `testing/corpus` and `testing/resources`). The oracle is
run once to build a golden store; the harness then compares our output against
it on every run and writes `conformance/scoreboard.json`, which is the fitness
function every piece of work optimizes.

Fidelity is **tiered**, because demanding bit-exactness everywhere would burn
effort on antialiasing noise:

- **Tier A — byte-exact.** Text extraction (the oracle's UTF-32LE output
  transcoded), the metadata, pageinfo, structure and annotation dumps, decoded
  embedded image bytes, page counts. These are contracts: field order,
  indentation and rounding are all behaviour.
- **Tier B — perceptual.** Rendered pages against golden PNGs, scored by
  exact-match, per-channel maximum difference, and SSIM. A global floor with
  per-test overrides in `conformance/thresholds.toml`, ratcheted down over
  time — **a passing test's threshold may never be loosened**.
- **Tier C — cross-backend.** `vello_cpu` against `tiny-skia` over our *own*
  engine. Large divergence means a backend bug rather than an engine bug.

### Current scoreboard

Generated `2026-08-29`, over 1675 files. Live numbers are in
`conformance/scoreboard.json`; per-milestone narrative is in `docs/status/`.

| | |
|---|---:|
| **Files fully passing every tier** | **1256 / 1675 (75.0%)** |
| Files that load without crashing | 100% |
| Page counts agreeing with the oracle | 1673 / 1675 |

**Tier A — byte-exact dumps**

| Artifact | Compared | Byte-exact | Rate |
|---|---:|---:|---:|
| `--show-metadata` | 1675 | 1675 | **100.0%** |
| `--show-pageinfo` | 1675 | 1675 | **100.0%** |
| `--show-structure` | 1675 | 1675 | **100.0%** |
| `--txt` (all pages) | 2061 | 2052 | **99.6%** |
| `--txt` (non-empty pages) | 999 | 990 | **99.1%** |
| `--annot` (page 0) | 1624 | 1620 | **99.8%** |

Four `--annot` files remain, all in one cluster: an annotation whose `/DA`
names a font its resources do not carry falls back **per character** to a
second face chosen by the character's charset, which this project has not
ported yet.

**Tier B — rendered pixels** (1628 files with a golden)

| | |
|---|---:|
| Mean SSIM | 0.9979 |
| Median SSIM | 1.0000 |
| At SSIM ≥ 0.99 | 1566 (96.2%) |
| At SSIM ≥ 0.95 | 1616 (99.3%) |
| Pixel-exact | 516 |

**Round-trip (save) fidelity** — the oracle reopens 100% of 1638 files we
wrote; saved-versus-original render parity is 100% over a 233-file sample;
incremental-append discipline is 100%.

Milestones M1–M3 and M5–M7 are complete; M8 (this API, docs and release
polish) is in flight. The pixel tail is 62 files over 51 documents, shallow
(43 of the 51 above 0.95) and inventoried in `docs/status/pdfrum-render.md`.

## Building and testing

Stable Rust, no external toolchain, no system libraries.

```bash
cargo build --workspace
cargo nextest run                    # the test runner this project uses
cargo test --doc --workspace         # nextest silently SKIPS doctests
./scripts/ci.nu                      # the full gate, exactly as CI runs it
```

`scripts/ci.nu` runs, in order: `cargo fmt --check`, `cargo clippy
--workspace --all-targets -D warnings`, `cargo nextest run`, `cargo test
--doc`, `cargo doc --no-deps -D warnings`, the public-API snapshot gate
(`./scripts/api-snapshot.nu check` — a drift is a change to what
`cargo add pdfrum` sees; update the baseline with
`./scripts/api-snapshot.nu update` deliberately), the public-constant
sentinel sweep, `cargo deny check`, the pure-Rust dependency-tree check, and
a `cargo check` of the `fuzz/` workspace — that last one because `fuzz/` is
a separate workspace the other steps never reach, and its targets call
library API deep enough to rot through an API change unnoticed. Running the
fuzzers stays out (hours); compiling them is seconds. It is the definition
of done for every change.

```bash
cargo binstall nu                      # required: the scripts are nushell
cargo install cargo-nextest --locked   # required by the gate
cargo install cargo-public-api --locked  # required by the snapshot gate
rustup toolchain install nightly         # rustdoc JSON is nightly-only
cargo install cargo-deny --locked      # optional; the gate skips it if absent
```

**Paths outside this repository.** The oracle checkout, the oracle binary,
the golden store and the build-tree roots are named by environment variables,
each with a default relative to this repository — so a fresh clone, a CI
checkout included, resolves every one of them without an edit, and a machine
laid out differently overrides only the one that differs.

| variable | what it names | default |
|---|---|---|
| `PDFRUM_ORACLE_CHECKOUT` | the read-only C++ PDFium checkout | `<repo>/../pdfium-c++` |
| `PDFRUM_ORACLE_BIN` | its built `pdfium_test` | `<checkout>/out/Release/pdfium_test` |
| `PDFRUM_GOLDENS` | the golden store (`.gitignore`d) | `<repo>/conformance/goldens` |
| `PDFRUM_TOOL` | the `pdfrum-tool` binary under test | `<repo>/target/release/pdfrum-tool` |
| `PDFRUM_TARGET_ROOT` | the directory holding per-agent `CARGO_TARGET_DIR` trees | `<repo>/../cargo-target` |
| `PDFRUM_WORKTREE_ROOT` | where `git worktree add` puts trees | `<repo>/../worktrees` |

`scripts/env.nu` is the nushell spelling of that table, the `conformance` CLI
reads the first four through clap (a flag still wins over its variable), and
the Python generators take the first. **A test that needs the oracle skips
with a printed message when the binary is absent**, never fails on a path, so
`cargo nextest run` is green on a machine that has only this repository.
`scripts/check-no-absolute-paths.nu` is the gate that keeps it that way.

Rust build trees are large and nothing removes them for you. Incremental
compilation is off workspace-wide (`.cargo/config.toml` says why), and
`scripts/clean-targets.nu` removes every `$PDFRUM_TARGET_ROOT/<name>` tree no
running process is using — `--dry-run` to see the plan, `--keep [name]` to
spare one. Measured 2026-09-02, the first sweep found 33 trees and freed
519 GB.

**Nushell is a hard dependency for contributors.** The gate and every bench
and check script under `scripts/` is a `.nu` script, and there is no bash
fallback, because maintaining two spellings of a gate is how the two stop
agreeing. (The one-shot fixture generators are Python run under `uv`, and
`fuzz/seed-corpus.sh` is bash because it belongs to the fuzz workspace rather
than to `scripts/`.) **Minimum version 0.110**;
`cargo binstall nu` fetches a prebuilt binary, and `cargo install nu --locked`
builds it from source when `binstall` is absent.
Nothing pdfrum *ships* depends on it: this is a contributor tool, and the
library's own dependency set is still the closed table in PLAN.md §3.

**Examples.** Runnable programs live in `crates/pdfrum/examples/`:

```bash
cargo run --example render-to-png -- input.pdf out/ 2.0
cargo run --example extract-text -- input.pdf [search-term]
cargo run --example fill-form-and-save -- in.pdf out.pdf "Field Name" "value"
cargo run --release --example parallel-render -- input.pdf 4.0
```

**Conformance and fuzzing** need the oracle checkout and golden store, and are
not part of the default gate:

```bash
cargo run -p conformance -- run          # score the corpus
cargo run -p conformance -- run --triage # cluster the failures
./scripts/fuzz-gate.nu 3600 parallel     # fuzz/ is its own workspace
```

## Parallelism

Every public type is `Send + Sync`, and rendering pages in parallel needs no
ceremony beyond adding `rayon` to your own manifest:

```rust
use rayon::prelude::*;
use pdfrum::{Document, RenderOptions, RenderSession, VelloCpuBackend};

let doc = Document::open("big.pdf")?;
let pages: Vec<_> = doc.pages().collect();
let backend = VelloCpuBackend::new();

let pixmaps: Vec<_> = pages
    .par_iter()
    .map_init(RenderSession::new, |session, page| {
        page.render_on(&backend, &RenderOptions::default(), session)
    })
    .collect::<Result<_, _>>()?;
# Ok::<(), pdfrum::Error>(())
```

The document is shared by reference; the engine is single-threaded *per page*
on purpose, because a page is the natural unit of data parallelism and the
rasterizer beneath is already vectorized within one. `map_init` gives each
worker its own font, image and glyph cache.

## Project documents

| File | What it governs |
|---|---|
| [`PLAN.md`](PLAN.md) | The operative plan: milestones, exit criteria, agent orchestration. Read first. |
| [`STYLE.md`](STYLE.md) | The binding Rust style constitution. Violations block review even when tests pass. |
| [`SPEC.md`](SPEC.md) | Concrete per-crate type and API contracts, plus the `[spec]` change protocol. |
| [`DEPS.md`](DEPS.md) | The closed dependency set, with rationale and rejections. |
| `docs/design/` | Per-module design briefs: behaviour inventory, divergences, module plan, test plan. |
| `docs/status/` | Live progress per crate and per milestone. |

## Licence

Dual-licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work shall be dual-licensed as
above, without any additional terms or conditions.

**Third-party material.** This is an independent reimplementation: no PDFium
*source code* is copied into this tree, and PDFium is used only as an external
test oracle — never built into, linked against, or shipped with anything here.
Some upstream **data** does travel with the repository, each item beside a
`PROVENANCE.md` that records its origin, its licence and how to reproduce it:

- the embedded Foxit fallback fonts in `crates/pdfrum-font/fontdata/`
  (from `core/fxge/fontdata`, BSD-3-Clause, copyright notices retained);
- generated lookup tables — the CJK CMaps in `crates/pdfrum-cmap/tables/` and
  the Unicode tables in `crates/pdfrum-text/tables/` — committed as build
  products and reproducible from the scripts in `scripts/`;
- a handful of small test PDFs under various crates' `tests/fixtures/`,
  copied unmodified from `testing/resources` (BSD-3-Clause).
