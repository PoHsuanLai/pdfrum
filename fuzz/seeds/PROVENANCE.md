# Fuzz seeds — provenance

The committed seed set: 247 files, about 64 KB. `fuzz/corpus/` is gitignored
working state that `fuzz/seed-corpus.sh` rebuilds from these plus, when the
oracle checkout is present, its full PDF sets. `fuzz/README.md` has the
per-target inventory; this file records where the bytes came from.

## Upstream ships no fuzzer corpora

`pdfium-c++/testing/fuzzers/` holds 50 `*_fuzzer.cc` files and a `BUILD.gn`
that only compiles them — deliberately, per its own comment: *"this only
compiles all the fuzzers, to prevent compile breakages. It does not link and
create fuzzer executables. That is done in Chromium."* There is no `.dict`
file, no `seed_corpus =` attribute, and no `*_corpus/` directory anywhere
outside `third_party/` (harfbuzz and icu ship their own, for their own code).
Chromium holds pdfium's seed corpora and dictionaries out of tree, so there
was nothing to copy.

What the fuzzer sources *did* supply is the shape of each entry point and its
input caps — `pdf_cmap_fuzzer.cc`'s 256 KiB ceiling and
`pdf_streamparser_fuzzer.cc`'s read-until-null loop are both reflected in our
targets.

## 1. Oracle PDF fixtures — BSD-3-Clause

From the read-only checkout at `/mnt/data2/pdfium/pdfium-c++`:

| source | files | licence |
|---|---|---|
| `testing/resources/**/*.pdf` | 287 | BSD-3-Clause (`pdfium-c++/LICENSE`) |
| `testing/corpus/**/*.pdf` (submodule) | 836 | BSD-3-Clause (`testing/corpus/LICENSE`) |

`seed-corpus.sh` copies all 1168 into the four whole-file targets' working
corpora, named by content hash so re-running never duplicates.

**Committed here** is a curated 38, chosen so the seed set alone covers the
structural space without the oracle checkout:

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
  one). These are the only inputs that reach the `/R` 2–6 key-derivation
  ladders, including the two `_bad_okey` variants.

The `.pdf` files under `parser_load/` are byte-identical copies. The
`parser_load_password/` seeds are the same PDFs with the target's
three-part header prepended, so they are not byte-identical.

## 2. Hand-written seeds

Everything else was written for this workspace, from each entry point's own
documented branch structure rather than from any upstream file: every
encoding `decode_text` sniffs a BOM for, every predictor tag and the
parameter sets that overflow a row-size product, every `/Filter` shape
`decoder_list` distinguishes, an `/Encrypt` dictionary per handler revision
and cipher, a CMap program per coding scheme, and one seed per token class
the lexer emits.

Regenerate with the two scripts recorded in `fuzz/README.md`'s history; they
are throwaway generators, not committed, because the seeds they produce are
the artifact and are small enough to read.

## 3. Regression seeds

Inputs that crashed during this workspace's bring-up, kept so they are re-run
forever:

| seed | what it pinned |
|---|---|
| `crypt_encrypt_dict/regression_int_range` | `parse_int` saturating an over-wide integer token past `INT_RANGE` |
| `filters_chain/regression_int_range` | the same bug, reached through `/Columns 999999999999999999999999` |
| `parser_xref/offset_past_eof` | *not* a bug — an xref offset outside the file, which the table stores by design |
| `parser_xref/freed_objstm_archive` | *not* a bug — an object-stream archive a later section freed, dropping the flag |
| `filters_chain/regression_chain_amplifies` | *not* a bug — `/Filter [/FlateDecode /RL /RL /RL /RL]` turning 210 bytes into 7.4 MB, every stage inside its own cap |

The last three are kept because they are the inputs that taught their targets
which properties they must **not** assert; see those targets' module docs.
