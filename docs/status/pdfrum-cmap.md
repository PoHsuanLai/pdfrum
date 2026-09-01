# `pdfrum-cmap` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

Fourth library crate of M1/M2. Contract: SPEC.md §6; behavior:
`docs/design/pdfrum-cmap.md`.

## What landed

The whole of SPEC §6's `pdfrum-cmap` half — the built-in CJK tables and their
generator, predefined-name resolution, the byte decoder, `CMap::decode`, and
the embedded-CMap program reader — in nine modules plus the committed blob.

| module | contents |
|---|---|
| `lib.rs` | `CMap` and its 12 methods, `predefined`, `from_encoding_name`, `parse_embedded`, `unicode_from_cid`, `has_cid2unicode`, `charcode_from_unicode`, `charset_from_ordering` |
| `ids.rs` | `CharCode`, `Cid`, `CidSet`, `CidCoding`, `CodingScheme` |
| `error.rs` | `Error` (one variant — almost nothing here can fail) |
| `blob.rs` | zero-copy checked reader over `include_bytes!("../tables/cmaps.bin")` |
| `static_lookup.rs` | the `use_offset` chain walk, both binary searches, the reverse scan |
| `predefined.rs` | the 32-row name table, the two-byte truncation, leading-byte sets |
| `decode.rs` | `Decoder`, `LeadingBytes`, `CodeRange`, `next_char`/`count_chars`/`char_size`/`append_char` |
| `lexer.rs` | `Words`, the public CMap-program tokenizer (re-used by `pdfrum-font`'s `ToUnicode` reader) |
| `parser.rs` | the `ParseWord` state machine, `get_code`, `get_code_range`, `DirectTable` |
| `cid2unicode.rs` | collection CID → `char` |
| `build.rs` + `tables/` | the generator, the 616 KiB blob, its manifest, provenance and independent verifier |

### The behaviors that mattered most

- **The name truncation is blind.** `GetPredefinedCMap` removes the last two
  bytes of any name longer than two and then compares — it does not look for
  `-H`/`-V`. Two of the brief's worked examples were wrong about the
  consequences and the tests now pin the real ones (see *Brief corrections*).
- **Two tables, keyed differently.** The 32-row decoder table is keyed by the
  truncated stem; the blob's CID tables by the *full* name. So a damaged name
  can land a correct decoder and no CID map — `GB-EUCXY` splits bytes exactly
  like `GB-EUC-H` while mapping every code to itself. That half-configured
  state is reachable, observable, and tested.
- **The `use_offset` chain is per-entry, not per-chain.** `UniCNS-UTF16-V`
  holds range records and defers to a *single*-record table; `UniJIS-UCS2-HW-H`
  has four records and defers two rows back to another single table;
  `KSCpc-EUC-H` hops six rows. All four shapes have tests that prove the CID
  really came from the far link.
- **The four-byte lookup's missing high-word recheck.** The dword comparator
  keys on `(hi_word, lo_word_high)` but the hit test only checks the low range,
  so a record from a *different* high word can match. Reproduced with a comment
  saying why, not tightened.
- **`CheckFourByteCodeRange`'s tolerance arm.** A partial prefix match whose
  input is already as wide as the range is reported as a *complete* code, so
  `81 FF` against `<8140>`–`<9FFC>` decodes as `0x81FF` rather than collapsing
  to single bytes. Ranges are also searched backwards, so a later declaration
  shadows an earlier one.
- **The lone-codespace-range quirk.** A block declaring exactly one range keeps
  only its *width* and throws the bounds away — and a width of 3 or 4 yields a
  **one**-byte scheme, since only exactly 2 maps to two bytes. Blocks
  accumulate across the program, so the scheme is re-decided each time.
- **`usecmap` is a no-op**, per SPEC §6's 2026-08-29 ruling: an embedded CMap
  inheriting `/GBK-EUC-H` gets *none* of it. Recorded as a diagnostic so the
  loss is visible. The `use_offset` chain is the "usecmap" the contract means.

  *Superseded 2026-09-02 (audit A1): this is an **oracle bug**, not a
  behaviour to match.* `cpdf_cmapparser.cpp:61` is an empty `else if` and the
  `/UseCMap` dictionary key is read nowhere in `core/fpdfapi/`, so both of
  ISO 32000-1 §9.7.5.3's inheritance channels are dead in the oracle and an
  inheriting CMap loses every base code to CID 0 — total loss, not
  degradation. pdf.js implements both, dictionary-key-first, child-wins
  (`cmap.js:608-613`, `:639-648`, `extendCMap` `:650-669`). Both channels now
  work here: the operator is resolved in `parse_embedded`, the dictionary key
  by `pdfrum-font`'s `use_cmap_parent` through `inherit_from`, which
  supersedes the operator. Codespace ranges are inherited only when the child
  declares none, and the chain is depth-guarded at
  `Limits::max_name_tree_depth`. Blast radius measured at **0 rows** — no
  corpus file uses either channel. `CMapUsecmapIgnored` retired in favour of
  `CMapUsecmapUnknown` and `CMapUsecmapDepth`; SPEC §6 amended in the same
  commit; the sites carry `// [oracle-bug]`.
- **`/Ordering (Japan1)` sets nothing.** The operand reader is a blunt
  two-byte slice, so a parenthesised PostScript string never matches a
  collection name — embedded CMaps effectively never set their own charset and
  the font's `/CIDSystemInfo` decides. Resolves the brief's OQ-3 by reading the
  code rather than probing: the slice is `word[2..]` and `(Japan1)[2..]` is
  `apan1)`. Both halves are pinned.
- **Nothing fails.** Decoding a truncated code yields character code 0 and
  terminates iteration; a program of pure garbage yields a working two-byte
  CMap; an unknown `/Encoding` yields an identity fallback. Eight new
  `DiagKind` variants make each recovery visible.

## The table blob

`tables/cmaps.bin`, **630 716 bytes** (616 KiB), committed with its generator
(`build.rs`), a per-entry JSON manifest, `PROVENANCE.md`, and an independent
verifier — as SPEC §6 requires. Regeneration is opt-in
(`PDFRUM_REGEN_CMAP_TABLES=1`), so an ordinary build needs no C++ checkout, and
it is deterministic: regenerating from the same source reproduces the blob
byte for byte.

**Verification is three-layered**, deliberately:

1. **At generation.** `build.rs` asserts every record count against both the
   values found and the array's declared `[N * K]` dimension, asserts
   sortedness on exactly the key each binary search compares
   (`high` / `code` / `(hi_word, lo_word_high)`), and asserts every chain is
   in-bounds, terminating and acyclic. A sortedness violation aborts the build
   and is escalated, never repaired — the runtime must reproduce the original's
   search on the original's ordering.
2. **Independently, in another language.** `tables/verify_blob.py` re-reads the
   committed blob with a separate parser and compares every value against the
   C++, so a bug shared between the generator and the Rust reader cannot hide.
   Last run: **all 292 900 `u16` values byte-identical** to the source
   (286 836 word + 6 064 dword, plus the 83 168 CID→Unicode entries).
3. **In the test suite, against the committed blob.** `blob.rs`'s tests
   re-check record counts, sortedness, chain termination, name uniqueness,
   U+FFFD at CID 0, and the byte identity of the six aliased array pairs — with
   no C++ checkout needed, so CI catches a corrupted blob on its own.

## Tests

`cargo nextest run -p pdfrum-cmap`: **101** unit tests.
`cargo test --doc -p pdfrum-cmap`: **13** doctests.
Workspace additions: `pdfrum-common` gains 8 `DiagKind` variants and one
`Limits` field (one assertion updated, no new tests).

The brief's §4 plan is ported, including every quirk pin it asked for: the two
`GetCode`/`GetCodeRange` unit-test tables from the oracle (11 and 6
assertions), the Korea1 six-row hop, both mixed-type chains, the four-byte
tolerance arm, the reversed range, the CID truncation, last-wins overwrite, the
`additional`-mappings gate, keyword-resets-state, and the lexer's edge cases.

Not yet written, and the reason: **`insta` snapshots** of all 34 names — the
per-name assertions plus the blob-integrity tests cover the same ground for now
and a snapshot would mostly re-encode them; **fuzz targets** `cmap_embedded`
and `cmap_lexer` — the `fuzz/` workspace does not exist yet (PLAN.md §5), and
the two properties they would check (termination, and tokens staying inside the
input) have deterministic tests here in the meantime; **conformance clusters** —
the harness has no corpus wired yet.

## `[spec]` changes

**SPEC §1 (`pdfrum-common`), two additive changes**, both inside what §1
already declares as growing:

- `DiagKind` gains eight variants: `CMapNameUnknown`, `CMapTableMissing`,
  `CMapUsecmapIgnored`, `CMapCodespaceDropped`, `CMapTruncatedCodespace`,
  `CMapReversedRange`, `CMapWideMappingsDropped`, `CMapRangeLimit`,
  `CMapOperandOverflow`. The enum is `#[non_exhaustive]` and documented as
  growing per crate, so this is the sanctioned mechanism.
- `Limits` gains `max_cmap_ranges` (default **65 536**), resolving the brief's
  OQ-4 in its proposed direction. PDFium caps neither the codespace-range list
  nor the wide-code mapping list; exceeding the cap drops further ranges with a
  diagnostic rather than erroring, matching the accepted
  `max_decoded_stream_len` precedent. No real CMap approaches it.

Nothing in SPEC §6 needed correcting: its 2026-08-29 rulings on `usecmap` (D1 /
OQ-1) and on committing the blob (OQ-2) are what the crate implements.

## Brief corrections

Four statements in `docs/design/pdfrum-cmap.md` are wrong about the oracle. The
code follows the oracle; the brief should be amended.

- **§1.1, the truncation examples.** The brief says `"GB-EUC-XY"` matches row 0
  and `"UniJIS-UCS2-HW"` matches row 23. Neither does: the cut is blind, so
  they become `"GB-EUC-"` and `"UniJIS-UCS2-"` — trailing hyphen included — and
  match nothing. Whether damage lands on a row turns on the name's length
  parity. `"GB-EUCXY"` is the example that does match `GB-EUC`. Both the
  §4.3 table row and the "garbage-name length-3" quirk pin are affected.
- **§1.1 header, "33 entries".** The table has **32** rows (the brief's own
  listing runs 0–31), plus the two Identity names handled by short-circuit.
- **§1.5, the lexer.** The delimiter and whitespace sets are right as far as
  they go, but PDFium's character table also classifies **`0x80` and `0xFF` as
  whitespace**, so those bytes separate words. Also `<` at end of data returns
  the one-byte token `"<"`, not an empty one.
- **§2 D2, `GetCodeRange`'s bounds.** The premise is wrong: `ByteStringView`'s
  `operator[]` is `CHECK`ed, so an out-of-range read would crash, not return a
  NUL. It is provably unreachable — `char_size` is derived from the same index
  the scan stopped at, and the last read is at `2 * char_size ≤ len - 1` for
  every length and every `>` position. The clamping is still right, but it
  guards nothing the original reaches, and the divergence should be withdrawn
  rather than described as safety-only.

One finding the brief does not mention: **`CNS-EUC-H` and `CNS-EUC-V` are
unreachable.** They have static tables — including two of the three four-byte
tables — but no row in the name table, and `CNS-EUC-H` truncates to `CNS-EUC`,
which no row carries. No `/Encoding` spelling can reach them. They are shipped
anyway so the blob stays a faithful copy, and a test pins the unreachability.

## Divergences from the oracle

- **D2 withdrawn** (above): no divergence, the clamped reads are unreachable.
- **D3** (`code_points` overflow guard) kept as written: unreachable through
  the original's own control flow, present so a fuzzer cannot find a way.
- **D5** (no global registry) as designed: the tables are a blob addressed by
  `CidSet`, and `predefined` is a free function over it. No process state.
- **D6 resolved as "no divergence"**: an embedded CMap always allocates its
  dense table, matching the original, because `has_no_direct_table()` — which
  the CID font's glyph lookup reads — must keep distinguishing embedded CMaps
  from predefined ones.
- **New, documented:** `char_size` and `append_char` disagree in exactly one
  case, and the original disagrees the same way. Under a mixed-two-byte scheme
  a code below `0x100` whose value is a lead byte reports width 1 but is
  written as two bytes. Relatedly, that code space has two holes — such a code,
  and a two-byte code whose high byte is not a lead byte — which the decoder
  can never produce and the encoder cannot round-trip. Both are pinned.

## Notes for `pdfrum-font`

- `lexer::Words` is public and is the tokenizer the `ToUnicode` reader needs;
  do not write a second one. It is *not* the content-stream lexer.
- `has_no_direct_table()` is the predicate the CID glyph ladder branches on:
  true for every predefined CMap, false for every embedded one.
- `charcode_from_unicode` is the O(collection) reverse scan, and it is the
  non-Windows path deliberately — the code-page conversion the oracle uses on
  Windows is permanently out of scope.
- `parse_embedded` takes already-decoded bytes; fetching the `/Encoding` stream
  and running its filter chain belongs to `pdfrum-font`, which owns the
  resolver.
