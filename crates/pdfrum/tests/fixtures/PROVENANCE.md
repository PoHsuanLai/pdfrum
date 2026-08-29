# Fixture provenance

The PDFs in this directory are copied verbatim from the PDFium
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

Those three were chosen to be the smallest files that between them reach
every read-side facade path, so the doctests stay readable and the crate
stays small.

Four more arrived with page mutation. Each is the file the corresponding
PDFium embedder test uses, so an assertion here and an assertion there are
about the same bytes:

| File | Size | What it exercises |
|---|---:|---|
| `split_streams.pdf` | 3502 B | Nineteen page objects across **three** `/Contents` elements — 15, 3 and 1. The only fixture that distinguishes a content-stream index from an object index, and the one `RemoveAllFromStream` uses to pin the index collapse. |
| `hello_world_split_streams.pdf` | 922 B | Three text objects across **two** elements, 2 and 1. The small case: enough to tell "the element still has a sibling" from "the element is now empty", which are the two sides of the array-shrink rule. |
| `rectangles.pdf` | 633 B | Eight path objects on one page, one element. The visibility and transform tests' fixture: eight distinguishable shapes with no fonts to complicate the resource sweep. |
| `hello_world_2_pages.pdf` | 962 B | Two pages **sharing one content stream and one resource dictionary**. Editing one page must copy rather than rewrite, and this is the file that fails if it does not. |
