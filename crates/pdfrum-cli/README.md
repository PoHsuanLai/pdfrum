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
pdfrum extract words paper.pdf --json           # every word: page, box, font, size, character range
pdfrum extract markdown paper.pdf > paper.md    # tags if present, typography if not
pdfrum extract links report.pdf --json
pdfrum extract toc report.pdf
pdfrum extract attachments invoice.pdf -o attachments/
pdfrum extract annotations reviewed.pdf
pdfrum extract signatures signed.pdf
pdfrum preview report.pdf --page 3           # the page, in the terminal
pdfrum view report.pdf                        # a pager: pages pre-rendered, j/k or arrows, / find, +/- zoom, q
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
pdfrum inspect object report.pdf 12 --json      # the object as JSON: names, strings, refs, a stream's summary
pdfrum inspect xref report.pdf                  # where every object lives and what it is, and the trailer
pdfrum inspect revisions edited.pdf             # the incremental-update history
pdfrum inspect revision edited.pdf --rev 1 -o original.pdf
pdfrum inspect structure tagged.pdf             # the structure tree, indented
pdfrum diff v1.pdf v2.pdf                       # text per page; exit 1 if they differ
pdfrum diff v1.pdf v2.pdf --visual -o diffs/    # pixels too, changes in red
pdfrum hash report.pdf                          # sha256, /ID, and a semantic hash
pdfrum schema extract words                     # the JSON shape a command's --json prints; no argument lists them
pdfrum completions zsh > ~/.zfunc/_pdfrum
pdfrum manpage -o man/

# Composition: `-` is stdin or stdout, several files at once, one object per line.
curl -s https://example.com/report.pdf | pdfrum info -
pdfrum pages slice report.pdf --pages 1 -o - | pdfrum extract words - --json | jq '.[0]'
pdfrum info *.pdf --json | jq '.[].pages'          # one document per file
pdfrum search "total" a.pdf b.pdf                  # a.pdf:page 2:… as grep -H does
pdfrum extract links report.pdf --jsonl | jq -c 'select(.kind == "uri")'
PDFRUM_PASSWORD=secret pdfrum extract text locked.pdf   # the password out of the history
pdfrum -q optimize big.pdf -o -  > small.pdf      # no notices; errors still print
pdfrum -v info damaged.pdf                         # every parser diagnostic, one per line
```

Every command takes `--password` for an encrypted file — or reads
`PDFRUM_PASSWORD` when the flag is absent — and prints notices on stderr,
never on stdout; `--quiet` drops the notices and `--verbose` lists every
parser diagnostic on open. Every reading command's FILE accepts `-` for
stdin, every writing command's `-o` accepts `-` for stdout (refused on a
terminal), and `info`, `hash`, `doctor` and `search` take several files, a
file that cannot be opened reported and skipped. `--json` output has
`snake_case` keys that do not change between releases; on several files it
is an array of the per-file documents. The commands whose answer is a list
also take `--jsonl`, one compact object per line. `pdfrum schema <command>`
prints an example of a command's `--json` document with every key, and
`pdfrum schema` alone lists the commands that have one. Exit codes: 0, 1 on error
(including a file among several that failed), 2 for a usage mistake, 3
from `doctor --strict`.

Human output follows `docs/design/cli-style.md`: one of four forms (a
record of key/value lines, a table with an uppercase header row, sections
per page, or a one-line summary for a written file), a fixed palette of
colour roles that is off in a pipe and under `NO_COLOR`, sizes in human
units, `no <things>` for an empty answer, and never an emoji.

## JavaScript

A default build runs no script. Build with the feature to get two more
things:

```sh
cargo install --path crates/pdfrum-cli --features javascript
pdfrum scripts run form.pdf --time 1700000000   # what the document's scripts said, one line each
pdfrum forms fill form.pdf --data v.json --scripts -o out.pdf   # then run the open scripts and keep what they assigned
```

`scripts run` runs the document's open-time scripts and then opens every
page the way a viewer does, and prints the transcript: every alert and
console line, nothing added, so it can be diffed against PDFium's own
expected files. `--time` freezes the scripts' clock. `forms fill
--scripts` saves the values, then opens the saved file the same way and
writes back whatever the scripts assigned to fields.

The feature is the facade's `javascript` feature: a pure-Rust engine
(boa) that is about 137 more crates and 12 MB more binary, and that
executes script out of untrusted documents — which is why it is off by
default, here as in the library.

The crate is a client of the `pdfrum` facade only — what it can print is
what `cargo add pdfrum` can reach. `pdfrum-tool` is a different binary: the
oracle mirror the conformance harness diffs against `pdfium_test`, and not
for people.
