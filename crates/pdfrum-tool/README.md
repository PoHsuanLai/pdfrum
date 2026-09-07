# pdfrum-tool

CLI that mirrors `pdfium_test` flags and output, for the conformance harness.

```bash
pdfrum-tool --show-metadata input.pdf
pdfrum-tool --txt input.pdf          # UTF-32LE
pdfrum-tool --png --md5 input.pdf
pdfrum-tool --annot input.pdf
```

`--md5` hashes the raw bitmap, as the oracle does. Unimplemented raster
flags are accepted and no-op (one stderr line) so a fixed pass set still
reports page counts.

Not the human CLI — that is `pdfrum-cli`. `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
