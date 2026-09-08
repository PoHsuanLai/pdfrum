# Compare

pdfrum (`cargo add pdfrum`, default features) against other Rust PDF
crates, on the same 44 files, scored against `pdfium_test`. Extra pdfrum
rows are the same facade with a different rasterizer — speed and SSIM sit
in the same tables.

Not the ratchet. The ratchet asks “did this commit get slower?” Compare
asks “where do we stand?” Harness: [`benches/compare/`](../../benches/compare/).

## Engines

| engine | C in the build | ops |
|---|---|---|
| **pdfrum** | no | open, render, text (`VelloCpuBackend`, default) |
| pdfrum-agg | no | render (`AggBackend`) |
| pdfrum-tinyskia | no | render (`TinySkiaBackend`) |
| pdfrum-vello-gpu | no | render (`VelloGpuBackend`; `--features gpu`, skipped if no adapter) |
| hayro | no | open, render |
| hayro-interpret | no | text (harness-assembled from `draw_glyph`) |
| pdf (`pdf-rs`) | no | open |
| pdf_oxide | no | open, render, text |
| pdf-extract | no | open, text |
| lopdf | no | open, text |
| pdfium-render | yes (`libpdfium.so`) | open, render, text |
| mupdf | yes (vendored C) | open, render, text |

`pdfrum` is `cargo add pdfrum`. `pdfrum-agg` and `pdfrum-tinyskia` compile
with the default compare build; they do not change the default engine's
code path. Open and text do not go through a rasterizer, so they stay on
`pdfrum`. The GPU row needs `--features gpu` (wgpu) and is "not run" when
no adapter is present (`PDFRUM_ALLOW_SOFTWARE_GPU=1` accepts lavapipe).

Pure-Rust peers are the default feature set. C engines need
`--features c-engines`. GPU needs `--features gpu`.

## Operations

| op | call | correctness |
|---|---|---|
| `open` | parse + page tree | opened |
| `render` | page 1, RGBA, 150 DPI | pixels vs `pdfium_test --png` |
| `text` | page 1 text API | characters vs `pdfium_test --txt` |

`--parity` turns off path AA, image interpolation, and annotations on
engines that have those knobs. Text AA stays on: no peer offers a knob.

## Tables

`compare report <json>` prints all of these from one run:

| table | reads |
|---|---|
| Engines | who ran, version, C, ops |
| Work matrix | path AA, text AA, image interp, annotations, form fields |
| Correctness — render | SSIM buckets vs oracle PNG (`>= 0.99` is the conformance floor) |
| Correctness — text | exact / whitespace-normalized / token F1 vs oracle text |
| Robustness — open | opened / error / panic / crash / timeout |
| Speed | warm median ms per file, then median and p95 across files |
| Memory | peak child RSS (VmHWM minus process floor) |
| Losses | files where a peer is closer to the oracle than pdfrum |
| Coverage | one corpus file per feature |

Other commands, other JSON:

| command | JSON suffix | table |
|---|---|---|
| `compare run` | `corpus44`, `pdfium-sample` | the tables above |
| `compare adoption` | `adoption` | crates, C, build time, stripped size, `unsafe`, licence |
| `compare throughput` | `throughput` | pages/s, 1 and N threads |

Raw files: [`data/`](data/). Latest: `2026-09-07-258e24cc72e3-*`.

## Method

Oracle: `pdfium_test` at PDFium `a043bed4a0d7`, scratch copy,
`--time=1399672130 --croscore-font-names --font-dir=<checkout>/third_party/test_fonts`.
pdfrum gets the same font dir via `SubstitutionOptions`. Peers do not.

Pixels: composite over white, grayscale SSIM (same as
`conformance/src/ssim.rs`). Text: exact, whitespace-normalized, token F1.

Each (engine, file, op) is a child process, 10 s deadline, 1 cold + up to 3
warm. Time is wall. `RAYON_NUM_THREADS=1` except throughput.

## Run

```sh
cd benches/compare
cargo build --release                       # pdfrum + CPU backends + pure-Rust peers
cargo build --release --features c-engines  # + pdfium-render, mupdf
cargo build --release --features gpu        # + pdfrum-vello-gpu (wgpu)
cargo run --release -- run --label corpus44 --corpus ../../benches/corpus \
    --checkout /path/to/pdfium-c++ --pdfium-lib /path/to/libpdfium.so \
    --out ../../docs/benchmarks/data/<date>-<commit>-corpus44.json
cargo run --release -- report <json>
cargo run --release -- report <json> --losses
```

## Current

`258e24cc72e3`, idle box, 2026-09-07.

| metric | run 3 | run 5 |
|---|---|---|
| corpus-44 render `>= 0.99` | 35/44 | **38/44** |
| corpus-44 median SSIM | 0.9995 | **0.9999** |
| corpus-44 text byte-exact | 21/44 | **42/44** |
| 209-file sample text exact | 165 | **204** |
| render RSS median | 51.5 MiB | **31.3 MiB** |
| peak (`image_bug_583804`) | 1539 MiB | **934 MiB** |
| throughput 1 / 8 threads | 27.8 / 34.7 pp/s | **36.0 / 48.4** |
