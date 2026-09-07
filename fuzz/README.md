# Fuzz ring

Twenty `cargo-fuzz` targets over every byte-consuming entry point.
`fuzz/` is its own workspace: `libfuzzer-sys` links C++, which the root
`cargo tree` check forbids.

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
bash fuzz/seed-corpus.sh
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run parser_load corpus/parser_load -- -max_total_time=60
```

A reproducing crash is a library bug. Fix it with a regression test and keep
the input under `fuzz/seeds/<target>/`.

```sh
scripts/fuzz-gate.nu                 # 10 min smoke
scripts/fuzz-gate.nu 3600
scripts/fuzz-gate.nu 86400 parallel  # 24 h, one process per target
```

CI compiles the targets (`cargo check --manifest-path fuzz/Cargo.toml`);
it does not run them.

| Target | Entry |
|---|---|
| `object_decode_text` | `decode_text` |
| `object_name_decode` | `name_decode` / `name_encode` |
| `crypt_encrypt_dict` | `SecurityHandler::from_encrypt_dict` |
| `crypt_decrypt` | `SecurityHandler::decrypt` |
| `filters_flate` / `_lzw` / `_a85` / `_ahx` / `_rle` / `_predictor` / `_chain` | the named decoder |
| `cmap_embedded` / `_predefined` | CMap parse / lookup |
| `parser_lexer` / `_object` / `_xref` / `_load` / `_load_password` | lexer, object, xref, `load` |
| `text_extract` / `_links` | extract / link scan |

Every target runs under `Limits` with `max_decoded_stream_len` = 1 MiB
(production default is 1 GiB). Seeds: [`seeds/PROVENANCE.md`](seeds/PROVENANCE.md).
