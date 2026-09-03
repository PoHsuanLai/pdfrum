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
- **No caller-supplied `/ToUnicode` CMap or `/CIDToGIDMap` on font load.**
  `DocEdit::embed_font(bytes, FontEncoding::Composite)` always generates both
  from the program's own cmap, so the oracle's `FPDFText_LoadCidType2Font`
  (`fpdfsdk/fpdf_edittext.cpp`) — which takes `to_unicode_cmap` and
  `cid_to_gid_map` spans alongside the program — has no counterpart.
  `crates/pdfrum/tests/load_font.rs::load_cid_type2_font_custom` is the
  `#[ignore]`d port waiting on it.
- **`/FontFile` stores the PFB wrapper but `/Length1-3` describe the unwrapped
  program.** `embed_program` (`crates/pdfrum-edit/src/font/embed.rs:451-485`)
  writes the caller's Type 1 bytes verbatim while
  `pdfrum_font::type1_program_lengths` returns segment-payload lengths, so the
  three disagree with the stream by the 20 bytes of PFB framing (113417 vs
  113397 on `FoxitSerifMM.pfb`) and do not partition it as ISO 32000-1 §9.9
  Table 127 requires. Pinned by the `#[ignore]`d
  `load_font.rs::type1_font_file_lengths_partition_the_stream`.
- ~~**Coloured tiling pattern paints grey where the oracle paints teal** on
  `corpus/fx/other/1.pdf`~~ — landed, and **it was never a pattern**. The
  file contains no `/Pattern` object at all: the teal is `Im1`, a 1x1
  eight-bit image in a `/Separation` (PANTONE 327 CV over `DeviceCMYK`)
  scaled across the region by a `cm`. Its lone `0xC6` sample is a 0.776
  *tint*, and `unpack` bucketed every one-component image straight into
  `Pixels::Gray8` without running the tint transform, so the tint byte
  reached the page as the grey level `(198, 198, 198)` rather than the teal
  `(0, 182, 162)` the transform produces. Every `Separation` and `DeviceN`
  image was wrong the same way. Fixed by giving those two families the
  conversion PDFium reaches through `LoadPalette` and
  `TranslateScanline24bpp`; SSIM 0.996609 -> 0.998815. See
  `docs/status/pdfrum-render.md` §"Tint-space images".

## Cleanliness before public CI (added 2026-09-03, user)

- **Rustdoc names internal phases and internal documents.** 238 `///`/`//!`
  lines across 18 crates cite `M12`/`M15`-style milestone numbers,
  `WP1`-style work packages, `§A.11`-style design-doc sections, or
  `docs/status/…`, `docs/design/…`, `PLAN.md`, `SPEC.md`, `STYLE.md`,
  `DEPS.md` by name (`pdfrum-font` 38, `pdfrum-render` 37, `pdfrum-page` 29,
  `pdfrum-edit` 19, `pdfrum-form` 18, …). None of that means anything on
  docs.rs. It is the WP4 provenance sweep with a wider pattern, applied
  workspace-wide — and **the lean is to delete, not relocate** (user): most
  of these sentences are diary entries (when we decided, which pass, which
  ruling superseded which) that git history and `docs/` already hold. Keep
  only an invariant a caller can get wrong (rustdoc, rewritten without the
  pointer) or a measured fact that changes how the code must be read (a
  short `//` stating the fact, not "see M12 §1.6"). If the next reader's
  need cannot be said in one sentence, it goes. Folded into the in-flight
  `pdfrum-object`/`pdfrum-render` and `pdfrum-form` rustdoc agents; the
  other crates remain. Then a CI grep in `scripts/ci.nu`
  over `///`/`//!` lines for `\bM[0-9]{1,2}[a-z]?\b|\bWP[0-9]+\b|§[A-Z]\.[0-9]|docs/(status|design|upstream)|PLAN\.md|SPEC\.md|STYLE\.md|DEPS\.md`
  so it does not regrow (non-vacuity control as the other checks have).
  Sequence after the in-flight rustdoc WP5 crates land, so it does not
  collide with them.
- **Hard-coded absolute paths.** 73 lines name `/mnt/data2/…` or
  `/home/r13921098/…`: `scripts/clean-targets.nu` (its two roots),
  `scripts/bench-oracle.nu`, `bench-rss.nu`, the three `extract-*.py` and
  `probe-tounicode.py`, `fuzz/seed-corpus.sh`, `conformance/README.md`
  (the `--goldens`/`--checkout` examples), `crates/pdfrum/tests/load_font*.rs`
  and `crates/pdfrum-page/tests/corpus.rs` (oracle binary / checkout
  paths), two PROVENANCE files, PLAN.md, and the `docs/reviews/` logs
  (leave the logs — they are records). Before GitHub CI: every script and
  test resolves the oracle checkout, the oracle binary, the goldens and the
  target root from **one place** — environment variables with documented
  defaults relative to the repo (`PDFRUM_ORACLE_CHECKOUT`,
  `PDFRUM_ORACLE_BIN`, `PDFRUM_GOLDENS`, `PDFRUM_TARGET_ROOT`, or a single
  `scripts/env.nu` the others source) — and a test that needs the oracle
  skips with a message when the variable is unset rather than failing on a
  path. The `conformance` binary's own defaults (`--tool`, `--checkout`)
  follow the same rule. A CI grep for the two path prefixes outside
  `docs/reviews/` and `docs/status/` keeps it clean.

## Rustdoc trim (`docs/design/rustdoc.md`)

- WP5 inner crates, host-reachable first: `pdfrum-text`, `pdfrum-form`,
  `pdfrum-render`, `pdfrum-object`, then any type page over 30 lines.
- WP6 the STYLE.md §6 paragraph, last.

## Performance (`docs/status/M13-perf-baseline.md`)

- ~~`forms_combo_box` third cost~~ — landed 30c0419 (2026-09-03): an AGG
  clip push allocated, walked and cloned a page-sized coverage plane per
  appearance form; forms geomean 2.29x → 1.17x, `combo_box` 3.76x → 1.54x,
  controls flat, board byte-identical (§11).
- `forms_text_field` is now the class's worst row at 3.10x with one of the
  lowest speedups (1.69x) — by §10.6's argument a *fourth* cost; its
  `--op forms` split is next (§11.7).
- tinyskia has a smaller instance of the same clip-plane clone (one clone,
  no `sync_clip`), noted in §11.4, unmeasured.
- A deep-clip-stack document outside `forms` would gain from 30c0419; none
  was looked for.
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
