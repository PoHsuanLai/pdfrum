# Open queue

What is known to be owed and not yet started, so the next session or agent
does not rediscover it. Dated when added; struck when landed. Milestone and
board context live in PLAN.md and `conformance/scoreboard.json`.

## Feature gaps (added 2026-09-03)

- **Image embedding.** `ImageBuilder::at(source: ObjRef, rect)` can only
  place an `/XObject` the document already has — the same shape the font
  gap had before `DocEdit::embed_font` (01df42a). The oracle's
  `FPDFImageObj_LoadJpegFile` / `FPDFImageObj_SetBitmap` /
  `FPDFPageObj_NewImageObj` have no equivalent. Belongs on `DocEdit`
  beside `embed_font`; the subsetting/collector logic is untouched by it.
- **JavaScript M15 step 2 — the `Doc`/`Field` object model.** 11 of 47
  transcript fixtures byte-exact; 22 fixtures reach `this.*`
  (`docs/status/M15.md` §"1. `Doc` as the global"). Two `pdfrum-form`
  defects measured by WP12 go with it (`docs/status/pdfrum-facade.md`):
  a format script's output is computed and dropped (`CommitOutcome::display`
  has no reader; `UpdateKind` cannot carry it), and `FieldRef::index`
  conflates a page-local id with a `/Fields` position.
- **Coloured tiling pattern paints grey where the oracle paints teal** on
  `corpus/fx/other/1.pdf` — the pattern sentinel carried forward from the
  render audit; not yet isolated.

## Rustdoc trim (`docs/design/rustdoc.md`)

- WP5 inner crates, host-reachable first: `pdfrum-text`, `pdfrum-form`,
  `pdfrum-render`, `pdfrum-object`, then any type page over 30 lines.
- WP6 the STYLE.md §6 paragraph, last.

## Performance (`docs/status/M13-perf-baseline.md`)

- An idle re-take of the §10.5 table. The box has never been idle (load
  30–66 through every run); the ratios are upper bounds until then.
- `vector_font_size14` 3.70x and `vector_font_feature` 3.42x — glyph-heavy
  pages, a different shape from the forms residue.
- `shading_type4_5` 4.38x, `image_en_fqa` 2.03x, `image_ccitt_3bigpreview`
  1.66x, `shading_tcpdf_058` 1.88x.

## Unwired oracle ports (`docs/status/unwired-oracle-ports.md`)

- 16-bpc high-byte arm: ours rounds, the oracle truncates — needs a ruling
  (correct over oracle says rounding; then the port arm is deleted).
- `palette_index` for packed multi-component images: an optimisation if
  pixel-identical — verify and delete, or wire.
- DCT reduced-resolution trio: blocked on `zune-jpeg` (`docs/upstream/zune/`).

## Policy-kept, decision owed

- `Limits::max_string_len` is consulted nowhere.
- Fifteen `DiagKind` variants are never recorded (`#[non_exhaustive]`, doc
  says the enum grows as crates land).
- `docs/design/idiomatic-api.md`'s leak-count derivation cannot see payload
  types of re-exported types (WP7's finding); the method is wrong, the
  gate that replaced it (`crates/pdfrum/tests/reexports.rs`) is right.

## Upstream, drafted and not filed (`docs/upstream/README.md`)

Four PDFium rendering issues, the `EnableStdConversion` dead-mechanism
note, one `hayro-jbig2`, one `zune-jpeg`. The user files these.
