# Comparative benchmarks

pdfrum vs pdfium-render, mupdf, hayro, pdf-rs, pdf-extract. Correctness
beside speed. Subject is `cargo add pdfrum` (facade, default features).
Tables are `compare report` of JSON under `data/`.

```sh
cd benches/compare
cargo build --release                       # pdfrum + pure-Rust peers
cargo build --release --features c-engines  # + pdfium-render, mupdf
compare run --label corpus44 --corpus ../../benches/corpus \
    --checkout /path/to/pdfium-c++ --pdfium-lib /path/to/libpdfium.so \
    --out ../../docs/benchmarks/data/<date>-<commit>-corpus44.json
compare report <json>
compare report <json> --losses
```

`--parity` turns off `smooth_paths`, `interpolate_images`, `annotations`.
Losses: [`losses-explained.md`](losses-explained.md).

## Method

| op | call | correctness |
|---|---|---|
| `open` | parse + page tree | opened |
| `render` | page 1, RGBA, 150 DPI, annotations on | pixels vs oracle PNG |
| `text` | page 1 text API | characters vs oracle text |

Oracle: `pdfium_test` at PDFium `a043bed4a0d7`, scratch copy, `--time=1399672130
--croscore-font-names --font-dir=<checkout>/third_party/test_fonts`. pdfrum
gets the same font dir via `SubstitutionOptions`. Peers do not — none expose
one.

Pixels: composite over white, then grayscale SSIM (same as
`conformance/src/ssim.rs`), exact match, max channel diff. Text: exact,
whitespace-normalized, token F1.

Each (engine, file, op) is a child process, 10 s deadline, 1 cold + up to 3
warm. Time is wall. Memory is child `VmHWM` minus process floor.
`RAYON_NUM_THREADS=1`.

## Current numbers — run 5

`258e24cc72e3`, idle box, 2026-09-07.
`data/2026-09-07-258e24cc72e3-*.json`. Unchanged peers reproduce run 3 to
within 2.5%; pdfrum movements are code.

| metric | run 3 | run 5 |
|---|---|---|
| corpus-44 render `>= 0.99` | 35/44 | **38/44** |
| corpus-44 median SSIM | 0.9995 | **0.9999** |
| corpus-44 text byte-exact | 21/44 | **42/44** |
| 209-file sample text exact | 165 | **204** |
| render RSS median | 51.5 MiB | **31.3 MiB** |
| peak (`image_bug_583804`) | 1539 MiB | **934 MiB** |
| throughput 1 / 8 threads | 27.8 / 34.7 pp/s | **36.0 / 48.4** |

Render warm ALL: 11.46 → **9.80 ms** (−14.4%). Open warm ALL: −19.3%.
Text cold ALL: −46.8%. Two corpus-44 losses remain (JPX numerics; F1 0.999
on `image_ccitt_3bigpreview`).

Older runs (1–4) are in `data/` and git. Do not quote their speed columns;
runs 1–2 were on a loaded box.
