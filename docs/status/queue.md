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
- ~~**What M15 still owes** — the `/AA` event path, `advance_time`, the four
  object-model slices, and `Doc.getPageNthWord`.~~ — landed 2026-09-03 in
  four commits. **23 of 47 transcript fixtures byte-exact became 33 of 47**,
  and the board moved 1526 → 1536 with `js-transcript` 21 → 11 and no row
  moving the other way. `docs/status/M15.md` §"Step 3" has the
  event-population table, the six trigger firing rules, the four timer rules,
  and the per-fixture accounting. The exit restates as **47/47 accounted for,
  40 byte-exact and 7 not achievable by construction** — `bug_1142688` joins
  the V8 declines, because boa's `RuntimeLimitError` is uncatchable *by
  design* and the fixture wants to catch a stack overflow.
- ~~**What M15 still owes after step 3** — `Field.setFocus`, the facade's
  document model, and the four residues.~~ — landed 2026-09-03 in three
  commits. **33 of 47 transcript fixtures byte-exact became 37 of 47**, and
  the board moved 1536 → 1539 with `js-transcript` 11 → 8 and no row moving
  the other way. `docs/status/M15.md` §"Step 4" has the routing of the focus
  request, the blur-then-focus proof, and the residues table.

  Two findings worth carrying forward. **Both missing wires moved the board
  by zero**, because neither had a fixture that could fail — they were found
  by reading for a value nothing consumes, and that sweep is worth repeating
  mid-milestone rather than at the end of one. And **two of the five owed
  items named fixtures that are suppressed for our build**
  (`bug_735912` `noxfa`, `named_action` `nov8`), so the corpus's 47 javascript
  templates are 44 board rows; `bug_735912`'s recorded order needs the XFA
  focus path and it joins the not-achievable bucket rather than staying owed.

  M15 now exits at **47/47 accounted for — 37 byte-exact, 8 not achievable by
  construction, 1 parser work (`bug_1314658`, a damaged file whose
  `/OpenAction` our parser does not reach), 1 oracle bug (`util_printd`)**.

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
  30–66 through every run, and 33–61 through §15/§16); the ratios are upper
  bounds until then. **§15 tried again and could not deliver it**: the brief
  made `shading` and `forms` conditional on load < 30, the 1-minute average
  read 27.4 when the run was armed and 33–49 through it, so those two classes
  were *not* re-taken and the item stood for the eighth time (§15.3).
  **§18.2 very nearly discharges it**: all 44 rows at **load 6–10**, the
  quietest any table here has had, and `shading` and `forms` finally taken.
  Not fully — §3 asks for idle and the 1-minute average moved 6.06 → 10.16
  across the run — but §18.2 is the first table in this document whose
  milliseconds are worth reading.
- ~~the AGG blit's per-row scaffolding~~ — landed as §14: `AggDevice::blit`
  now walks the rows itself through one `Target::blit_image` instead of a
  `blend_span_with` per row. **The blit is 1.56-1.61x faster**, worth 1.5 ms
  of each vector render — ten times §13's item — and roughly **half of
  `image_bug_583804`'s whole render**, because that document spends 59-75% of
  itself in one whole-pixel image blit. Board byte-identical, tier-c
  unchanged, no new device primitive (the API snapshot matches). **§13.6's
  proportions were wrong and §14.1 corrects them**: the per-row scaffolding
  was 6.5-6.8% of the blit, and the other 93% was the per-pixel
  `Option`-returning sampler closure re-deriving the source index from the
  destination column. The same hoist removes both.
- ~~the split above the blit~~ — landed as §16: the glyph's **two per-pixel
  loops**, `LcdBitmap::gray_coverage_into` and `recolour_ref_into`, which
  §16.2's `Instant` pairs put at 25.0% and 16.5% of the two vector renders —
  `recolour_glyph_into` is *larger than `blit_image`* on `size14`. Both are
  §14.2's shape one level up: a per-pixel loop re-deriving an index the row
  already knows and bounds-checking one that cannot be out of range. Hoisting
  the row makes the pair **2.22–2.26x** faster, worth **1.09–1.14 ms of each
  vector render** and 4.4 → 2.0 ns per glyph pixel. Board byte-identical (`per_file`
  equal, both binaries run), tier-c unchanged, API snapshot matching, no new
  device primitive, `BitmapKey` untouched. The wall clock **resolves** it this
  time: 1.047x / 1.089x / 1.043x against a zero-glyph control at 0.995x
  (§16.6).
- ~~**`vector_font_size14` and `vector_font_feature` are still not closed.**~~
  — **closed as defects in §17**, and not where §13–§16 were looking. The
  ~50 ms those sections could not account for was `ap::FormFonts::load`:
  **78% of `size14`'s render and 44% of `feature`'s**, on documents with
  **zero annotations**. §5's memoization keys on the `/AcroForm`
  *reference*, and six of the corpus's 44 files write theirs as a direct
  `<</Fields[]>>` dictionary, so all six missed the cache **once per page per
  render** — 9 calls, 9 misses on `size14`, against `forms_text_field`'s 3
  calls and **0** misses. Keying on the `/DR /Font` the faces actually depend
  on instead: `size14` **53.85 → 10.31 ms (5.22x)**, `feature` 1.97x,
  `text_foxit_products` 4.20x, `text_cjk_page` 4.80x, `text_cjk_structure`
  4.25x, `image_jpx_123` 1.41x, against four zero-form controls at
  **0.975–1.053x**. Against the oracle the five re-taken rows go 2.73x →
  **0.59x**, 2.86x → 1.29x, 2.83x → 0.63x, 3.53x → 0.77x, 2.59x → 1.73x
  (§17.6). Board byte-identical (`per_file` equal across 1757 entries, both
  binaries run), tier-c unchanged. **Inside `render_page_with` the blit is
  still the largest line** — §13–§16's figures all stand, they were shares of
  a denominator that was 78% something else — and `place_glyphs_into` at
  1.39 ms is still the unsplit item above it.
- ~~**`--sample` and `--walk` cannot see a whole render.**~~ — **landed as
  §2's instrument**: `pdfrum_page::renderprofile`, eight stages placed
  *around* `render_page_with` rather than inside it, so their sum plus one
  printed remainder is the ms/iteration figure above them (0.1–4.8% across the
  corpus). `form fonts` carries a **miss count beside its call count**, which
  is the row §17 needed and did not have. It reproduces §17.2's `size14`
  split on the pre-§17 binary to within a point — `FormFonts` **78.2%** against
  §17.2's 77.2%, 9 calls and **9 misses**, raster 13.1% against 13.9%, 99.1%
  attributed against 99.3% — and §17.2's three controls likewise. Read it with
  `scripts/profile.nu render <file> <iters> <backend> --warm --walk`. No public
  surface moved: the module is `pub` only under the feature, as
  `walkprofile` is, and the two crates above it forward a `walk-profile` of
  their own.
- ~~**every oracle-relative ratio is our whole pipeline against the oracle's
  raster half**~~ — **corrected in §18**, and the correction is worth more
  than any change §11–§17 landed. §18.1 takes option (a): ours is measured the
  way the oracle amortizes, `whole − (content parse + interpretation)`, both
  terms out of **one** run of §2's instrument so the subtraction is a
  measurement rather than an estimate. §17's "4–8% of `size14`" was wrong —
  the graph build is **38% of `size14`, 63% of `en_system` and 65% of
  `en_fqa`**. All 44 rows re-taken on the corrected pairing at **load 6–10**,
  the quietest table in the document: **nine rows above 1.5x, against
  nineteen on the old pairing**. `image_en_fqa` **3.34x → 1.16x** and
  `vector_en_system` **2.15x → 0.79x** are not defects at all. `text`,
  `forms`, `shading` and `mixed` have oracle-relative rows for the first
  time; geomeans 0.13x / 0.52x / 0.56x / 0.78x / 0.68x / 0.63x (§18.2).
- **DECISION OWED: should `Page` retain its built graph across `render_on`
  calls, the way `CPDF_Page` does?** §18.1 argues it and **deliberately does
  not implement it** — it is a design change, not a measurement one. The cost
  is measured: a page graph holds its **decoded images**, so
  `image_bug_583804` retains **176 MB for one page** and `image_en_fqa`
  +21.8 MB over four. `CPDF_PageImageCache` is the oracle's equivalent and it
  has an eviction policy we would not. It would make our loop natively
  comparable and remove §18.1's subtraction; it also changes what
  `Page::render_on` promises about lifetime. **The user's call.**
- ~~**`pdfrum-tool --md5` renders nothing.**~~ — **landed**: the no-format arm
  now rasterizes, as the oracle's `default:` case does ("Other formats won't
  write the output to a file, but still rasterize"). `vector_font_size14`
  goes 0.04 s → 1.85 s, which is nine pages of render where there were none.
  **§17.1's "prints no `MD5:` line" was a defect report about the wrong half**:
  the oracle prints none either, verified against both its source and its
  binary — `BitmapPageRenderer::Write` returns early on a null writer, and
  `--md5`'s own help text says it writes "output image **paths** and their md5
  hashes", so with no file there is no line. Only the *render* was missing.
  `render` split into `rasterize` plus `encode` so the no-output path pays
  neither the hash nor the PNG encoder, which the oracle does not pay there.
  No conformance pass uses `--md5` alone, so the board cannot move — and does
  not (`per_file` equal across all 1757 entries, both binaries run).
- **`BitmapCache::get_or_insert`'s second hash probe is named, measured and
  cannot be removed.** The hit path is 100% of calls and asks `contains_key`
  then `get`. Returning the occupied entry's borrow is NLL problem case 3 —
  rejected by today's borrow checker, accepted by Polonius — and the `Entry`
  API does not rescue it because the budget check needs `render()`'s result
  and `render()` cannot run while a `Vacant` entry holds the borrow. Three
  spellings were tried and none compiled. Worth **85–99 ns of a 1000 ns
  per-glyph chain** (§16.4); revisit if Polonius lands.
- **A coverage-to-pixel lookup table in `recolour_ref_into` is a measured
  negative result.** It is the obvious next hoist — 256 entries derived once
  per occurrence turn four `mul255`es into one array read — and it measured
  **3.9x slower** on both fixtures, because a glyph is ~50 pixels and the
  table is 256 entries. The code carries a comment saying so; do not
  re-propose it without re-measuring (§16.4).
- ~~whether the rest of the `image` class moves with §14~~ — **measured in
  §15, and the answer is no.** `image_bug_583804` is the only row §14 reaches
  and the only one that improved on a comparable load (0.79x → **0.72x** at
  load 35). `image_ccitt_3bigpreview`, `image_en_fqa` and `image_jpx_123` all
  read *worse* than §10.5 at loads 40–49 and none of them has a whole-pixel
  blit of the kind §14 serves — `en_fqa`'s cost is its decode and its
  resample. The class geomean reads 0.11x against §10.5's 0.09x, and the
  difference is the load, not the change.
- ~~the glyph blit's per-occurrence allocations~~ — landed 3e2efe6
  (2026-09-03): `to_gray` and `recolour` now fill `RenderCaches`-owned
  buffers. Worth **18.6 ns of a 210 ns per-glyph chain, 0.13-0.25% of a
  render** — measured in process, because the wall clock cannot resolve it
  here: a zero-glyph control read 1.055x in the same table (§13.5). Board
  byte-identical, tier-c unchanged. Kept for the allocations it removes, not
  for a figure it moves.
- ~~`shading_type4_5` 4.38x and `shading_tcpdf_058` 1.88x have not been
  re-taken~~ — **both re-taken in §18.2**, like-for-like at load 6–10:
  `shading_type4_5` **2.67x** (a 0.27 ms oracle row — the least trustworthy
  band in the table) and `shading_tcpdf_058` **8.67x**, which is the corpus's
  largest ratio and is split in §18.3. `image_en_fqa` reads **1.16x**
  like-for-like against §15's 3.15x — 65% of it was the graph build — and
  `image_ccitt_3bigpreview` **2.45x**, which is real residue.
- ~~**`shading_tcpdf_058` at 8.67x: off-page clip paths are sorted and swept
  and then discarded.**~~ — **landed as §19**: `Rasterizer::keep_rows` records
  the row range its caller will keep and drops a cell outside it before the
  store grows, so nothing off-target is sorted or swept. **Exact rather than
  approximate**, and the reason is a property of this rasterizer that AGG's
  `rasterizer_sl_clip` does not share: `sweep` resets the running cover at
  every row boundary, so a row's spans are a function of that row's cells and
  nothing else, and dropping a row can change no other (§19.2). Edge clipping
  was declined for that reason — the more invasive change buys the same answer.
  The discard is rows only: applied to *columns* it would drop the cover an
  on-target cell to the right needs, and that mutation is one of four planted
  and caught. **§18.3's two targets are met**: the cell sort **22.5 → 0.345 ms**
  and the row walk **4.84 → 1.42 ms**, both to `forms_text_field`'s scale, with
  cell rows per iteration **530 072 → 9 323** and `coverage_of` **33.1 →
  1.71 ms**. Wall clock **45.30 → 16.68 ms, 2.72x**, against two controls at
  0.98x / 1.00x, at load 8.9–10.0. Board byte-identical (`per_file` equal
  across all 1757 entries, both binaries run), tier-c unchanged to the digit.
- ~~**`shading_tcpdf_058`'s residue is `AggDevice::pop` at 10.05 ms on 77
  calls.**~~ — **landed as §20, and §19.7's attribution is corrected with it.**
  The figure was right; the arm was misread. `pop` has two, and splitting them
  puts the **clip** arm — `recycle` and its band clear, the mechanism §19.7
  named — at **0.045 ms of the 10.05**, with the clear itself at **0.032 ms**
  for 3.82 MB. That is a `memset` at what a `memset` costs; §12.3's "half a
  megabyte" described the volume and §19.7 read it as the cost. §12.2's own
  table had already read `pop` at 0.007 ms on a document with no layers.
  **The band needs no tightening either**: `coverage_of` folds it from the
  writes rather than from a bbox, and the band measures **7285.1 rows per
  iteration against 7285.1 rows the plane holds non-zero coverage in** — equal
  to the row. The 10 ms was the **`Frame::Layer`** arm, compositing the layer
  back **one pixel per `blend_span` call** — 500 395 calls per layer, 13.6
  layers per render, each paying `span_range`, `clip_span`, the destination
  offset and a `chunks_exact_mut` to reach four bytes. **This is §14.1's defect
  and §16's, one level over**, and the third instance of "a loop re-deriving an
  index the row already knows". `Target::composite_layer` walks the rows in
  `blit_image`'s established shape, unclipped by construction because the layer
  already carries its clip. Layer arm **11.47 → 2.08 ms**, whole render **17.03
  → 9.32 ms wall (1.83x)** against **five controls at 0.987x–1.010x** at load
  10.5–12, like-for-like **4.37x → 2.29x**. Four mutations planted and caught;
  a fifth — dropping the transparent-pixel skip — **did not fail**, because a
  zero-alpha source is already the identity under every blend mode, so the skip
  is recorded as an optimisation rather than a behaviour and
  `a_transparent_source_pixel_is_the_identity_under_every_mode` checks it.
  Board `per_file` byte-identical across 1757 entries, tier-c unchanged.
- **`shading_tcpdf_058` at 2.29x is no longer the corpus's largest ratio and
  has no single large line left.** With the layer arm at 2.08 ms and
  `coverage_of` at 1.71, the residue is spread across `draw_image`,
  `push_layer` and the sweep. **No census was taken of which other corpus
  documents push layers** — all five of §20's controls push none — so a
  document with a deep layer stack would gain from §20 and was not looked for.
- **`mixed_en_uicase` at 3.52x is now the corpus's largest ratio, and is a
  different shape from both §19's and §20's.** 257 619 cell rows per iteration but **837 sweeps**, row walk
  37.3 ms against 5.2 ms of sort, and **zero** off-page clips. Many small
  sweeps. §18.3 said §19's fix would not move it and **§19.6 confirms it did
  not**: 1.03x on the wall clock, with its row and sweep counts identical
  before and after to the row. **§20 does not move it either** — it pushes no
  layer at all, and reads 1.002x as one of that section's controls. Its
  `pop` is 0.96 ms per iteration and **0.85 of that is the band clear**, over
  574 pops and 122.7 MB per iteration: the one row in the corpus where the
  clear is a measurable share of anything, and still under 2% of its render.
- **Seven more rows above 1.5x like-for-like, all of them in the rasterizer.**
  `vector_font_feature` 2.67x (90.9% raster), `shading_type4_5` 2.67x,
  `image_ccitt_3bigpreview` 2.45x (87.5% raster), `image_ccitt_transfer`
  2.31x, `forms_list_box` 1.74x, `image_jpx_123` 1.70x (99.4% raster),
  `forms_number` 1.56x. That is the opposite of §17's finding and is the
  correction the pairing was for: with §17's annotation-pass defect fixed and
  the graph build paired correctly, **everything left is raster** (§18.2).
  Four of the seven have oracle columns under 1.1 ms and their ratios are the
  least trustworthy in the table. **None of the seven has off-page geometry**
  and §19 moved none of them: a census of every clip and fill path per render
  found `0 of 730`, `0 of 144`, `0 of 518`, `0 of 418` and `0 of 52` off the
  device, against `shading_tcpdf_058`'s `17 of 81` (§19.6). `image_jpx_123`'s
  one off-target fill is a whole-page image footprint overhanging by a pixel.

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
