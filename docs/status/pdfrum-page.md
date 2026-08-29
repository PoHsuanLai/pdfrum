# `pdfrum-page` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

The project's largest crate. Contract: SPEC.md §7 (including the 2026-08-29
rulings); behavior: `docs/design/pdfrum-page.md` (§1.0–1.21).

## What landed

Everything SPEC §7 names: the `ops!` table, `parse_content`, `build_page`,
`ColorSpace` with all eleven families, `Function` types 0/2/3/4, shadings 1–7
with mesh reading, both pattern kinds, the image ladder with three codecs
behind SPEC §12's entry points, transparency, optional content, and the
graphics-state stack.

| module | contents |
|---|---|
| `ops.rs` + `ops_table.rs` | the `ops!` macro and its 73 rows; `LineCap`, `LineJoin`, `FillRule`, `TextRenderMode`, `TextItem`, `MarkProperties`, `InlineImage` |
| `tokenize.rs` | the second tokenizer — 255-byte words, 32767-byte strings, the strict dictionary grammar, the `allow_nested_array` rule |
| `content.rs` | `parse_content`, the 16-slot operand ring with oldest-eviction, the `m`-triggered path run |
| `inline_image.rs` | the 9 key and 11 value abbreviations, length inference per filter, the `EI` resync |
| `state/` | `GraphicsState` + `StateStack`; `StrokeParams` and the dash ladder; `TextState`/`TextCursor`; `GeneralState` and `BlendMode`; the 24-key `/ExtGState` table; `ClipStack`; `ContentMarks` |
| `build.rs` | the fold, path assembly with its three repairs, form recursion with the buffer-identity guard, text-object construction, `Do` and `sh` dispatch |
| `page.rs` | `Page`, `PageObject` and the five records, box derivation, `Rotation`, page-dictionary validation |
| `resources.rs` | the whole-dictionary choice plus the one-level category fallback |
| `color/` | the eleven families, both conversion paths, the two verbatim tables, `ColorValue`, the loader with its cycle guards |
| `function/` | the four types, `FunctionCache`, the bit reader, the 42-operator PostScript engine |
| `shading/` | the seven kinds, validation, the 256-entry LUT, the mesh reader, Coons interiors |
| `pattern/` | the matrix rule, `/PatternType` dispatch, the tiling step and tile-range ladders |
| `image/` | `ImageDict` validation, `DecodeMap`, the three codecs, masking, scanline unpacking, the `(ObjRef, RequestedSize)` cache |
| `transparency.rs` | groups, soft masks, the `/BC` backdrop |
| `optional.rs` | `OcContext`, the four `/P` policies, `/VE` expressions |
| `transfer.rs` | the 3×256 transfer LUT |
| `tools/gen_cmyk_table.py` | the checked-in generator for the 6561-entry table |

### The behaviors that mattered most

- **The tokenizer takes its first byte unconditionally.** `/` is a delimiter,
  so a loop that checks before consuming stops at byte zero and every name
  comes back empty. The C++ pushes the first character and only then tests the
  *next* one. Getting this wrong made every name, and therefore every resource
  lookup, silently fail — the first bug the tests caught.
- **The array loop consults the word the *nested* read left behind**, not a
  freshly scanned one. Re-reading loses that word, and the "spin forward past a
  refused nested array" behaviour then either loops or terminates early. The
  reader carries `last_word` for exactly this.
- **There is no "repair a bad bit depth to 8".** Only two coercions exist and
  both are driven by the filter. `/BitsPerComponent 3` on a Flate image is a
  hard rejection. This is the single most commonly assumed behaviour PDFium
  does *not* have, and `image/dict.rs` says so at the top.
- **`FX_RGB_STRUCT`'s fields are named `red`/`green`/`blue` and laid out
  B,G,R** — the C++ has a `TODO` wondering the same thing. So `.red = rgb.blue`
  is not a swap, it is writing blue into byte zero. The three `DeviceCMYK` bulk
  formulas each map channels differently on top of that, and only one of the
  three keeps cyan in byte zero.
- **`Font` is not `PartialEq`.** `TextState` and `TextObject` compare it by
  `FontId`, which is the identity that matters anyway.

## Divergences

All of D1–D22 are implemented as the brief specifies, including the two
pixel-visible quirks the orchestrator ruled must be ported verbatim: the
shading LUT's sample-at-`i/256`, index-at-`s*255` off-by-one (D16) and the
sampled function's negative-index wrap to the *top* cell (D12). The two C++
bugs the brief declines (D7's stale transfer output, D18's mesh-bbox counter
mutation) are not ported.

The three additive `Limits`-shaped caps (Q1) are constants in the modules that
enforce them rather than `Limits` fields, since nothing outside this crate
configures them: `color::load::DEFAULT_MAX_DEPTH` (32),
`function::MAX_DEPTH` (32) and `function::MAX_OUTPUTS` (1024). Promoting them
to `Limits` is a one-line change if a caller ever needs it.

## Brief errata

Four places where the brief and the C++ disagree, checked against the oracle:

1. **§1.14.2: `kSRGBSamples2` is 208 bytes, not 253.** The brief's own
   max-index note (`1023/4 - 48 = 207`) is consistent with 208 and not with
   253. Transcribed at its real length.
2. **§1.1: `[1 [2] 3]` at operator level yields `[1, 2]`, not `[1, 3]`.**
   Tracing `ReadNextObject`: the nested `[` returns null, the loop spins, the
   next read takes `2` as an ordinary element, and the inner `]` then
   terminates the *outer* array — leaving `3` to be tokenized as content. The
   brief's stated shape and its stated mechanism disagree; the mechanism is
   right.
3. **§1.15.5: a malformed `if` aborts *before* popping its condition.** The
   structural check precedes the pop, so `{5 9 if 7}` leaves `5 9` on the
   stack rather than consuming the `9`.
4. **§1.3: `Tf` sets the size even when the font name fails**, which the brief
   states, but the consequence — a subsequent `Tj` still shows text in the
   *previous* font at the *new* size — is worth naming because it is what
   `FindFont`'s Helvetica fallback interacts with.

## A defect found in a landed crate

`pdfrum_object::Object::clone_direct` **panics on real corpus files.** It
flattens a whole object subtree, and when a reference inside a dictionary
resolves to a stream it stores that stream as a direct dictionary value —
tripping `Dict::push`'s own debug assertion that ISO 32000-1 §7.3.8.1 forbids
exactly that. A `/Resources` dictionary whose `/XObject` entries are indirect
streams is the common case, so this fires on ordinary files.

This crate no longer calls `clone_direct` anywhere; every site wanted one
level of resolution and now uses `Object::resolve`. **The defect itself is
unfixed** — the obvious repairs each break `pdfrum-object`'s own tested
contract (dropping the entry loses data a `/Resources` needs; leaving the
reference in place contradicts the documented "dangling references are
dropped" behaviour), so choosing between them is that crate's call, not this
one's. Recorded here rather than patched around silently.

## Tests

| suite | count |
|---|---|
| unit (`--lib`) | 343 |
| integration (`tests/corpus.rs`) | 8 |
| never-panic sweeps (`tests/never_panics.rs`) | 9 |
| doctests | 4 |

Ported from the C++ unittests: the two `cpdf_streamcontentparser` abbreviation
tables including the `WW`/`II` prefix cases; all five `cpdf_streamparser`
`ReadHexString` vectors; `cpdf_page`'s eight `IsValidPageDict` cases; both
`cpdf_colorspace` `TranslateImageLine` vectors (the `CalRGB` one is what pins
the scalar/bulk disagreement); every `cpdf_devicecs` `GetRGB` vector including
the four Adobe CMYK ones that validate the 6561-entry transcription; all four
`cpdf_function` negative cases; and `cpdf_psengine`'s whole file — the
46-entry operator table, `Basic`, `DivByZero`, the four rounding groups
including `round -5.5 → -5` and the `truncate` saturation case, `Comparisons`,
`Logic` and `MathFunctions`; plus `cpdf_dib`'s three default-decode scanline
cases at 4, 8 and 16 bits.

The never-panic sweeps run about 12000 generated inputs across
`parse_content`, `build_page`, `decode_image`, the three codec entry points,
`Function::eval`, the PostScript engine and colorspace loading. Eight fuzz
targets cover the same surface continuously: `page_parse_content` (which also
asserts purity), `page_inline_image`, `page_colorspace`, `page_psengine`,
`page_mesh_stream`, `page_decode_image`, `page_jbig2` and `page_jpx`.

The integration suite builds pages from the oracle's own files: `hello_world`
for text and box derivation, `rectangles` for paths, `embedded_images` for the
`Do` dispatch, plus sweeps over ~290 resource files and 120 corpus files. That
last pair is what found the `clone_direct` panic and the `/XObject` lookup
failure it caused — neither was reachable from a synthesized fixture.

## Not yet exercised

Three areas are implemented and unit-tested but have no pixel-level
verification until `pdfrum-render` lands, which is where they will actually be
measured:

- **ICC transforms** go through `moxcms` where PDFium uses lcms. The *ladder*
  around a rejected profile is pinned exactly; which profiles the two engines
  disagree about is brief Q2's M4 sweep.
- **Type 3 fonts.** `d0`/`d1` are parsed and the char procs are reachable
  through `Type3Font`; executing a glyph's content stream re-enters
  `build_page` and belongs to the render crate (brief Q7).
- **Tiling pattern content.** The load, the step ladder and the tile-range
  arithmetic are here; rendering the cells is the render crate's.
