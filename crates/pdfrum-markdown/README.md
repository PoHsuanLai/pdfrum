# pdfrum-markdown

Markdown from a page. A tagged PDF (ISO 32000-1 §14.8) names headings,
paragraphs, lists, tables and figures in its structure tree; an untagged
one is read by typography (body size, heading scale, lists, code fences,
running headers dropped).

`document_blocks` reads a whole document and drops lines that repeat in the
header/footer band.

Part of [pdfrum](https://crates.io/crates/pdfrum). Facade feature `markdown`.

MIT OR Apache-2.0
