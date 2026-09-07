# Roadmap

pdfrum reads, repairs, renders, extracts, edits and signs PDFs. It is not a
typesetting system. Markup-to-layout is Typst's problem.

Shipped: canvas (`pdfrum::Canvas`), SVG export and ingestion, PDF/A, the C
ABI, WASM, and the `pdfrum` CLI.

Open work is in the issue tracker. Strictly behind a peer, nothing bought
in exchange:

- **Warm median render** vs mupdf (`vello_cpu`'s scalar `F32Kernel::pack` /
  `unpack`, filed upstream).

Public-key (`Adobe.PubSec`) encryption is not implemented.
