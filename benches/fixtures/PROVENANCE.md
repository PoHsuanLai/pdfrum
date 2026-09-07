# Benchmark fixture provenance

The seven PDFs in this directory are copied verbatim from the PDFium checkout
that serves as this project's conformance oracle:

- **Source:** `testing/resources/` — and, for `foxittext.pdf`,
  `testing/corpus/fx/other/` — of the PDFium repository, at commit
  `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28).
- **License:** BSD-3-Clause, "Copyright 2014 The PDFium Authors" — see the
  `LICENSE` file at the root of that checkout, and `testing/corpus/LICENSE`,
  which carries the same terms for the corpus half. Redistribution in source form
  is permitted with the copyright notice retained, which this file does.
- **Modifications:** none. The bytes are unchanged, and deliberately so: both
  sides of the comparison in render *these* files, so the
  criterion column and the `pdfium_test` column describe the same work.

| File | Pages | Size | Why it is in the set |
|---|---:|---:|---|
| `bookmarks.pdf` | 2 | 2239 B | Multi-page and tiny. The control for cache reuse: two pages sharing one font, where a per-page render pays for the font twice and a session pays once. |
| `bug_650.pdf` | 1 | 85296 B | The heaviest single page here — a dense embedded-font text page. Dominates the render and text columns, and is the closest thing in this set to a real document's page. |
| `cropped_text.pdf` | 4 | 1855 B | Four pages, each with a `/CropBox` strictly inside its `/MediaBox`. Exercises the crop path four times over, and is the longest document in the set. |
| `embedded_images.pdf` | 1 | 34279 B | Image decoding, and a **deliberately invalid cross-reference table** — the oracle prints `Document has invalid cross reference table` on it. The one fixture whose `open` number is recovery rather than parsing. |
| `foxittext.pdf` | 1 | 62961 B | A dense page of ordinary body text: **2891 glyph occurrences drawn from 82 distinct glyphs.** The one fixture where the glyph *cache* is what is being measured rather than the glyph *rasterizer*, and the shape a real document's page has. The six fixtures above draw at most a few hundred glyphs and none of them reuses one enough to tell a cache from a miss. |
| `latin_extended.pdf` | 1 | 19213 B | A page of accented Latin text through a non-trivial encoding, so glyph lookup rather than glyph count is what costs. It is also the **worst case** for a glyph cache and kept partly for that: 256 glyphs, every one distinct, so every draw is a miss. |
| `many_rectangles.pdf` | 1 | 45529 B | Thousands of filled paths and no text at all. The path-rasterizer column, and the one fixture where the glyph cache is provably irrelevant. |

## Why these seven and not the oracle's whole corpus

The set is chosen to *separate* costs rather than to average them: one
fixture where text dominates, one where paths do, one where image decoding
does, one where cross-reference recovery does, two that are multi-page so
per-document cache reuse has something to reuse, and one whose glyph
repertoire is small against its glyph count, which is the only shape in
which a glyph cache's hit rate is visible at all. A benchmark whose
fixtures all look alike reports one number seven times.

Its honest limitation is size. PDFium's `testing/resources` are unit-test
inputs — the largest is 85 KB and only two files exceed two pages — so
nothing here resembles a 300-page report, and these numbers should not be
extrapolated to one. `testing/corpus` holds larger and more realistic files —
`foxittext.pdf` is one — but it is a *conformance* corpus, and taking more of
it would make the benchmark set drift toward the thing the conformance harness
already measures. records this as a stated limitation
rather than working around it.
