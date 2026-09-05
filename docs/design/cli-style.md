# `pdfrum` output style

The rules every command's human output follows. They are the modern
command-line conventions — the ones `gh`, `cargo`, `rg`, `kubectl` and the
Command Line Interface Guidelines (clig.dev) converge on — pinned so that
forty commands read as one tool. `crates/pdfrum-cli/src/out.rs` and
`term.rs` implement them; a command prints through those helpers and
through nothing else.

## 1. Streams and modes

- **stdout is the answer, stderr is commentary.** Data, tables, records,
  summaries go to stdout. Notices (a rebuilt cross-reference table),
  warnings, progress and errors go to stderr, so `| head`, `> file` and
  `$(…)` never see them.
- **`--json` replaces the human output with one JSON document** carrying
  the same facts under stable `snake_case` keys. Nothing is printed for
  people in JSON mode. Every command whose output is data has it.
- **Streams are raw.** `extract text`, `extract markdown`, `inspect object`,
  `--decode`, `render -o -` and `completions` write their bytes and add
  nothing: no heading, no colour, no trailing summary. Two streams are
  read by people as often as by diffs — the object dump and the script
  transcript — and those paint their tokens on a terminal only, leaving
  the bytes in a pipe untouched.
- **A closed pipe is not an error.** Exit 0, quietly.
- **Composition.** `-` is stdin wherever a FILE is read and stdout
  wherever `-o` names a file to write; a file written to stdout is refused
  on a terminal, and its summary line becomes a stderr notice, `pdfrum: -:
  3 pages`, so stdout is the file and nothing else. The commands that
  answer about a document — `info`, `hash`, `doctor`, `search` — take
  several files: one record per file with a blank line between, `--json`
  an array of the per-file documents, `search` naming the file before the
  page as `grep -H` does; a file that cannot be opened is reported on
  stderr and the run continues, exit 1 at the end. A command whose answer
  is a list also has `--jsonl`, one compact object per line. `--quiet`
  drops the notices, `--verbose` prints every parser diagnostic.

## 2. Colour

Colour is on when stdout is a terminal, off in a pipe, off under
`NO_COLOR` or `TERM=dumb`, and forced either way by `--color always|never`.
It never carries information that the plain text lacks.

Only these roles exist. Nothing else is ever painted, and a role is never
reused for a different meaning:

| Role | SGR | Used for |
|---|---|---|
| `Heading` | bold | section headings: `page 3`, `trailer` |
| `Key` | dim | the key column of a record, the header row of a table |
| `Ident` | cyan | things a person would copy: paths, page numbers, object numbers, field names, font names |
| `Muted` | dim | secondary detail after the main fact: sizes, counts, offsets |
| `Ok` | green | a good state: `clean`, `allowed`, `valid` |
| `Warn` | yellow | a state to notice: `suspicious`, `denied`, `rebuilt` |
| `Error` | bold red | an error line (stderr only) |
| `Match` | bold yellow | the matched text in `search` |
| `Added` / `Removed` | green / red | `diff` lines |
| `Bar` | inverse | the pager's status bar, and nothing else |
| `Link` | underlined cyan | the text of a hyperlink, so it reads as one at rest; terminals mostly mark OSC 8 links on hover only |

No 256-colour or truecolor for text (pictures use truecolor; that is a
picture). No background colours. No blinking. **No emoji, anywhere, ever** —
not in output, not in notices, not in help text. No box-drawing frames, no
ASCII art, no banners, no spinners.

## 3. Layout: four forms, nothing else

Every human output is one of these.

**Record** — one thing, its properties. `info`, `hash`, `doctor`'s summary.
Two columns: the key, left-aligned and padded to the longest key in the
block plus two spaces, in `Key`; the value, plain. Keys are lowercase
words with spaces, no colon. A missing value is the word `none`, never a
blank. Multi-line values continue under the value column.

```
file         report.pdf
version      1.7
pages        12
page size    595.28 x 841.89 pt
security     none
```

**Table** — many things of one kind, one per line. Links, annotations,
fonts, images, fields, attachments, cross-reference entries, revisions,
notices. A header row first, uppercase, in `Key`. Columns separated by two
spaces, text left-aligned, numbers right-aligned, the last column never
padded. Identity columns (`PAGE`, `NAME`, `OBJ`) come first, detail
columns last, and a column whose every cell would be empty is dropped.
The header is printed in a pipe too, so `awk 'NR>1'` is the whole recipe.

```
PAGE  KIND  RECT                            TARGET
   1  page  72.75 465.14 543.05 496.34      page 2
   1  uri   72.75 433.94 543.05 465.14      https://example.com
```

**Section** — a table or record repeated per page (or per signature): a
`Heading` line, `page 3`, then the rows indented by two spaces. Pages with
nothing in them are skipped.

**Summary** — one line, for a command that wrote a file: the path in
`Ident`, a colon, then what was done and how much, the detail in `Muted`.

```
out.pdf: 3 pages
locked.pdf: encrypted, AES-256, print allowed
```

## 4. Words

- Lowercase throughout, except proper names (`AES-256`, `TrueType`) and
  the table header. No trailing full stops on lines.
- **Headers and keys are plain words**, never the specification's
  abbreviations: `OBJECT` not `OBJ`, `WHERE` and `byte 471` not `PLACE`
  and `@471`, `CONTENT ID` not `MCID`, `document id` not `id`, `table`
  not `xref`. Most readers have not read ISO 32000, and the ones who have
  lose nothing. The specification's own names appear only as values — an
  object's `/Type`, a filter's name — because there they are the fact.
- **Nothing found is one line, `no <things>`**: `no images`, `no fonts`,
  `no attachments`, `no bookmarks`, `no form fields`, `no signatures`,
  `no annotations`, `no notices`. Exit 0 — an empty answer is an answer.
  The one exception is `search`, which is `grep`: exit 1 on no hit.
- Numbers: integers unadorned, thousands unseparated (they are copied
  into scripts). Points to two decimals. Sizes in human units — `3 B`,
  `1.2 KB`, `24.3 KB`, `1.8 MB` — with the exact byte count in `--json`.
  Percentages to two decimals.
- Page numbers are 1-based everywhere a person sees them and 0-based in
  no place they see.
- Rectangles are `x0 y0 x1 y1` in points, y-up, as the file has them.
- Written paths are printed as given, not canonicalised.

## 5. Notices and errors

- A notice is `pdfrum: <file>: <what happened>` on stderr, one line.
- An error is `pdfrum: <what failed>: <why>` on stderr, in `Error`, and
  the exit code is 1. The cause chain reads left to right, outermost
  first: `pdfrum: cannot open a.pdf: wrong password`. When one flag would
  fix it, the message names it.
- A usage mistake is clap's message and exit 2. `doctor --strict` exits 3
  when it found anything. `diff` exits 1 when the documents differ.

## 6. Links and pictures

- With hyperlinks on (a terminal, or `--hyperlinks always`), page targets,
  URIs and written paths are OSC 8 links whose text is unchanged.
- Pictures (`preview`, `view`) use the protocol the terminal announces and
  fall back to half-blocks; `--graphics` overrides.

## 7. What a new command must do

1. Decide its form from §3. If it is none of the four, the form is wrong,
   not the rule.
2. Print through `out::record`, `out::Table`, `out::heading`,
   `out::summary`, `out::none`, or raw `out!`/`outln!` for a stream.
3. Paint only through `Term::paint(Style, …)`; never write an escape.
4. Carry a `--json` twin with the same facts.
5. Pin the text output in `tests/expected/` and the JSON keys in a test.

## 8. The session

`pdfrum serve --stdio` is the commands as JSON-RPC 2.0 methods, one
request and one response per line. The rules that keep it one tool:

- **A method is a command.** Its params are the command's flags under
  the same names, its result is the command's `--json` document exactly;
  a method with a shape of its own (`render`, `image`, the writers) is
  listed with that shape in `rpc::methods` and printed by `pdfrum schema
  serve`. A command that gains `--json` gains a method, and a method
  gains an entry in the table and a run in `tests/serve.rs`.
- **Stdout is the wire.** Responses and nothing else; notices are off,
  and `--verbose` logs to stderr only.
- **A method writes no file.** Bytes go back as `bytes_base64`; a
  picture as `png_base64`. The rows that carry `written` on the command
  line never do here.
- **An error is the CLI's line under a code.** `-32602` when the request
  is wrong, `-32000` when the work failed, the message `pdfrum: …` as
  `out::error_line` builds it — no second wording of any mistake.
