# `pdfrum-text` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green; M2's text
exit criterion met

Contract: SPEC.md §9 (including the 2026-08-29 rulings); behavior:
`docs/design/pdfrum-text.md`.

## Conformance

| Metric | Before | After |
|---|---|---|
| `--txt` Tier-A, all pages | 872/1832 (47.6%) | **1816/1832 (99.1%)** |
| `--txt` Tier-A, non-empty pages | 0/960 (0%) | **944/960 (98.3%)** |

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

## The remaining 16 pages

Sixteen non-empty pages across thirteen files still differ, and every one of
them is a **font-metric** difference rather than a heuristic one.

Every space and line-break threshold in the extraction pipeline is a function
of `GetCharWidth`, which for a non-embedded font without `/Widths` comes from
the *substituted face*. A different substitute than the oracle's hermetic
`third_party/test_fonts` set gives different widths, which give different
thresholds, which move a generated space — a whole-page Tier-A failure with
no local cause and no local fix. That is exactly the Q6 risk the design brief
names, and it says where to look: `pdfrum-font`, not here.

The clearest cases are `bug_1769.pdf` (characters the overlap logic should
drop and does not), `bug_1388_3.pdf` (one space too many), `bug_1442723.pdf`
(private-use codes reaching different Unicodes), and four CJK files whose
codes map to nothing at all. One embedder test is `#[ignore]`d against
`bug_1769`.

The text side has no further work pending on any of them.

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
