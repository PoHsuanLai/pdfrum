# pdfrum-markdown

Markdown from a page. Tagged PDFs use the structure tree; untagged PDFs use
typography (body size, heading scale, lists, code fences, running headers
dropped).

`document_blocks` reads a whole document and drops lines that repeat in the
header/footer band.

Part of [pdfrum](https://crates.io/crates/pdfrum). Facade feature `markdown`.

MIT OR Apache-2.0
