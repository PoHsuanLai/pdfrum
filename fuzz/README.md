# Fuzz ring

33 `cargo-fuzz` targets. Own workspace: `libfuzzer-sys` links C++, which the
root `cargo tree` check forbids.

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
bash fuzz/seed-corpus.sh
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run parser_load corpus/parser_load -- -max_total_time=60
```

A crash is a library bug. Fix it with a regression test and keep the input
under `fuzz/seeds/<target>/`.

CI typechecks (`cargo check --manifest-path fuzz/Cargo.toml`); it does not
run. `scripts/fuzz-gate.nu` runs the parser-facing 18. `page_*`, `text_*`,
`edit_*` are extra — `cargo +nightly fuzz run` them, or pass `--targets`.

```sh
scripts/fuzz-gate.nu                 # 10 min smoke
scripts/fuzz-gate.nu 3600
scripts/fuzz-gate.nu 86400 parallel  # 24 h, one process per target
```

Stream-decoding targets run under `Limits` with `max_decoded_stream_len` =
1 MiB (production is 1 GiB). The rest are panic-only.

## Targets

| Target | Entry | Gate | Seeds |
|---|---|---|---|
| `object_decode_text` | `decode_text` | yes | committed |
| `object_name_decode` | `name_decode` / `name_encode` | yes | committed |
| `crypt_encrypt_dict` | `SecurityHandler::from_encrypt_dict` | yes | committed |
| `crypt_decrypt` | `SecurityHandler::decrypt` | yes | committed |
| `filters_flate` / `_lzw` / `_a85` / `_ahx` / `_rle` / `_predictor` / `_chain` | the named decoder | yes | committed |
| `cmap_embedded` / `_predefined` | CMap parse / lookup | yes | committed |
| `parser_lexer` / `_xref` / `_load` / `_load_password` | lexer, xref, `load` | yes | committed; also take oracle PDFs |
| `parser_object` | object grammar | yes | committed |
| `page_parse_content` | `parse_content` | | |
| `page_inline_image` | `BI … ID … EI` | | |
| `page_colorspace` | `load_colorspace` | | |
| `page_psengine` | type 4 PostScript calculator | | |
| `page_mesh_stream` | mesh shading types 4–7 | | |
| `page_decode_image` | `decode_image` | | |
| `page_jbig2` | `decode_jbig2` | | |
| `page_jpx` | `decode_jpx` | | |
| `text_extract` | `extract` | | |
| `text_links` | web / mail link scan | | |
| `edit_save_roundtrip` | `save` then `load` | | oracle PDFs |
| `edit_subset` | `subset` | | committed (`tiny.ttf`) |
| `edit_import` | `import_pages` / `n_page_to_one` | | oracle PDFs |
| `doc_pdfa_check` | `pdfa::check` | | oracle PDFs |

`seeds/` is committed. `corpus/` is gitignored working state that
`fuzz/seed-corpus.sh` rebuilds from those plus, when present, the oracle's
`testing/resources` and `testing/corpus` PDFs. Provenance:
[`seeds/PROVENANCE.md`](seeds/PROVENANCE.md).
