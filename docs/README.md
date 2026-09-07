# Documentation

Crate contracts — the C ABI, WASM, the CLI, canvas, SVG export, PDF/A — live
on that crate's README and in its rustdoc, not here. What is here is the
material that belongs to no single crate.

| | |
|---|---|
| [`benchmarks/`](benchmarks/README.md) | Engines, operations, tables, current numbers |
| [`benchmarks/data/`](benchmarks/data/) | Raw JSON for those runs |
| [`assets/cli/`](assets/cli/README.md) | CLI showcase GIF and the tape that records it |
| [`upstream/`](upstream/README.md) | Bug reports against PDFium, vello, hayro and zune, with repros |
| [`roadmap.md`](roadmap.md) | What's next, and what is deliberately out of scope |
| [`api-baseline/`](api-baseline/) | Public-API snapshot the CI gate diffs |

These are repository paths, not published documents. The rendered API
documentation is [docs.rs/pdfrum](https://docs.rs/pdfrum); the crate map and
the conformance numbers are in the [root README](../README.md).

The **oracle** is a read-only PDFium checkout and its `pdfium_test` binary,
kept outside this repo. Nothing in the build needs it. The conformance board
does, which is why a first patch can skip the board and a maintainer runs it
before landing — see [CONTRIBUTING.md](../CONTRIBUTING.md).
