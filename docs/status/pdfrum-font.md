# `pdfrum-font` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

The M2 centrepiece. Contract: SPEC.md §6; behavior:
`docs/design/pdfrum-font.md` (29 subsections).

## What landed

Every part of SPEC §6's `pdfrum-font` half: the three-way font dispatch, the
descriptor and its metric derivation, the nine encodings and `/Differences`,
`/ToUnicode`, the four glyph ladders, widths in both formats, the whole
substitution ladder, vertical writing, and the glyph cache.

| module | contents |
|---|---|
| `lib.rs` | `Font` and its 20 methods, `CharItem`, `FontCache`, `load`, the font-dictionary key table, the GBK-name rescue |
| `ids.rs` | `Gid`, `FontId`, `GlyphName`, `FontFlags` |
| `error.rs` | `Error` — three variants, all meaning "this resource is not there" |
| `descriptor.rs` | `/FontDescriptor`, both 1000/em normalizers, `CheckFontMetrics` |
| `widths.rs` | `/Widths` `/FirstChar` `/LastChar` `/MissingWidth`; `/W` `/DW` `/W2` `/DW2` |
| `tounicode.rs` | the `bfchar`/`bfrange` reader — the Tier-A-critical part |
| `encoding/` | the 8 code→Unicode and 7 glyph-name tables, `/Differences`, the AGL adapter |
| `simple/` | `LoadCommon`, the Type 1 ladder (3 branches), the TrueType ladder (5 rungs) |
| `cid/` | Type0 assembly, the two-half glyph ladder, `GSUB` vertical substitution, the Japan1 transform table |
| `type3.rs` | glyph procedures as content streams |
| `glyphs/` | the two-backend `Face`, `GlyphSource`, `GlyphCache` |
| `subst/` | the 15-step ladder, the standard 14, style parsing, `SubstFont`, charsets, the `FontDb` seam |
| `fontdata/` | the 14 Foxit base-14 CFF blobs + `PROVENANCE.md` |
| `tests/fixtures/` | 12 synthesized TrueType fonts + `PROVENANCE.md` |

### The behaviors that mattered most

- **`skrifa` cannot open a bare CFF, and all fourteen base-14 blobs are bare
  CFF.** `FontRef` needs a table directory. This was not a detail: without a
  second backend the entire standard-14 set draws nothing, and the standard 14
  are what every unembedded `Helvetica` in the corpus resolves to. `Face` is
  therefore two backends — `skrifa` for SFNT, `read_fonts::ps::cff` for bare
  CFF — which is the same split PDFium's own Rust bridge makes.
- **The charmap is a value, not face state.** PDFium's ladders call
  `FT_Set_Charmap` and then a bare `FT_Get_Char_Index`, so the lookup's meaning
  depends on a mutation three steps earlier. Threading a `Charmap` selector
  through every lookup keeps the ladders' sequence recognisable while removing
  the hidden state, and it is what makes a `Face` shareable at all.
- **Glyph 0 is not "no glyph".** `.notdef` is a real glyph a font may
  deliberately map a code to; PDFium's `-1` is the different thing. `Gid`
  cannot hold both, so `CharItem` carries `has_glyph` and every ladder returns
  `Option<Gid>`.
- **The unset-width sentinel is `0xFFFF`, and it is nonzero on purpose.** The
  all-caps aliasing tests `width != 0` before copying, so an *unset* width
  propagates where a zero width does not. A zero sentinel would silently change
  which codes inherit metrics.
- **`/ToUnicode`'s two-phase count check discards whole blocks.** Too few
  entries is as fatal as too many, and a single character code above `0xFFFF`
  invalidates every mapping in its block. Both are recorded as diagnostics,
  because losing every character mapping in a file silently is exactly what the
  damage channel exists to make visible.
- **The high-code mask forces a `bfrange` into one 256-code block.** It
  truncates a declared 512-code range to 256 silently, and it turns
  `<0001> <10000>` into an inverted range that drops the section. It also
  doubles as the safety property that bounds the array form's unconditional
  token consumption.
- **A code mapping to exactly U+FFFF is unrepresentable** and reads back as
  multi-character entry 0. Reproduced, including the wasted `multi_char_vec_`
  push that shifts every later indicator index.
- **`ParseStyles` aborts on `"Bold,Italic"` but not on `"BoldItalic"`**, and
  `"Bold,Bold"` yields weight 900. The abort throws away the parsed family and
  falls back to the whole name, which changes which font a document gets.
- **`internal_subst` ignores its own arguments for a base-14 index.** Weight,
  angle and pitch are all discarded when the name resolved to one of the
  fourteen — the blob is already the right face. Only the generic
  Multiple-Master rung records them, because its design space is how weight is
  applied at all.
- **The Multiple-Master width solve was built, tested and never driven**, and
  the render burn-down's wave 5 found out. `GlyphSource::mm_instance` is a
  faithful port of `AdjustVariationParams` (`cfx_face.cpp:1561-1605`) and
  `GlyphKey` carries `dest_width` precisely so a caller can drive it — but
  `pdfrum-render` built every key through `GlyphKey::plain`, which hardcodes
  zero, so `mm_instance` took its `dest_width == 0` early-out on every glyph
  in the corpus and the generic was always drawn at its axis defaults.

  The consequence is not subtle. `CPDF_Font::GetCharPosList`
  (`cpdf_font.cpp:440-444`) passes `/Widths` down as `dest_width` for any font
  that is neither embedded nor CID, and the generics are MM Type 1 faces, so
  *every non-embedded font in the corpus* is meant to be drawn at the width
  its PDF declares. On `5.5_simple_font.pdf`, whose `/Widths` deliberately say
  `a = 800, b = 100, c = 400` against face advances near 450, the outlines
  overran their advances and merged — which the render triage recorded as two
  separate defects, a dropped character and an advance bug, and which was
  neither. See `docs/status/pdfrum-render.md`, wave 5.

  Two tests now pin it here rather than only at the call site: the solve lands
  exactly on a requested advance inside the axis, and saturates rather than
  degenerating outside it, because the interpolated design *coordinate* is
  unclamped by design while the blend then clamps it — which is what
  `FT_Set_MM_Design_Coordinates` does.
- **The Adobe CourierStd `+= 31` rescue** has no general rule behind it: four
  literal font names and a magic offset. Reproduced, and pinned by the oracle's
  own `Bug920636` assertions.
- **`kWeightPow11` has three descending steps** at indices 52, 59 and 63. They
  look like transcription errors frozen into PDFium's output; they are the only
  three descents in the whole ramp and a test pins each one so a future reader
  cannot "fix" them.
- **The Japan1 transform's byte unpacking splits at 255, not 256**, so byte
  `0xFF` is exactly zero rather than a small negative — which is what makes the
  table's `0xFF` entries mean "no offset".
- **`UseCIDCharmap` asks for Johab, not Wansung.** A Korean CID font is driven
  through encoding id 6, so one carrying only the far commoner Wansung subtable
  (id 5) falls through to Unicode instead. It reads like an oversight; it is
  what the oracle does, and a test pins all four legacy ids.

## The OQ-3 oracle probes

The brief escalated two `/ToUnicode` behaviors as *measurement, not decision*.
Both were measured with `pdfium_test --txt` over purpose-built one-page PDFs
(the generator is `scripts/probe-tounicode.py`), and **both contradict the
brief**.

**Probe method.** Each PDF shows a Type0/Identity-H font whose `/ToUnicode`
stream is the exact program under test, at charcodes chosen to expose the
answer. The oracle's UTF-32LE output is then read back directly. The oracle
was run with the determinism recipe of PLAN.md §4.

### OQ-3(a) — `Lookup(0x20)` for `<0010><00ff><fff0>`

**Answer: one `U+0000` character, not an empty sequence.**

```
codes shown: 0x0010 0x001F 0x0020 0x0011
oracle --txt: FEFF FFF0 001F 0000 FFF1
```

The value at code 0x20 is `0x10000`, whose low half is 0, and `Lookup` returns
a one-character string holding NUL. The 0x001F in the output is *not* the map's
answer — it is the text layer's `unicode += charcode` fallback firing on the
genuinely empty result at code 0x1F, which is the U+FFFF indicator collision.
So the brief is right that `Lookup(0x1F)` is empty and wrong that `Lookup(0x20)`
is; the C++ unittest's `EXPECT_EQ(L"", ...)` reads that way because a
`WideString` holding a single NUL compares equal to `L""` under its own
comparison, not because the result is empty.

### OQ-3(b) — `StringDataAdd`'s carry

**Answer: the carry arm never fires, and is unreachable from any input.**

```
program: 1 beginbfrange <0030> <0032> <0041FFFF> endbfrange
oracle --txt: FEFF 0041 FFFF 0041 10000 0041 10001
```

The brief describes a base-65536 increment in which `0xFFFF` wraps to 0 and
carries, emitting U+0000 and lengthening the string on a full carry. It does
not: PDFium's `wchar_t` is **32 bits** on the platform the oracle is built for,
so `0xFFFF + 1` is `0x10000` — larger, not wrapped — and `ch < str[i-1]` is
false. And since `StringToWideString` emits a fresh unit every four hex digits,
no element can start above `0xFFFF` either, so nothing can reach the carry.
The destination units are consequently stored as `u32`, not `u16`.

## Brief errata

Five statements in `docs/design/pdfrum-font.md` are wrong about the oracle. The
code follows the oracle.

1. **§1.6.1 / §1.6.6, `Lookup(0x20)`** — resolved by OQ-3(a) above: one NUL,
   not empty. The brief declined to pick; the measurement picks.
2. **§1.6.6, `StringDataAdd`** — resolved by OQ-3(b) above: base-2³², no
   reachable carry, and the string never lengthens. The brief's worked examples
   (`[0xFFFF] → [0x0001, 0x0000]` and the two-unit carries) are all wrong.
3. **§1.10.3, "150 entries"** — `kJapan1VerticalCIDs` has **154**. Counted
   twice against the C++ source, independently of the extractor. The last row
   is `{8819, 0, 129, 127, 0, 218, 108}`.
4. **§1.7 / §3.2, "nine code→Unicode tables"** — there are **eight**, and one
   of them is not where the brief says: `kPDFDocEncoding` is defined in
   `core/fpdfapi/parser/fpdf_parser_decode.cpp`, not in `cpdf_fontencoding.cpp`.
   (The brief's own parenthetical concedes the count.)
5. **§1.14 / §3.1, the `GlyphSource` sketch** — a `FontRef` cannot hold a bare
   CFF, so the enum's `Fontations` arm needed the two-backend `Face` described
   above. The brief records the fact in §1.16.3 while sketching a shape that
   contradicts it.

Two further notes, not errata: the `cmap` crate's `Words` lexer terminates on a
`/Name` that runs to end-of-data, so a `/Adobe-Japan1-UCS2` with no following
token sets nothing — real CMaps always have the `usecmap` operand after it. And
a `format 0` cmap subtable returns `Some(0)` for an unmapped code where format 4
returns `None`; the ladders test `!= 0` so the distinction is invisible to them,
but it is recorded in the fixtures' provenance.

## Tests

`cargo nextest run -p pdfrum-font`: **337** tests (333 unit + 4 integration).
`cargo test --doc -p pdfrum-font`: **9** doctests.

The brief's §4 plan is ported:

- **`cpdf_tounicodemap_unittest.cpp`, all 13 tests** — `StringToCode`'s 18
  rows, `StringToWideString`'s 14, both count checks, all six high-code-mask
  cases, the mismatched bracket, the U+FFFF collision, the three
  `InsertIntoMaps` collision scenarios and the non-BMP round trip.
- **`cfx_standardfont_unittest.cpp`, all 5** — plus our own: the alias table has
  exactly 89 entries, every alias resolves, every canonical name round-trips,
  and the intra-family Regular/Bold/BoldOblique/Oblique order holds.
- **`cfx_substfont_unittest.cpp`, all 5** — `EffectiveSkew`, `EffectiveWeight`,
  `EmboldenLevels` including the i64-overflow case and the MM zeroing,
  `EstimatedStemV`, `IsActualFontLoaded`.
- **`cfx_fontmapper_unittest.cpp`** — the end-to-end
  `FindSubstFaceForRegularStandardFontWithBoldWeight` trace, asserting the
  known-buggy weight 700 with its `crbug.com/500640684` citation, and
  `SetSubstFontNameWhenGetFaceNameFails`.
- **`cpdf_cidfont_unittest.cpp` `Bug920636`** — the CourierStd rescue, with the
  U+2212 MINUS in the encoding name that makes the lookup fail.
- **`cpdf_simplefont_unittest.cpp` `BaseFontNameWithSubsetting`**.
- **`cpdf_truetypefont_unittest.cpp` `AllUnicodeCmapsTreatedEqually`**, over
  regenerated fixtures rather than copied bytes.
- **`cfx_folderfontinfo_unittest.cpp` `TestFindFont`**'s name rule — including
  the rows that matter most, `"Book"` not matching `"Bookshelf Symbol 7"` while
  `"Oxygen"` matches `"Oxygen-Sans"`.
- **`fx_font_unittest.cpp`**'s two AGL tables, 7 rows each.

Plus the ladders, which the C++ has no unit coverage for at all: the Type 1
branches, the TrueType rungs including the fall-through to identity, the CID
ladder's two halves, `/Widths` and `/W` damage, the `/Encoding` decision table,
and `never_panic` sweeps over every parse entry point with malformed input.

**Integration** (`tests/corpus.rs`, 4 tests): loads real corpus PDFs through
`pdfrum-parser` and checks that every character the oracle's `--txt` extracted
from a page is reachable through some font on that page — 8 of 8 goldens
checked. Plus: 133 fonts across 178 corpus files load and decode without
panicking, and 48 Unicode-compatible fonts each map at least one code.

## `[spec]` changes

Five shape corrections to SPEC §6, recorded there in the implementing commit:
the two-backend `Face` behind `GlyphSource::Fontations` plus a `None` variant;
`CharItem::has_glyph`; `SimpleFont`'s `SimpleWidths` field; `load`'s signature;
and the five public items `pdfrum-doc` asked for. Plus five open questions
resolved by implementation — OQ-3 (measured), OQ-4 (always unhinted), OQ-6(a)
(`skip_font_enumeration` defaults false), OQ-6(b) (`SimilarityScore` ported),
OQ-7 (no new `Limits` field).

`pdfrum-common` gains **7 `DiagKind` variants**, which is the sanctioned
additive mechanism for a `#[non_exhaustive]` enum documented as growing:
`ToUnicodeBlockRejected`, `ToUnicodeLoneSurrogate`, `FontProgramUnreadable`,
`CidToGidStreamShort`, `FontWidthsTruncated`, `GsubUnreadable`,
`FontSubstitutionFailed`.

## Divergences

The brief's D1–D13 are implemented as written, with these notes:

- **D3** (unpaired surrogates → U+FFFD) as designed, with a diagnostic. No
  corpus file has tripped it in the integration sweep.
- **D7 / OQ-6(a)**: `skip_font_enumeration` is a public
  `SubstitutionOptions` field defaulting to **`false`** — the oracle's
  enumeration behavior, since conformance measures against the oracle. The
  brief's architecturally-honest `true` is one struct-update away, and a test
  pins that the two settings really do produce different weights.

  **2026-08-29 — the default was correct and irrelevant, because nothing was
  enumerating.** `--font-dir` and `--croscore-font-names` were parsed as
  accepted-but-unimplemented in `pdfrum-tool`, so `SubstitutionOptions` was
  never built with a non-default value and `load_with_options` had no callers
  outside this crate's tests. Every substitution ran against an empty
  database and short-circuited at step 7 into the built-in generics. The
  options are now threaded tool → `BuildContext` → `load_with_options`; the
  Croscore rename (`croscore_name`) is ported from
  `testing/test_fonts.cpp:19-45` and applied at the request boundary, where
  the C++ wrapper applies it. Text Tier-A on non-empty pages: 98.3% → 99.1%,
  `pixel-fail` 553 → 495, no regressions. See `docs/status/pdfrum-text.md`.

- **`SystemFontDb::scan` sorts its faces by name.** The C++'s `font_list_` is
  a `std::map<ByteString, ...>` (`cfx_folderfontinfo.h:98`) and two decisions
  read it in order: a face displaces the incumbent only by scoring *strictly*
  higher, so a `similarity_score` tie goes to whichever came first, and rung 5
  takes the first face claiming the charset outright. `fontdb` enumerates in
  directory order, which would make both answers depend on the filesystem.
- **D8** (artificial emboldening) deferred to M3 as the brief proposes: the
  tables and levels are computed and unit-tested against the oracle's own
  assertions, and `pdfrum-render` applies them. `SubstFont::embolden_level_for_render`
  and `_for_load` are public for that purpose.
- **D13 / OQ-7**: **no new `Limits` field.** `kOutOfSpecBFLimit` (160 000) and
  `kMaxType3FormLevel` (4) are ported verbatim as constants because they are
  rejections real files depend on; the `/W` and `bfchar` collections are already
  bounded by `Limits::max_array_len`, so a `max_cid_width_records` would be a
  second cap on the same thing.
- **New:** the `GSUB` reader takes only *single* substitutions from vertical
  features, and reads the script-reachable set first with a whole-list fallback
  — PDFium's policy, not OpenType's. A substitution to glyph 0 is "no
  substitution", because the C++ tests the result against zero.

## What is deliberately absent

- **Apple-only paths** (D1): CoreText branches, `ext_gid_`, the
  `kGlyphNameSubsts` ligature remap. The oracle is built on Linux.
- **Windows codepage conversion** (D2): the `kFX_CharsetUnicodes` tables and
  the `FX_MultiByteToWideChar` tails. The static-table scan is normative.
- **Knockout, and the `tricky` font list** (OQ-4). Hinting was here too until
  burn-down wave 7b measured what it is worth inside the oracle's glyph-bitmap
  raster; see the note at the end of this entry.

  **2026-08-29 — OQ-4 was reopened by the orchestrator and re-closed on
  measurement. The ruling stands, but the *reason* recorded above was wrong,
  and the corrected reason is the useful part.** The claim was that the only
  hinting path needs a face that is both SFNT and on FreeType's `tricky`
  list. That is true of the oracle's **outline** path
  (`CFX_Face::LoadGlyphPath`, `cfx_face.cpp:948-951`, whose predicate really
  is `!IsTtOt() || !IsTricky()`), but the oracle does not reach that path for
  ordinary text. Below `|char2device.a| + |char2device.b| > 50` it renders
  **glyph bitmaps** instead (`cfx_renderdevice.cpp:1240-1250`), and that path
  hints every SFNT face:

  ```cpp
  // cfx_face.cpp:841-843, CFX_Face::RenderGlyph
  int load_flags = FT_LOAD_NO_BITMAP | FT_LOAD_PEDANTIC;
  if (!IsTtOt()) {                       // !(face_flags & FT_FACE_FLAG_SFNT)
    load_flags |= FT_LOAD_NO_HINTING;
  }
  ```

  No `FT_LOAD_TARGET_*` is ever passed, so the target is `NORMAL` by
  omission; the autofitter is compiled out of pdfium's FreeType
  (`ftmodule.h`), and `FT_Property_Set` is never called, so the TrueType
  bytecode interpreter runs at its v40 default. So the oracle **is** hinting,
  on a much wider set of faces than OQ-4 assumed — and hinting it is still not
  worth porting, for a reason nobody had measured:

  **The oracle hints at a fixed 64 ppem, never at the device size.**
  `CFX_Face::New` calls `FT_Set_Pixel_Sizes(face_rec, 64, 64)` once
  (`cfx_face.cpp:376`) and nothing ever changes it; the per-glyph size arrives
  through `FT_Set_Transform` with the matrix pre-divided by 64
  (`cfx_face.cpp:822-825`). FreeType applies the transform *after* hinting, so
  the bytecode interpreter always grid-fits to a 64-pixel grid whose alignment
  is then scaled away. Measured with `skrifa` 0.46.2's `HintingInstance`
  (`Engine::Interpreter`, `Target::Smooth`/`Normal`) over three real TrueType
  faces — the `/FontFile2` from `example_063.pdf`, the one from `en_fqa.pdf`,
  and DejaVuSans/LiberationSerif:

  | face | hinted at 64 ppem, scaled to 9 pt | hinted at 9 ppem |
  |---|---|---|
  | `en_fqa` `/FontFile2` | mean 0.040 device px | mean 0.306 device px |
  | DejaVuSans | mean 0.037 device px | mean 0.260 device px |
  | LiberationSerif | mean 0.049 device px | mean 0.619 device px |

  Hinting in the oracle's own configuration moves outline points by about
  **1/25 of a device pixel** — an order of magnitude less than hinting at the
  real size, and far too little to be the pixel tail. (The
  `example_063` face is more pointed still: its `prep` program fails in
  skrifa's interpreter with `InvalidDefinition`, which under
  `FT_LOAD_PEDANTIC` is exactly the case where `cfx_face.cpp:849-857` reloads
  the glyph *unhinted*.)

  Porting the hinter would therefore buy a sub-1/20-pixel geometry change.
  The tail it was supposed to close is elsewhere; see
  `pdfrum-render.md`'s wave 4 section for what it actually is.

  **2026-08-29 — OQ-4 is now closed the other way, and the measurement above
  is why it took three waves.** Burn-down wave 7b built the oracle's glyph
  *bitmap* pipeline — the outline hinted at 64 ppem, rasterized three times as
  wide, FIR5-filtered, gamma-averaged — and inside it the sub-1/20-pixel
  geometry change is worth **up to 10 counts per pixel** on a 6 pt stem. A
  3×-wide grid resolves a third of the horizontal displacement an ordinary one
  does, and the filter then spreads that difference over five columns.

  Nothing above is retracted. Hinting alone really does move points by ~0.04
  device px, and hinting alone really would not have closed the tail; the
  error was measuring one stage of a four-stage pipeline against the finished
  output of a different one. `Face::hinted_outline` and `Font::hinted_glyph_path`
  are the port, gated exactly as `RenderGlyph` gates it — SFNT faces only,
  `Engine::Interpreter` because the oracle's FreeType has the autofitter
  compiled out, and a `None` fallback to the unhinted outline wherever the
  interpreter refuses the face, which is upstream's `FT_LOAD_PEDANTIC` retry.
  The instance is memoized per face in a `OnceLock`: building one costs ~50 µs
  and rebuilding it per glyph made a text page 30% slower than not hinting at
  all. See `pdfrum-render.md`'s wave 7b section.
- **CFF/Type 2 charstrings, OpenType wrapping, TTC parsing** — `read-fonts`
  owns all of those.

## Not yet done

- **`cargo-fuzz` targets** `tounicode_parse` and `font_load` (§4.7). The
  never-panic sweeps cover the same entry points deterministically and run in
  CI; the 24-hour run is an M2 exit criterion, and the `fuzz/` workspace is
  being stood up by a concurrent agent.
- **`insta` snapshots** of ~20 corpus fonts (§4.7). The per-ladder tests and the
  integration sweep cover the same ground; a snapshot would mostly re-encode
  them, and the highest-value version of this waits for `pdfrum-text` so the
  dump can include extracted text.
- **Tier-B differential against the oracle's pixels** for the substitution
  cluster. Needs the render stack; the conformance clusters `font-subst` and
  `vertical` are where it lands.
- **The M2 exit criterion itself** — `--txt` Tier-A byte-exact on ≥ 98% of the
  corpus — needs `pdfrum-page` and `pdfrum-text`. This crate's integration
  tests check the half that is ours: that the mapping can produce every
  character the oracle extracted.
