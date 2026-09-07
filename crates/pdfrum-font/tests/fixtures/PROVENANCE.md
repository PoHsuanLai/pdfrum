# Test fixtures — provenance

## `tt_*.ttf` — minimal TrueType cmap fixtures

**What they are.** Twelve synthetic TrueType faces, each stripped to the
smallest SFNT `read-fonts` will still open, whose only interesting content is
the `cmap` table. Together they cover every branch the TrueType charcode →
glyph lookup has to take: the two Unicode subtable spellings PDFium insists are
interchangeable, the `(3,0)` symbolic table with its `0xF0xx` private-use
convention, the `(1,0)` MacRoman byte table, a platform neither Windows nor Mac,
the combinations that force a preference order, degenerate tables that map
nothing, and a face with glyph names but no `cmap` at all, and a legacy CJK subtable
sitting *after* a Unicode one so the CID charmap chooser has to prefer it on
the coding scheme rather than on position.

**Where they come from.** Nothing here is copied from the read-only C++ oracle
checkout — every byte is generated. What is borrowed is the *shape of the idea*:
`core/fpdfapi/font/cpdf_truetypefont_unittest.cpp` hand-assembles a 508-byte TTF
for its `AllUnicodeCmapsTreatedEqually` test, in two variants that differ only
in the cmap subtable's platform and encoding IDs — `(0,3)` versus `(3,1)` — each
mapping U+002E to glyph 1. `tt_unicode_03.ttf` and `tt_unicode_31.ttf` are that
pair, rebuilt from scratch; the other ten generalize the same skeleton.

| fixture | bytes | glyphs | cmap subtables, in order | mapping |
|---|---|---|---|---|
| `tt_unicode_31.ttf` | 388 | 2 | `(3,1)` fmt 4 | U+002E → 1 |
| `tt_unicode_03.ttf` | 388 | 2 | `(0,3)` fmt 4 | U+002E → 1 |
| `tt_symbol_30.ttf` | 412 | 6 | `(3,0)` fmt 4 | U+F041 → 5 |
| `tt_macroman_10.ttf` | 656 | 8 | `(1,0)` fmt 0 | 0x41 → 7 |
| `tt_custom_40.ttf` | 588 | 2 | `(4,0)` fmt 0 | *(nothing)* |
| `tt_symbol_and_macroman.ttf` | 728 | 8 | `(3,0)` fmt 4, `(1,0)` fmt 0 | U+F041 → 5; 0x41 → 7 |
| `tt_unicode_03_and_symbol.ttf` | 484 | 6 | `(0,3)` fmt 4, `(3,0)` fmt 4 | U+002E → 1; U+F041 → 5 |
| `tt_unicode_31_and_symbol.ttf` | 484 | 6 | `(3,1)` fmt 4, `(3,0)` fmt 4 | U+002E → 1; U+F041 → 5 |
| `tt_symbol_empty.ttf` | 372 | 6 | `(3,0)` fmt 4, terminator segment only | *(nothing)* |
| `tt_macroman_empty.ttf` | 624 | 8 | `(1,0)` fmt 0, all 256 entries zero | *(nothing)* |
| `tt_sjis_and_unicode.ttf` | 484 | 6 | `(3,1)` fmt 4, `(3,2)` fmt 4 | U+002E → 1; 0x889F → 3 |
| `tt_named_no_cmap.ttf` | 376 | 3 | *(no `cmap` table)* | — |

`tt_custom_40.ttf` exists so a test can assert that *neither* Windows nor Mac
platform support is detected: platform 4 is neither. `tt_named_no_cmap.ttf`
carries a `post` version 2.0 table naming `.notdef`, `A`, `B` and omits `cmap`
entirely, which is the only way to reach the code path guarded by
`has_glyph_names() && charmaps().is_empty()`.

**How to regenerate them.** From the workspace root:

```
python3 crates/pdfrum-font/tests/fixtures/make.py [OUT_DIR]
```

`OUT_DIR` defaults to this directory. The script is pure standard library and
its output is deterministic — timestamps in `head` are pinned to zero — so a
re-run reproduces the committed bytes exactly. The fixtures are committed so the
test suite needs neither the generator nor the oracle checkout to run.

**License.** Synthesized for this project; same terms as the rest of the
workspace. No third-party font data is embedded.

**What they contain.** Every fixture is an `sfntVersion 0x00010000` TrueType
font with `head`, `hhea`, `hmtx`, `maxp`, `loca` (short format), `glyf`, `post`
and — apart from `tt_named_no_cmap.ttf` — `cmap`; `post` is version 3.0 (no
names) except in that one face. `unitsPerEm` is 1000 throughout, advances are a
uniform 600, and every table carries a correct checksum with
`head.checkSumAdjustment` patched so the whole file sums to `0xB1B0AFBA`. Glyph
0 and all unreferenced glyphs are empty (zero-length `loca` entries); every glyph
a `cmap` actually targets — 1, 5, or 7, depending on the fixture — is a
non-degenerate one-contour triangle at (50,0)–(650,0)–(350,700), so outline
extraction has something real to draw.

**Verified against.** `skrifa 0.46.2` / `read-fonts 0.43.3`: each file opens with
`FontRef::new`, reports `unitsPerEm` 1000 and the glyph count above, exposes its
`cmap` encoding records in exactly the order tabulated, resolves the listed
codepoints through `map_codepoint`, and draws a non-empty outline for each
triangle glyph. Note that a format 0 subtable answers `Some(0)` rather than
`None` for an unmapped code — the 256-byte array always has an entry.
