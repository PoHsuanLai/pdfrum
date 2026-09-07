# Documentation

Crate contracts (C ABI, WASM, CLI, canvas, SVG, PDF/A) live on that crate's
README and rustdoc.

| | |
|---|---|
| [`benchmarks/`](benchmarks/README.md) | Engines, operations, tables, current numbers |
| [`benchmarks/data/`](benchmarks/data/) | Raw JSON for those runs |
| [`assets/cli/`](assets/cli/README.md) | CLI showcase GIF and the tape that records it |
| [`upstream/`](upstream/README.md) | Bug reports against PDFium, vello, hayro, zune, plus repros |
| [`roadmap.md`](roadmap.md) | What's next |
| [`api-baseline/`](api-baseline/) | Public-API snapshot the CI gate diffs |

The **oracle** is a read-only PDFium checkout and `pdfium_test`, kept outside
this repo. The build does not need it; the conformance board does.
