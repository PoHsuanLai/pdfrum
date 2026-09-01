# `pdfrum-page` status

**Updated:** 2026-08-30 · **State:** implemented, all gates green

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
| `mutate.rs` | the editing half (M11): `dirty`/`active`/`content_stream` on each object, `dirty_streams` and `stream_ctms` on the page, and the mutation functions that set them |
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

**Corrected 2026-09-02 (oracle-divergence audit).** Three amendments:

- **D7 and D18 are relabelled `[oracle-bug]` (audit A11, A23).** Not
  reproducing them was recorded as our choice; PLAN.md §212–229 makes it
  obligatory. D7 is also worse than "stale": `cpdf_docrenderdata.cpp:132-137`
  guards the `Call` but not the read, and `OutputCount()` is loop-invariant,
  so `output[0]` is never written at all and the curve is **all-black**. D18's
  cumulative `point_count -= 4` is contradicted by PDFium's own second copy of
  the loop at `cpdf_rendershading.cpp:900-917`. Both cost 0 rows.
- **D15 is relabelled (audit A24) and the claim above is not accurate for
  it.** There is **no pattern cache** in this crate at all — patterns load
  afresh at each use — where the brief specifies a `(ObjRef, parent_matrix)`
  key. That over-satisfies D15 rather than under-satisfying it (no cache
  cannot alias), and it also covers the second half of the oracle's defect the
  brief does not mention: `GetPattern` and `GetShading` share **one** map
  (`cpdf_docpagedata.cpp:388`, `:415`) while constructing with opposite
  `bShading` flags. A cache added later must key on
  `(ObjRef, parent_matrix, is_shading)`.
- **`/ExtGState /Font` now resolves the spec's `[<ref> size]` form (audit
  A18)**, where the brief said to port the oracle's name-in-the-resources
  reading. `cpdf_allstates.cpp:87-89` reads the first element as a byte
  string, so a reference yields `""`, the lookup misses, and
  `cpdf_streamcontentparser.cpp:1239` substitutes stock Helvetica — the
  conformant spelling silently draws the wrong font. Table 58 makes it an
  indirect reference; pdf.js resolves it (`evaluator.js:1142-1154`, and
  `loadFont`'s "Loading by ref" at `:1256-1261`). The resource-name lookup is
  kept as **tolerance** for files written against the oracle. 0 rows.

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

## A defect found in a landed crate — since fixed

`pdfrum_object::Object::clone_direct` **panicked on real corpus files.** It
flattens a whole object subtree, and when a reference inside a dictionary
resolves to a stream it stores that stream as a direct dictionary value —
tripping `Dict::push`'s own debug assertion that ISO 32000-1 §7.3.8.1 forbids
exactly that. A `/Resources` dictionary whose `/XObject` entries are indirect
streams is the common case, so this fired on ordinary files.

**Fixed in `pdfrum-object`**: neither repair this crate weighed was the right
one. The assertion was, and §7.3.8.1 is a *file-format* constraint rather than
an in-memory invariant — the C++ clone path writes straight into `map_` /
`objects_` and so produces inline streams too, which `GetStreamFor` then reads
back. `Array::push` and `Dict::push` now accept a stream value; the reader
still drops one found inline while parsing, and the writer (M7) hoists it back
out. See `docs/status/pdfrum-object.md`.

**This crate is unchanged, deliberately.** It stopped calling `clone_direct`
because every site wanted *one level* of resolution, and `Object::resolve` is
the C++-faithful call there:
`CPDF_StreamContentParser::FindResourceObj` reaches a named resource through
`GetMutableDirectObjectFor` (`cpdf_streamcontentparser.cpp:1227-1232`), a
one-level resolve — and `Do` reads its form or image with exactly that,
`ToStream(FindResourceObj("XObject", name))` (`:792`), never a clone. Nothing
on the page path calls `CloneDirectObject` at all; its only C++ callers are
the two in `cpdf_interactiveform.cpp`. Reverting these sites to `clone_direct`
would deep-flatten whole resource subtrees the C++ never copies.

## `/TR` at M3: the storage was right, the sampler was not

The render status doc reported this crate's `TransferFunc` as storing the
`/TR` array reversed against its own doc comment, and `pdfrum-render`
un-reversed it at the boundary. **The storage was right; the doc comment and
the workaround were wrong**, and the workaround cancelled a correct reversal,
so every rendered `/TR` array came out with red and blue swapped. The
derivation — the oracle's expected-sample constants are misnamed, and its ten
`TranslateColor` pairs settle the channel mapping without relying on any
constant's name — is recorded in `docs/status/pdfrum-render.md` and in
`transfer.rs`'s module docs. `array[2]` drives red.

Porting that fixture verbatim then exposed a second divergence this crate
*did* have: **`sample_channel` clamped where PDFium wraps.** The C++ writes
`size_t o = FXSYS_roundf(output[0] * 255)` into a `uint8_t` with no clamp, so
a function whose `/Range` admits negatives folds its lower half back to the
top of the byte range. The type 4 fixture is one: at input `0xCC` it produces
`-121.26`, which the oracle stores as `0x87` and we stored as `0x00`. Two of
the ten asserted pairs turn on it. Clamping quietly flattened every signed
transfer function's lower half to black, and no synthetic fixture built from
non-negative functions could have caught it.

The doc comment, the storage rule and the wrap are now stated together in
`transfer.rs`; the render crate's compensation is gone, and its
`the_oracle_fixtures_ten_translate_colour_pairs` runs the oracle's own three
functions end to end.

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

## M8: three image behaviours the pixel burn-down found

Each was implemented, unit-tested, and unreachable — which is the shape a
crate boundary produces when a decision is recorded rather than applied.

**A row past the end of the stream skips the decode entirely.**
`CPDF_DIB::GetScanline` picks a source in three arms and then, if it found
none, fills the *output* buffer with zeros and returns — before
`TranslateScanline24bpp`, before the stencil inversion. So an absent row is
literally black, whatever `/Decode` maps a zero sample to. `scanline` now
reports [`Availability`] with three states rather than a "was padded" bool,
because *partial* and *absent* are different behaviours and only the second
one skips the decode. `bug_554151.in` — `/Decode [1.0]` over a 612×104000
image with a kilobyte of data — painted the whole page red and now paints it
black, byte-exact.

The arm only exists when the stream is read directly: a JBIG2, JPX, DCT or
CCITT decoder always returns a full row, so `src_line` is never empty and
every row decodes. `reads_the_stream_directly` is that gate, and without it a
truncated JBIG2 mask stops inverting partway down (`bug_674771.in`).

**A `/Mask` array was parsed into a `ColorKey` nothing ever asked.**
`ImageMask::alpha_at` returned 255 for the colour-key arm unconditionally —
and it had to, because the predicate is on the *raw samples* and by draw time
those are gone. PDFium runs it inside `GetScanline`, filling a parallel buffer
whose alpha byte is `IsColorIndexOutOfBounds(...) ? 0xFF : 0` — in range means
*transparent*, which reads backwards until you notice the predicate is named
for the opposite. `resolve_color_key` now turns it into an alpha plane at
decode time and the renderer sees an ordinary mask. Three files byte-exact.

**A JPX image under an `/Indexed` space was dropping its palette.** The
decoder was correctly asked for raw indices (`JpxAction::UseIndexed`), and
then the result became `Pixels::Gray8` and reached the page as grey. The
indices also need `>> (8 - bpc)`, because the decoder hands back eight-bit
samples whatever the dictionary's `/BitsPerComponent` says —
`cpdf_dib.cpp:698-706`. `jpxdecode_indexed2.in` is byte-exact;
`jpxdecode_indexed.in` improved but still differs, and the residual is in the
codestream decode rather than in this path.

## A `/Filter` array may name its filters by reference

`last_filter` read the array's last element with `Array::name_at`, which does
not resolve. `GetDecoderArray` reaches each element through
`GetByteStringAt`, which follows one level of indirection, and reaches
`/Filter` itself through `GetDirectObjectFor` — so both levels resolve, and
the filters crate's own `decoder_list` had this right all along via
`Array::get`.

`bug_1986` is the file that shows it: `/Filter [6 0 R /LZWDecode 7 0 R]` with
object 7 = `/JPXDecode`, and **no `/ColorSpace`**, which is legitimate because
a JPX codestream carries its own. An unrecognised last filter means the
missing colour space is read as "this is a stencil", and
`ImageDict::load`'s mask forcing sends the image down the one-bit path — so a
red page rendered as a black-on-white mask. Byte-exact after the fix.

`/DecodeParms` elements now resolve too, by the same rule: `dict_at` in place
of `raw_at().as_dict()`.

This was recorded in the render status doc as a *parser* object-recovery gap,
on the theory that objects terminated `enbobj` do not load. They do —
`GetIndirectObject` never looks for `endobj` at all, and neither do we — and
the whole filter chain already ran correctly through to a valid JP2. The gap
was one accessor here.

## `FXSYS_IsFloatZero` is a fixed 1e-4

`shading/radial.rs` tested `f32::EPSILON`, roughly 840 times too tight. The
macro is `(f) < 0.0001 && (f) > -0.0001` (`fx_system.h:36`), and the
difference decides which branch a near-degenerate radial takes — the linear
`a == 0` one, which has no negative-radius skip, or the quadratic one, which
does. `pdfrum-doc`'s `geom.rs` already had it right. See the render status
doc for what it cost on `radial_shading_point_at_border`.

## M11: the editing half, and the two fields the brief did not foresee

The edit brief's E6 asked for `dirty` and `active` on each page object and a
dirty-stream set on the page. Those landed as proposed, as plain fields with
the operations beside them in `mutate.rs` rather than inside them. Two things
about the shape are worth recording because they were choices, not
transcription.

**Removal is the only case that needs the page-level set.** A modified or
hidden object still exists, so the regenerator finds it, reads its `dirty`,
and knows which stream to rewrite. A *removed* object is gone and nothing is
left pointing at the stream that has to lose it — which is why
`Page::remove_object` takes an index and records the stream before the object
goes, rather than being a `Vec::retain` at the call site.

**Two references the brief did not anticipate, and the reason is structural.**
A parsed `ImageObject` holds decoded pixels; a parsed `TextObject` holds a
loaded `Font`. Neither can name the `/XObject` or `/Font` entry that a
regenerated `Do` or `Tf` must spell — the decoded form has thrown that away.
So `ImageObject` and `FormObject` gained `source: Option<ObjRef>` and
`TextObject` gained `font_source`. The interpreter had already resolved both
for its caches; they now travel on the object as well. `None` means the
resource was written inline, or the object came from an annotation's `/AP`,
and such an object is dropped when its stream is rewritten — which is what the
C++ does with an inline image for the same reason.

**`content_stream` is populated for real now**, which it was not before: it
existed as a field and was hardcoded to `0`. `StreamBounds` carries where each
`/Contents` element's operators begin, found by parsing each element on its own
and counting. That is exact rather than approximate because of the separating
space the join inserts after every element — it terminates whatever token the
element ended on, so no operator can span a boundary. Only the editor pays for
it: `build_page_from_dict` is unchanged and the render and text paths still
use it.

## A defect M11's oracle comparison found: the real parser was two ulps out

`word_to_number` accumulated a real digit by digit into an `f32`, adding each
digit times a repeatedly-multiplied `0.1`. That is the *obsolete* `FX_atof`
this crate was ported from; current PDFium replaced it with a correctly-rounded
parse (`fx_string.cpp:124-140`, `fast_float::from_chars`). The accumulator also
stopped at an `e`, so `1.2e34` read as `1.2`.

Both are failures against PDFium's own `ByteStringToFloat` vectors
(`fx_string_unittest.cpp:123-164`), which are now ported here — with **exact
equality**, because the tolerance the surrounding tests use is precisely what
let this sit unnoticed: every existing numeric assertion compared to `1e-4` or
`1e-6`, and a two-ulp error passes all of them.

**Why two ulps were visible at all.** A rectangle's edge runs through `floor`
and `ceil` on its way to a device rect (`pdfrum-render`'s `snap_rect`), so a
1e-7 relative error is quantised into a whole row of pixels. M11's mutation
sweep is what surfaced it: a regenerated content stream undoes the transform it
inherited by writing that transform's inverse, and a page whose first element
leaves `399 0 0 400 cm` in force gets a second element beginning
`.0025062656 0 0 .0025 0 0 cm`. Reading those two literals a hair high made our
rectangle one row taller and one column wider than PDFium's, on seven corpus
files, at a stable SSIM of 0.986289.

Worth recording because the first two hypotheses were both wrong and both
plausible: that our `f64` matrix composition diverged from PDFium's `f32`
(`CFX_Matrix::operator*` does round to `f32` at every step — but composing in
`f32` reproduces the bug just as faithfully), and that the rect fast path was
being rejected (it was not; every pixel we painted was the fill colour, never
an antialiased blend). The rect machinery and the matrix width were both
correct and both fed bad numbers.

`pdfrum-parser`'s object-level `parse_real` already used `str::parse`, so the
two parsers in the tree had quietly disagreed with each other; they now agree.

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
