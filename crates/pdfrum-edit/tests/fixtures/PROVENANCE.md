# Fixture provenance

The files in this directory are copied verbatim from the PDFium checkout that
serves as this project's conformance oracle:

- **Source:** `testing/resources/` of the PDFium repository, at commit
  `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28).
- **License:** BSD-3-Clause, "Copyright 2014 The PDFium Authors" — see the
  `LICENSE` file at the root of that checkout. Redistribution in source form
  is permitted with the copyright notice retained, which this file does.
- **Modifications:** none.

They are copies rather than reaches into `crates/pdfrum/tests/fixtures`: a
doctest is rendered documentation, and a path climbing out of the crate reads
as a mistake to anyone who meets it in the docs. `crates/pdfrum-cli` keeps its
own copies for the same reason. The bytes are identical, so a fixture the
oracle renders and one a doctest opens are still the same file.

| File | Size | What it exercises |
|---|---|---|
| `hello_world.pdf` | 840 B | A 200x200 page, two standard Type1 fonts, two `Tj` strings. The canonical smoke test for loading, rendering and text extraction. Note its content stream deliberately carries **no `/Length`**, so it also exercises the parser's stream-length recovery. |
| `hello_world_2_pages.pdf` | 962 B | Two pages **sharing one content stream and one resource dictionary**. Editing one page must copy rather than rewrite, and this is the file that fails if it does not. |
| `annotiter.pdf` | 2136 B | Three 612x792 pages that **share one field's four widget kids** and differ only in `/Tabs` — `R`, `C`, `S` — so the same four annotations traverse in three different orders. The kids sit at the corners of a square in an annotation order that is none of the three traversal orders (`Sub_LeftBottom` first, then `RightTop`, `LeftTop`, `RightBottom`), which is what makes a wrong tab implementation unable to agree by accident. It is also the only fixture where focus has to be document-wide rather than per-page, since all three pages list the same annotations. |
| `embedded_attachments.pdf` | 10447 B | `testing/resources/embedded_attachments.pdf`, verbatim. Two embedded files (`1.txt`, `attached.pdf`) with `/Params` — `Size`, `CreationDate`, a hex `CheckSum` — and a `text/plain` subtype: `fpdf_attachment_embeddertest.cpp`. |
| `mona_lisa.jpg` | 6167 B | `testing/resources/mona_lisa.jpg`, verbatim. The **only** JPEG in the oracle's `testing/resources/`, and the one its own `FPDFEditEmbedderTest` JPEG cases load. A 120x120 baseline SOF0, three components, eight-bit precision, JFIF with no Adobe APP14 — which is exactly the combination that pins the DCT passthrough: `/DeviceRGB`, `/BitsPerComponent 8`, `/Filter /DCTDecode`, and **no** `/DecodeParms /ColorTransform 0`, since libjpeg reads a marker-less three-component frame as YCbCr. Its 6 KB of entropy-coded scan also makes it a real round-trip rather than a header exercise: the same bytes must come back out of the saved file and decode to the same picture. |
| `gray.jp2` | 211 B | `testing/resources/gray.jp2`, verbatim. A 4x4 one-component JP2 file — the smallest JPEG 2000 in the corpus — carrying the full twelve-byte `jP  ` signature box before its `jp2c`. It is the fixture for the `/JPXDecode` arm, where the point is what is **absent**: §7.4.9 leaves `/ColorSpace` and `/BitsPerComponent` to the codestream, so the dictionary this writes has four keys and a `/Filter`, and a reader that finds a `/BitsPerComponent` there would be reading something we should not have written. |

## `svg/` — written for this project

Not from the oracle. `image_png.svg` is a 200x200 document whose only mark is
a 16x16 truecolour PNG carried inline as a `data:` URI: the input for the
`<image>` arm of SVG ingestion, where the point is that the PNG is decoded
rather than refused. Copied from `crates/pdfrum/tests/fixtures/svg/`, which
holds the wider set the facade's ingestion tests walk.
