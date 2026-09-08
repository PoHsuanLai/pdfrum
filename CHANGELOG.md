# Changelog

[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Semver from the
first crates.io release.

## [Unreleased]

`pdfrum` is `0.1.0` in the manifests and has not been published.

### Added

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
  its bounding-box gate discards the object; we keep the space. Written up in
  [`docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md`](docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md),
  which records it as drafted and not yet filed.
- Signatures are parsed and reported as written. Nothing verifies the
  cryptography, the certificate chain, or the byte ranges they cover.
- OTTO subsetting is skipped: an OpenType/CFF program is embedded whole as
  `/FontFile3` and passed over by the subsetter.
- PDF/A is the two *basic* levels, A-1b and A-2b; the `a` levels need a
  logical structure model this engine does not have. The checker reads the
  object graph and not the content streams, so a colour set by `1 0 0 rg`
  rather than through a named `/ColorSpace`, and a `gs`-less inline
  transparency, are invisible to it. An empty report means the checks it runs
  passed, which is weaker than ISO 19005 conformance.
- `full` is a host feature set: it includes `system-fonts`, which is never
  compiled for `wasm32`.
- JavaScript is boa, not a viewer's engine. Some documents' scripts diverge
  from what Acrobat or PDFium produce.
- An image whose declared `/Width` and `/Height` are both above 65536 is
  built at full size rather than reduced, which for the dimensions
  `/Width` and `/Height` still admit is an allocation no machine will
  satisfy. Bound untrusted input with `Limits` and a `Deadline`; a
  pixel-count limit is not among the knobs yet.
- Weakest rendering: vertical text, uncoloured tiling patterns, and image
  transformers.
