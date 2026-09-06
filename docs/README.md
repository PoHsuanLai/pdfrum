# Documentation

What is here, and what each document is for.

## Benchmarks

| File | What it is |
|---|---|
| [`benchmarks/README.md`](benchmarks/README.md) | Comparative measurements against pdfium-render, mupdf, hayro, pdf-rs and pdf-extract: open, render, text extraction, correctness, memory. The method is stated in full so a reader can re-run it. |
| [`benchmarks/losses-explained.md`](benchmarks/losses-explained.md) | Every row where pdfrum loses, with the cause measured and a verdict: fixed, tradeoff, oracle bug, or open. Losses are not omitted from the tables above. |
| [`benchmarks/data/`](benchmarks/data/) | The raw JSON each published run was computed from. |
| [`roadmap.md`](roadmap.md) | What is planned next -- the canvas, SVG export and ingestion, PDF/A -- each with its exit criteria, and the line against typesetting. Nothing in it has started. |

## Design

Architecture documents. Each one states a rule or a decision that a future
change could break without a gate noticing.

| File | What it governs |
|---|---|
| [`design/capi.md`](design/capi.md) | The C ABI: the five rules `libpdfrum`'s boundary is built on. |
| [`design/wasm.md`](design/wasm.md) | The WebAssembly binding, and where JavaScript changes the C ABI's answers. |
| [`design/cli-style.md`](design/cli-style.md) | The output style every command follows: streams, modes, the four layout forms, the palette. |
| [`design/pdfrum-cli.md`](design/pdfrum-cli.md) | The CLI's scope and command surface, and why it is separate from the conformance harness. |
| [`design/cargo-features-and-backends.md`](design/cargo-features-and-backends.md) | Feature flags, backend selection, and the dependency boundaries between crates. |
| [`design/rustdoc.md`](design/rustdoc.md) | What `///` and `//!` are for in this codebase, and the caps each crate page is held to. |
| [`design/image-rows.md`](design/image-rows.md) | The type-driven image pipeline: decode, reduce, unpack, paint as separate stages. |
| [`design/dod-layout.md`](design/dod-layout.md) | The data-layout pass: what the profile said to touch, and what it bought. |
| [`design/backend-verification.md`](design/backend-verification.md) | `vello_cpu` against `tiny-skia`, verified against the crate sources rather than inferred. |
| [`design/mupdf-comparison.md`](design/mupdf-comparison.md) | Where the render-speed gap against mupdf goes, read from mupdf's source. A study; nothing in it is implemented. |
| [`design/pdfrum-script.md`](design/pdfrum-script.md) | The JavaScript engine's design and the seam it reaches the form model through. |

## Upstream

[`upstream/`](upstream/) holds bug reports drafted against other projects —
PDFium, vello, hayro, zune — and the PDFs that reproduce them. They are drafts,
not filed issues; [`upstream/README.md`](upstream/README.md) says which are
filed, which are not, and how confident each one is.

## Work

[`issues-to-file.md`](issues-to-file.md) is every open work item written as an
issue: context with a citation, the problem with its measurement, a proposed
fix or an honest "not determined", and what would count as done.

## The oracle

Several documents refer to "the oracle" or "the read-only checkout". That is a
PDFium source tree and a built `pdfium_test`, kept outside this repository and
never modified. It is the behaviour reference the conformance harness measures
against. Nothing in the build needs it; the conformance board does.
