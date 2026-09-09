# Changelog

[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Semver from the
first crates.io release.

## [Unreleased]

## [0.1.1] - 2026-09-10

### Added

- `pdfrum-edit` gains `svg-import` and `svg-text` features, an
  `Error::Svg` variant, and the SVG ingestion behind them.
- Facade re-exports for the write-side surface its owning crates now hold:
  `PdfaPolicy`, `flatten`, and the attachment operations `add_attachment`,
  `delete_attachment`, `remove_attachment`, `set_attachment_file`,
  `set_attachment_description` and `set_attachment_param`.
- `tt_composite_instructions.ttf`, a fixture carrying a simple glyph, an
  uninstructed composite and an instructed one.
- CLI README documents terminal support: where `preview` and `view` draw,
  what each `--graphics` mode maps to, the tmux caveat and the pager keys.
- `scripts/record-cli-macos.nu` records the CLI demo natively on macOS by
  capturing a real Kitty window, which the Xvfb recorder cannot do.

### Changed

- Write-side operations move out of the `pdfrum` facade into the crates
  that own them: `PageEdit` and `transform_object` to `pdfrum-page`;
  `content_segments` and `revision_end` to `pdfrum-parser`; `Canvas`,
  `stamp`, `attach`, `flatten`, SVG import, object builders, `build_graph`,
  `apply_page`, `page_state` and PDF/A `convert` to `pdfrum-edit`; PDF/A
  `policy`, `xmp_write` and `signature` to `pdfrum-doc`. Modules take
  explicit `limits` and `diags` parameters rather than a `&Document`
  bundle, and `string_width` becomes a free function over `Resolve +
  Limits`. The facade drops from 16,296 to 8,533 lines. **Every public
  path is unchanged**; the API baseline diff is re-export re-spelling.
- The CLI demo is retimed and re-recorded. Hardcoded `sleep`s of up to
  3.5s stood in for work that had already finished, so paging now reads
  as instant. MP4 1.0 MB -> 512 KB, GIF 2.5 MB -> 907 KB.

### Fixed

- A TrueType composite that carries its own instruction stream is run
  through the interpreter. The component offsets are only half the
  placement; the bytecode moves the components into their final
  positions, and stroke-assembled CJK faces scattered without it. Only
  instructed composites take the grid-fitted outline, and outlines are
  memoized per glyph.
- `stamp` called `note()` before `build_graph`, so graph-building
  diagnostics never reached the session.
- `scripts/publish-order.nu` parsed a line-broken `or` as an external
  command, so the script printed nothing, the release job published zero
  crates, and still exited 0. The predicate stays on one line and an
  empty order now fails.
- Replace `doc_auto_cfg`, removed in 1.92, so docs.rs builds on nightly.
  The 0.1.0 docs build failed with E0557; local `cargo doc` never sets
  `docsrs`, so the gate did not see it.

## [0.1.0] - 2026-09-09

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

[unreleased]: https://github.com/PoHsuanLai/pdfrum/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/PoHsuanLai/pdfrum/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/PoHsuanLai/pdfrum/releases/tag/v0.1.0
