# The pdfrum fuzz ring

Twenty `cargo-fuzz` targets over every byte-consuming entry point in the
parser-facing crates, plus the two that drive text extraction's geometry and
its string scanners, and the gate that PLAN.md §6 makes an M1 exit
criterion: *parser fuzzers running clean for 24h*.

## Why this is a separate workspace

`fuzz/` is **not** a member of the root workspace, and that is structural
rather than stylistic. DEPS.md's Pure-Rust guarantee is that no published
crate's dependency tree compiles C or C++; `libfuzzer-sys` links LLVM's C++
libFuzzer runtime. Keeping this directory out of the root `members` list is
what makes `cargo tree --workspace` in `scripts/ci.sh` stay clean — the
guarantee holds by construction, not by anyone remembering.

Two consequences worth knowing:

- The root workspace's lints (`unsafe_code = "forbid"`, `clippy::panic =
  "deny"`) are not inherited here. `unsafe_code = "forbid"` is re-declared in
  `fuzz/Cargo.toml` anyway, because no target needs it: `fuzz_target!` hands
  the body a safe `&[u8]`. The panic lints are deliberately *not* re-declared
  — a panic is what these targets exist to find.
- `cargo build`, `cargo test` and `scripts/ci.sh` at the repo root never see
  this directory. Building it is an explicit `cd fuzz`.

## Running

Requires a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
```

Then, from the repo root:

```sh
bash fuzz/seed-corpus.sh          # populate fuzz/corpus/ (see "Seeds" below)
cd fuzz
cargo +nightly fuzz list          # every target
cargo +nightly fuzz build         # build them all
cargo +nightly fuzz run parser_load corpus/parser_load -- -max_total_time=60
```

Reproducing a crash, and minimising it:

```sh
cd fuzz
cargo +nightly fuzz run   parser_load artifacts/parser_load/crash-<hash>
cargo +nightly fuzz tmin  parser_load artifacts/parser_load/crash-<hash>
```

A crash that reproduces is a bug in the library crate, not in the target:
every entry point in this ring is documented as either infallible or
`Result`-returning, and STYLE.md §3 forbids panics in library code outright.
Fix it in the crate with a regression test; keep the crashing input as a seed
under `fuzz/seeds/<target>/` so it is never lost.

## The M1 gate

`scripts/fuzz-gate.sh` runs every parser-facing target and fails on the first
crash.

```sh
scripts/fuzz-gate.sh                 # 10 minutes total — a smoke check
scripts/fuzz-gate.sh 3600            # an hour, split between the targets
scripts/fuzz-gate.sh 86400 parallel  # THE M1 GATE: 24h, one process per target
```

`parallel` gives each target the full budget and runs them at once, so
`86400 parallel` costs 24 hours of wall clock rather than 24 × 18. That is
the command PLAN.md's exit criterion means. It wants 18 cores and about 8 GB;
`FUZZ_TARGETS="parser_load parser_xref parser_object parser_lexer"` narrows it
to the four strictly-parser targets on a smaller machine.

Sequential mode divides the budget instead, which is what you want for a
quick pre-commit check.

The gate is **not** in `scripts/ci.sh`. The fast gate is seconds; this is
minutes to days.

## Targets

Each is named `<crate>_<entry point>`. The inventories come from the
deferred-fuzz notes in `docs/status/pdfrum-{object,crypt,filters,cmap,parser}.md`.

| Target | Entry point | What the input is |
|---|---|---|
| `object_decode_text` | `decode_text` | raw text-string bytes |
| `object_name_decode` | `name_decode` / `name_encode` | raw name bytes, round-tripped |
| `crypt_encrypt_dict` | `SecurityHandler::from_encrypt_dict` | file-id chunk, password chunk, then an `/Encrypt` dict *parsed by `pdfrum-parser`* |
| `crypt_decrypt` | `SecurityHandler::decrypt` | handler selector, object number, generation, then ciphertext |
| `filters_flate` | `decode_flate` | 4-byte size estimate, then deflate data |
| `filters_lzw` | `decode_lzw` | `/EarlyChange` byte, then LZW codes |
| `filters_a85` | `decode_ascii85` | raw |
| `filters_ahx` | `decode_ascii_hex` | raw |
| `filters_rle` | `decode_run_length` | raw |
| `filters_predictor` | `predictor` | kind, colors, bits-per-component, columns, then rows |
| `filters_chain` | `decode_chain` | 2-byte estimate, a stream dict, then the stream body |
| `cmap_embedded` | `parse_embedded` (+ the lexer behind it) | text chunk, then a CMap program |
| `cmap_predefined` | `predefined` / `from_encoding_name` | name selector, then text to decode |
| `parser_lexer` | `Lexer::next_word` / the string readers | raw |
| `parser_object` | `parse_object` | strictness byte, then an object body |
| `parser_xref` | `read_xref` | a whole file |
| `parser_load` | `load` | a whole file |
| `parser_load_password` | `load` with `/Encrypt` | password selector, password chunk, then a whole file |
| `text_extract` | `pdfrum_text::extract` and the query half | a whole file; the geometry reaching the heuristics is what is being fuzzed, not the bytes |
| `text_links` | `check_web_link` / `check_mail_link` | raw UTF-8 text, capped at 4 KiB |

Targets that need more than one byte string split the input with
`fuzz/src/lib.rs`'s `Split`: one length byte, then that many bytes, repeated,
with everything left over as the bulk payload. It is thirty lines instead of
an `arbitrary` dependency, and it keeps a crash file readable in a hex dump.

Every target runs under `Limits` with `max_decoded_stream_len` at **1 MiB**
(`pdfrum_fuzz::limits`), the figure `docs/status/pdfrum-filters.md` asks for.
The production default is 1 GiB; a fuzzer allowed to allocate that finds
libFuzzer's RSS killer, not bugs.

## Seeds

`fuzz/seeds/` is committed — 247 files, about 64 KB. `fuzz/corpus/` is
gitignored working state that `fuzz/seed-corpus.sh` rebuilds from the seeds
plus, when the oracle checkout is present, its full PDF sets.

### Provenance

The full accounting, with licences and per-file rationale, is in
[`seeds/PROVENANCE.md`](seeds/PROVENANCE.md). In short:

**Upstream ships no fuzzer corpora.** `pdfium-c++/testing/fuzzers/` has 50
`*_fuzzer.cc` files, a `BUILD.gn` that only compiles them, and no `.dict`,
no `seed_corpus =` attribute, and no `*_corpus/` directory anywhere outside
`third_party/` (harfbuzz and icu ship their own, for their own code). Chromium
holds pdfium's seed corpora and dictionaries out-of-tree, so there is nothing
to copy. What the fuzzer sources *did* give us is the shape of each entry
point and its input-size caps — `pdf_cmap_fuzzer.cc`'s 256 KiB ceiling, and
`pdf_streamparser_fuzzer.cc`'s read-until-null loop are both reflected above.

So the seeds are from two places:

1. **The oracle's PDF fixtures** — `pdfium-c++/testing/resources/*.pdf` (287
   files) and the `testing/corpus` submodule (836 files). These are BSD-3
   licensed alongside pdfium itself. `seed-corpus.sh` copies all 1168 into the
   four whole-file targets' corpora, named by content hash. A curated 38 are
   committed under `seeds/parser_load/` and `seeds/parser_load_password/`:
   one per structural feature (`hello_world_compressed_stream.pdf`,
   `hello_world_split_streams.pdf`), the deliberately broken ones
   (`bad_dict_keys.pdf`, `bug_xrefv4_loop.pdf`, `empty_xref.pdf`,
   `parser_rebuildxref_error_notrailer.pdf`, `trailer_unterminated.pdf`), and
   all seven `encrypted_*.pdf` fixtures, which are the only inputs that reach
   the `/R` 2–6 key-derivation ladders.

2. **Hand-written seeds**, generated for this workspace, covering the branch
   structure each entry point documents: every encoding `decode_text` sniffs
   a BOM for, every predictor tag, every `/Filter` shape `decoder_list`
   distinguishes, an `/Encrypt` dict per handler revision, a CMap program per
   coding scheme, and one per token class the lexer emits.

Two seeds are **regression seeds** — inputs that crashed during this
workspace's own bring-up, kept so they are re-run forever:
`seeds/crypt_encrypt_dict/regression_int_range` and
`seeds/filters_chain/regression_int_range` (see below), plus
`seeds/parser_xref/offset_past_eof`, `seeds/parser_xref/freed_objstm_archive`
and `seeds/filters_chain/regression_chain_amplifies`, which are not bugs but
pin damage-tolerant behaviours their targets document they do *not* assert
against.

## Bugs this ring has found

**`pdfrum-parser::syntax::parse_int` violated `pdfrum-object`'s `INT_RANGE`**
(fixed 2026-08-29). Found independently by `filters_chain`
(`/Columns 999999999999999999999999`) and `crypt_encrypt_dict`, both landing on
the `debug_assert!` in `as_c_int`.

`parse_int` accumulated into an `i64` and *saturated*, so an over-wide token
became `i64::MAX` — far outside the `-2^31 ..= 2^32-1` that `INT_RANGE`
documents as the only range a parse can produce, and the range every
`Object::Int` accessor is only faithful within. PDFium's `FX_Number` (in
`core/fxcrt/fx_number.cpp`) accumulates into a `uint32_t` and folds *to zero*
on overflow, with a second, tighter ceiling for a spelling that led with a
sign. `parse_int` now reproduces both, and `syntax.rs`'s
`integers_outside_the_c_int_range_are_zero` pins every boundary spelling.

Saturating was the more dangerous of the two behaviours, which is why it is
worth spelling out: it turns an absurd `/Columns` into a merely enormous one,
where folding to zero turns it into nothing at all.

## Assertions this ring got wrong

Not every crash is a bug in a crate; some are a target claiming a property the
library never promised. Three have been corrected so far — two during target
authoring, and one from the 24h gate:

**`filters_chain` asserted a whole-chain output cap that does not exist**
(corrected 2026-08-29, from the M1 gate). The target read
`Limits::max_decoded_stream_len` as a ceiling on `decode_chain`'s output. It is
not one: only Flate and LZW consult it. The filters brief's divergence D2
(`docs/design/pdfrum-filters.md`) makes RunLength's 20 MiB `kMaxStreamSize` a
*separate* filter-specific constant on purpose, because that cap is a rejection
PDFium really performs and files depend on it, and the ASCII filters have no
cap at all — `ASCII85Decode`'s `z` even spells four bytes in one.

So chains amplify, and `/Filter [/FlateDecode /RL /RL /RL /RL]` took 210 raw
bytes to 7,450,260 with every stage inside its own limit. The target now walks
the decoder list and composes each stage's real ceiling
(`chain_ceiling`) rather than asserting one constant, which still catches a
stage that ignores its own cap. Kept as
`seeds/filters_chain/regression_chain_amplifies`.
