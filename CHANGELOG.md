# Changelog

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project intends to follow [Semantic Versioning](https://semver.org/) from
its first release.

## [Unreleased]

Nothing has been released yet. `pdfrum` is at `0.1.0` in the manifests and has
never been published to crates.io, so there is no release history to record —
this section describes what exists on `main`, at a coarse level, so a reader
knows what they are looking at.

### Added

- **The engine.** Parsing and cross-reference recovery, the object model,
  filters, encryption, fonts and CMaps, the content-stream interpreter, page
  and graphics-state semantics, colorspaces, functions, patterns and shadings,
  rasterization, text extraction, document navigation and annotations, form
  interaction, JavaScript, and PDF editing and serialization — across
  twenty-odd crates under `crates/`.
- **Four render backends**, selectable by cargo feature: `vello_cpu` (the
  default), `tiny-skia`, an AGG-derived rasterizer, and a `wgpu`-backed GPU
  backend that no default build reaches.
- **A command-line tool.** `pdfrum` renders, extracts text and Markdown,
  inspects, edits and diagnoses; it is composable, machine-readable on demand,
  and documented by `docs/design/cli-style.md`.
- **A C ABI.** `crates/pdfrum-capi` builds `libpdfrum.so` and `libpdfrum.a` and
  generates `include/pdfrum.h`, proved by a C test program rather than by a
  Rust test asserting things about one.
- **A WebAssembly binding.** `crates/pdfrum-wasm` builds a module and its
  JavaScript loader, tested under Node on `wasm32-unknown-unknown`.
- **A conformance harness.** Every change is measured against a read-only
  PDFium checkout across four tiers — byte-exact text, structural dumps,
  rendered pixels by SSIM, and JavaScript transcripts —
  with the result recorded in `conformance/scoreboard.json`.
- **Published benchmarks.** `docs/benchmarks/` measures pdfrum against
  pdfium-render, mupdf, hayro, pdf-rs and pdf-extract, with every loss
  explained in `docs/benchmarks/losses-explained.md`.
- **A fuzzing ring** under `fuzz/`, kept compiling by the gate even though
  running it is not part of it.

### Known limitations

The open work list is `docs/issues-to-file.md`. The ones a user is most likely
to hit:

- Peak render memory reaches 1539 MiB on one pathological image document where
  every other engine stays under 1 GiB.
- Warm median render is slower than pdfium-render and mupdf; correctness leads
  both.
- Public-key (`Adobe.PubSec`) encryption is not implemented, so one file in a
  209-file sample does not open.
- Text extraction generates spurious spaces on some documents; the cause is
  measured but not yet diagnosed.
