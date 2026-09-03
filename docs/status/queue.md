# Open queue

What is known to be owed and not yet started, so the next session or agent
does not rediscover it. Dated when added; struck when landed. Milestone and
board context live in PLAN.md and `conformance/scoreboard.json`.

## Feature gaps (added 2026-09-03)

- ~~**Image embedding.** `ImageBuilder::at(source: ObjRef, rect)` can only
  place an `/XObject` the document already has~~ — landed.
  `DocEdit::embed_jpeg(bytes)` is the `FPDFImageObj_LoadJpegFile` path (DCT
  and JPX passthrough, header-parsed dimensions and colour space, the Adobe
  CMYK `/Decode`), and `DocEdit::embed_image(pixels, w, h, PixelFormat)` the
  `FPDFImageObj_SetBitmap` one (raw samples, flate through the writer's
  existing filter decision, an `/SMask` split off `Rgba8`, `/ImageMask` for
  `Mask1`). The subsetting/collector logic was indeed untouched.
  `docs/design/pdfrum-edit.md` §3.9.
- ~~**JavaScript M15 step 2 — the `Doc`/`Field` object model.**~~ — landed
  2026-09-03: 11 of 47 transcript fixtures byte-exact became **23 of 47**, and
  both WP12 defects are closed. `docs/status/M15.md` §2 has the per-name table
  and §3 the correction to the seven owed regressions — five of them turn out
  to be *timer* tests needing `advance_time`, not object-model work.
- **What M15 still owes**, from `docs/status/M15.md`'s per-fixture table:
  - the **`/AA` event path** — four fixtures (`event_properties`,
    `mouse_events`, `public_methods`, `bug_1142688`) print nothing because
    nothing fires their field actions. The largest single remaining bucket.
  - **`advance_time`** — M14's D14 reserved it and nothing calls it. Five of
    the seven V8-gated formfill regressions and `bug_1447268` wait on it.
  - the object-model slices step 2 did **not** take: `color` (2 fixtures),
    the nine constant namespaces (1), `global`'s interceptors (1), and
    `constructor`'s `illegal constructor` shape (1).
  - **`Doc.getPageNthWord`** needs a content-stream word extraction; it is
    declined with the oracle's own message and its range check, and
    `document_methods` is the one fixture that wants the words.
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

- ~~**Rustdoc names internal phases and internal documents.**~~ Landed
  09d7cb5..46d11e8 (2026-09-03): passes A and B-1 swept the crates, WP5 took
  `pdfrum-form` (207 provenance + 21 internal → 0) and `pdfrum-script`. A CI
  gate, `scripts/check-no-internal-refs.nu`, landed with it and was removed
  again the same day (see below). Two corrections to the pattern, both found by running
  it workspace-wide: `SPEC\.md` under-matched the unsuffixed `(SPEC §15.8)`
  spelling the tree actually carries, so the doc names are now
  `\b(SPEC|PLAN|STYLE|DEPS)(\.md)?\b`; and `§[A-Z]\.[0-9]` over-matched the
  legitimate `ISO/IEC 15444-1 §A.4.1`, so a line citing a standard before its
  section marker is spared for that alternative alone. The widening found 19
  further lines across seven crates, swept in the gate's commit. The
  non-vacuity control caught a live bug on its first run: `-- 'crates/*/src'`
  matches no file under git's default pathspec globbing, so the scan was
  reporting a clean tree by looking at nothing (`:(glob)crates/*/src/**`).
  Original entry: 238 `///`/`//!`
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
- ~~**Hard-coded absolute paths.**~~ Landed 58a09fb..eb17508 (2026-09-03): six `PDFRUM_*` variables with repo-relative defaults via `scripts/env.nu` and a per-test-file resolver that prints its skip reason once; `scripts/check-no-absolute-paths.nu` in CI; a variables-only board reproduces 1757/1514/243. The real count was 44 lines in 30 files (the 73 included the record dirs). Original entry: 73 lines name `/mnt/data2/…` or
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

## Oracle checkout hygiene (added 2026-09-03)

- ~~**The board should refuse a modified oracle checkout.**~~ — landed
  (2026-09-03): `oracle::require_clean_checkout` runs
  `git -C <checkout> status --porcelain --untracked-files=no` and refuses
  before `run`, `generate-goldens`, `tier-c`, `save-round-trip` and
  `mutate-round-trip` read a file, with `--allow-dirty-oracle` /
  `PDFRUM_ALLOW_DIRTY_ORACLE=1` as the override and a one-line skip for a
  non-repository export. `CorpusArgs::checkout()` is the only way to learn
  where the tree is, so a new subcommand cannot forget the check.

  The refusal names the class it exists for rather than the count: **a
  tracked `.pdf` regenerated from a template that disagrees with it.** Of
  the 340 files, the 334 `/Length` regenerations were render-neutral and the
  golden store's content-hash keying already immunised the board against
  them (a drifted input maps to a different golden directory; worst case
  `missing-golden`), and the six overwritten expected-output files are read
  by nothing of ours. The one behaviourally significant case was
  `resources/viewer_ref.pdf`, whose `.in` says `/Count 1` while the
  committed file has five pages — the regeneration replaced a five-page
  fixture with a one-page one. The harmless churn is the symptom that made
  it findable, which is why the check refuses on *any* tracked modification.

  Invoker audit: `generate.rs` (per-file scratch copy, `--output-dir` on the
  fixup), `scripts/bench-rss.nu` (copies to `mktemp -d`), and
  `crates/pdfrum/tests/{load_font,load_font_subset,embed_image}.rs` (write
  under `std::env::temp_dir()`) were already safe; `scripts/bench-oracle.nu`
  passes `--md5` without `--png` and writes nothing (measured);
  `fuzz/seed-corpus.sh` only `cp`s out of the checkout;
  `scripts/probe-tounicode.py` and `docs/status/data/v8probe/gen_pdfs.py`
  build their own PDFs into their own output directory. Nothing ran in
  place. `conformance/README.md` §"The oracle checkout is read-only" carries
  the rule.

## Rustdoc trim (`docs/design/rustdoc.md`)

- ~~WP5 inner crates, host-reachable first: `pdfrum-text`, `pdfrum-form`,
  `pdfrum-render`, `pdfrum-object`~~ — all four landed 2026-09-03, plus
  `pdfrum-script`. §8's definition of done is ticked but for the one box a
  human has to tick.
- ~~WP6 the STYLE.md §6 paragraph, last.~~ — landed 2026-09-03.
- ~~The remaining WP5 crates — `pdfrum-page`, `pdfrum-parser`, `pdfrum-cmap`,
  `pdfrum-crypt`, `pdfrum-filters`, `pdfrum-type1`, `pdfrum-raster-*`.~~ —
  landed 2026-09-03, one commit per crate. Every crate root is at or under its
  cap (55 → 11 the largest fall, `pdfrum-raster-agg`) and indexed items over
  the cap went 20 → 0; 642 doc lines removed against 198 added and 237 `//`
  lines added, so the essays moved rather than vanishing. No non-doc line
  changed and the API snapshot is unmoved. `pdfrum-raster-vello` was included
  despite `publish = false` because it was grossly over on both halves.
  rustdoc.md §7 carries the per-crate table and the re-derived contracts.
- ~~**C++ provenance outside the facade.**~~ — landed 2026-09-03,
  workspace-wide. The real count was **416**, not the 83 this entry estimated:
  that number was WP4's five alternatives over the ten crates WP5 had just
  finished, and it is still right for that (84 today). What it could not see
  was that `CFX_` and `CJS_` were never in WP4's pattern, and that four crates
  — `pdfrum-doc` 105, `pdfrum-tool` 76, `pdfrum-edit` 61, `pdfrum-font` 60 —
  had been in no rustdoc work package at all. One commit per crate, no non-doc
  line changed, the API snapshot unmoved. Every `[oracle-bug]` site keeps both
  its citations, moved to `//` at the site. Three contracts were found stated
  **wrong** and corrected against the code, which is the case for reading each
  site rather than pattern-matching it: `pdfrum-edit`'s `content/text.rs`
  claimed the emitter's lost text state was a requirement when its own module
  had established it is a limit; `write/reach.rs` named a `seen_ref_objects`
  filter that does not exist (the C++'s name for `seen_sources`); and
  `pdfrum-render`'s `to_straight_bgra` explained its flag by naming the
  oracle's bitmap format rather than saying what a caller gets. `pdfrum-tool`
  needed no exception despite being a `pdfium_test`-compatibility CLI — it
  already says "the oracle" everywhere, and that reads better than the binary's
  name. rustdoc.md §7 carries the per-crate table and the rest.
- ~~**A CI gate to keep C++ citations out of rustdoc.**~~ — **not doing it**,
  and `scripts/check-no-internal-refs.nu` went with the decision (2026-09-03,
  user). A draft widened that script with the C++ pattern as a second class and
  it worked, but the shape it grew is the argument against it: nine
  alternatives, an ISO exemption for the annex citations it would otherwise
  refuse, a per-line opt-out for identifiers that are legitimately ours, and a
  fourth planted-line control to prove the opt-out both fires and does not
  over-fire — each one there because the one before it was too blunt, and none
  of them any help in writing a better doc comment. The rule is one sentence in
  STYLE.md §6 and a person can follow it. A citation that comes back comes back
  in review, where a human can tell an `[oracle-bug]` record that must keep its
  citation from a diary entry that must not.
- ~~**`scripts/extract-font-tables.py` regenerates rustdoc the sweep removed.** — landed c0f6caa (2026-09-03): the generator emits the tables' own one-line docs; header and usage name `scripts/extract-font-tables.py`.
  `crates/pdfrum-font/src/encoding/tables.rs` is `@generated`, and the sweep
  above rewrote its fifteen table docs from ``/// `kFoo` (path.cpp).`` to a
  sentence naming the code page. The extractor still emits the old form (lines
  ~147 and ~160), so re-running it reverts all fifteen. Nothing in
  `scripts/ci.nu` regenerates the file, so this is a trap rather than a break.
  Give the two `chunks.append` templates the same treatment the checked-in file
  got. While there: the generator's `HEADER` names a `scripts/extract_tables.py`
  that does not exist.

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
- **`vector_font_size14` 3.70x and `vector_font_feature` 3.42x are split but
  not closed** (§13). The caches are not the cost: `BitmapCache` hits 100% of
  183 813 occurrences with zero outline extractions, and `BitmapKey` needed no
  change. The cost is the **AGG blit's per-row scaffolding** —
  `Target::blend_span_with` once per glyph row, each call re-deriving
  `span_range`, `clip_span` and the destination slice before an
  `Option`-returning closure per pixel — **11.8 ns per glyph pixel, 5.0-5.1 ms
  of each render**. `blit` has one caller but serves every whole-pixel image
  blit, so tier-c gates it, and it must not be bought with a seventh device
  primitive (§13.4). **This is the next thing to take**; the two rows stand at
  3.70x/3.42x until it lands.
- ~~the glyph blit's per-occurrence allocations~~ — landed 3e2efe6
  (2026-09-03): `to_gray` and `recolour` now fill `RenderCaches`-owned
  buffers. Worth **18.6 ns of a 210 ns per-glyph chain, 0.13-0.25% of a
  render** — measured in process, because the wall clock cannot resolve it
  here: a zero-glyph control read 1.055x in the same table (§13.5). Board
  byte-identical, tier-c unchanged. Kept for the allocations it removes, not
  for a figure it moves.
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
