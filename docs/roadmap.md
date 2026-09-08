# Roadmap

pdfrum reads, repairs, renders, extracts and edits PDF files, and reports the
signatures it finds without verifying them. It is not a typesetting system:
markup-to-layout is Typst's problem, and a document this library writes is one
it was given the pages for.

## Shipped

The canvas API (`pdfrum::Canvas`), SVG export and ingestion, PDF/A conversion
and conformance reporting, the C ABI, the WebAssembly binding, and the
`pdfrum` CLI.

## Open

Tracked as issues. Everything here is strictly behind a peer, with nothing
bought in exchange — a gap we can name and measure, not a feature we lack.

| | against | where it is |
|---|---|---|
| Warm median render | mupdf | `vello_cpu`'s scalar `F32Kernel::pack` / `unpack`, filed upstream |

## Not planned

| | |
|---|---|
| Public-key encryption (`Adobe.PubSec`) | ISO 32000-1 §7.6.4; the standard security handler is complete |
| XFA | superseded, and deprecated in PDF 2.0 |
| A viewer | no window, caret or widget chrome — an embedder owns those |
| Typesetting | see above |
