# pdfrum-tool

**CLI mirroring the pdfium_test oracle's flags and output formats.**

A command-line PDF dumper: render pages to PNG, extract text, and print a
file's metadata, page geometry, tagged structure tree and annotations.

Its flags, its output formats, its stdout and stderr chatter and its file
naming are all a deliberate mirror of PDFium's own `pdfium_test` harness tool,
byte for byte where it matters, so that a differential test harness can run one
fixed set of passes against either binary and diff like for like.

```bash
cargo install pdfrum-tool

pdfrum-tool --show-metadata input.pdf
pdfrum-tool --show-pageinfo input.pdf
pdfrum-tool --show-structure input.pdf
pdfrum-tool --txt input.pdf                 # <input>.<page>.txt, UTF-32LE
pdfrum-tool --png --md5 input.pdf           # <input>.<page>.png, plus MD5 lines
pdfrum-tool --annot input.pdf               # <input>.<page>.annot.txt
pdfrum-tool --password=secret --pages=0-3 --show-pageinfo input.pdf
```

`--md5` hashes the **raw bitmap buffer** rather than the PNG, as the oracle
does. The other raster targets (`--ppm`, `--bmp`, `--skp`, the PostScript
family) and the `--save-*` family are *accepted and do nothing*, on purpose: a
harness runs one fixed pass set against every candidate, and a tool that
rejected a flag would report a tool error instead of an unimplemented tier —
and would lose the page counts the same invocation reports. Each such flag
prints one line on stderr naming what would supply it.

## Part of pdfrum

`pdfrum-tool` is the CLI of [pdfrum](https://crates.io/crates/pdfrum), a
pure-Rust PDF engine — it is a thin shell over the `pdfrum-parser`,
`pdfrum-page`, `pdfrum-render`, `pdfrum-text` and `pdfrum-doc` libraries. To do
any of this from Rust rather than a shell, use the `pdfrum` facade crate.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
