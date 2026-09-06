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
pdfrum extract markdown paper.pdf -o paper/            # tags if present, typography if not; running headers dropped; -o writes paper/paper.md and its images
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
pdfrum pages delete report.pdf --pages 3,7 -o fewer.pdf
pdfrum pages rotate scan.pdf --pages 2-end --by 90 -o upright.pdf   # added to each page's own rotation
pdfrum metadata set report.pdf --title "Q3 report" --author "A. Person" --clear keywords -o titled.pdf
pdfrum attach add report.pdf data.csv notes.txt --description "the raw numbers" -o with-data.pdf
pdfrum attach remove with-data.pdf notes.txt -o report.pdf
pdfrum stamp text draft.pdf DRAFT --angle 30 --opacity 0.3 --size 96 --rgb cc0000 -o stamped.pdf
pdfrum stamp image report.pdf logo.png --width 72 --position top-right -o branded.pdf
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
pdfrum serve --stdio                            # a session: the commands as JSON-RPC methods, documents parsed once
pdfrum serve --stdio --mcp                      # the same as a Model Context Protocol tool set
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
pdfrum render --max-pixels 100M poster.pdf        # a page above 100 megapixels is refused, exit 4
pdfrum --time-limit 5s extract text huge.pdf      # five seconds for the whole command, then exit 4
```

Every command takes `--password` for an encrypted file — or reads
`PDFRUM_PASSWORD` when the flag is absent — and prints notices on stderr,
never on stdout; `--quiet` drops the notices and `--verbose` lists every
parser diagnostic on open. Two ceilings for untrusted input, both off by
default: `--max-pixels N` (`100M`, `2G`, or a number) refuses any render
that would come out larger — `render`, `preview`, `view`, `diff --visual`
— before a pixel is allocated, and `--time-limit DURATION` (`500ms`,
`5s`, `2m`) is the whole command's budget, after which the work stops
where it is. Either exits 4 with the library's one-line reason, distinct
from a broken file's 1, so a script can tell a refused job from a bad
one. Every reading command's FILE accepts `-` for
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

## A session for agents

`pdfrum serve --stdio` keeps documents open between requests and answers
the commands as JSON-RPC 2.0 methods over stdin and stdout, one request
and one response per line, each result the command's `--json` document:

```sh
$ pdfrum serve --stdio
{"jsonrpc":"2.0","id":1,"method":"open","params":{"file":"report.pdf"}}
{"jsonrpc":"2.0","id":1,"result":{"doc":1,"file":"report.pdf","pages":12,…}}
{"jsonrpc":"2.0","id":2,"method":"words","params":{"doc":1,"pages":"3"}}
{"jsonrpc":"2.0","id":2,"result":[{"page":3,"text":"Total","x0":72.0,…}]}
{"jsonrpc":"2.0","id":3,"method":"render","params":{"doc":1,"page":3,"dpi":100}}
{"jsonrpc":"2.0","id":3,"result":{"page":3,"width":850,"height":1100,"png_base64":"iVBOR…"}}
{"jsonrpc":"2.0","id":4,"method":"close","params":{"doc":1}}
{"jsonrpc":"2.0","id":4,"result":{"doc":1}}
```

`open` takes a path or `bytes_base64`; the writers (`pages.slice`,
`pages.merge`, `pages.delete`, `pages.rotate`, `forms.fill`,
`metadata.set`, `attach.add`, `attach.remove`, `stamp.text`,
`stamp.image`) hand the file back as `bytes_base64` and write nothing; `--max-docs` caps what is open at once; `shutdown` or
closing stdin ends the session. `--max-pixels` and `--time-limit` on
`serve` are the ceilings every `open` starts from, and `open` takes
`limits: {max_pixels, time_limit_ms}` over them per document — the
budget runs from that open, and every later call on the document stops
when it is spent. Errors are the CLI's messages under JSON-RPC codes
(`-32601` no such method, `-32602` the request is wrong, `-32000` the
work failed, `-32001` a limit refused it). `pdfrum schema serve` prints
every method with
its params as JSON Schema and its result shape.

With `--mcp` the same process speaks the Model Context Protocol, so an
agent host lists the methods as tools and calls them; this is the
configuration such a host wants:

```json
{"mcpServers": {"pdfrum": {"command": "pdfrum", "args": ["serve", "--stdio", "--mcp"]}}}
```

Tool names are the method names with `_` for `.` (`forms_dump`);
`render` and `image` return the picture as an image block beside the
JSON.

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
