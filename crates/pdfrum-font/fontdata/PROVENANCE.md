# Base-14 font data — provenance

## `Foxit*.cff` (14 files)

**What they are.** The fourteen faces PDFium substitutes for the PDF base-14
standard fonts when a document names one without embedding it: four Courier,
four Helvetica, four Times, plus Symbol and ZapfDingbats. They are the rung of
the substitution ladder *above* the two Multiple-Master fallbacks
(`crates/pdfrum-type1/tests/fixtures/PROVENANCE.md`) — a document that names a
base-14 font by name lands on one of these, and only an unrecognised name falls
through to `FoxitSansMM` / `FoxitSerifMM`.

**Where they come from.** The read-only C++ oracle checkout, as C++ array
initializers under `core/fxge/fontdata/chromefontdata/`:

| fixture | oracle source | symbol | bytes | CFF FontName |
|---|---|---|---|---|
| `FoxitFixed.cff` | `FoxitFixed.cpp` | `kFoxitFixedFontData` | 17 597 | `ChromFixedOTF` |
| `FoxitFixedBold.cff` | `FoxitFixedBold.cpp` | `kFoxitFixedBoldFontData` | 18 055 | `ChromFixedOTF-Bold` |
| `FoxitFixedBoldItalic.cff` | `FoxitFixedBoldItalic.cpp` | `kFoxitFixedBoldItalicFontData` | 19 151 | `ChromFixedOTF-BoldItalic` |
| `FoxitFixedItalic.cff` | `FoxitFixedItalic.cpp` | `kFoxitFixedItalicFontData` | 18 746 | `ChromFixedOTF-Italic` |
| `FoxitSans.cff` | `FoxitSans.cpp` | `kFoxitSansFontData` | 15 025 | `ChromSansOTF` |
| `FoxitSansBold.cff` | `FoxitSansBold.cpp` | `kFoxitSansBoldFontData` | 16 344 | `ChromSansOTF-Bold` |
| `FoxitSansBoldItalic.cff` | `FoxitSansBoldItalic.cpp` | `kFoxitSansBoldItalicFontData` | 16 418 | `ChromSansPS-BoldItalic` |
| `FoxitSansItalic.cff` | `FoxitSansItalic.cpp` | `kFoxitSansItalicFontData` | 16 339 | `ChromSansOTF-Italic` |
| `FoxitSerif.cff` | `FoxitSerif.cpp` | `kFoxitSerifFontData` | 19 469 | `ChromSerifOTF` |
| `FoxitSerifBold.cff` | `FoxitSerifBold.cpp` | `kFoxitSerifBoldFontData` | 19 395 | `ChromSerifOTF-Bold` |
| `FoxitSerifBoldItalic.cff` | `FoxitSerifBoldItalic.cpp` | `kFoxitSerifBoldItalicFontData` | 20 733 | `ChromSerifOTF-BoldItalic` |
| `FoxitSerifItalic.cff` | `FoxitSerifItalic.cpp` | `kFoxitSerifItalicFontData` | 21 227 | `ChromSerifOTF-Italic` |
| `FoxitSymbol.cff` | `FoxitSymbol.cpp` | `kFoxitSymbolFontData` | 16 729 | `ChromSymbolOTF` |
| `FoxitDingbats.cff` | `FoxitDingbats.cpp` | `kFoxitDingbatsFontData` | 29 513 | `ChromDingbatsOTF` |

The row order is `CFX_StandardFont::Index` order (`core/fxge/cfx_standardfont.h`),
which is also the order of `kFoxitFonts` and `kBase14FontNames` in
`core/fxge/cfx_standardfont.cpp`.

**How to regenerate them.** From the workspace root, with the oracle checkout
present:

```
uv run crates/pdfrum-font/fontdata/extract.py [ORACLE_ROOT]
```

The script verifies each blob's length twice — against the `std::array<uint8_t,
N>` bound declared in `chromefontdata.h`, and against the length hard-coded in
the script — so a silent mis-extraction, or an oracle rebase that changed a
blob, fails loudly instead of writing a truncated file. Note that unlike the MM
sources these `.cpp` files use minimal-width hex literals (`0x1`, `0xd`), so the
byte regex must accept one or two digits; a two-digit-only regex silently drops
roughly a sixth of every blob.

**License.** PDFium, BSD 3-clause — `LICENSE` at the oracle checkout root
(`$PDFRUM_ORACLE_CHECKOUT/LICENSE`, first 27 lines; the Apache text that
follows in that file covers other third-party components, not `core/fxge`).
There is no separate LICENSE file under `core/fxge/fontdata`. Each `.cpp`
carries the standard two-part PDFium header:

```
// Copyright 2014 The PDFium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Original code copyright 2014 Foxit Software Inc. http://www.foxitsoftware.com
```

**What they contain.** Each file is a bare CFF font (no OpenType wrapper): the
CFF header `01 00 04 02` — major 1, minor 0, hdrSize 4, offSize 2 — followed by
the Name INDEX carrying the `Chrom*` FontName above. They are consumed by
FreeType as `FT_New_Memory_Face` inputs in the C++ oracle, and here by
`read_fonts::ps::cff` — **not** by `skrifa`, whose `FontRef` requires a table
directory and therefore cannot open a bare CFF at all. That is why
`glyphs::Face` carries two backends.
