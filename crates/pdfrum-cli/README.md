# pdfrum-cli

![pdfrum CLI: preview, view, doctor, search, stamp](https://raw.githubusercontent.com/PoHsuanLai/pdfrum/main/docs/assets/cli/pdfrum-cli.gif)

The `pdfrum` command: inspect, render, extract, edit and repair PDF files
from a shell, and serve the same operations to an agent over JSON-RPC.

```sh
cargo install pdfrum-cli
```

```sh
pdfrum preview gradients.pdf
pdfrum search -i "total due" invoice.pdf
pdfrum stamp text draft.pdf DRAFT --angle 30 --opacity 0.3 -o stamped.pdf
pdfrum extract text report.pdf --pages 2,5-end
pdfrum doctor damaged.pdf --strict
```

## Commands

| | |
|---|---|
| `info` | pages, metadata, security, signatures, identity |
| `doctor` | what the parser recovered or dropped, without touching the file |
| `preview` | one page, drawn here in the terminal |
| `view` | read the document in the terminal: pages, search, zoom |
| `search` | every hit with its line and page, `grep`-style |
| `render` | pages to PNG files |
| `extract` | text, words, markdown, links, toc, images, fonts, attachments, annotations, signatures |
| `pages` | merge, split, slice, delete, rotate, reorder, nup, booklet, create from images |
| `attach` | add an embedded file, or take one out |
| `stamp` | text or a picture drawn over every page |
| `metadata` | set or clear the `/Info` keys |
| `forms` | `dump` the fields, `fill` them from JSON, `flatten` them into the page |
| `repair` | open with recovery, write a clean fully-rewritten file |
| `optimize` | rewrite compactly: unreferenced objects dropped, streams re-encoded |
| `security` | encrypt and decrypt |
| `inspect` | objects, the cross-reference table, revisions, structure |
| `diff` | text per page, and with `--visual` the pixels; exit 1 when they differ |
| `hash` | SHA-256, the trailer's `/ID`, and a semantic hash ignoring timestamps |
| `serve` | JSON-RPC 2.0 over stdin and stdout, or MCP tools |
| `schema` | the JSON shape a command's `--json` prints |
| `completions`, `manpage` | generated from the command tree |

`pdfrum <command> --help` is the full flag list for any of them.

```sh
pdfrum render report.pdf --pages 1-3 --dpi 200 -o out/{stem}-{n}.png
pdfrum extract text --layout two-column.pdf
pdfrum extract markdown paper.pdf -o paper/
pdfrum pages nup slides.pdf --grid 2x2 --sheet a4 -o handout.pdf
pdfrum pages booklet zine.pdf -o print-me.pdf
pdfrum pages create scan1.jpg scan2.png --dpi 300 -o scans.pdf
pdfrum forms fill form.pdf --data values.json -o filled.pdf
pdfrum security decrypt locked.pdf --password secret -o open.pdf
pdfrum inspect object report.pdf 12
pdfrum completions zsh > ~/.zfunc/_pdfrum
```

## Terminals

`preview` and `view` are only supported on terminals that can show a picture.
The mode is picked from the terminal's own announcements; `--graphics` sets it.

| terminal | `--graphics` |
|---|---|
| kitty, Ghostty, WezTerm, Konsole | `kitty` |
| iTerm2, VS Code, WezTerm | `iterm` |
| any 24-bit colour terminal — Alacritty, foot, GNOME Terminal, xterm | `halfblock` |
| a pipe, a file, `TERM=dumb` | `off` — use `render` for a PNG |

The first two send a PNG, at the terminal's own resolution. Half-blocks are
plain SGR colour, two pixels per cell, so they survive `ssh` and multiplexers.
Under tmux the inner `TERM` still names the outer terminal, so ask for
`halfblock` there unless APC passthrough is on.

```sh
pdfrum preview report.pdf --graphics halfblock --width 100
```

`view` is the pager: `j`/`k` for pages, `g`/`G` for first and last, `+`/`-`
and `0` for zoom, `/` and `n` to search, `q` to quit.

## Conventions

Stdout is data, stderr is commentary. `-` is stdin or stdout. `--json` is
one document; `--jsonl` is one compact object per line, for `jq -c` and
`xargs`. Human output is a record, a table, a section, or a one-line summary,
and colour is off in a pipe and under `NO_COLOR`.

A password comes from `--password` or `PDFRUM_PASSWORD`, so it need not land
in shell history; with neither, a terminal is asked once, silently.

| exit | |
|:---:|---|
| 0 | success |
| 1 | error — including `diff` on a difference |
| 2 | usage |
| 3 | `doctor --strict` found something |
| 4 | a limit: `--max-pixels`, `--time-limit` |

`--max-pixels` refuses an oversized render before anything is drawn, and
`--time-limit` stops work where it is. Both are global.

## For agents

```sh
pdfrum serve --stdio        # JSON-RPC 2.0, one object per line
pdfrum serve --stdio --mcp  # the same operations as MCP tools
```

Each method is a command and each result is that command's `--json`. A
document is parsed once per session rather than once per call.
`pdfrum schema serve` lists the methods.

## Features

| feature | default | adds |
|---|:---:|---|
| `javascript` | off | runs a document's own scripts, via the facade's feature |

```sh
cargo install pdfrum-cli --features javascript
```

This is a client of the [`pdfrum`](https://crates.io/crates/pdfrum) facade
and nothing else — it reaches no internal crate. `pdfrum-tool` is the
oracle-mirror binary for the conformance harness, not this.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
