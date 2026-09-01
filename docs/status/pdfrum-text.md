# `pdfrum-text` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green; M2's text
exit criterion met

Contract: SPEC.md §9 (including the 2026-08-29 rulings); behavior:
`docs/design/pdfrum-text.md`.

## Conformance

| Metric | Before | After |
|---|---|---|
| `--txt` Tier-A, all pages | 872/1832 (47.6%) | **2052/2061 (99.6%)** |
| `--txt` Tier-A, non-empty pages | 0/960 (0%) | **990/999 (99.1%)** |

(The corpus grew from 1832 pages to 2061 across M2–M6, so the rates compare
and the counts do not. The "after" column is current as of the Q6 fix below,
which took the non-empty rate from 98.3% to 99.1%.)

The non-empty rate is the M2 exit criterion (SPEC.md §9's `text-nonempty`
ruling, PLAN.md §6): **≥ 98% is met at 98.3%.** The "before" column is the
baseline where the tool emitted nothing, which scored 47.6% overall purely
from the 872 pages whose golden is a bare byte-order mark — the exact
flattering number that ruling exists to defuse.

metadata and pageinfo remain at zero mismatches, so the monotone rule holds.

## What landed

Everything SPEC §9 names, plus the `--txt` emitter and the content assembly
the tool needed to reach it.

| module | contents |
|---|---|
| `unicode.rs` | bidi class, mirroring, the four normalization tables, `is_alpha`/`is_alnum`/`to_lower`, all read from the committed blob |
| `bidi.rs` | the four-bucket segmenter, the auto-order heuristic, the leading zero-count segment |
| `charinfo.rs` | `CharBox`, `CharType`, `GetLooseBounds`' four shapes, the matrix helpers |
| `object.rs` | the typeset view of a `TextObject`: per-glyph codes, positions, kernings and both boxes, derived by one pen walk |
| `orientation.rs` | the page-global mask scan and the per-object five-degree cone |
| `line.rs` | the paired staging buffers, space collapsing, `CloseTempLine`, `AddCharInfo` |
| `pipeline.rs` | the reorder batch, `ProcessInsertObject`'s whole decision tree, per-object emission, duplicate suppression, `/ActualText` |
| `dedup.rs` | object-level repeat detection with its five-text-object lookback |
| `index.rs` | the segment table bridging the two index spaces |
| `find.rs` | needle splitting, the space-tolerant match loop with its bounded restart, whole-word testing |
| `links.rs` | web and mail recognition, bracket trimming, the ported index mismatch |
| `select.rs` | rect arrays, index-at-point, text-by-rect and text-by-object |
| `build.rs` + `tables/` | the committed unicode blob and its opt-in regeneration |

`pdfrum-tool` grew `content.rs` (assembling `/Contents` with the one-space
join) and `text.rs` (the UTF-32LE emitter and the `/ViewerPreferences
/Direction` read), and `run::dump_page` gained a file-writing sibling.

### The behaviors that mattered most

- **The two outputs are different sequences, and `--txt` reads the one you
  would not guess.** `TextPage::chars` is the `--txt` stream; `TextPage::text`
  is what search and selection see. They disagree at a soft hyphen (`U+0002`
  against `U+FFFE`), at a control character (present against absent) and at a
  ligature (one against several). Routing `--txt` through the text string
  would have looked right on 98% of the corpus and been wrong on exactly the
  19 goldens that hold the rare values.
- **Fonts must be shared by reference.** Duplicate suppression compares font
  identity, and `pdfrum-page` was loading a fresh `Font` per `Tf` — so it
  never fired, and every double-drawn page extracted twice. The design brief
  warned about this in §1.7a as a hypothetical; it was actually happening.
  Fixed with a font-instance cache in `BuildContext` keyed on the resource's
  reference.
- **An unmapped code in a substituted font borrows the space's metrics.**
  `LoadCharMetrics(32)`. Without it a run of unmappable codes advances the pen
  by nothing, the text object's box comes out zero-width, and the whole object
  is dropped by the degenerate-width check — which is how 22 NUL characters
  went missing from `bug_425244539.pdf`.
- **A Type 3 glyph's box lives in its content stream.** Nothing in the font
  dictionary says how big a Type 3 glyph is; the `d0`/`d1` operator inside the
  procedure does, and a degenerate declaration means "measure what the
  procedure paints". Six fixtures extracted nothing at all until this landed.
- **The angle comes from the y-into-x coefficient.** The two matrix
  conventions agree on `a` and disagree on which off-diagonal term is called
  `b`; reading the wrong one reflects every angle about the x axis, which
  reads as the first quadrant coming back as the fourth.
- **A CID-keyed bare CFF needs its charset consulted.** The composite-font
  layer hands down a CID, because that is what PDFium hands FreeType, and
  FreeType maps it through the font's charset silently. A subsetted
  CID-keyed program holds sixteen glyphs numbered from zero while its CIDs
  are wherever the original collection put them, so skipping the mapping asks
  for a glyph that does not exist and the font draws *nothing at all* — which
  the width ladder then reads as a zero-width text object and drops whole.
- **`FX_Number`'s two halves disagree about repeated signs.** A word
  containing a period goes to the float parser, which skips a *run* of signs
  and keeps the last, so `--40.34` is `-40.34`. A word without one goes to
  the integer parser, which takes one sign and stops, so `--40` is zero. A
  real generator writes the doubled minus, and reading it as zero puts a
  whole line of text on top of the one above it.

## Divergences

D1–D10 are implemented as the brief specifies. Three are worth restating:

- **D1**: the character-class tables are generated from the oracle's own ICU
  78.2 (Unicode 17.0) rather than from a Unicode crate, and `unicode-bidi` is
  **not** linked — PDFium's bidi is a four-way bucket, not the UBA. See
  `tables/PROVENANCE.md`.
- **D4**: the hyphen path's empty-container dereference is declined, not
  ported. It is a crash in the C++ on a crafted file, and a crashing oracle
  writes no golden.

  *Relabelled 2026-09-02 (audit A50): an **oracle bug**, so declining it is
  obligatory rather than our choice, and the brief's Q5 no longer needs an
  answer.* Verified at the line: `cpdf_textpage.cpp:1357` is
  `CharInfo& charinfo = temp_char_list_.back();` with no emptiness guard,
  straight after the `while` at `:1352-1356` that pops trailing spaces from
  both staging containers; the arm is reachable with the list empty because
  `IsHyphen` consults the *finished* `text_buf_` when the staging buffer is
  empty. `CHECK` failure in debug, undefined behaviour in release. Nothing in
  §9.10 asks an extractor to crash, and pdf.js has no such staging list to
  dereference — its soft hyphen is normalised to `-` (`unicode.js:57-58`) and
  rejoined at query time (`pdf_find_controller.js:290-307`). Q5 asked what a
  release-mode oracle produces instead of crashing; the oracle-bug rule
  settles the case without that answer. `pipeline.rs` carries
  `// [oracle-bug]`. No code change, 0 rows.
- **D5**: link extraction's character-index-into-text-buffer slice is ported
  as written, with a bounds-checked slice that yields an empty candidate where
  the C++ yields an empty string.

## Brief errata

Four places where the design brief and the C++ disagree, checked against the
source and, where it was decidable, against the built oracle:

1. **§1.4: a page with no text objects comes back `Horizontal`, not
   `Unknown`.** The brief traces the empty scan to `Unknown`; the arithmetic
   says otherwise. With nothing scanned, `nEndV - nStartV` is `0 - page_height`
   — a large negative — and `nDoubleLineHeight` is zero, so the *first* test
   fires. This matters because it is exactly the case the brief raises: a page
   whose text lives entirely inside form `XObject`s scans nothing.
2. **§1.10b: an RTL character with no normalization still becomes a
   `kPiece`.** `GetUnicodeNormalization` never returns empty — it returns the
   character itself — so `normalized.empty()` is false for *every* consulted
   character and the multi-character path always runs. The brief's reading
   implies identity normalization takes the single-character path. The
   consequence is real: ordinary Hebrew letters carry `CharType::Piece`, which
   is what the hyphen look-back accepts.
3. **§1.13's corpus frequencies count the golden store, not a harness run.**
   The store holds 5783 per-page dumps; the harness compares 1832, because it
   keys by content hash and many corpus files expand to the same bytes. The
   19 rare-value canaries are the same files either way.
4. **§1.2's claim that `GetMirrorChar` leaves a supplementary-plane character
   alone is wrong.** The property lookup falls back to zero above the BMP, and
   zero's mirror field is index *zero* rather than the sentinel — so every
   character above `U+FFFF` mirrors to `)`. Reproduced, and pinned by a test,
   because it is reachable from any RTL run containing one.

## The Q6 cluster, resolved 2026-08-29

The diagnosis was right and the cause was one wire, not a scoring subtlety.
`pdfrum-tool` classified **`--font-dir=` and `--croscore-font-names` as
accepted-but-unimplemented** and dropped both into `Options::unsupported`, so
nothing ever built a non-default `SubstitutionOptions`. `font_dirs` was always
empty, `SystemFontDb::scan` was never called, and every substitution resolved
against an *empty* database — short-circuiting the ladder at step 7 straight
into the built-in Foxit MM generics, while the oracle enumerated the 31
Croscore/Noto faces in `test_fonts` and picked Arimo, Tinos or Cousine.
Different face, different `hmtx`, different `space_threshold`.

`load_with_options` already existed and had no callers outside its own tests.
Three things closed the gap:

- the two flags are parsed into real `Options` fields;
- `BuildContext` carries a `SubstitutionOptions`, set once per document
  (substitution must not vary between two pages of one file), and both
  `pdfrum-page` font-load sites go through `load_with_options`;
- the Croscore rename is ported (`croscore_name`, from
  `testing/test_fonts.cpp:19-45`) and applied at the *request* boundary,
  where the C++ wrapper applies it. Without it a `/BaseFont /Helvetica` looks
  for a face `test_fonts` does not contain.

One thing found while wiring it: `SystemFontDb::scan` returned faces in
`fontdb`'s enumeration order, which is directory order. The C++'s list is a
`std::map` keyed by face name, and two decisions read it *in order* — a tie in
`similarity_score` goes to the incumbent, and rung 5 takes the first face
claiming the charset. The scan now sorts by name.

| metric | before | after |
|---|---|---|
| text Tier-A, all pages | 2040/2061 (99.0%) | **2052/2061 (99.6%)** |
| text Tier-A, non-empty | 978/999 (97.9%) | **990/999 (99.1%)** |
| files passing outright | 1087 | **1146** |
| `pixel-fail` | 553 | **495** |

`--check-regressions` reports none. The pixel column moves because the
substituted face draws as well as measures.

### The 9 pages that remain

Nine non-empty pages across nine path entries, which dedup to **six** distinct
sources — and they are no longer one cluster:

- `bug_1769.{in,pdf}` — `world wo d` where the oracle gives `wo d wo d`. The
  overlap logic should drop those characters and does not; the cause is in
  the dedup pass, **not** in metrics. Its embedder test stays `#[ignore]`d,
  with the reason corrected.
- `bug_1388_3.{in,pdf}` — one space too many. This file embeds its font, so
  substitution was never its problem either.
- `bug_1442723.{in,pdf}` — private-use codes reaching different Unicodes.
- `example_055.pdf`, `example_062.pdf`, `1_10_watermark.pdf` — one page each.

The four CJK files the previous note listed (`bug_1402`, `bug_1355` and their
`.in` twins) are fixed: they produced *no text at all* because the empty
database gave them no glyphs, and they now match byte for byte.

## Tests

| suite | count |
|---|---|
| unit (`--lib`) | 47 |
| `tests/embedder.rs` (ported from `fpdf_text_embeddertest.cpp`) | 35 (3 ignored) |
| `tests/heuristics.rs` (design brief §5.3, no upstream test exists) | 21 |
| doctests | 11 |

The 41 upstream `CheckWebLink` cases and 17 `CheckMailLink` cases are ported
verbatim in `links.rs` and all pass. `extraction_never_panics_on_any_resource_fixture`
sweeps every `testing/resources` PDF through extraction, search, links and the
UTF-32 dump.
