# Fuzz seeds

Committed: 247 files, ~64 KB. `fuzz/corpus/` is gitignored working state
that `fuzz/seed-corpus.sh` rebuilds from these plus, when present, the
oracle's PDF sets.

## Oracle PDFs — BSD-3-Clause

From `$PDFRUM_ORACLE_CHECKOUT` (default `<repo>/../pdfium-c++`):

| source | files | licence |
|---|---|---|
| `testing/resources/**/*.pdf` | 287 | BSD-3-Clause (`pdfium-c++/LICENSE`) |
| `testing/corpus/**/*.pdf` | 836 | BSD-3-Clause (`testing/corpus/LICENSE`) |

`seed-corpus.sh` copies them into the whole-file targets, named by content
hash. Committed here is a curated 38 so the seed set covers the structural
space without the oracle:

- `parser_load/` (31) — one per structural feature
  (`hello_world_compressed_stream.pdf`, `hello_world_split_streams.pdf`,
  `hello_world_2_pages.pdf`), the deliberately damaged ones
  (`bad_dict_keys.pdf`, `bad_page_type.pdf`, `bug_xrefv4_loop.pdf`,
  `empty_xref.pdf`, `parser_rebuildxref_error_notrailer.pdf`,
  `trailer_unterminated.pdf`, `trailer_as_hexstring.pdf`), and the small
  crash-regression bugs (`bug_113`, `bug_298`, `bug_325_a`, `bug_343`,
  `bug_344`, `bug_355`, `bug_360`, `bug_451830`, `bug_454695`, `bug_544880`,
  `bug_782596`, `bug_1324189`, `bug_1327884`, `bug_1328389`).
- `parser_load_password/` (14) — all seven `encrypted_*.pdf` fixtures, each
  wrapped twice in the target's input format (empty password, and a supplied
  one).

The `.pdf` files under `parser_load/` are byte-identical copies. The
`parser_load_password/` seeds prepend the target's three-part header.

## Hand-written

Everything else was written for this workspace from each entry point's
branch structure.

## Regression

| seed | what it pinned |
|---|---|
| `crypt_encrypt_dict/regression_int_range` | `parse_int` saturating an over-wide integer token past `INT_RANGE` |
| `filters_chain/regression_int_range` | the same bug, via `/Columns 999999999999999999999999` |
| `parser_xref/offset_past_eof` | not a bug — an xref offset outside the file, which the table stores |
| `parser_xref/freed_objstm_archive` | not a bug — an object-stream archive a later section freed |
| `filters_chain/regression_chain_amplifies` | not a bug — `/Filter [/FlateDecode /RL /RL /RL /RL]` turning 210 bytes into 7.4 MB, every stage inside its own cap |
