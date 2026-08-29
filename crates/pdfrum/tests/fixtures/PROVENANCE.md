# Fixture provenance

The three PDFs in this directory are copied verbatim from the PDFium
checkout that serves as this project's conformance oracle:

- **Source:** `testing/resources/` of the PDFium repository, at commit
  `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28).
- **License:** BSD-3-Clause, "Copyright 2014 The PDFium Authors" — see the
  `LICENSE` file at the root of that checkout. Redistribution in source form
  is permitted with the copyright notice retained, which this file does.
- **Modifications:** none. The bytes are unchanged, deliberately: they are
  the same inputs the oracle renders, so a doctest's expectations and a
  conformance golden describe the same file.

| File | Size | What it exercises |
|---|---:|---|
| `hello_world.pdf` | 840 B | A 200x200 page, two standard Type1 fonts, two `Tj` strings. The canonical smoke test for loading, rendering and text extraction. Note its content stream deliberately carries **no `/Length`**, so it also exercises the parser's stream-length recovery. |
| `text_form.pdf` | 932 B | A 300x300 page with an AcroForm holding one `/Tx` (text) field named `Text Box`, present both in `/AcroForm /Fields` and as a `/Widget` annotation in the page's `/Annots`. Drives field enumeration, reading, filling and appearance regeneration. |
| `bookmarks.pdf` | 2239 B | Two 612x792 pages and a four-entry outline tree two levels deep, mixing `/Dest` names, an explicit `/XYZ` destination array and a `/URI` action. Drives the outline walk, page-count and multi-page paths. |

These three were chosen to be the smallest files that between them reach
every facade path, so the doctests stay readable and the crate stays small.
