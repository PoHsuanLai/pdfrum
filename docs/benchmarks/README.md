# Comparative benchmarks — pdfrum beside its peers

PLAN.md §M21 asks for numbers against the other Rust PDF crates and the C
engines they wrap, on one corpus, one machine, one script, with the method
published and the losses listed. This file is the method and the first
numbers. Everything in it is generated from the JSON files under `data/` by
`benches/compare`, and can be regenerated with one command per table.

Two rules govern how to read it:

- **Correctness is measured beside speed.** A fast wrong answer is not a
  result, so every render row carries its distance from the oracle and every
  text row its match rate, and the speed tables count only rows that
  succeeded.
- **The subject is `cargo add pdfrum`.** pdfrum is measured through its
  facade with default features, the same way a peer is measured through its
  own public API. Nothing below reaches into the engine.

## The script

`benches/compare/` — its own cargo workspace, deliberately not a member of
the root one, on the same argument as `fuzz/`: every peer is a competing
engine, two wrap C, and none may enter the tree `scripts/ci.nu`'s pure-Rust
check walks (DEPS.md, "Benchmark peers"). Build it with the peers you want:

```sh
cd benches/compare
cargo build --release                       # pdfrum + the pure-Rust peers
cargo build --release --features c-engines  # + pdfium-render and mupdf
```

`mupdf` compiles the vendored MuPDF (`cc`, `bindgen`, a clang on the path).
`pdfium-render` binds a `libpdfium.so` at runtime — pass its path with
`--pdfium-lib` (or `PDFIUM_DYNAMIC_LIB_PATH`); without one the engine is
reported as "not run, needs libpdfium" and its rows stay in every table.

```sh
compare run --label corpus44 --corpus ../../benches/corpus \
    --checkout /path/to/pdfium-c++ --pdfium-lib /path/to/libpdfium.so \
    --scratch /tmp/compare --out ../../docs/benchmarks/data/<date>-<commit>-corpus44.json
compare run --label pdfium-sample --corpus /path/to/pdfium-c++/testing/corpus --every 4 ...
compare run --label coverage --corpus /path/to/pdfium-c++/testing --spec coverage.json ...
compare adoption --scratch /tmp/compare-adoption --pdfium-lib ... --out ...-adoption.json
compare report <json>                       # the tables, from the JSON alone
compare report <json> --losses              # the loss list as JSON
```

## What is measured, and how

### Operations

| op | what the engine is asked | correctness |
|---|---|---|
| `open` | parse the file and reach its page tree; `lopdf` and `pdf` walk their objects too, because that is what their `load` does | opened without error |
| `render` | page 1 to RGBA at 150 DPI (2.0833 px/pt), annotations on, one thread | pixels against the oracle's PNG |
| `text` | page 1's text through the engine's text API | characters against the oracle's text |

An engine whose API does not offer an operation gets a "not supported" row,
never a missing one. The engines and the exact calls:

| engine | version | open | render | text |
|---|---|---|---|---|
| `pdfrum` | this tree | `Document::open` | `Page::render_on(&VelloCpuBackend, RenderOptions::scaled(150/72), &mut RenderSession)` with a `BuildContext::with_substitution` pointed at the oracle's font directory | `Page::text_on(&mut session).to_string()` |
| `hayro` | 0.7.1 | `Pdf::new` | `hayro::render(page, &RenderCache, &InterpreterSettings::default(), &RenderSettings{x_scale, y_scale, bg WHITE})` | not supported |
| `hayro-interpret` | 0.7.0 | — | — | **harness-assembled**: a `Device` whose `draw_glyph` appends `Glyph::as_unicode()` in draw order, a newline when the baseline moves more than a tenth of an em, a space when the pen jumps more than 0.12 em past the previous advance. The crate has no text API; this row says what its interpreter *could* give a caller willing to write that device, and no more |
| `pdf-extract` | 0.12.0 | `Document::load_mem` (its own `lopdf` 0.42) | not supported | `output_doc_page(&doc, &mut PlainTextOutput, 1)` |
| `lopdf` | 0.44.0 | `Document::load_mem`, then every object visited | not supported | `extract_text(&[1])` |
| `pdf` (pdf-rs) | 0.10.0 | `FileOptions::cached().open`, every page's resources loaded | not supported | not supported |
| `pdf_oxide` | 0.3.77 | `PdfDocument::open` | `rendering::render_page(&doc, 0, RenderOptions::with_dpi(150).as_raw())` | `extract_text(0)` |
| `pdfium-render` | 0.9.3 | `load_pdf_from_file` | `render_with_config(PdfRenderConfig::new().set_target_size(w, h))` with `w, h` computed the way `pdfium_test` does (points × scale, truncated) | `page.text()?.all()` |
| `mupdf` | 0.8.0 | `Document::open` | `Page::to_pixmap(Matrix::new_scale(s, s), DeviceRGB, alpha=false, show_extras=true)` | `Page::text(TextExtractOptions::default())` |

Passwords, where the coverage spec gives one, go to every engine that has a
password API (all of them) and to the oracle's `--password=`.

### The oracle

`pdfium_test` from the PDFium checkout (the commit is in every JSON), on a
**scratch copy** of the file — it writes beside its input — with the
conformance harness's determinism recipe: `--time=1399672130
--croscore-font-names --font-dir=<checkout>/third_party/test_fonts`. Two
invocations per file, because the flags do not share one: `--png --md5
--scale=2.0833333333 --pages=0` and `--txt --pages=0`. The outputs are cached
by content hash under the scratch directory.

The same font directory is handed to pdfrum through its public
`SubstitutionOptions`, so a non-embedded font resolves to the same face on
both sides. **No peer is given it**: none of them has a font-directory knob a
crates.io user could reach, so they resolve non-embedded fonts however they
ship — hayro and pdf_oxide from embedded standard-14 substitutes, mupdf from
its base-14 set, pdfium-render from the system through its `libpdfium.so`.
That is a real difference between the engines as delivered, and it is also
why `pdfium-render`, which *is* PDFium, does not score 1.0 on every file:
its library is a different PDFium build (the pypdfium2 wheel MinerU installed,
found under `/mnt/data2/r13921098/mineru-models/pdfium/`) with system fonts,
not the oracle's build with the hermetic set.

### Pixels

Every image — the oracle's PNG (RGB for an opaque page, RGBA otherwise), and
each engine's buffer (premultiplied for pdfrum, hayro and pdf_oxide, straight
for pdfium-render and mupdf) — is composited over opaque white first, so "the
page over white" is the one thing compared. Then, over the common area:

- **SSIM**: the conformance harness's own grayscale mean-SSIM (8×8 box
  windows, K1 0.01, K2 0.03, Rec. 601 luma), copied from
  `conformance/src/ssim.rs` so this crate can stay outside the workspace and
  so the metric here can never drift from the one there without a visible
  diff. The `>= 0.99` bucket is the conformance floor (`thresholds.toml`).
- **exact**: every byte equal and the sizes equal.
- **max channel diff**, and **differing px**: the share of pixels where some
  channel differs by more than 8.

Engines round `points × scale` differently — `pdfium_test` truncates, pdfrum
rounds up, some peers go through `f32` — so sizes that differ by at most 2 px
per axis are compared over their top-left common area and the sizes are
recorded; a larger difference is a **size mismatch** (hayro on a rotated page,
for instance) and is counted rather than scored.

### Text

The oracle's UTF-32LE is decoded. Three readings of "the same":

- **exact**: the strings are identical (PDFium writes `\r\n` line breaks;
  pdfrum reproduces them, the peers mostly do not, which is most of the gap
  between this column and the next);
- **whitespace-normalized**: identical after every run of whitespace
  collapses to one space and the ends are trimmed;
- **token F1**: F1 over the multisets of whitespace-separated tokens, for the
  distribution between those two.

### Time, memory, isolation

Every (engine, file, op) runs in **its own child process** with a 10 s
deadline: a panic, an abort or a hang in one engine on one file is a row,
not a lost run. Inside the child, the operation runs once cold, then up to
three times warm — holding the document, and for pdfrum the `RenderSession`,
across the runs — as long as the next run is predicted to fit a 6 s budget;
the warm number is the median of those, or the cold run when none fit. Wall
time, `Instant`-clocked around the call. `RAYON_NUM_THREADS=1` is set for
the children so an engine that parallelises by default (`lopdf`) is measured
on one core like the others; vello_cpu under pdfrum and hayro is
single-threaded by construction.

Memory is `VmHWM` from the child's `/proc/self/status` at exit, minus the
same reading taken before the engine ran (the process's own floor), so it is
the engine's peak resident set including the file's bytes.

**The machine.** `frieren`, 32 CPUs, Linux 6.8, rustc 1.97.1, shared with
other jobs: the load average during every run is recorded in the JSON and
quoted with the tables. It was **not** idle — between 18 and 88 on 32 cores
across the runs below — so the absolute milliseconds are noisier than the
M12 numbers in `benches/baseline.json`, and the ratios between engines,
measured back to back on the same files, are the more reliable reading.

### Losses

For every file and op, any peer whose score is better than pdfrum's — SSIM
higher by more than 0.005, or a normalized text match pdfrum did not get, or
token F1 higher by more than 0.01, or an open that succeeded where pdfrum's
did not — is a loss, listed by file name in the JSON (`compare report
--losses`) and summarised below. They are work items, not footnotes.

### Adoption

`compare adoption` writes a minimal consumer crate per engine (one
dependency line, one `main` that opens `argv[1]`) into scratch and asks
cargo: `cargo tree -e normal --prefix none | sort -u | wc -l` for crates in
the tree; `cargo tree -e build` for any `cc`, `cmake`, `bindgen`,
`pkg-config` or `*-sys`; `cargo build --release` from an empty target
directory, wall-clocked; the binary after `strip`; `cargo metadata` for the
licence; and, for every crate in the normal tree, the count of the token
`unsafe` in its `.rs` files under `~/.cargo/registry/src` (workspace crates
for pdfrum) — a grep, approximate, counts `unsafe fn`, `unsafe impl` and
`unsafe {` alike and counts nothing in comments away.

### Coverage

`benches/compare/coverage.json` names one PDFium corpus file per feature in
PLAN.md §M21's list. A cell is what the engine did with that file — `ok`
with its score against the oracle, or `err`/`panic`/`crash`/`timeout` — and
nothing else; no cell comes from a README.

## Numbers

Two runs, both at commit `9139cd2` of this repository (the harness commit;
the engine under test is the same tree), oracle `pdfium_test` at PDFium
`a043bed4a0d7`, 150 DPI, 10 s per (engine, file, op), 3 warm runs.
`pdfium-render` is bound to the `libpdfium.so` named above. The machine was
shared throughout; the load averages are in each run's header line. Every
table below is `compare report data/<file>.json`, verbatim.

### Run 1 — `benches/corpus` (44 files, the M12 measurement set)

`data/2026-09-05-114527a71d4c-corpus44.json`

Run `corpus44` — commit 9139cd2f48f0 — 44 files — 150 DPI — timeout 10 s — 3 warm runs — generated 2026-09-05T10:11:11Z
Machine: frieren (32 CPUs, rustc 1.97.1 (8bab26f4f 2026-07-14)). Load before: `18:11:11 up 10 days, 19:25,  4 users,  load average: 39.93, 32.73, 39.59`; after: `18:14:22 up 10 days, 19:28,  4 users,  load average: 30.60, 29.22, 36.77`.
Corpus: `/mnt/data2/pdfium/pdfrum/.claude/worktrees/agent-a58f791ade9594178/benches/corpus` — every .pdf of the recursive listing.
Oracle: `/mnt/data2/pdfium/pdfium-c++/out/Release/pdfium_test` at a043bed4a0d7 with fonts `/mnt/data2/pdfium/pdfium-c++/third_party/test_fonts`.

### Engines

| engine | version | C in build | ops | ran |
|---|---|---|---|---|
| pdfrum | 0.1.0 | no | open, render, text | yes |
| hayro | 0.7.1 | no | open, render | yes |
| hayro-interpret | 0.7.0 | no | text | yes |
| pdf-extract | 0.12.0 | no | open, text | yes |
| lopdf | 0.44.0 | no | open, text | yes |
| pdf | 0.10.0 | no | open | yes |
| pdf_oxide | 0.3.77 | no | open, render, text | yes |
| pdfium-render | 0.9.3 | yes | open, render, text | yes |
| mupdf | 0.8.0 | yes | open, render, text | yes |

### Correctness — render, page 1 at 150 DPI against `pdfium_test --png`

| engine | rendered | exact | >= 0.99 | 0.95-0.99 | 0.80-0.95 | < 0.80 | size mismatch | error | panic | crash | timeout | median SSIM | mean differing px |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| pdfrum | 44/44 (100%) | 1 | 35 | 8 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0.9995 | 1.28% |
| hayro | 42/44 (95%) | 0 | 10 | 21 | 10 | 1 | 2 | 0 | 0 | 0 | 0 | 0.9765 | 8.33% |
| hayro-interpret | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| pdf-extract | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| lopdf | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| pdf | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| pdf_oxide | 39/44 (89%) | 0 | 8 | 15 | 12 | 4 | 0 | 5 | 0 | 0 | 0 | 0.9593 | 13.31% |
| pdfium-render | 44/44 (100%) | 15 | 31 | 11 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0.9995 | 3.01% |
| mupdf | 42/44 (95%) | 0 | 13 | 19 | 10 | 0 | 0 | 2 | 0 | 0 | 0 | 0.9750 | 6.29% |

### Correctness — text, page 1 against `pdfium_test --txt`

| engine | extracted | exact | whitespace-normalized | token F1 >= 0.9 | median token F1 | error | panic | crash | timeout |
|---|---|---|---|---|---|---|---|---|---|
| pdfrum | 44/44 (100%) | 21/44 (48%) | 39/44 (89%) | 43/44 (98%) | 1.000 | 0 | 0 | 0 | 0 |
| hayro | not supported |  |  |  |  |  |  |  |  |
| hayro-interpret | 44/44 (100%) | 11/44 (25%) | 30/44 (68%) | 37/44 (84%) | 1.000 | 0 | 0 | 0 | 0 |
| pdf-extract | 32/44 (73%) | 8/32 (25%) | 27/32 (84%) | 31/32 (97%) | 1.000 | 3 | 9 | 0 | 0 |
| lopdf | 38/44 (86%) | 10/38 (26%) | 13/38 (34%) | 20/38 (53%) | 0.914 | 6 | 0 | 0 | 0 |
| pdf | not supported |  |  |  |  |  |  |  |  |
| pdf_oxide | 44/44 (100%) | 9/44 (20%) | 18/44 (41%) | 30/44 (68%) | 1.000 | 0 | 0 | 0 | 0 |
| pdfium-render | 44/44 (100%) | 42/44 (95%) | 42/44 (95%) | 42/44 (95%) | 1.000 | 0 | 0 | 0 | 0 |
| mupdf | 42/44 (95%) | 11/42 (26%) | 28/42 (67%) | 33/42 (79%) | 1.000 | 2 | 0 | 0 | 0 |

### Robustness — open

| engine | opened | error | panic | crash | timeout |
|---|---|---|---|---|---|
| pdfrum | 44/44 (100%) | 0 | 0 | 0 | 0 |
| hayro | 44/44 (100%) | 0 | 0 | 0 | 0 |
| hayro-interpret | not supported |  |  |  |  |
| pdf-extract | 41/44 (93%) | 3 | 0 | 0 | 0 |
| lopdf | 41/44 (93%) | 3 | 0 | 0 | 0 |
| pdf | 29/44 (66%) | 15 | 0 | 0 | 0 |
| pdf_oxide | 44/44 (100%) | 0 | 0 | 0 | 0 |
| pdfium-render | 44/44 (100%) | 0 | 0 | 0 | 0 |
| mupdf | 44/44 (100%) | 0 | 0 | 0 | 0 |

### Speed — milliseconds, warm median per file, then median and p95 across files (successful rows only)

| engine | op | files | cold median | warm median | warm p95 | warm max |
|---|---|---|---|---|---|---|
| pdfrum | open | 44 | 0.40 | 0.18 | 4.24 | 29.17 |
| pdfrum | render | 44 | 75.88 | 15.88 | 56.06 | 1527.11 |
| pdfrum | text | 44 | 7.18 | 0.58 | 3.62 | 8.32 |
| hayro | open | 44 | 0.21 | 0.04 | 0.60 | 3.27 |
| hayro | render | 44 | 25.14 | 14.99 | 477.03 | 1048.13 |
| hayro | text | not supported |  |  |  |  |
| hayro-interpret | open | not supported |  |  |  |  |
| hayro-interpret | render | not supported |  |  |  |  |
| hayro-interpret | text | 44 | 1.70 | 0.50 | 4.37 | 23.40 |
| pdf-extract | open | 41 | 0.63 | 0.38 | 9.31 | 54.18 |
| pdf-extract | render | not supported |  |  |  |  |
| pdf-extract | text | 32 | 2.36 | 1.38 | 82.22 | 112.54 |
| lopdf | open | 41 | 1.00 | 0.68 | 15.09 | 63.69 |
| lopdf | render | not supported |  |  |  |  |
| lopdf | text | 38 | 0.48 | 0.25 | 52.45 | 61.14 |
| pdf | open | 29 | 0.49 | 0.12 | 1.41 | 2.23 |
| pdf | render | not supported |  |  |  |  |
| pdf | text | not supported |  |  |  |  |
| pdf_oxide | open | 44 | 0.62 | 0.29 | 2.10 | 4.21 |
| pdf_oxide | render | 39 | 78.50 | 22.59 | 1118.27 | 2948.06 |
| pdf_oxide | text | 44 | 4.07 | 0.21 | 1.96 | 7.38 |
| pdfium-render | open | 44 | 0.21 | 0.11 | 0.62 | 0.89 |
| pdfium-render | render | 44 | 13.58 | 8.30 | 312.82 | 608.71 |
| pdfium-render | text | 44 | 0.12 | 0.05 | 0.55 | 0.78 |
| mupdf | open | 44 | 0.77 | 0.05 | 0.66 | 2.87 |
| mupdf | render | 42 | 23.99 | 6.09 | 41.75 | 207.16 |
| mupdf | text | 42 | 4.73 | 0.29 | 2.66 | 18.33 |

### Memory — peak RSS of the child process (VmHWM), MiB, over successful rows; the process floor before the engine ran is subtracted

| engine | op | files | median | p95 | max | max on |
|---|---|---|---|---|---|---|
| pdfrum | open | 44 | 1.12 | 8.84 | 20.25 | mixed_formfield.pdf |
| pdfrum | render | 44 | 52.57 | 319.54 | 1539.43 | image_bug_583804.pdf |
| pdfrum | text | 44 | 11.19 | 37.21 | 67.60 | forms_signature.pdf |
| hayro | open | 44 | 0.80 | 2.69 | 5.83 | mixed_formfield.pdf |
| hayro | render | 44 | 29.96 | 207.97 | 946.61 | image_bug_583804.pdf |
| hayro-interpret | text | 44 | 3.22 | 8.63 | 9.97 | mixed_tcpdf_006.pdf |
| pdf-extract | open | 41 | 1.27 | 8.27 | 54.05 | forms_widgets_407.pdf |
| pdf-extract | text | 32 | 3.42 | 117.48 | 122.82 | image_bug_583804.pdf |
| lopdf | open | 41 | 1.72 | 8.95 | 56.31 | forms_widgets_407.pdf |
| lopdf | text | 38 | 2.93 | 51.34 | 54.95 | forms_widgets_407.pdf |
| pdf | open | 29 | 1.64 | 4.91 | 6.21 | mixed_formfield.pdf |
| pdf_oxide | open | 44 | 2.75 | 6.96 | 13.06 | mixed_formfield.pdf |
| pdf_oxide | render | 39 | 40.84 | 684.96 | 999.09 | image_bug_583804.pdf |
| pdf_oxide | text | 44 | 7.15 | 16.45 | 22.62 | mixed_formfield.pdf |
| pdfium-render | open | 44 | 2.81 | 3.30 | 3.68 | forms_widgets_407.pdf |
| pdfium-render | render | 44 | 29.14 | 63.74 | 913.92 | image_bug_583804.pdf |
| pdfium-render | text | 44 | 4.93 | 24.76 | 30.21 | mixed_tcpdf_006.pdf |
| mupdf | open | 44 | 2.70 | 2.91 | 3.00 | text_quick_start.pdf |
| mupdf | render | 42 | 28.35 | 54.77 | 914.41 | image_bug_583804.pdf |
| mupdf | text | 42 | 4.34 | 5.39 | 6.77 | image_ccitt_3bigpreview.pdf |

### Losses — files where a peer is closer to the oracle than pdfrum (16)

| peer | op | files where the peer is closer |
|---|---|---|
| hayro | render | 1 |
| hayro-interpret | text | 1 |
| lopdf | text | 1 |
| mupdf | render | 1 |
| pdf_oxide | render | 1 |
| pdf_oxide | text | 1 |
| pdfium-render | render | 5 |
| pdfium-render | text | 5 |

| file | op | peer | peer's score | pdfrum's score |
|---|---|---|---|---|
| image_ccitt_3bigpreview.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.994 |
| image_en_fqa.pdf | render | pdfium-render | SSIM 1.0000 | SSIM 0.9769 |
| image_jpx_123.pdf | render | pdfium-render | SSIM 1.0000 | SSIM 0.9874 |
| mixed_en_uicase.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.991 |
| shading_tcpdf_058.pdf | render | pdfium-render | SSIM 0.9962 | SSIM 0.9908 |
| text_bug_1029.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.957 |
| text_quick_start.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.641 |
| text_tcpdf_055.pdf | text | hayro-interpret | F1 1.000 (normalized match) | F1 0.953 |
| text_tcpdf_055.pdf | text | lopdf | F1 0.981 | F1 0.953 |
| text_tcpdf_055.pdf | text | pdf_oxide | F1 0.976 | F1 0.953 |
| text_tcpdf_055.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.953 |
| vector_en_system.pdf | render | hayro | SSIM 0.9802 | SSIM 0.9600 |
| vector_en_system.pdf | render | pdf_oxide | SSIM 0.9813 | SSIM 0.9600 |
| vector_en_system.pdf | render | pdfium-render | SSIM 1.0000 | SSIM 0.9600 |
| vector_en_system.pdf | render | mupdf | SSIM 0.9818 | SSIM 0.9600 |
| vector_tcpdf_009.pdf | render | pdfium-render | SSIM 0.9995 | SSIM 0.9593 |

**Reading run 1.** On the corpus that was built to measure pdfrum, pdfrum
renders every file, 35 of 44 pages inside the conformance floor (SSIM ≥
0.99), median 0.9995 — the same median as `pdfium-render`, which *is*
PDFium, and which is exact on 15 files because its bytes are PDFium's.
hayro, the closest pure-Rust peer, has 10 files inside the floor and two
size mismatches (a page box read differently from the oracle); pdf_oxide's renderer
refuses five files outright ("Node missing Type", "Pages node missing
Kids") and lands 8 inside the floor; mupdf, a C engine, lands 13. On text,
pdfrum matches the oracle whitespace-normalized on 39 of 44 pages (89%),
the highest of the Rust engines; pdf-extract *panics* on 9 of 44 files
(an `unsupported encoding` `panic!` in its font code, every time on a CJK
CMap), lopdf returns an error on 6 and pdf-rs cannot open 15 of the 44 at
all. Speed is where pdfrum loses: warm render median 15.9 ms against
8.3 ms for `pdfium-render`, 6.1 ms for mupdf and 15.0 ms for hayro; the
cold render (first page of a fresh session, fonts enumerated) is 76 ms
against 14–25 ms. Memory is the other loss: pdfrum's peak on
`image_bug_583804.pdf` (one very large image) is **1.5 GiB** where
every other engine stays under 1 GiB, and that file takes pdfrum 1.5 s a
render against 0.16 s for mupdf.

### Run 2 — PDFium's `testing/corpus`, every 4th file (209 files)

`data/2026-09-05-114527a71d4c-pdfium-sample.json`. PLAN.md asked for a
200-file sample "every 8th file sorted by name"; `testing/corpus` holds 836
PDFs, so every 8th is 105 files and every 4th is the 209 used here. The
sample is taken over the sorted recursive listing from index 0, so it is
reproducible from the rule alone.

Run `pdfium-sample` — commit 9139cd2f48f0 — 209 files — 150 DPI — timeout 10 s — 3 warm runs — generated 2026-09-05T10:14:38Z
Machine: frieren (32 CPUs, rustc 1.97.1 (8bab26f4f 2026-07-14)). Load before: `18:14:38 up 10 days, 19:29,  4 users,  load average: 34.62, 30.18, 36.96`; after: `18:21:01 up 10 days, 19:35,  4 users,  load average: 40.26, 32.96, 35.59`.
Corpus: `/mnt/data2/pdfium/pdfium-c++/testing/corpus` — every 4th .pdf of the sorted recursive listing, from index 0.
Oracle: `/mnt/data2/pdfium/pdfium-c++/out/Release/pdfium_test` at a043bed4a0d7 with fonts `/mnt/data2/pdfium/pdfium-c++/third_party/test_fonts`.

### Correctness — render, page 1 at 150 DPI against `pdfium_test --png`

| engine | rendered | exact | >= 0.99 | 0.95-0.99 | 0.80-0.95 | < 0.80 | size mismatch | error | panic | crash | timeout | median SSIM | mean differing px |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| pdfrum | 208/209 (100%) | 2 | 180 | 28 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0.9992 | 0.72% |
| hayro | 205/209 (98%) | 1 | 122 | 71 | 11 | 1 | 2 | 2 | 0 | 0 | 0 | 0.9913 | 2.22% |
| hayro-interpret | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| pdf-extract | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| lopdf | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| pdf | not supported |  |  |  |  |  |  |  |  |  |  |  |  |
| pdf_oxide | 167/209 (80%) | 0 | 90 | 50 | 23 | 4 | 1 | 40 | 0 | 0 | 1 | 0.9931 | 4.30% |
| pdfium-render | 208/209 (100%) | 43 | 158 | 44 | 6 | 0 | 0 | 1 | 0 | 0 | 0 | 0.9980 | 1.53% |
| mupdf | 205/209 (98%) | 0 | 101 | 87 | 11 | 6 | 0 | 4 | 0 | 0 | 0 | 0.9897 | 4.48% |

### Correctness — text, page 1 against `pdfium_test --txt`

| engine | extracted | exact | whitespace-normalized | token F1 >= 0.9 | median token F1 | error | panic | crash | timeout |
|---|---|---|---|---|---|---|---|---|---|
| pdfrum | 208/209 (100%) | 165/208 (79%) | 198/208 (95%) | 199/208 (96%) | 1.000 | 1 | 0 | 0 | 0 |
| hayro | not supported |  |  |  |  |  |  |  |  |
| hayro-interpret | 207/209 (99%) | 76/207 (37%) | 143/207 (69%) | 150/207 (72%) | 1.000 | 2 | 0 | 0 | 0 |
| pdf-extract | 143/209 (68%) | 69/143 (48%) | 132/143 (92%) | 136/143 (95%) | 1.000 | 46 | 20 | 0 | 0 |
| lopdf | 156/209 (75%) | 69/156 (44%) | 110/156 (71%) | 124/156 (79%) | 1.000 | 53 | 0 | 0 | 0 |
| pdf | not supported |  |  |  |  |  |  |  |  |
| pdf_oxide | 205/209 (98%) | 72/205 (35%) | 115/205 (56%) | 155/205 (76%) | 1.000 | 3 | 0 | 0 | 0 |
| pdfium-render | 208/209 (100%) | 204/208 (98%) | 204/208 (98%) | 207/208 (100%) | 1.000 | 1 | 0 | 0 | 0 |
| mupdf | 205/209 (98%) | 78/205 (38%) | 168/205 (82%) | 184/205 (90%) | 1.000 | 4 | 0 | 0 | 0 |

### Robustness — open

| engine | opened | error | panic | crash | timeout |
|---|---|---|---|---|---|
| pdfrum | 208/209 (100%) | 1 | 0 | 0 | 0 |
| hayro | 207/209 (99%) | 2 | 0 | 0 | 0 |
| hayro-interpret | not supported |  |  |  |  |
| pdf-extract | 163/209 (78%) | 46 | 0 | 0 | 0 |
| lopdf | 163/209 (78%) | 46 | 0 | 0 | 0 |
| pdf | 137/209 (66%) | 72 | 0 | 0 | 0 |
| pdf_oxide | 208/209 (100%) | 1 | 0 | 0 | 0 |
| pdfium-render | 208/209 (100%) | 1 | 0 | 0 | 0 |
| mupdf | 208/209 (100%) | 1 | 0 | 0 | 0 |

### Speed — milliseconds, warm median per file, then median and p95 across files (successful rows only)

| engine | op | files | cold median | warm median | warm p95 | warm max |
|---|---|---|---|---|---|---|
| pdfrum | open | 208 | 0.15 | 0.03 | 7.76 | 46.37 |
| pdfrum | render | 208 | 41.91 | 10.36 | 32.84 | 328.93 |
| pdfrum | text | 208 | 14.91 | 0.07 | 3.10 | 45.48 |
| hayro | open | 207 | 0.14 | 0.03 | 0.84 | 25.28 |
| hayro | render | 207 | 7.08 | 5.37 | 44.39 | 261.95 |
| hayro | text | not supported |  |  |  |  |
| hayro-interpret | open | not supported |  |  |  |  |
| hayro-interpret | render | not supported |  |  |  |  |
| hayro-interpret | text | 207 | 0.31 | 0.04 | 1.21 | 3.59 |
| pdf-extract | open | 163 | 0.21 | 0.10 | 2.22 | 6.32 |
| pdf-extract | render | not supported |  |  |  |  |
| pdf-extract | text | 143 | 0.17 | 0.07 | 3.60 | 83.67 |
| lopdf | open | 163 | 0.41 | 0.15 | 3.95 | 5.99 |
| lopdf | render | not supported |  |  |  |  |
| lopdf | text | 156 | 0.03 | 0.01 | 0.65 | 77.58 |
| pdf | open | 137 | 0.18 | 0.05 | 0.37 | 4.89 |
| pdf | render | not supported |  |  |  |  |
| pdf | text | not supported |  |  |  |  |
| pdf_oxide | open | 208 | 0.41 | 0.07 | 8.62 | 83.76 |
| pdf_oxide | render | 168 | 52.41 | 8.36 | 39.26 | 463.83 |
| pdf_oxide | text | 206 | 0.62 | 0.02 | 0.51 | 2.54 |
| pdfium-render | open | 208 | 0.16 | 0.05 | 0.17 | 1.73 |
| pdfium-render | render | 208 | 4.71 | 4.10 | 17.28 | 719.02 |
| pdfium-render | text | 208 | 0.05 | 0.01 | 0.24 | 1.38 |
| mupdf | open | 208 | 0.79 | 0.07 | 0.55 | 4.49 |
| mupdf | render | 205 | 6.58 | 3.44 | 15.93 | 165.08 |
| mupdf | text | 205 | 0.73 | 0.03 | 0.36 | 6.51 |

### Memory — peak RSS of the child process (VmHWM), MiB, over successful rows; the process floor before the engine ran is subtracted

| engine | op | files | median | p95 | max | max on |
|---|---|---|---|---|---|---|
| pdfrum | open | 208 | 0.89 | 13.62 | 100.21 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdfrum | render | 208 | 43.86 | 98.27 | 380.61 | fx/form/signature_4.pdf |
| pdfrum | text | 208 | 17.72 | 50.64 | 191.54 | fx/new/form/fillform.pdf |
| hayro | open | 207 | 0.70 | 4.18 | 25.63 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| hayro | render | 207 | 26.06 | 38.20 | 99.01 | fx/text/en_uicase_2_.pdf |
| hayro-interpret | text | 207 | 2.29 | 6.46 | 26.62 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdf-extract | open | 163 | 0.88 | 3.79 | 50.00 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdf-extract | text | 143 | 1.32 | 6.29 | 50.84 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| lopdf | open | 163 | 1.37 | 3.80 | 50.46 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| lopdf | text | 156 | 1.37 | 5.82 | 50.36 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdf | open | 137 | 1.24 | 2.95 | 26.14 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdf_oxide | open | 208 | 2.87 | 10.48 | 52.31 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdf_oxide | render | 168 | 31.96 | 57.18 | 187.91 | fx/FRC_3.5_part1/FRC_3.5_AuthEvent_EFOpen.pdf |
| pdf_oxide | text | 206 | 5.00 | 16.29 | 53.64 | fx/FRC_8.5_part1/FRC_8.5_Screen_Fo_JavaScript.pdf |
| pdfium-render | open | 208 | 2.99 | 3.30 | 3.45 | fx/FRC_3.5_part1/FRC_3.5_AuthEvent_EFOpen.pdf |
| pdfium-render | render | 208 | 26.97 | 41.43 | 93.75 | fx/text/en_uicase_2_.pdf |
| pdfium-render | text | 208 | 4.18 | 24.29 | 26.89 | fx/FRC_3.5_part1/FRC_3.5_v_1_length_40_Filter_standard.pdf |
| mupdf | open | 208 | 2.69 | 2.95 | 3.24 | fx/FRC_8.2.4_part1/FRC_11_8.2.4__remove_ModDate_all.pdf |
| mupdf | render | 205 | 26.14 | 32.52 | 93.11 | fx/text/en_uicase_2_.pdf |
| mupdf | text | 205 | 3.69 | 5.45 | 11.44 | fx/FRC_3.5_part1/FRC_3.5_AuthEvent_EFOpen.pdf |

### Losses — files where a peer is closer to the oracle than pdfrum (17)

| peer | op | files where the peer is closer |
|---|---|---|
| mupdf | render | 2 |
| pdf_oxide | open | 1 |
| pdfium-render | render | 5 |
| pdfium-render | text | 9 |

| file | op | peer | peer's score | pdfrum's score |
|---|---|---|---|---|
| fx/FRC_3.5_part1/FRC_3.5_Filter_PubSec_SubFilter_s5.pdf | open | pdf_oxide | opened | error (cannot open document: unsupported encryption: Adobe.PubSec: unsupported encryption: Adobe.PubSec) |
| fx/FRC_8.2.4_part1/FRC_11_8.2.4__remove_ModDate_all.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_13_8.2.4_remove_Size_value.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_15_8.2.4_remove_Size_obj.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_19_8.2.4__remove_CreationDate_value.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_21_8.2.4__remove_CreationDate_obj.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_2_8.2.4_Type_8.6__remove_value.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_4_8.2.4_Schema_8.6__remove_all.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_6_8.2.4_Schema_8.6__remove_obj.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/FRC_8.2.4_part1/FRC_8_8.2.4_View_D.pdf | text | pdfium-render | F1 1.000 (normalized match) | F1 0.889 |
| fx/action/123.pdf | render | pdfium-render | SSIM 1.0000 | SSIM 0.9874 |
| fx/image/1_image.pdf | render | pdfium-render | SSIM 1.0000 | SSIM 0.9857 |
| fx/image/1_image.pdf | render | mupdf | SSIM 0.9977 | SSIM 0.9857 |
| fx/path/transparent1.pdf | render | pdfium-render | SSIM 1.0000 | SSIM 0.9844 |
| fx/path/transparent1.pdf | render | mupdf | SSIM 0.9995 | SSIM 0.9844 |
| third_party/tcpdf/example_009.pdf | render | pdfium-render | SSIM 0.9995 | SSIM 0.9593 |
| third_party/tcpdf/example_025.pdf | render | pdfium-render | SSIM 0.9989 | SSIM 0.9913 |

**Reading run 2.** On PDFium's own corpus the gap is wider. pdfrum
renders 208 of 209 files, 180 inside the floor, median SSIM 0.9992 — above
`pdfium-render`'s 0.9980, which on this corpus pays for its system fonts
and its different PDFium build (it is exact on 43 files and off the floor on
50). hayro is inside the floor on 122, mupdf on 101, pdf_oxide on 90 — and
pdf_oxide errors on 40 of 209 renders. On text, pdfrum matches
whitespace-normalized on 198 of 208 (95%) against mupdf's 168 and
pdf_oxide's 115; pdf-extract errors on 46 and panics on 20 more; lopdf and
pdf-extract cannot open 46 of the 209 files, pdf-rs 72 — the FRC
conformance files with deliberately damaged trailers and page trees, which
pdfrum, hayro, pdf_oxide and the two C engines all recover. The one file
pdfrum cannot open is `FRC_3.5_Filter_PubSec_SubFilter_s5.pdf`: public-key
(`Adobe.PubSec`) encryption, which pdfrum, mupdf and `pdfium-render` all
report as unsupported (the oracle itself produces nothing for it without a
certificate) and which pdf_oxide reports as opened — its render then
errors and its text is empty, so the "loss" is a return code. Speed:
warm render median 10.4 ms against 4.1 ms for `pdfium-render`, 3.4 ms for
mupdf and 5.4 ms for hayro — pdfrum is the slowest renderer on this corpus
by a factor of two to three at the median, and the cold render (41.9 ms) is
six to nine times the C engines'.

The nine text losses to `pdfium-render` in the `FRC_8.2.4` family are one
cause: PDFium's text for those pages carries a U+0003 control character between
"Foxit" and "PhantomPDF" (`Foxit^CPhantomPDF`), and pdfrum's does not. The
oracle-bug rule (implement the correct behaviour, cite both) applies: the
character is not text, and the row is recorded here rather than fixed
toward it.

### Parallel-render throughput

`data/2026-09-05-114527a71d4c-throughput.json`, `compare throughput` on
`benches/corpus`: every page of every file at 150 DPI on N threads, one
untimed pass then one timed pass per (file, thread count), pages per second
summed over the files. This is the batch-conversion shape — fresh document
and fresh caches per file — not the warm per-page number above.

**Each engine is driven in its own multi-threading model**, because they do
not have the same one, and a column of numbers cannot say that by itself.
The harness names the model as a type (`engines::Sharing`) and prints it
under every table:

- **pdfrum, hayro, pdf_oxide — one shared document.** `Document` is `Sync`,
  so one opened document is read by every thread; `std::thread::scope` deals
  page indices round-robin, each thread with its own `VelloCpuBackend` and
  `RenderSession`. hayro shares its `Pdf` with a `RenderCache` per thread;
  pdf_oxide's `PdfDocument` is `Sync` and is shared as is. **Parse and
  rasterization are both parallel.**
- **mupdf — shared display lists.** This is MuPDF's own documented model
  (`docs/examples/multi-threaded.c`), and the harness now follows it:
  `mupdf-sys` builds its base `fz_context` with `FZ_LOCK_MAX` pthread
  mutexes as MuPDF's lock callbacks, `Context::get()` hands each thread its
  own `fz_clone_context` of it, and `DisplayList` is `Send + Sync`. Only
  `Document` and `Page` are not `Send` — so the calling thread loads every
  page and records it into a display list, then N workers rasterize those
  lists in parallel. **The parse is serial; the rasterization is parallel.**
- **pdfium-render — one thread.** PDFium keeps one global state and requires
  every call on one thread. Its columns are the one-thread figure repeated,
  and they are the only cells still marked `(1 thread)`.

| engine | files | pages | 1 thread pages/s | 4 threads pages/s | 8 threads pages/s |
|---|---|---|---|---|---|
| pdfrum | 44 | 188 | 18.2 | 20.5 | 21.8 |
| hayro | 44 | 188 | 23.7 | 31.3 | 32.3 |
| pdf_oxide | 39 | 183 | 20.0 | 24.1 | 23.8 |
| pdfium-render | 44 | 188 | 49.7 (1 thread) | 53.1 (1 thread) | 59.0 (1 thread) |
| mupdf | 44 | 186 | 81.3 | 94.1 | 98.1 |

Thread counts are clamped to a file's page count, and 21 of the 44 files
have one page, so the corpus-wide column barely moves. On the 18 files
with four or more pages (156 pages) the scaling is visible — that table is
the same command with `--min-pages 4`:

| engine | files | pages | 1 thread pages/s | 4 threads pages/s | 8 threads pages/s |
|---|---|---|---|---|---|
| pdfrum | 18 | 156 | 39.2 | 71.5 | 75.5 |
| hayro | 18 | 156 | 70.3 | 156.4 | 167.2 |
| pdf_oxide | 18 | 156 | 72.2 | 162.2 | 179.0 |
| pdfium-render | 18 | 156 | 153.4 (1 thread) | 151.4 (1 thread) | 152.0 (1 thread) |
| mupdf | 18 | 156 | 148.8 | 220.0 | 229.1 |

pdfrum scales 1.9× from one to four threads and flattens after, and its
one-thread figure is half hayro's and a quarter of the C engines'. The gap
to its own warm per-page median says where the time goes: every thread
builds a `RenderSession` whose substitution enumerates the font directory,
and that, plus the cold interpretation of each page, is paid once per file
per thread in this shape. The per-session font enumeration is a work item.

mupdf scales 1.5× on the multi-page set and 1.2× corpus-wide — real
scaling, but the least of any threaded engine here, and the reason is
Amdahl. Its display-list build is serial and is inside the timed pass, as
the parse is for every other engine in this table; timed on its own over
the 44 files it is **0.38 s against 1.81 s of rasterization, a 17.7 %
serial share**, which caps the achievable speed-up at about 5.6× however
many threads are given. The measured 1.5× is well under that cap, so the
serial build is the floor rather than the binding constraint at these
thread counts: MuPDF's per-page rasterization is fast enough that the
scope's thread startup and this machine's contention take the rest.
pdfrum, which parallelizes the parse as well, has no such floor — and it
is the reason its 1.9× beats mupdf's 1.5× while its absolute numbers do
not come close.

Only the mupdf rows of the throughput JSON were re-measured; every other
engine's rows are run 1's, byte for byte, and both tables above reproduce
run 1's numbers for them exactly. Machine: frieren (32 CPUs). Load before:
`22:18:08 up 10 days, 23:32,  1 user,  load average: 20.01, 17.36, 15.54`;
after: `22:18:34 up 10 days, 23:33,  1 user,  load average: 20.03, 17.49,
15.63`. **That is a lighter machine than run 1 saw** (load 30–40 on those
same 32 cores), so mupdf's figures here are measured under less contention
than the rows they sit beside; the model correction is the point of the
re-measurement, and the cross-engine comparison at these thread counts
should be read with that caveat until a whole run is redone at once.

### Adoption

`data/2026-09-05-114527a71d4c-adoption.json`, `compare adoption`, one
consumer crate per engine, this machine, clean release builds on 32 cores
with a warm registry cache.

| engine | version | licence | crates in tree | C in build | clean release build | stripped hello-world | `unsafe` in crate | `unsafe` in tree |
|---|---|---|---|---|---|---|---|---|
| pdfrum | 0.1.0 | MIT OR Apache-2.0 | 88 | none | 16 s | 1.1 MiB | 4 | 6932 |
| hayro | 0.7.1 | Apache-2.0 OR MIT | 72 | none | 19 s | 1.5 MiB | 1 | 7691 |
| pdf-extract | 0.12.0 | MIT | 60 | none | 23 s | 2.1 MiB | 0 | 2523 |
| lopdf | 0.44.0 | MIT | 68 | none | 19 s | 1.1 MiB | 0 | 3609 |
| pdf | 0.10.0 | MIT | 66 | none | 9 s | 1.8 MiB | 7 | 2288 |
| pdf_oxide | 0.3.77 | MIT OR Apache-2.0 | 138 | none | 54 s | 3.5 MiB | 73 | 7672 |
| pdfium-render | 0.9.3 | MIT OR Apache-2.0 | 25 | none | 14 s | 2.7 MiB | 16704 | 18475 |
| mupdf | 0.8.0 | AGPL-3.0 | 12 | bindgen, cc, clang-sys, mupdf-sys, pkg-config | 20 s | 4.8 MiB | 787 | 1884 |

"C in build" is the crates named `cc`, `cmake`, `bindgen`, `pkg-config`
or `*-sys` in `cargo tree -e normal,build`. It catches `mupdf-sys`
(vendored MuPDF compiled by `cc` under `make`, bindings by `bindgen`), and
it says "none" for `pdfium-render`, which is true of the build and false of
the program: the crate compiles nothing but binds a C++ `libpdfium.so`
through `libloading` at runtime, and its 16 704 `unsafe` are the generated
FFI surface. pdfrum's 4 are the word in doc comments — the crate
`forbid(unsafe_code)`s — and its 6 932 in the tree are its dependencies'
(vello_cpu, skrifa, the RustCrypto crates, fontdb). `pdf_oxide` with its
`rendering` feature is the largest tree here at 138 crates and the slowest
clean build.

### Coverage — one PDFium corpus file per feature

`data/2026-09-05-114527a71d4c-coverage.json`, `compare run --spec
benches/compare/coverage.json` over `<checkout>/testing`. The spec names
the file, and for the five encryption revisions the user password PDFium's
own `cpdf_security_handler_embeddertest.cpp` uses (R2 and R6 take "âge",
passed as UTF-8). A cell is what the engine did with that file — every
op, `ok` with its score against the oracle (SSIM for render, token F1 for
text), or `err` / `panic` / `crash` / `timeout` / `-` for not supported —
and nothing else. The password column of the spec is why `hayro`,
`lopdf`, `pdf` and `pdf-extract` have rows on the encrypted files at all:
each was given the password through its own API.

Run `coverage` — commit 9139cd2f48f0 — 22 files — 150 DPI — timeout 10 s — 3 warm runs — generated 2026-09-05T10:21:23Z
Machine: frieren (32 CPUs, rustc 1.97.1 (8bab26f4f 2026-07-14)). Load before: `18:21:23 up 10 days, 19:35,  4 users,  load average: 37.83, 32.97, 35.52`; after: `18:21:47 up 10 days, 19:36,  4 users,  load average: 29.80, 31.42, 34.94`.

### Coverage — one corpus file per feature; a cell is what the engine did with that file

| feature | file | pdfrum | hayro | hayro-interpret | pdf-extract | lopdf | pdf | pdf_oxide | pdfium-render | mupdf |
|---|---|---|---|---|---|---|---|---|---|---|
| encryption R2 (RC4 40-bit) | resources/encrypted_hello_world_r2.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=err render=err text=- | open=- render=- text=err | open=ok render=- text=err | open=ok render=- text=ok(0.00) | open=err render=- text=- | open=err render=err text=err | open=ok render=ok(0.995) text=ok(1.00) | open=ok render=ok(0.986) text=ok(1.00) |
| encryption R3 (RC4 128-bit) | resources/bug_1124998.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.987) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=err | open=ok render=- text=ok(0.57) | open=err render=- text=- | open=ok render=ok(0.974) text=ok(1.00) | open=ok render=ok(0.995) text=ok(1.00) | open=ok render=ok(0.986) text=ok(1.00) |
| encryption R4 (AESV2) | resources/encrypted.pdf | open=ok render=ok(0.998) text=ok(1.00) | open=ok render=ok(0.999) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=err | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=ok(0.998) text=ok(1.00) | open=ok render=ok(0.998) text=ok(1.00) | open=ok render=ok(0.998) text=ok(1.00) |
| encryption R5 (AESV3, Adobe extension) | resources/bug_644.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) |
| encryption R6 (AES-256, ISO 32000-2) | resources/encrypted_hello_world_r6.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.987) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=err | open=ok render=- text=ok(0.57) | open=ok render=- text=- | open=ok render=ok(0.974) text=ok(1.00) | open=ok render=ok(0.995) text=ok(1.00) | open=ok render=ok(0.986) text=ok(1.00) |
| JBIG2 | corpus/pdfium/bug_880920.pdf | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(0.996) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(0.00) | open=err render=- text=- | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(0.996) text=ok(1.00) |
| JPX (JPEG 2000) | corpus/fx/action/123.pdf | open=ok render=ok(0.987) text=ok(1.00) | open=ok render=ok(0.989) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=err text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.991) text=ok(1.00) |
| CCITT fax | corpus/fx/other/3bigpreview.pdf | open=ok render=ok(0.880) text=ok(0.99) | open=ok render=ok(0.864) text=- | open=- render=- text=ok(0.74) | open=ok render=- text=panic | open=ok render=- text=err | open=ok render=- text=- | open=ok render=ok(0.837) text=ok(0.68) | open=ok render=ok(0.883) text=ok(1.00) | open=ok render=ok(0.867) text=ok(0.54) |
| shading type 1 (function) | corpus/fx/shading/2_shading_type1.pdf | open=ok render=ok(0.999) text=ok(1.00) | open=ok render=ok(0.999) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=err text=err | open=ok render=ok(0.999) text=ok(1.00) | open=ok render=ok(0.545) text=ok(1.00) |
| shading type 2 (axial) | resources/pixel/axial_shading_point_at_border_no_extend.pdf | open=ok render=ok(0.988) text=ok(1.00) | open=ok render=ok(1.000) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(0.988) text=ok(1.00) | open=ok render=ok(0.993) text=ok(1.00) |
| shading type 3 (radial) | corpus/fx/shading/2_shading_type3.pdf | open=ok render=ok(0.989) text=ok(1.00) | open=ok render=ok(0.982) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=err text=err | open=ok render=ok(0.989) text=ok(1.00) | open=ok render=ok(0.931) text=ok(1.00) |
| shading type 4 (free-form Gouraud) | corpus/fx/shading/2_shading_type4_h.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.372) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=ok(0.972) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.372) text=ok(1.00) |
| shading type 5 (lattice Gouraud) | corpus/fx/shading/2_shading_type5_h.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.998) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=ok(0.742) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.999) text=ok(1.00) |
| shading type 6 (Coons patch) | corpus/fx/shading/2_shading_type_6_00.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.957) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=ok(0.645) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.961) text=ok(1.00) |
| shading type 7 (tensor patch) | resources/pixel/shade-tensor.pdf | open=ok render=ok(0.995) text=ok(1.00) | open=ok render=ok(0.926) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=ok(0.939) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(0.927) text=ok(1.00) |
| Type3 font | resources/pixel/type3.pdf | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(0.997) text=- | open=- render=- text=ok(0.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(0.00) | open=ok render=- text=- | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(1.000) text=ok(0.40) | open=ok render=ok(0.997) text=ok(1.00) |
| CID font (Type0) | corpus/fx/text/ch_1.pdf | open=ok render=ok(0.999) text=ok(1.00) | open=ok render=ok(0.766) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=panic | open=ok render=- text=err | open=ok render=- text=- | open=ok render=ok(0.703) text=ok(1.00) | open=ok render=ok(0.826) text=ok(1.00) | open=ok render=ok(0.750) text=ok(0.00) |
| annotations with appearance streams | resources/annotation_highlight_square_with_ap.pdf | open=ok render=ok(0.998) text=ok(1.00) | open=ok render=ok(0.999) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=ok(0.998) text=ok(1.00) | open=ok render=ok(0.999) text=ok(1.00) | open=ok render=ok(0.999) text=ok(1.00) |
| AcroForm fields (widgets) | resources/text_form.pdf | open=ok render=ok(0.996) text=ok(1.00) | open=ok render=ok(0.995) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=ok(0.993) text=ok(1.00) | open=ok render=ok(0.995) text=ok(1.00) | open=ok render=ok(0.994) text=ok(1.00) |
| JavaScript (document-level /JavaScript) | resources/js.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) |
| incremental update (/Prev chain) | resources/embedded_images.pdf | open=ok render=ok(0.995) text=ok(1.00) | open=ok render=ok(0.992) text=- | open=- render=- text=ok(1.00) | open=err render=- text=err | open=err render=- text=err | open=err render=- text=- | open=ok render=ok(0.990) text=ok(1.00) | open=ok render=ok(0.993) text=ok(1.00) | open=ok render=ok(0.993) text=ok(1.00) |
| tagged PDF (/StructTreeRoot) | resources/tagged_expansion.pdf | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=- | open=- render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=ok(1.00) | open=ok render=- text=- | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) | open=ok render=ok(1.000) text=ok(1.00) |

Cell grammar: `open`/`render`/`text` each as `ok` (with SSIM or token F1 against the oracle), `err`, `panic`, `crash`, `timeout`, `-` (not supported).

pdfrum: every file opens, renders and extracts; 18 of the 22 renders are
inside the conformance floor, and the four outside are the CCITT scan
(0.880, where every engine is between 0.84 and 0.88), JPX (0.987) and the
two axial/radial fixtures (0.988–0.989). The peers' cells are the
informative ones: hayro cannot open the R2 file and draws the type-4
Gouraud mesh at 0.372 (mupdf too); pdf_oxide errors on R2, on the JPX
render, and on the function and radial shadings, and draws types 5 and 6
at 0.742 / 0.645; pdf-extract panics on the CCITT and CID files and cannot
open R5, the five `fx/shading` files, `js.pdf` or the incremental update;
pdf-rs opens 12 of 22; lopdf's text is empty on R2, JBIG2 and Type3.
