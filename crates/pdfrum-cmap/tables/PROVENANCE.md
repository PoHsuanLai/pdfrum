# `cmaps.bin` — provenance

`cmaps.bin` is generated data, committed to the repository. This file records
where it came from, how to reproduce it, and how to prove it is right.

## What it holds

The charcode→CID tables of the 59 built-in CJK CMaps across four character
collections, and the CID→Unicode table of each collection.

| collection | index entries | CID→Unicode entries |
|---|---|---|
| Adobe-GB1 | 14 | 30 284 |
| Adobe-CNS1 | 14 | 19 088 |
| Adobe-Japan1 | 20 | 15 444 |
| Adobe-Korea1 | 11 | 18 352 |

**630 716 bytes** (616 KiB): 286 836 `u16` of word records, 6 064 `u16` of
four-byte records, 83 168 `u16` of CID→Unicode data, and ~2 KiB of index.

Several index entries share one data array — the UTF-16 spellings alias the
UCS-2 ones — and the blob dedupes by source symbol, so those entries point at
the same bytes exactly as they do in the source.

## Source

| | |
|---|---|
| Project | PDFium |
| Path | `core/fpdfapi/cmaps/{CNS1,GB1,Japan1,Korea1}/` |
| Revision | `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28) |
| License | BSD-3-Clause (`LICENSE` at the PDFium root) |
| Upstream origin | Foxit Software, contributed to PDFium in 2014 |

The tables are themselves derived from Adobe's published CMap resources for the
four character collections. Redistribution is covered by the BSD-3-Clause terms
above; the workspace's `deny.toml` allowlist includes BSD-3-Clause for this
reason.

## Regenerating

```sh
PDFRUM_REGEN_CMAP_TABLES=1 \
PDFRUM_ORACLE_CHECKOUT=/path/to/pdfium-c++ \
  cargo build -p pdfrum-cmap
```

`build.rs` does nothing at all without `PDFRUM_REGEN_CMAP_TABLES`, so an
ordinary build needs no C++ checkout and no network — which is the point of
committing the blob rather than generating it every time.

Generation is deterministic: the same source produces a byte-identical blob,
so a regeneration that changes anything is a real change and shows up as a
binary diff plus a readable diff of `cmaps.manifest.json`, which lists every
symbol, record count, type and chain offset.

## What the generator refuses to do

`build.rs` checks, and aborts the build naming the symbol on any failure:

- every declared `word_count` / `dword_count` matches both the values found and
  the array's own `[N * K]` dimension;
- every array is **sorted on exactly the key the runtime's binary search
  compares** — `high` for range records, `code` for single records,
  `(hi_word, lo_word_high)` for four-byte records;
- every `use_offset` chain stays inside its table, terminates, and never
  cycles;
- every CID→Unicode table has the expected length and U+FFFD at index 0.

A sortedness violation is **escalated, never repaired**. The runtime reproduces
the original's binary search on the original's ordering, so silently re-sorting
a table would change the CIDs real files resolve to.

## Verifying

`verify_blob.py` re-reads the committed blob with a separate parser, in a
different language, and compares every value against the C++ it claims to come
from — so a bug shared between the writer and the Rust reader cannot hide:

```sh
python3 crates/pdfrum-cmap/tables/verify_blob.py [/path/to/pdfium-c++]
```

Last run: all 292 900 `u16` values byte-identical to the source.

The Rust side re-checks the structural invariants against the *committed* blob
rather than the generator's inputs — record counts, sortedness, chain
termination, name uniqueness, and the aliased arrays' byte identity — in
`src/blob.rs`'s tests, so `cargo nextest run` catches a corrupted blob even
with no C++ checkout present.

## Files

| file | |
|---|---|
| `cmaps.bin` | the blob, read by `src/blob.rs` via `include_bytes!` |
| `cmaps.manifest.json` | one reviewable row per index entry |
| `verify_blob.py` | independent verifier against the C++ source |
| `../build.rs` | the generator |
