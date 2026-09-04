# Fixture provenance

The PDFs in this directory are copied verbatim from the PDFium checkout that
serves as this project's conformance oracle:

- **Source:** `testing/resources/` of the PDFium repository, at commit
  `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28).
- **License:** BSD-3-Clause, "Copyright 2014 The PDFium Authors" — see the
  `LICENSE` file at the root of that checkout. Redistribution in source form
  is permitted with the copyright notice retained, which this file does.
- **Modifications:** none.

Each row says which command's expected output in `../expected/` it pins.

| File | Size | What it exercises |
|---|---|---|
| `hello_world_2_pages.pdf` | 962 B | Two pages of `Hello, world!` / `Goodbye, world!` — `extract text` with the form feed between pages, `render`, `--pages`. |
| `weblinks.pdf` | 1.2 KB | Three URLs written as plain text — `extract links` finds them in the text (`kind: text`) with their rectangles. |
| `parser_rebuildxref_correct.pdf` | 615 B | A wrong `startxref`; the cross-reference table is rebuilt on open — `doctor` lists four recovered notices and `--strict` exits 3. |
| `two_signatures.pdf` | 2.8 KB | Two `/FT /Sig` fields with `/SubFilter` and `/M` — `info` and `extract signatures`. |
| `signature_reason.pdf` | 1.8 KB | One signature carrying `/Reason` — `extract signatures` prints it. |
| `embedded_attachments_with_desc.pdf` | 1.4 KB | Four embedded files, the first with a `/Desc` — `extract attachments` lists and writes them. |
| `annotiter.pdf` | 2.1 KB | Eight widgets over two pages, each with a `/T` — `extract annotations`. |
| `bookmarks.pdf` | 2.2 KB | A four-entry outline two levels deep — `extract toc`, and `info`'s entry count. |
| `annots_action_handling.pdf` | 1.9 KB | Four link annotations: a URI, a `GoTo` to page 2, and two other actions — `extract links` with all three target kinds. |
| `text_form.pdf` | 927 B | One `/Tx` field named `Text Box` — `forms dump`, `forms fill`, `forms flatten`. |
| `encrypted.pdf` | 10.6 KB | RC4-encrypted, user password `1234`, owner `5678` — `security decrypt`, `--password`. |
| `mona_lisa.jpg` | 6.2 KB | A 120 by 120 baseline JPEG — `pages create` embeds it unchanged. |
| `tagged_alt_text.pdf` | 2.4 KB | A tagged page: `Document` › `P` › `Figure` with `/Alt (Black Image)` — `extract markdown` reads the tree and emits the image with its alternative text. |
| `tagged_actual_text.pdf` | 2.4 KB | The same tree with `/ActualText` on the paragraph — the tree's own text wins over the page's. |
| `tagged_marked_content.pdf` | 1.9 KB | Four `NonStruct` elements over four marked-content ids, two of them naming no page and so invalid to the tree, as in PDFium — `extract markdown` emits the two claimed texts from the tree and the two unclaimed ones after them, so nothing is lost. |
