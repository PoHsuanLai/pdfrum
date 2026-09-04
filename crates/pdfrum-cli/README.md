# pdfrum-cli

The `pdfrum` command line: inspect, render and extract from PDF files, on
the pure-Rust `pdfrum` engine.

```sh
pdfrum info report.pdf                 # pages, metadata, security, signatures, identity
pdfrum info report.pdf --json          # the same as one JSON document
pdfrum doctor damaged.pdf --strict     # what the parser recovered; exit 3 if anything
pdfrum render report.pdf --pages 1-3 --dpi 200 -o out/{stem}-{n}.png
pdfrum extract text report.pdf --pages 2,5-end
pdfrum extract links report.pdf --json
pdfrum extract toc report.pdf
pdfrum extract attachments invoice.pdf -o attachments/
pdfrum extract annotations reviewed.pdf
pdfrum extract signatures signed.pdf
```

Every command takes `--password` for an encrypted file and prints notices
on stderr, never on stdout. `--json` output has `snake_case` keys that do
not change between releases. Exit codes: 0, 1 on error, 2 for a usage
mistake, 3 from `doctor --strict`.

The crate is a client of the `pdfrum` facade only — what it can print is
what `cargo add pdfrum` can reach. `pdfrum-tool` is a different binary: the
oracle mirror the conformance harness diffs against `pdfium_test`, and not
for people.
