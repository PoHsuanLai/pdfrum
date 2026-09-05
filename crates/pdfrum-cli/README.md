# pdfrum-cli

The `pdfrum` command line: inspect, render and extract from PDF files, on
the pure-Rust `pdfrum` engine.

```sh
pdfrum info report.pdf                 # pages, metadata, security, signatures, identity
pdfrum info report.pdf --json          # the same as one JSON document
pdfrum doctor damaged.pdf --strict     # what the parser recovered; exit 3 if anything
pdfrum render report.pdf --pages 1-3 --dpi 200 -o out/{stem}-{n}.png
pdfrum extract text report.pdf --pages 2,5-end
pdfrum extract text --layout two-column.pdf     # columns stay columns
pdfrum extract markdown paper.pdf > paper.md    # tags if present, typography if not
pdfrum extract links report.pdf --json
pdfrum extract toc report.pdf
pdfrum extract attachments invoice.pdf -o attachments/
pdfrum extract annotations reviewed.pdf
pdfrum extract signatures signed.pdf
pdfrum preview report.pdf --page 3           # the page, in the terminal
pdfrum view report.pdf                        # a pager: j/k, /find, q
pdfrum search -i "total due" invoice.pdf      # grep for a document
pdfrum pages merge a.pdf b.pdf -o both.pdf
pdfrum pages split both.pdf -o pages/          # one file per page
pdfrum pages slice report.pdf --pages 2-4 --rotate 90 -o part.pdf
pdfrum pages reorder report.pdf --pages 3,1,2 -o reordered.pdf
pdfrum pages create scan1.jpg scan2.png --dpi 300 -o scans.pdf
pdfrum pages nup slides.pdf --grid 2x2 --sheet a4 -o handout.pdf
pdfrum pages booklet zine.pdf -o print-me.pdf
pdfrum forms dump form.pdf --json
pdfrum forms fill form.pdf --data values.json -o filled.pdf
pdfrum forms flatten filled.pdf -o static.pdf
pdfrum repair damaged.pdf -o fixed.pdf
pdfrum optimize big.pdf --deterministic -o small.pdf
pdfrum security decrypt locked.pdf --password secret -o open.pdf
pdfrum security encrypt open.pdf --user-password read --owner-password admin --allow print -o locked.pdf
pdfrum extract images brochure.pdf -o images/   # one row per picture, repeats counted, spacers left out; --all for every draw
pdfrum extract fonts brochure.pdf -o fonts/     # the embedded programs, .ttf/.cff/.otf/.pfb
pdfrum inspect object report.pdf 12             # one object, highlighted, each reference told what it is
pdfrum inspect object report.pdf 12 --decode > content.txt
pdfrum inspect xref report.pdf                  # where every object lives and what it is, and the trailer
pdfrum inspect revisions edited.pdf             # the incremental-update history
pdfrum inspect revision edited.pdf --rev 1 -o original.pdf
pdfrum inspect structure tagged.pdf             # the structure tree, indented
pdfrum diff v1.pdf v2.pdf                       # text per page; exit 1 if they differ
pdfrum diff v1.pdf v2.pdf --visual -o diffs/    # pixels too, changes in red
pdfrum hash report.pdf                          # sha256, /ID, and a semantic hash
pdfrum completions zsh > ~/.zfunc/_pdfrum
pdfrum manpage -o man/
```

Every command takes `--password` for an encrypted file and prints notices
on stderr, never on stdout. `--json` output has `snake_case` keys that do
not change between releases. Exit codes: 0, 1 on error, 2 for a usage
mistake, 3 from `doctor --strict`.

Human output follows `docs/design/cli-style.md`: one of four forms (a
record of key/value lines, a table with an uppercase header row, sections
per page, or a one-line summary for a written file), a fixed palette of
colour roles that is off in a pipe and under `NO_COLOR`, sizes in human
units, `no <things>` for an empty answer, and never an emoji.

The crate is a client of the `pdfrum` facade only — what it can print is
what `cargo add pdfrum` can reach. `pdfrum-tool` is a different binary: the
oracle mirror the conformance harness diffs against `pdfium_test`, and not
for people.
