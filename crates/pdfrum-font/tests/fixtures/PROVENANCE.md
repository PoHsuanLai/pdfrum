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
| `tt_composite_instructions.ttf` | 460 | 4 | `(3,1)` fmt 4 | U+002E → 1 |
| `tt_hint_reliant.ttf` | 512 | 4 | `(3,1)` fmt 4 | U+002E → 1 |

`tt_custom_40.ttf` exists so a test can assert that *neither* Windows nor Mac
platform support is detected: platform 4 is neither. `tt_named_no_cmap.ttf`
carries a `post` version 2.0 table naming `.notdef`, `A`, `B` and omits `cmap`
entirely, which is the only way to reach the code path guarded by
`has_glyph_names() && charmaps().is_empty()`.

`tt_composite_instructions.ttf` holds the three glyph shapes the
instructed-composite predicate distinguishes: glyph 1 is the usual triangle,
glyph 2 a two-component composite of it with no instruction stream, and glyph 3
the same composite carrying one instruction byte (`0x4B`, `MIAP[0]`) behind
`WE_HAVE_INSTRUCTIONS`. Both composites place their components by plain
translation.

`tt_hint_reliant.ttf` holds those same glyphs under a `name` table declaring
the family `DFKai-SB`, which is on FreeType's hardcoded list of faces whose
outlines are wrong unless the interpreter runs. It is the only fixture with a
`name` table, and the pair differs in nothing else, so a test that compares the
two is reading the family name and not the glyphs.

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



## `cid_keyed.cff` / `cid_keyed_otto.otf` — one CID-keyed CFF, two packagings

**What they are.** A three-glyph CID-keyed CFF program, emitted twice: bare
(230 B) and inside an `OTTO` shell with the usual `head`/`hhea`/`hmtx`/`maxp`/
`post` beside its `CFF ` table (508 B). Every glyph draws a square. What makes
them worth having is the charset: glyph 0 is CID 0, glyph 1 is CID 4000 and
glyph 2 is CID 4001, so a caller that hands down a CID without putting it
through the charset asks a three-glyph font for glyph 4000 and gets nothing.

That is the shape of a subsetted CIDFontType0 embedded as `/FontFile3` with
`/Subtype /OpenType`, which is what Acrobat and InDesign emit for CJK. The
bare twin exists because FreeType's CFF driver gates the mapping on
CID-keyedness alone — `cid_registry != 0xFFFF && charset.cids`, with no
bare-versus-wrapped condition (`third_party/freetype/src/src/cff/cffgload.c:222-236`)
— so a test needs both packagings to prove the gate is really that and not the
packaging.

**Where they come from.** Synthesized by `make.py` like the `tt_*` files;
nothing is copied from the oracle checkout. The `OTTO` tag is stamped over the
SFNT version after layout, so the file's `head.checkSumAdjustment` is the one
computed for the TrueType tag — no reader verifies it, and the alternative
would be a second checksum pass for no test's benefit.

| fixture | bytes | glyphs | CIDs |
|---|---|---|---|
| `cid_keyed.cff` | 230 | 3 | 0, 4000, 4001 |
| `cid_keyed_otto.otf` | 508 | 3 | the same program, `OTTO`-wrapped |

Advances differ between the two on purpose rather than by accident: the SFNT
reads `hmtx` (a uniform 600 across the skeleton) while the bare CFF reads each
charstring's own width operand. That is a genuine difference between the
packagings, so the test compares outlines and boxes and leaves advances alone.


## `roboto.ttf` — not synthesized

Unlike the `tt_*.ttf` files above, this one is copied verbatim from the PDFium
checkout that serves as the conformance oracle:

- **Source:** `testing/resources/fonts/roboto.ttf` of the PDFium repository, at
  commit `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28).
- **License:** Apache-2.0 (the Roboto project); redistribution in source form is
  permitted with the notice retained, which this file does.
- **Modifications:** none.

It is a copy rather than a reach into `crates/pdfrum/tests/fixtures` so that
this crate's tests resolve every path inside their own crate. The bytes are
identical to the facade's copy.

| `roboto.ttf` | 35636 B | `testing/resources/fonts/roboto.ttf`, verbatim. The TrueType program `latin_extended.pdf` embeds; used here as the caller-supplied bytes for `DocEdit::embed_font`, so a test can write "Hello" in a font the page did not already have. |

The substitution test wants a *real* face here: the cmap-only `tt_*.ttf`
fixtures are too stripped for `fontdb` to accept.
