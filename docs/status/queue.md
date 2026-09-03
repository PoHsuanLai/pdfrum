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
- ~~**No caller-supplied `/ToUnicode` CMap or `/CIDToGIDMap` on font load.**~~
  — landed 2026-09-03 as `DocEdit::embed_cid_font(program, to_unicode,
  cid_to_gid)`, the `FPDFText_LoadCidType2Font` counterpart. `/W` is computed
  per CID from the caller's map, both blobs are written verbatim, and
  `EmbeddedFont::encode` inverts the caller's CMap (the oracle's
  `CharCodeFromUnicode` is the same reverse lookup). `load_cid_type2_font_custom`
  and `load_cid_type2_font_custom_generated_widths` are un-ignored.
- ~~**`/FontFile` stores the PFB wrapper but `/Length1-3` describe the unwrapped
  program.**~~ — fixed 2026-09-03. `embed_program` now unwraps the container:
  `pdfrum_type1::font_file` returns the raw program *and* the three lengths
  together (`FontFile`), so they cannot disagree. 113397 B stored,
  10710 + 102155 + 532, the PFB's 20 bytes of framing dropped.
  `load_font.rs::type1_font_file_lengths_partition_the_stream` is un-ignored
  and asserts the partition on the written stream.
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
- ~~`forms_text_field` fourth cost~~ — landed cd1a511 (2026-09-03): the clip
  plane's *allocation*. §11 made the intersection cheap and left
  `AlphaMask::new` — half a megabyte of `memset` per push, 133–276 pushes
  per render, 9–16% of a whole forms render — so a pop now returns the plane
  to a pool by clearing the band §11 already records. Board byte-identical,
  tier-c unchanged (§12).
- ~~tinyskia's clip-plane clone~~ — landed cd1a511 with it: §11.4 described the
  push's clone (inherent, `intersect_path` mutates in place) and missed the
  larger one — `fill_path`/`stroke_path`/`draw_image` each cloned the whole
  mask **per draw call** to end a borrow. A hand-split borrow removes it —
  worth 6.0% of a `forms_text_field` render and 7.0% of a `forms_combo_box`
  one, and nothing on `forms_number`, whose draws mostly run with no clip in
  force (§12.5, §12.9). The push's own clone stays.
- **A wall-clock A/B of §12 is still owed.** Its claim rests on the in-process
  profile (§12.7), because the box ran at load 43–131 — the other user's jobs
  *plus* three sibling agents' boards and test suites — and at four times the
  core count the interleaved before/after stopped resolving the effect
  (§12.6). Re-take §11.6's table for §12 when the box is quiet.
- A deep-clip-stack document outside `forms` would gain from 30c0419; none
  was looked for.
- An idle re-take of the §10.5 table. The box has never been idle (load
  30–66 through every run); the ratios are upper bounds until then.
- `vector_font_size14` 3.70x and `vector_font_feature` 3.42x — glyph-heavy
  pages, a different shape from the forms residue.
- `shading_type4_5` 4.38x, `image_en_fqa` 2.03x, `image_ccitt_3bigpreview`
  1.66x, `shading_tcpdf_058` 1.88x.

## Unwired oracle ports (`docs/status/unwired-oracle-ports.md`)

- ~~16-bpc high-byte arm~~ — **decided 2026-09-03**: both readers scale-and-
  round (pdf.js exactly, PDFium's `>> 8` within one count), our shipping path
  truncated and was a count low against both. Path fixed, arm deleted.
- ~~`palette_index` for packed multi-component images~~ — **decided
  2026-09-03**: pixel-identical (every packed index enumerated for 2-bpc RGB
  and 1-bpc CMYK), and the corpus has no multi-component image with
  `bpc * components <= 8` at all. Deleted.
- DCT reduced-resolution trio: still blocked on `zune-jpeg` — re-checked
  2026-09-03 against 0.5.16-rc1, `DecoderOptions` has no `scale_denom`.

## Policy-kept, decision owed

*(Emptied 2026-09-03. The `DiagKind` item is decided: six variants wired, nine
deleted — see the commit and `crates/pdfrum-common/src/diagnostics.rs`.)*

## ~~Conformance gaps found while auditing `DiagKind`~~, 2026-09-03

~~Three real divergences surfaced by asking "does our port reach this
condition?".~~ — **all three ruled and landed 2026-09-03.** The board did not
move (1757 / 1514 / 243, per-file byte-identical, `--check-regressions`
clean): each condition is reachable only from an input the corpus does not
contain, so no golden pinned either answer and nothing went to the
not-achievable bucket. The two divergences are written up as A72 and A73 in
`docs/status/oracle-divergence-audit.md` §11.

- ~~**A negative quarter turn empties a widget's box.**~~ — landed `4e48501`.
  Ruled **against the oracle**. ISO 32000-1 table 189 makes `/R`
  counterclockwise, so `-90` is `270`, and the correct fold is
  `rem_euclid(360)`; PDFium's `abs(GetRotation() % 360)`
  (`cpdfsdk_widget.cpp:1029`, `:1049`) answers `90` — the same axis swap for
  the box, the **wrong matrix** (`:1053-1062` builds different ones for 90 and
  270). pdf.js normalizes exactly as we now do
  (`annotation.js`, `setRotation`). `[oracle-bug]` marked. The two readers now
  share one type, `pdfrum_doc::geom::WidgetRotation`, so they cannot drift
  again; `pdfrum_form::Rotation` re-exports it. A non-multiple of 90 is
  **upright**, which all three sources agree on, and no `/R` empties the box
  any more. The test that pinned the bug now pins the ruling.
- ~~**A field whose fully-qualified name is empty is kept, not skipped.**~~ —
  landed `fd185fc`. Ruled **with the oracle**, on a stronger ground than the
  oracle's: ISO 32000-1 §12.7.3.2 makes the fully qualified name a field's only
  address, *and* `name` is this crate's field **identity** — the merge in
  `visit` keys on it, `Form::field` looks up by it, `pdfrum-form` allocates a
  `FieldId` per distinct name — so keeping `""` merged every unnamed field in a
  document into one phantom field. `FieldSkippedNoName` came back with the
  branch it names.
- ~~**A malformed `/Kids[0]` does not abandon the subtree.**~~ — landed
  `586edfe`. Ruled **ours correct**. The oracle's `return` on a null
  `kids->GetDictAt(0)` (`cpdf_interactiveform.cpp:871-874`) loses every sibling
  to one broken reference; the call is a *probe* for the terminal-vs-branch
  decision (`:876-880`), so the loss is a side effect rather than a decision.
  pdf.js skips the entry and keeps walking (`document.js`,
  `#collectFieldObjects`). `[oracle-bug]` marked, with a fixture whose first
  kid is a junk reference and whose second is a real field.

## Upstream, drafted and not filed (`docs/upstream/README.md`)

Four PDFium rendering issues, the `EnableStdConversion` dead-mechanism
note, one `hayro-jbig2`, one `zune-jpeg`. The user files these.
