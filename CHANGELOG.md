# Changelog

[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Semver from the
first crates.io release.

## [Unreleased]

`pdfrum` is `0.1.0` in the manifests and has not been published.

### Changed

- Cargo feature names: `tinyskia` → `tiny-skia`, `svg` → `svg-export`,
  `svg-ingest` → `svg-import`, `jpx` → `jpeg2000`. No aliases.

### Added

- `full` cargo feature: every published capability except `vello-gpu` and
  the internal `profiling` timers.

- Parse, recover, render, extract text, forms, JavaScript, edit and save.
- Four raster backends: `vello_cpu` (default), `tiny-skia`, AGG, GPU `wgpu`
  (never in a default build).
- CLI: [`pdfrum-cli`](crates/pdfrum-cli/README.md).
- C ABI: `libpdfrum` + `pdfrum.h`.
- WebAssembly binding.
- Conformance harness vs PDFium (`conformance/scoreboard.json`).
- Benchmarks vs pdfium-render, mupdf, hayro, pdf-rs, pdf-extract
  (`docs/benchmarks/`).
- Fuzz targets under `fuzz/` (compiled by the gate, not run by it).

### Known limitations

- Warm median render is slower than pdfium-render and mupdf.
- No public-key (`Adobe.PubSec`) encryption.
- Text extraction keeps one space PDFium drops. A page whose only text
  object draws nothing but spaces comes back empty from the oracle, because
  its bounding-box gate discards the object; we keep the space. Reported as
  `crbug.com/40643656` and written up in
  [`docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md`](docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md).
- Signatures are parsed and reported as written. Nothing verifies the
  cryptography, the certificate chain, or the byte ranges they cover.
- OTTO subsetting is skipped: an OpenType/CFF program is embedded whole as
  `/FontFile3` and passed over by the subsetter.
- Weakest rendering: vertical text, uncoloured tiling patterns, and image
  transformers.
