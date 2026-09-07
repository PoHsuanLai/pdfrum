# Test fixtures — provenance

## `FoxitSansMM.pfb`, `FoxitSerifMM.pfb`

**What they are.** The two Multiple-Master Type 1 faces PDFium ships as the
terminal rung of its font-substitution ladder: when a document names a font
that neither the embedded stream nor the system can supply, one of these two is
what draws it, instantiated at whatever weight and width the substitution
decided on. They are the single reason `pdfrum-type1` must implement Multiple
Master at all.

**Where they come from.** The read-only C++ oracle checkout, as C++ array
initializers:

| fixture | oracle source | symbol | bytes |
|---|---|---|---|
| `FoxitSansMM.pfb` | `core/fxge/fontdata/chromefontdata/FoxitSansMM.cpp` | `kFoxitSansMMFontData` | 66 919 |
| `FoxitSerifMM.pfb` | `core/fxge/fontdata/chromefontdata/FoxitSerifMM.cpp` | `kFoxitSerifMMFontData` | 113 417 |

**How to regenerate them.** From the workspace root, with the oracle checkout
present:

```
uv run crates/pdfrum-type1/tests/fixtures/extract.py [ORACLE_ROOT]
```

The script verifies each blob's length against the `std::array<uint8_t, N>`
bound declared in `chromefontdata.h`, so a silent mis-extraction fails loudly.
Byte-for-byte identical output is expected; the fixtures are committed so the
test suite runs without the oracle checkout.

**License.** PDFium, BSD 3-clause (`LICENSE` in the oracle checkout). Original
code copyright 2014 Foxit Software Inc.; the PFB blobs themselves are
`ChromeSansMM` / `ChromeSerifMM` 001.000, dated 2006. records these
as redistributable with the project.

**What they contain.** Both are PFB (`80 01 …`), `/FontType 1`,
`/FontMatrix [0.001 0 0 0.001 0 0]`, 230 glyphs, two design axes
(`/BlendAxisTypes [/Weight /Width]`) over four masters at the unit-square
corners:

| | `/BlendDesignMap` (weight axis) | (width axis) | `/WeightVector` |
|---|---|---|---|
| Sans | `[[50 0][1450 1]]` | `[[50 0][1450 1]]` | `[0.3158 0.1349 0.3849 0.1644]` |
| Serif | `[[110 0][790 1]]` | `[[100 0][900 1]]` | `[0.2702 0.1048 0.4504 0.1746]` |
