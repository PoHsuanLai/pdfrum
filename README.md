# pdfrum

**A pure-Rust PDF engine: parse, render, extract text, edit.**

A rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/) — Chrome's PDF
engine — not a binding. PDFium stays alongside as a differential test oracle,
so recovery behaviour survives even where the code shares nothing.

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

The PDF spec describes files that are *correct*. Almost none are. Two decades
of a browser opening whatever the web threw at it left PDFium with undocumented
recovery folklore — how to find objects when the xref table lies, what to do
with a stream whose `/Length` is wrong. That folklore is ported. Everything
else is designed for Rust: enums, ownership, `Result`, an explicit diagnostics
channel.

No C or C++ is compiled into any library build, and no `-sys` crate appears in
the tree (`cargo-deny` plus a graph grep in CI). `unsafe_code = "forbid"` in
every crate except the C ABI.

Out of scope, permanently: XFA, and any viewer (no window, caret, or widget
chrome). This turns pages into pixels and text into strings.

## Features

| feature | default | adds |
|---|:---:|---|
| `vello-cpu` | on | default rasterizer, `VelloCpuBackend` |
| `tinyskia`, `agg` | off | `TinySkiaBackend`, `AggBackend` |
| `vello-gpu` | off | GPU rasterizer over `wgpu`; never in a default tree |
| `edit` | on | save, edit, attachments, flatten, font subsetting |
| `forms` | on | form reader and interactive session |
| `javascript` | off | run the document's own scripts (implies `forms`) |
| `codecs-all` = `jpx` + `jbig2` + `ccitt` | on | JPEG 2000, JBIG2, CCITT; Flate and JPEG are always in |
| `system-fonts` | on | substitute a missing font from the host (never on wasm32) |
| `png` | off | `Pixmap::encode_png` / `save_png` |
| `profiling` | off | stage timers for `scripts/profile.nu` |

`default-features = false` is a parser, text extractor, outline and signature
reader. A render always names its backend.

```toml
pdfrum = { version = "0.1", default-features = false, features = ["tinyskia", "codecs-all", "system-fonts"] }
```

## JavaScript is off by default

Default features read scripts as data and never run them
(`scripts/check-no-boa.nu` asserts no engine is in the tree). The
`javascript` feature turns them on behind [boa](https://boajs.dev/).
`app.alert`, `Doc.submitForm` and `app.launchURL` come back as values for
the host; no socket, file or process is reachable from a script.

```toml
pdfrum = { version = "0.1", features = ["javascript"] }
```

## Crate map

The facade is the API. Everything under it is a public library; reach past
`pdfrum` when you need to. Dependencies flow leaf-to-root: nothing below
`pdfrum-page` depends on rendering.

| Crate | Replaces (C++) | Contents |
|---|---|---|
| **`pdfrum`** | `fpdfsdk/` public API | Facade: `Document`, `Page`, `TextPage`, `Form`. Composition only. |
| `pdfrum-common` | `fxcrt` residue | `Diagnostics`, `Limits`; re-exports `kurbo`. |
| `pdfrum-object` | parser objects | `Object`, `Dict`/`Array`/`Stream`, `ObjRef`, `Resolve`. |
| `pdfrum-crypt` | `fdrm` | RC4, AES-CBC, standard security handler rev 2–6. |
| `pdfrum-filters` | `fxcodec` (basic) | Flate, LZW, RunLength, ASCIIHex/85, CCITT, predictors. |
| `pdfrum-parser` | `fpdfapi/parser` | Lexer, xref, object streams, encryption, **damaged-file recovery**. |
| `pdfrum-cmap` | `fpdfapi/cmaps` | CJK CMap tables. |
| `pdfrum-type1` | FreeType Type1 | PFA/PFB and charstrings. |
| `pdfrum-font` | `fpdfapi/font` + `fxge` | Encodings, ToUnicode, outlines (`skrifa`), substitution (`fontdb`). |
| `pdfrum-page` | `fpdfapi/page` | Content interpreter, colorspaces, functions, shadings 1–7, transparency. |
| `pdfrum-render` | `fpdfapi/render` + `fxge` | `RenderDevice` / `RasterBackend`, compositor, resampling. |
| `pdfrum-raster-vello-cpu` | Skia | Default rasterizer. |
| `pdfrum-raster-tinyskia` | AGG | Cross-check rasterizer. |
| `pdfrum-text` | `fpdftext` | Extraction, search, selection, links. |
| `pdfrum-doc` | `fpdfdoc` | Bookmarks, dests, annots, AcroForm, structure tree. |
| `pdfrum-edit` | `fpdfapi/edit` | Serializer, incremental update, page import, subsetting. |
| `pdfrum-tool` | `pdfium_test` | Oracle-flag CLI for the harness. |
| `pdfrum-markdown` | — | Markdown from a page. Facade feature `markdown`. |
| `pdfrum-cli` | — | Human CLI: `info`, `doctor`, `render`, `extract …`. |
| `pdfrum-capi` | C ABI | [`libpdfrum`](crates/pdfrum-capi/README.md) |
| `pdfrum-wasm` | — | [JavaScript binding](crates/pdfrum-wasm/README.md) |
| `pdfrum-svg` | — | SVG export. Facade feature `svg`. |
| `conformance/` | `testing/tools` | Differential harness. Not published. |

## Conformance

Correctness is agreement with PDFium over 1675 real and broken files
(`testing/corpus` + `testing/resources`). Goldens are built once; every run
writes `conformance/scoreboard.json`.

- **Tier A — byte-exact.** Text, metadata, pageinfo, structure, annot dumps,
  decoded image bytes, page counts.
- **Tier B — perceptual.** Rendered pages vs golden PNGs, SSIM. A passing
  test's threshold is never loosened (`conformance/thresholds.toml`).
- **Tier C — cross-backend.** `vello_cpu` vs `tiny-skia` on our engine.

Generated `2026-08-29`, 1675 files. Live numbers: `conformance/scoreboard.json`.

| | |
|---|---:|
| Files passing every tier | **1256 / 1675 (75.0%)** |
| Load without crashing | 100% |
| Page counts agree | 1673 / 1675 |

**Tier A**

| Artifact | Compared | Byte-exact | Rate |
|---|---:|---:|---:|
| `--show-metadata` | 1675 | 1675 | **100.0%** |
| `--show-pageinfo` | 1675 | 1675 | **100.0%** |
| `--show-structure` | 1675 | 1675 | **100.0%** |
| `--txt` (all pages) | 2061 | 2052 | **99.6%** |
| `--txt` (non-empty pages) | 999 | 990 | **99.1%** |
| `--annot` (page 0) | 1624 | 1620 | **99.8%** |

**Tier B** (1628 files with a golden)

| | |
|---|---:|
| Mean SSIM | 0.9979 |
| Median SSIM | 1.0000 |
| SSIM ≥ 0.99 | 1566 (96.2%) |
| SSIM ≥ 0.95 | 1616 (99.3%) |
| Pixel-exact | 516 |

Round-trip: the oracle reopens 100% of 1638 files we wrote.

## Building

```bash
cargo build --workspace
cargo nextest run
cargo test --doc --workspace
./scripts/ci.nu
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for tools, the oracle, and the board.

```bash
cargo run --example render-to-png -- input.pdf out/ 2.0
cargo run --example extract-text -- input.pdf
cargo run -p conformance -- run
```

## Parallelism

Every public type is `Send + Sync`. One `RenderSession` per worker:

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

## Docs

| | |
|---|---|
| [CONTRIBUTING.md](CONTRIBUTING.md) | Build, gate, board, style |
| [CHANGELOG.md](CHANGELOG.md) | What changed |
| [SECURITY.md](SECURITY.md) | Vulnerability reports |
| [docs/roadmap.md](docs/roadmap.md) | What's next |
| [docs/](docs/README.md) | Benchmarks, API snapshots, upstream bugs |

## Licence

Apache-2.0 or MIT, at your option. Contributions are dual-licensed the same
way.

This is an independent reimplementation: no PDFium *source* is in the tree.
PDFium is an external test oracle only. Upstream **data** that does travel
with the repo sits beside a `PROVENANCE.md`:

- Foxit fallback fonts in `crates/pdfrum-font/fontdata/` (BSD-3-Clause)
- CJK CMaps in `crates/pdfrum-cmap/tables/` and Unicode tables in
  `crates/pdfrum-text/tables/`
- a handful of test PDFs copied from `testing/resources` (BSD-3-Clause)
