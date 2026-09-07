# pdfrum-tool

The oracle-mirror binary: a CLI that reproduces `pdfium_test`'s flags, output
formats and byte layouts so the conformance harness can run one command
against both engines and diff the results. Not a human interface — that is
[`pdfrum-cli`](https://github.com/PoHsuanLai/pdfrum/blob/main/crates/pdfrum-cli/README.md).

```bash
pdfrum-tool --show-metadata input.pdf
pdfrum-tool --txt input.pdf          # UTF-32LE, as the oracle writes it
pdfrum-tool --png --md5 input.pdf
pdfrum-tool --annot input.pdf
```

Fidelity to the oracle is the whole specification, including the parts that
are not obviously right. `--txt` writes UTF-32LE because `pdfium_test` does.
`--md5` hashes the raw bitmap rather than the encoded PNG, because
`pdfium_test` does, and a hash of the encoder's output would compare encoders
instead of rasterizers.

Unimplemented raster flags are accepted and no-op with one line on stderr
rather than exiting non-zero. A harness run over a fixed pass set otherwise
loses the page counts for every file that happens to carry an unsupported
flag, which turns a missing feature into missing data.

`publish = false`: this crate is harness infrastructure and has no meaning
outside the repository.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
