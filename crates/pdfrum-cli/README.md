# pdfrum-cli

![pdfrum CLI: info, doctor, extract, search](../../docs/assets/cli/pdfrum-cli.gif)

info · doctor · extract · search. Pages also draw in the terminal (`preview`, `view`).

```sh
pdfrum info report.pdf
pdfrum info report.pdf --json
pdfrum doctor damaged.pdf --strict
pdfrum render report.pdf --pages 1-3 --dpi 200 -o out/{stem}-{n}.png
pdfrum extract text report.pdf --pages 2,5-end
pdfrum extract text --layout two-column.pdf
pdfrum extract words paper.pdf --json
pdfrum extract markdown paper.pdf -o paper/
pdfrum extract links report.pdf --json
pdfrum extract toc report.pdf
pdfrum extract attachments invoice.pdf -o attachments/
pdfrum extract annotations reviewed.pdf
pdfrum extract signatures signed.pdf
pdfrum preview report.pdf --page 3
pdfrum view report.pdf
pdfrum search -i "total due" invoice.pdf
pdfrum pages merge a.pdf b.pdf -o both.pdf
pdfrum pages split both.pdf -o pages/
pdfrum pages slice report.pdf --pages 2-4 --rotate 90 -o part.pdf
pdfrum pages reorder report.pdf --pages 3,1,2 -o reordered.pdf
pdfrum pages create scan1.jpg scan2.png --dpi 300 -o scans.pdf
pdfrum pages nup slides.pdf --grid 2x2 --sheet a4 -o handout.pdf
pdfrum pages booklet zine.pdf -o print-me.pdf
pdfrum pages delete report.pdf --pages 3,7 -o fewer.pdf
pdfrum pages rotate scan.pdf --pages 2-end --by 90 -o upright.pdf
pdfrum metadata set report.pdf --title "Q3 report" --author "A. Person" -o titled.pdf
pdfrum attach add report.pdf data.csv --description "raw numbers" -o with-data.pdf
pdfrum stamp text draft.pdf DRAFT --angle 30 --opacity 0.3 -o stamped.pdf
pdfrum forms dump form.pdf --json
pdfrum forms fill form.pdf --data values.json -o filled.pdf
pdfrum forms flatten filled.pdf -o static.pdf
pdfrum repair damaged.pdf -o fixed.pdf
pdfrum optimize big.pdf --deterministic -o small.pdf
pdfrum security decrypt locked.pdf --password secret -o open.pdf
pdfrum extract images brochure.pdf -o images/
pdfrum extract fonts brochure.pdf -o fonts/
pdfrum inspect object report.pdf 12
pdfrum inspect xref report.pdf
pdfrum diff v1.pdf v2.pdf
pdfrum hash report.pdf
pdfrum serve --stdio
pdfrum serve --stdio --mcp
pdfrum completions zsh > ~/.zfunc/_pdfrum
```

`--password` or `PDFRUM_PASSWORD`. Notices on stderr. `-` is stdin/stdout.
`--json` / `--jsonl`. Exit 0, 1 error, 2 usage, 3 `doctor --strict`, 4 a
limit (`--max-pixels`, `--time-limit`).

Human output is a record, a table, a section, or a one-line summary. Colour
off in a pipe and under `NO_COLOR`.

`pdfrum serve --stdio` is JSON-RPC, one request/response per line, result =
the command's `--json`. `--mcp` is the same as MCP tools.

```sh
cargo install pdfrum-cli
cargo install pdfrum-cli --features javascript
pdfrum scripts run form.pdf
```

Facade client only. `pdfrum-tool` is the oracle-mirror binary, not this.

MIT OR Apache-2.0
