# Design Brief — `pdfrum-font` (and the `pdfrum-type1` sub-crate)

**Behavior source (read-only oracle):** `/mnt/data2/pdfium/pdfium-c++`

*Font dictionaries and decoding* — `core/fpdfapi/font/`:
- `cpdf_font.h` / `.cpp` — the base record, `Create` dispatch, `LoadFontDescriptor`,
  `CheckFontMetrics`, `GetCharPosList`, the fallback-font ladder, `GetAdobeCharName`
- `cpdf_simplefont.h` / `.cpp` — `/Encoding` + `/Differences` resolution
- `cpdf_facebasedsimplefont.h` / `.cpp` — `LoadCommon`, widths, char metrics, substitution
- `cpdf_type1font.h` / `.cpp` — Type1 glyph mapping (5 branches)
- `cpdf_truetypefont.h` / `.cpp` — TrueType glyph mapping (4 rungs)
- `cpdf_type3font.h` / `.cpp`, `cpdf_type3char.h` / `.cpp` — Type3
- `cpdf_cidfont.h` / `.cpp` — CID fonts, `/W` `/DW` `/W2` `/DW2`, `GlyphFromCharCode`
- `cpdf_fontencoding.h` / `.cpp` — the 9 code→Unicode tables + 7 glyph-name tables
- `cpdf_tounicodemap.h` / `.cpp` — ToUnicode CMap parsing (Tier-A critical)
- `cpdf_fontglobals.cpp`, `cpdf_stockfontarray.cpp` — global registries we erase

*Font loading, mapping, substitution* — `core/fxge/`:
- `cfx_standardfont.h` / `.cpp` — the 14 base names, the 89-entry alias table, the Foxit blobs
- `cfx_fontmapper.h` / `.cpp` — `FindSubstFace`, the whole substitution ladder
- `cfx_substfont.h` / `.cpp` — skew/embolden/weight tables
- `cfx_face.h` / `.cpp` — glyph loading, outlines, metrics, charmaps, MM axes
- `cfx_font.h` / `.cpp` — the face wrapper, charset inference, name derivation
- `cfx_glyphcache.h` / `.cpp` — cache key composition
- `cfx_cttgsubtable.h` / `.cpp` — GSUB `vert`/`vrt2` vertical substitution
- `cfx_folderfontinfo.h` / `.cpp`, `systemfontinfo_iface.h` — the interface `fontdb` replaces
- `fx_font.h` / `.cpp` — flags, AGL wrappers, `NormalizeFontMetric`, subset-prefix strip
- `fx_fontencoding.h`, `core/fxcrt/fx_codepage.h` / `.cpp` — charset/codepage enums and tables
- `fontdata/chromefontdata/` — the 16 embedded Foxit blobs
- `core/fxge/skrifa/src/main.rs` — **upstream PDFium's own skrifa bridge**; a normative
  reference for what a Rust font backend must expose

*Tests*: `cpdf_tounicodemap_unittest.cpp`, `cpdf_cidfont_unittest.cpp`,
`cpdf_simplefont_unittest.cpp`, `cpdf_truetypefont_unittest.cpp`,
`cfx_fontmapper_unittest.cpp`, `cfx_standardfont_unittest.cpp`,
`cfx_substfont_unittest.cpp`, `cfx_font_unittest.cpp`,
`cfx_folderfontinfo_unittest.cpp`, `fx_font_unittest.cpp`

**Contract:** SPEC.md §6. **Style:** STYLE.md. **Deps:** DEPS.md (`skrifa`,
`read-fonts`, `fontdb`, `smallvec`, `thiserror`; `pdfrum-cmap` for CMaps and the
PostScript word lexer).

This brief is written to be sufficient on its own: an implementing agent should
need SPEC.md + STYLE.md + `docs/design/pdfrum-cmap.md` + this file, and never the
C++.

**Reading order.** §1 is the behavior inventory in dependency order: the font-type
dispatch (§1.1), what every font shares (§1.2–§1.5), ToUnicode (§1.6, the
Tier-A-critical part), then the four glyph-mapping ladders (§1.7–§1.10), then
substitution (§1.11–§1.14) and the skrifa surface (§1.15). §2 is divergences,
§3 the module plan, §4 tests, §5 open questions. The `pdfrum-type1` scope is
**§3.6**, with its behavior inventory folded into §1.16.

---

## 1. Behavior inventory

### 1.1 `CPDF_Font::Create` — the font-type dispatch

`cpdf_font.cpp:314-347`. Reads `/Subtype` and picks a class:

```
type == "TrueType" -> the Chinese-name special case (below), else CPDF_TrueTypeFont
type == "Type3"    -> CPDF_Type3Font
type == "Type0"    -> CPDF_CIDFont
anything else      -> CPDF_Type1Font      // including "Type1", "MMType1",
                                          // a missing /Subtype, and garbage
```

Then `if (!font->Load()) return nullptr;`.

**The Chinese-name special case** (`:320-330`) is a real, load-bearing quirk. For
`/Subtype /TrueType`, PDFium takes the **first 4 bytes** of `/BaseFont` and
compares them against five hard-coded GBK-encoded family names
(`kChineseFontNames`, `cpdf_font.cpp:41-47`):

| bytes | GBK meaning |
|---|---|
| `CB CE CC E5` | 宋体 (SimSun) |
| `BF AC CC E5` | 楷体 (KaiTi) |
| `BA DA CC E5` | 黑体 (HeiTi) |
| `B7 C2 CB CE` | 仿宋 (FangSong) |
| `D0 C2 CB CE` | 新宋 (XinSong) |

If the prefix matches **and** the font has no `/FontDescriptor` or that
descriptor has no `/FontFile2`, the font is built as a **`CPDF_CIDFont`**, not a
TrueType font. `CPDF_CIDFont::Load` then sees `/Subtype == "TrueType"` and takes
its `LoadGB2312()` path (§1.10). A `break` after the first prefix match means at
most one comparison succeeds; note the `if (!font)` guard at `:331` means a
matched-but-embedded font still falls through to `CPDF_TrueTypeFont`.

**`GetStockFont`** (`cpdf_font.cpp:286-311`) synthesizes a standard-14 font dict:
`GetStandardFontIndex(name)` (§1.11) → canonical name → a dict with
`/Type /Font /Subtype /Type1 /BaseFont <canonical> /Encoding /WinAnsiEncoding`,
memoized per document. Used by `pdfrum-doc`'s appearance generation, not by page
content. The memo is a global (`CPDF_FontGlobals::stock_map_`) we erase.

### 1.2 `/FontDescriptor` — `LoadFontDescriptor`

`cpdf_font.cpp:122-197`. Every non-Type3 font runs this. In order:

```
flags_ = /Flags, defaulting to kFontStyleNonSymbolic (32)
if /ItalicAngle exists:
    italic_angle = its integer; if < 0 { flags_ |= kFontStyleItalic; italic_angle_ = it }
if /StemV exists: stem_v_ = its integer
if /FontWeight exists and > 0: font_weight_ = it            // Some(w) only when > 0
if /Ascent  exists: ascent_  = its integer
if /Descent exists: descent_ = its integer
if all of {ItalicAngle, Ascent, CapHeight, Descent} exist AND (StemV or a valid FontWeight):
    flags_ |= kFontUseExternAttr                            // 0x80000
if descent_ > 10: descent_ = -descent_                      // sign repair
if /FontBBox is an array: font_bbox_ = (a[0], a[1], a[2], a[3]) as (l, b, r, t)
```

Notes that matter:
- **`ItalicAngle >= 0` does not clear the italic flag**, and does not store the
  angle. Only a negative angle both sets the flag and is remembered.
- `/CapHeight`'s *value* is never read — only its presence, and only as a term in
  the `kFontUseExternAttr` conjunction.
- The `descent_ > 10` repair is unconditional and applies to a legitimately
  positive descent too.
- `kFontUseExternAttr` is the single most consequential flag downstream:
  `FindSubstFace` **discards the caller's weight and italic angle entirely**
  when it is absent (§1.12 step 0).

Then the font-program lookup (`:176-197`), which is the same for all font types:

```
font_file = /FontFile ?? /FontFile2 ?? /FontFile3      // first present wins
if none: return
font_file_ = document.GetFontFileStreamAcc(font_file)  // decodes filters
if load_face(font_file_.bytes) fails:
    purge font_file_ (so IsEmbedded() becomes false)
```

**`/FontFile3`'s `/Subtype` is never read** (no `Type1C` / `CIDFontType0C` /
`OpenType` string appears anywhere on the read path). Format detection is
entirely the font library's job. The three keys are interchangeable: a TrueType
program under `/FontFile` loads fine, and so does a CFF program under
`/FontFile2`.

**`/Length1 /Length2 /Length3`** (`core/fpdfapi/page/cpdf_docpagedata.cpp:481-505`)
are summed (saturating to 0 on overflow or on any negative value) and passed as
`estimated_size` to the filter chain — a **buffer-sizing hint only**
(`pdfrum-filters` brief §1.3). They are **never used to split PFB segments** and
are discarded after decode. Any Type1 container sniffing is done by the font
library on the whole blob.

**Damage tolerance:** a face that fails to construct nulls `font_file_`, so
`IsEmbedded()` becomes false, so the *later* `if (font_file_) … else
LoadSubstFont()` in `LoadCommon` (§1.4) takes the substitution branch. The
ordering is exact: `LoadFontDescriptor` runs at
`cpdf_facebasedsimplefont.cpp:146-148`, the branch at `:150-155`.

### 1.3 `CheckFontMetrics` — deriving a bbox when the PDF lies

`cpdf_font.cpp:199-238`. Runs at the end of every simple-font and CID-font load.

**Step 1** — if `font_bbox_` is all zeros (`top == bottom == left == right == 0`):
- **With a face**: take the face's raw bbox and units-per-em and write, *with
  the top and bottom deliberately swapped* (the C++ comment at `:204` says so):
  ```
  font_bbox_.left   = normalize(raw.left,   upem)
  font_bbox_.bottom = normalize(raw.top,    upem)     // <- raw.top
  font_bbox_.right  = normalize(raw.right,  upem)
  font_bbox_.top    = normalize(raw.bottom, upem)     // <- raw.bottom
  ascent_  = normalize(face.ascender,  upem)
  descent_ = normalize(face.descender, upem)
  ```
  `CFX_Face::GetBBox()` returns an `FX_RECT` in y-down convention already, so
  this "flip" is what makes the result y-up in text space. Port the assignment
  literally rather than reasoning about it.
- **Without a face**: loop `i` over 0..255 calling `GetCharBBox(i)`, skipping any
  rect with `left == right`, and take the union
  (`min` left, `max` top, `max` right, `min` bottom), seeding from the first
  non-degenerate rect.

**Step 2** — if `ascent_ == 0 && descent_ == 0`:
```
r = GetCharBBox('A'); ascent_  = (r.bottom == r.top) ? font_bbox_.top    : r.top
r = GetCharBBox('g'); descent_ = (r.bottom == r.top) ? font_bbox_.bottom : r.bottom
```

**`NormalizeFontMetric(value: i64, upem: u16) -> i32`** (`fx_font.cpp:208-215`):
```
if upem == 0 { return saturating_cast_i32(value) }
saturating_cast_i32((value as f64 * 1000.0 + (upem / 2) as f64) / upem as f64)
```
Note `upem / 2` is **integer** division before the float divide. This is the
1000/em normalization used for ascent/descent/bbox/`GetGlyphTTWidth`.

A **second, different** normalizer exists: `CFX_Face::EmAdjust(v: i32) -> i32`
(`cfx_face.cpp:589-591`) = `if upem == 0 { v } else { v * 1000 / upem }` —
truncating integer division, no rounding, no saturation. Used only by
`CFX_Face::GetGlyphWidth`. **They give different results** for the same input
(e.g. `v = 1, upem = 3`: `NormalizeFontMetric` → `(1000 + 1)/3 = 333`;
`EmAdjust` → `1000/3 = 333`; but `v = 2, upem = 3`: `(2000+1)/3 = 667` vs
`2000/3 = 666`). Keep both, named distinctly.

A **third** appears in `CFX_Font::GetBBox()` (`cfx_font.cpp:376-391`), float-based:
`saturating_cast_i32(coord as f32 * 1000.0 / upem as f32)` per edge, applied only
when `upem != 0`.

### 1.4 `LoadCommon` — the simple-font load sequence

`cpdf_facebasedsimplefont.cpp:144-186`. Type1 and TrueType both run this; the
**order is behavior**, because each step reads state the previous one wrote.

```
1. font_desc = /FontDescriptor;  if present: LoadFontDescriptor(font_desc)   (§1.2)
2. LoadCharWidths(font_desc)                                                 (§1.5)
3. if font_file_.is_some():  strip_subset_prefix(&mut base_font_name)
   else:                     LoadSubstFont()                                 (§1.13)
4. if !FontStyleIsSymbolic(flags_):  base_encoding_ = FontEncoding::Standard
5. LoadPDFEncoding(embedded = font_file_.is_some(), truetype = font.is_ttf())  (§1.7)
6. LoadGlyphMap()                            // virtual: §1.8 (Type1) / §1.9 (TT)
7. char_names_.clear()                       // /Differences names freed here
8. if !has_face(): return true               // success with no glyphs at all
9. if FontStyleIsAllCaps(flags_): the all-caps aliasing (below)
10. CheckFontMetrics()                                                       (§1.3)
11. return true                              // ALWAYS
```

**`LoadCommon` cannot fail.** Every path returns `true`, so
`CPDF_Font::Create`'s `if (!font->Load()) return nullptr` never fires for a
simple font. A simple font always constructs, even with no face and no glyphs.

**Step 4 is a reset, not a default.** `CPDF_Type1Font::Load` may already have set
`base_encoding_` to `AdobeSymbol` / `ZapfDingbats` / `Standard` (§1.8); step 4
then **overwrites** it with `Standard` for any non-symbolic font. Since Symbol
and ZapfDingbats always have `kFontStyleSymbolic` set (`cpdf_type1font.cpp:98-99`
when there is no descriptor `/Flags`), their encodings survive — but a
`/Flags` entry that omits the symbolic bit on a font named `Symbol` will clobber
it. Reproduce.

**The all-caps aliasing** (`:166-183`). When `flags_ & kFontStyleAllCap` (bit 16):
for each range in `{('a','z'), (0xE0, 0xF6), (0xF8, 0xFD)}`, and each `i` in that
inclusive range:
```
if glyph_index_[i] != 0xffff && font_file_.is_some() { continue }   // embedded: keep
j = i - 32
glyph_index_[i] = glyph_index_[j]
if char_width_[j] != 0 { char_width_[i] = char_width_[j]; char_bbox_[i] = char_bbox_[j] }
```
Note the guard is `!= 0xffff && embedded` — a non-embedded font has its lowercase
glyphs replaced **even when they mapped successfully**. The `char_width_[j] != 0`
test means an unset width (0xffff, which is nonzero) *does* propagate.

### 1.5 Widths — `/Widths`, `/FirstChar`, `/LastChar`, `/MissingWidth`

`CPDF_FaceBasedSimpleFont::LoadCharWidths`, `cpdf_facebasedsimplefont.cpp:88-116`.
`char_width_` is `[u16; 256]` initialized to **`0xffff`** ("unset") in the
constructor (`:21`), alongside `glyph_index_` (also `0xffff`) and `char_bbox_`
(all `FX_RECT(-1,-1,-1,-1)`).

```
width_array = /Widths
use_font_width_ = width_array.is_none()          // "take widths from the face"
if width_array.is_none() { return }

if font_desc has /MissingWidth:
    char_width_.fill(/MissingWidth as integer)   // fills ALL 256, including
                                                 // outside [FirstChar, LastChar]
start = /FirstChar as integer, default 0         // read as size_t: negative wraps huge
end   = /LastChar  as integer, default 0
if start > 255 { return }                        // widths silently dropped
if end == 0 || end >= start + width_array.len() {
    end = start + width_array.len() - 1
}
if end > 255 { end = 255 }
for i in start..=end:
    char_width_[i] = width_array[i - start] as integer
```

Quirks to reproduce exactly:
- `/MissingWidth` fills the **whole** table, so a code outside
  `[FirstChar, LastChar]` gets `MissingWidth`, and a code inside gets the array
  value. Without `/MissingWidth`, out-of-range codes stay `0xffff` and fall
  through to the face (§ below).
- `/LastChar` of **0** is treated as "absent" and recomputed from the array
  length — so `/FirstChar 0 /LastChar 0 /Widths [500]` yields
  `end = 0 + 1 - 1 = 0`, i.e. exactly code 0. Same result, different reason.
- A `/LastChar` **larger** than the array implies is clamped to the array's
  extent; a smaller one is honoured, leaving the tail of `/Widths` unused.
- `width_array[i - start]` is `GetIntegerAt`, which yields **0** for a missing or
  non-numeric element (`pdfrum-object` brief §1.2), not an error.
- `/FirstChar` is read into a `size_t`. A negative `/FirstChar` becomes a huge
  value and trips the `> 255` early return, dropping all widths. Our port reads
  `i64` and returns early for anything outside `0..=255`, which is equivalent.

**`GetCharWidth(charcode)`** (`:118-130`):
```
if charcode > 0xff { charcode = 0 }
if char_width_[charcode] == 0xffff {
    LoadCharMetrics(charcode)
    if still 0xffff { char_width_[charcode] = 0 }
}
char_width_[charcode]
```
So an unset width resolves lazily from the face, and permanently caches 0 if the
face has nothing. Note `charcode > 0xff` maps to **code 0**, not to a miss.

**`LoadCharMetrics(charcode)`** (`:46-86`) — where face metrics enter:
```
if !has_face() { return }
if charcode not in 0..=0xff { return }
gid = glyph_index_[charcode]
if gid == 0xffff {
    // no glyph: borrow space's metrics, but only for non-embedded fonts
    if font_file_.is_none() && charcode != 32 {
        LoadCharMetrics(32)                       // recursion, depth 1
        char_bbox_[charcode] = char_bbox_[32]
        if use_font_width_ { char_width_[charcode] = char_width_[32] }
    }
    return
}
if face.load_glyph(gid, scale=false).is_err() { return }
char_bbox_[charcode] = face.glyph_bbox(gid)
if use_font_width_ {
    tt_width = face.glyph_tt_width(gid)           // NormalizeFontMetric of horiAdvance
    if char_width_[charcode] == 0xffff {
        char_width_[charcode] = tt_width
    } else if tt_width != 0 && !IsEmbedded() {
        // rescale the bbox horizontally to match the PDF's declared width
        char_bbox_[charcode].right = char_bbox_[charcode].right * char_width_[charcode] / tt_width
        char_bbox_[charcode].left  = char_bbox_[charcode].left  * char_width_[charcode] / tt_width
    }
}
```
The `else if` branch is unreachable in practice (`use_font_width_` is true only
when `/Widths` is absent, in which case `char_width_` is all `0xffff` unless
`/MissingWidth` filled it — which requires `/Widths`… so it needs
`/MissingWidth` **and** no `/Widths`, which `LoadCharWidths` returns before
doing). Keep the code path anyway with a comment; it costs nothing and a future
reader will otherwise "simplify" it away.

**`GetCharBBox(charcode)`** (`:132-142`): `> 0xff` → code 0; if
`char_bbox_[charcode].left == -1`, `LoadCharMetrics`; return it.

**`HasFontWidths()`** (`:218-220`) = `!use_font_width_` — i.e. "the PDF declared
widths". Consumed by the glyph-spacing heuristic (§1.14).

### 1.6 ToUnicode — **the Tier-A-critical component**

`cpdf_tounicodemap.h` / `.cpp`. Text extraction's byte-exactness against the
oracle depends on reproducing this *precisely*, including its rejection rules and
its collision policy. Every statement in this section is load-bearing.

#### 1.6.1 Storage model

Two `std::map<u32, u32>` (ordered, `map_` charcode→value and `reverse_map_`
value→charcode) plus `multi_char_vec_: Vec<WideString>` and an optional
`base_map_: &CPDF_CID2UnicodeMap`.

The value stored in `map_` is **either** a single UTF-16 code unit **or** a
packed index into `multi_char_vec_`:

```
GetMultiCharIndexIndicator() = saturating(multi_char_vec_.len() * 0x10000 + 0xffff)
                               with overflow -> 0
```
so a multi-char entry's value is `(index << 16) | 0xFFFF`.

**`Lookup(charcode)`** (`:57-75`):
```
match map_.get(charcode):
  None    => if base_map_.is_none() { "" }
             else { one char: base_map_.unicode_from_cid(charcode as u16) }
  Some(v) => let unicode = (v & 0xffff) as u16;
             if unicode != 0xffff { one-char string of `unicode` }
             else { let idx = v >> 16;
                    multi_char_vec_.get(idx).cloned().unwrap_or_default() }
```

**Consequence — a single mapping to exactly U+FFFF is unrepresentable.** It is
read back as multi-char index 0, which is either some *other* entry's string or
empty. This is a real, observable behavior: the unittest
`HandleBeginBFRangeDestLargeValue` (`:338-358`) pins
`Lookup(0x1f) == ""` for a range whose 16th element would be `0xFFFF`, with a
standing `TODO(thestig)` asking whether it should be `"\xffff"`. **Reproduce the
`""`.**

Note the `base_map_` fallback is consulted only on a **miss**, and it returns a
`char` even when that char is 0 (an unmapped CID), producing a one-element
`WideString` containing NUL. Callers treat a non-empty result as success.

**`ReverseLookup(unicode)`** (`:77-80`): `reverse_map_.get(unicode).copied()
.unwrap_or(0)`. Note the key is the *value* stored in `map_`, so a multi-char
entry's reverse key is its packed indicator, not any real character — which is
why the `NonBmpUnicodeLookup` unittest (`:395-404`) asserts
`ReverseLookup(0x20676) == 0` for a surrogate pair that `Lookup` returns
correctly.

**`InsertIntoMaps(code, destcode)`** (`:375-385`) — the **collision policy**:
```
map_.entry(code).and_modify(|v| *v = min(*v, destcode)).or_insert(destcode);
reverse_map_.entry(destcode).and_modify(|c| *c = min(*c, code)).or_insert(code);
```
**Lowest value wins in both directions.** Two `bfchar` entries mapping code 0 to
U+0041 and U+0042 leave `Lookup(0) == "A"`. Two entries mapping different codes
to the same unicode leave the reverse map pointing at the lower code.

**`SetCode(srccode, destcode: WideString)`** (`:362-373`):
```
if destcode.is_empty() { return }                 // silently dropped
if destcode.len() == 1 { InsertIntoMaps(srccode, destcode[0] as u32) }
else { InsertIntoMaps(srccode, multi_char_indicator());
       multi_char_vec_.push(destcode) }
```
Note the push happens **after** the insert, so the indicator's index is correct.
But if `InsertIntoMaps` loses the `min` race (an earlier, smaller value already
present), **the string is still pushed** — `multi_char_vec_` accumulates
unreachable entries. Harmless but must not be "optimized" away, because it shifts
every subsequent indicator index.

#### 1.6.2 The load loop

`Load(stream)` (`:144-175`). Decodes the stream through the filter chain, then
runs the **same `CPDF_SimpleParser` word lexer** as the embedded-CMap parser
(`pdfrum-cmap` brief §1.5 — reuse `pdfrum_cmap::lexer::Words` verbatim). Per
word:

```
"beginbfchar"          -> word = HandleBeginBFChar(parser, previous_word)
"beginbfrange"         -> word = HandleBeginBFRange(parser, previous_word)
"/Adobe-Korea1-UCS2"   -> cid_set = Korea1
"/Adobe-Japan1-UCS2"   -> cid_set = Japan1
"/Adobe-CNS1-UCS2"     -> cid_set = CNS1
"/Adobe-GB1-UCS2"      -> cid_set = GB1
previous_word = word                       // note: the *returned* word after a handler
```
Loop ends on the first empty word. Afterwards, if `cid_set != Unknown`,
`base_map_` is set to that registry's static CID→Unicode table
(`pdfrum-cmap` brief §1.13).

The four `/Adobe-*-UCS2` names are matched **exactly, as whole tokens including
the leading slash**. They appear in real ToUnicode CMaps as the `usecmap`
operand. Note there is **no `/Adobe-Identity-UCS`** in the list — that spelling
leaves `base_map_` unset.

`previous_word` is the count token that precedes `beginbfchar`/`beginbfrange`.
After a handler returns, `previous_word` becomes the handler's return value (the
terminating `endbfchar`/`endbfrange`, or the empty word on truncation).

#### 1.6.3 `StringToCode` — the charcode scanner

`:90-112`, `static`, unit-tested. **Different from `CPDF_CMapParser::GetCode`**
(`pdfrum-cmap` brief §1.7) — this one *validates*.

```
if len <= 2 || str[0] != '<' || str[len-1] != '>' { return None }
code: u32 = 0
for c in str[1..len-1] {
    if is_pdf_whitespace(c) { continue }          // whitespace ignored, crbug 42271078
    if !is_hex_digit(c) { return None }
    code = code * 16 + hex_value(c);  if overflow { return None }
}
Some(code)
```

Pinned assertions (`cpdf_tounicodemap_unittest.cpp:13-43`):

| input | result |
|---|---|
| `"<0001>"` | `Some(1)` |
| `"<c2>"` | `Some(194)` |
| `"<A2>"` | `Some(162)` |
| `"<Af2>"` | `Some(2802)` |
| `"<FFFFFFFF>"` | `Some(4294967295)` |
| `"<00\n0\r1>"` | `Some(1)` |
| `"<c 2>"` | `Some(194)` |
| `"<A2\r\n>"` | `Some(162)` |
| `"<100000000>"` | `None` (u32 overflow) |
| `"<1abcdFFFF>"` | `None` (u32 overflow) |
| `""` | `None` |
| `"<>"` | `None` (len 2, fails `len <= 2`) |
| `"12"` | `None` |
| `"<12"` | `None` |
| `"12>"` | `None` |
| `"<1-7>"` | `None` (`-` is neither whitespace nor hex) |
| `"00AB"` | `None` |
| `"<00NN>"` | `None` |

**PDF whitespace** here is `\0 \t \n \f \r space` (`PDFCharIsWhitespace`).

#### 1.6.4 `StringToWideString` — the destination scanner

`:115-142`, `static`, unit-tested. **Never fails**; returns a possibly-empty
string.

```
if len <= 2 || str[0] != '<' || str[len-1] != '>' { return "" }
result = ""; byte_pos = 0; ch: u16 = 0
for c in str[1..len-1] {
    if is_pdf_whitespace(c) { continue }
    if !is_hex_digit(c) { break }                  // stop, keep what we have
    ch = ch * 16 + hex_value(c)                    // wrapping u16
    byte_pos += 1
    if byte_pos == 4 { result.push(ch); byte_pos = 0; ch = 0 }
}
result
```

**Only complete groups of 4 hex digits are emitted.** A trailing partial group is
discarded. The result is a sequence of **UTF-16 code units**, not chars —
surrogate pairs arrive as two units and are kept as such.

Pinned assertions (`:45-64`):

| input | result |
|---|---|
| `""`, `"1234"`, `"<c2"`, `"<c2D2"`, `"c2ab>"` | `""` |
| `"<c2ab>"` | `[0xC2AB]` |
| `"<c2abab>"` | `[0xC2AB]` (trailing `ab` discarded) |
| `"<c2abFaAb>"` | `[0xC2AB, 0xFAAB]` |
| `"<c2abFaAb12>"` | `[0xC2AB, 0xFAAB]` |
| `"<c2ab FaAb>"` | `[0xC2AB, 0xFAAB]` |
| `"<c2ab FaAb12>"` | `[0xC2AB, 0xFAAB]` |
| `"<c2ab FaAb 12>"` | `[0xC2AB, 0xFAAB]` |
| `"< c 2 a b  F a A b  1 2 >"` | `[0xC2AB, 0xFAAB]` |

#### 1.6.5 `HandleBeginBFChar` — the two-phase count check

`:177-221`. **This is not a streaming parser.** It collects everything, then
commits only if the collected count exactly equals the declared count.

```
kCidLimit = 0xffff
kOutOfSpecBFLimit = 160000        // deliberately > the spec's 100

raw_count = parse_int(previous_word)                 // StringToInt; 0 on garbage
is_valid = raw_count >= 0 && raw_count <= 160000
expected = if is_valid { raw_count as usize } else { 0 }
code_words: Vec<(u32, &[u8])> = Vec::with_capacity(expected)

loop {
    word = next_word()
    if word.is_empty() || word == "endbfchar" { break }
    if !is_valid { continue }                        // drain, do nothing
    code = StringToCode(word)
    if code.is_none() || code > 0xffff { is_valid = false; continue }
    word = next_word()                               // the destination
    code_words.push((code, word))
    if code_words.len() > expected { is_valid = false }
}
if is_valid && code_words.len() == expected {
    for (code, w) in code_words { SetCode(code, StringToWideString(w)) }
}
return word          // the terminating token
```

**Nothing is committed unless the count matches exactly.** Both too few and too
many entries discard the whole block. Pinned by `HandleBeginBFCharBadCount`
(`:82-103`): `"1 beginbfchar<1><0041><2><0042>endbfchar"` and
`"3 beginbfchar<1><0041><2><0042>endbfchar"` both yield an empty map.

`HandleBeginBFCharTolerateOutOfSpecCount` (`:105-228`) pins that a block
declaring **112** entries (>100) is honoured: `ReverseLookup(0x0001) == 9`,
`ReverseLookup(0x0067) == 111`, and both have exactly 1 unicode.

`HandleBeginBFCharRejectsInvalidCidValues` (`:66-80`) pins that
`"1 beginbfchar<00NN><0041>endbfchar"` (bad hex) and
`"1 beginbfchar<100000000><0041>endbfchar"` (overflow) both yield nothing:
`Lookup(0) == ""`, `ReverseLookup(0x41) == 0`, count 0.

Note the **`code > kCidLimit` rejection is a hard invalidation of the whole
block**, not a skip of that entry. And note `is_valid = false; continue` keeps
draining words to find `endbfchar`, so the outer loop resumes correctly after the
block.

#### 1.6.6 `HandleBeginBFRange` — three range shapes

`:223-354`. Same two-phase discipline; three variants of range destination.

```
raw_count / is_valid / expected exactly as above
ranges: Vec<Range> = Vec::with_capacity(expected)

loop {
    word = next_word()
    if word.is_empty() || word == "endbfrange" { break }
    if !is_valid { continue }

    lowcode = StringToCode(word);  if none { is_valid = false; continue }
    word = next_word()
    highraw = StringToCode(word);  if none { is_valid = false; continue }

    // ***THE HIGH-CODE MASK***
    highcode = (lowcode & 0xffffff00) | (highraw & 0xff)

    if lowcode > 0xffff || highcode > 0xffff || lowcode > highcode {
        is_valid = false; continue
    }

    word = next_word()
    if word == "[" {
        // ARRAY FORM
        let mut words = Vec::with_capacity(1 + highcode - lowcode);
        for _ in lowcode..=highcode { words.push(next_word()) }
        ranges.push(CodeWordRange { low: lowcode, words });
        if ranges.len() > expected { is_valid = false; continue }
        word = next_word()
        if word != "]" { is_valid = false }
        continue
    }

    destcode = StringToWideString(word)
    if destcode.len() == 1 {
        // SINGLE-DEST FORM: consecutive unicode values
        ranges.push(SingleDest { low: lowcode, high: highcode, start: destcode[0] as u32 })
    } else {
        // MULTI-DEST FORM: increment the *string* per code
        let mut retcodes = Vec::with_capacity(1 + highcode - lowcode);
        retcodes.push(destcode);
        for _ in (lowcode+1)..=highcode {
            retcodes.push(StringDataAdd(retcodes.last()))
        }
        ranges.push(MultiDest { low: lowcode, retcodes })
    }
    if ranges.len() > expected { is_valid = false }
}
if is_valid && ranges.len() == expected { commit all }
return word
```

**The high-code mask (`:272`) is the single most surprising rule here.**
`highcode = (lowcode & 0xffffff00) | (highcode & 0xff)` forces the range to lie
within one 256-code block starting at `lowcode`'s block base. Consequences:

- `<0010> <00ff>` → high stays `0x00FF`; range is 0x10..=0xFF (240 codes).
- `<0001> <10000>` → `highcode = (0x0001 & 0xffffff00) | 0x00 = 0x0000`, so
  `lowcode (1) > highcode (0)` ⇒ **`is_valid = false`**, whole block dropped.
  Pinned by `HandleBeginBFRangeRejectsInvalidCidValues` case 4 (`:253-261`):
  `Lookup(0x0001)`, `Lookup(0xffff)`, `Lookup(0x10000)` all `""`.
- `<0100> <02FF>` → `highcode = 0x0100 | 0xFF = 0x01FF`. The declared 0x02FF is
  **silently truncated to 0x01FF** — a 512-code range becomes 256.
- `<FFFFFFFF> <FFFFFFFF>` → `highcode = 0xFFFFFF00 | 0xFF = 0xFFFFFFFF`, then
  `lowcode > 0xffff` ⇒ invalid. Pinned three ways (`:230-251`, array / single /
  multi destination forms).
- `<10000> <10001>` → `lowcode > 0xffff` ⇒ invalid (`:262-269`).
- `<0006> <0004>` → `highcode = 0x0000 | 0x04 = 0x0004 < 6` ⇒ invalid
  (`:270-278`): `Lookup(4)`, `Lookup(5)`, `Lookup(6)` all `""`.

**Array form.** Exactly `high - low + 1` words are consumed unconditionally,
*then* a `]` is required. `HandleBeginBFRangeRejectsMismatchedBracket`
(`:281-287`) pins that `"1 beginbfrange<3><3>[<0041>}endbfrange"` (a `}` instead
of `]`) yields nothing. Note the words are consumed **before** the count check,
so a runaway range in a malformed stream can eat up to 65536 tokens; the
`kCidLimit` gate bounds it.

**Single-dest form.** `destcode` must be exactly 1 UTF-16 unit. The value is
incremented per code with plain `u32` arithmetic (`InsertIntoMaps(code,
value++)`). It is **not** clamped: `HandleBeginBFRangeDestLargeValue`
(`:338-358`) uses `<0010><00ff><fff0>` and pins:
```
Lookup(0x10) == "\u{fff0}"
Lookup(0x11) == "\u{fff1}"
Lookup(0x1f) == ""            // value would be 0xFFFF -> the indicator collision (§1.6.1)
Lookup(0x20) == ""            // value 0x10000: (v & 0xffff) == 0 -> one NUL char?
ReverseLookup(0xfff0) == 16
ReverseLookup(0xfff1) == 17
ReverseLookup(0xffff) == 31
ReverseLookup(0x10000) == 32  // only when wchar_t is 32-bit; see D2
```
Wait — `Lookup(0x20)` is asserted `""`. With value `0x10000`,
`unicode = (0x10000 & 0xffff) = 0`, which is `!= 0xffff`, so `Lookup` returns a
one-char string containing NUL — and the test's `EXPECT_EQ(L"", ...)` compares a
`WideString` of length 1 containing `\0` against the empty literal. In PDFium's
`WideString`, `L""` constructs length 0, so this assertion says the result really
is empty… **This is exactly the kind of detail that must be measured, not
reasoned about.** See OQ-3: the implementing agent runs a one-off probe and
records the answer. Our `Lookup` returns `SmallVec<[char;2]>`; the two candidate
behaviors are "empty" and "one NUL char", and downstream (`pdfrum-text`) treats
them very differently.

**Multi-dest form** and `StringDataAdd` (`:31-47`) — the string increment:
```
StringDataAdd(str) -> String:
    ret = ""; value: u16 = 1
    for i in (0..str.len()).rev() {
        ch = str[i].wrapping_add(value)
        if ch < str[i] { ret.insert_front(0) }        // carried: emit NUL, keep value=1
        else           { ret.insert_front(ch); value = 0 }
    }
    if value != 0 { ret.insert_front(value) }         // overall carry: prepend U+0001
    ret
```
This is base-65536 big-endian increment over UTF-16 units, with a quirk: a
position that carries emits **U+0000**, not the wrapped value, and the carry
continues. A full carry out of the leading unit **prepends U+0001**, lengthening
the string. E.g. `[0xFFFF]` → `[0x0001, 0x0000]`.

**Count checks.** `HandleBeginBFRangeBadCount` (`:289-314`) pins that
`"1 beginbfrange<1><2><0040><4><5><0050>endbfrange"` (2 ranges, declared 1) and
the same with `"3"` both commit nothing. `HandleBeginBFRangeGoodCount`
(`:316-336`) pins the `"2"` case works:
`ReverseLookup(0x40) == 1`, `(0x41) == 2`, `(0x42) == 0`, `(0x50) == 4`,
`(0x51) == 5`, `(0x52) == 0`, and per-charcode unicode counts
`{0:0, 1:1, 2:1, 3:0, 4:1, 5:1, 6:0}`.

**`InsertIntoMaps` collision semantics** are pinned by `InsertIntoMaps`
(`:360-393`):
1. `"2 beginbfchar<1><0041><2><0042>endbfchar"` — distinct both ways:
   `ReverseLookup(0x41)==1`, `(0x42)==2`, counts `{1:1, 2:1}`.
2. `"2 beginbfrange<0><0><0041><0><0><0042>endbfrange"` — same CID, two unicodes:
   `ReverseLookup(0x41)==0`, `(0x42)==0`, and **`count(0) == 2`** — the reverse
   map has two entries pointing at charcode 0, while `map_[0]` holds `min` =
   0x41.
3. `"1 beginbfrange<0><0>[<0041>]endbfrange\n1 beginbfchar<0><0041>endbfchar"` —
   the *same* pair twice: `ReverseLookup(0x41)==0` and **`count(0) == 1`**.

`GetUnicodeCountByCharcodeForTesting(charcode)` counts entries in `reverse_map_`
whose value equals `charcode`. We expose the same as a `#[cfg(test)]` helper.

**Non-BMP** (`NonBmpUnicodeLookup`, `:395-404`):
`"1 beginbfchar<01><d841de76>endbfchar"` ⇒ `Lookup(1)` is the two-unit sequence
`[0xD841, 0xDE76]` (which is U+20676), and `ReverseLookup(0x20676) == 0` — the
reverse map is keyed on the multi-char indicator, not the scalar.

**Our `CharItem.unicode` is `SmallVec<[char; 2]>`** per SPEC §6, i.e. *chars*,
not UTF-16 units. The conversion from the stored UTF-16 sequence to `char`s
happens at the `Lookup` boundary: pair up valid surrogates, and map an unpaired
surrogate to… see D3.

### 1.7 `/Encoding` resolution for simple fonts

`CPDF_SimpleFont::LoadPDFEncoding(embedded, truetype)`,
`cpdf_simplefont.cpp:66-115`. `base_encoding_` starts at `FontEncoding::Builtin`
(`cpdf_simplefont.h:45`) and may have been set by `Type1Font::Load` (§1.8) and
then reset by `LoadCommon` step 4 (§1.4).

```
enc = /Encoding (resolved)
if enc.is_none() {
    if base_font_name == "Symbol" {
        base_encoding_ = if truetype { MsSymbol } else { AdobeSymbol }
    } else if !embedded && base_encoding_ == Builtin {
        base_encoding_ = WinAnsi
    }
    return
}
if enc.is_name() {
    if base_encoding_ is AdobeSymbol or ZapfDingbats { return }       // pinned
    if FontStyleIsSymbolic(flags_) && base_font_name == "Symbol" {
        if !truetype { base_encoding_ = AdobeSymbol }
        return
    }
    let mut s = enc.as_name()
    if s == "MacExpertEncoding" { s = "WinAnsiEncoding" }             // unconditional!
    set_predefined(s, &mut base_encoding_)
    return
}
let dict = enc.as_dict();  if none { return }                         // e.g. an array
if base_encoding_ is not AdobeSymbol and not ZapfDingbats {
    let mut s = dict.name("BaseEncoding")
    if truetype && s == "MacExpertEncoding" { s = "WinAnsiEncoding" }  // truetype only!
    set_predefined(s, &mut base_encoding_)
}
if (!embedded || truetype) && base_encoding_ == Builtin {
    base_encoding_ = Standard
}
LoadDifferences(dict)
```

`set_predefined` (`GetPredefinedEncoding`, `:20-30`) recognizes exactly four
names and **leaves the value unchanged** for anything else (including
`/StandardEncoding` and `/MacExpertEncoding`):

| name | encoding |
|---|---|
| `WinAnsiEncoding` | `WinAnsi` |
| `MacRomanEncoding` | `MacRoman` |
| `MacExpertEncoding` | `MacExpert` |
| `PDFDocEncoding` | `PdfDoc` |

Note `/StandardEncoding` as an explicit `/Encoding` name is a **no-op** — the
encoding stays whatever it was (usually `Standard` already, from `LoadCommon`
step 4). And `MacExpertEncoding` reaches the `MacExpert` arm only through the
`/BaseEncoding` path of a **non-TrueType** font, because the name form rewrites
it to WinAnsi unconditionally and the dict form rewrites it for TrueType.

**`LoadDifferences`** (`:40-64`):
```
diffs = dict.array("Differences");  if none { return }
char_names_.resize(256)                    // allocated only when /Differences exists
cur_code: u32 = 0
for element in diffs (each resolved through references) {
    if element is None { continue }        // a null or dangling ref is SKIPPED,
                                           // and does NOT advance cur_code
    if element is a Name {
        if cur_code < 256 { char_names_[cur_code] = name }
        cur_code += 1                      // advances even past 256
    } else {
        cur_code = element.as_integer()    // ANY non-name resets the cursor
    }
}
```
`as_integer()` on a real, a string, or a dict yields 0 (`pdfrum-object` §1.2), so
a stray `/Foo (bar) /Baz` sequence resets `cur_code` to 0 mid-array. A negative
integer becomes a huge `u32` and the subsequent names are dropped by the
`< 256` guard while `cur_code` keeps incrementing.

**`GetAdobeCharName(base_encoding, char_names, charcode)`**
(`cpdf_font.cpp:512-534`) — the merge point every glyph mapper calls:
```
if charcode >= 256 { return None }
if !char_names.is_empty() && !char_names[charcode].is_empty() {
    return Some(char_names[charcode])        // /Differences ALWAYS wins
}
if base_encoding == Builtin { return None }
CharNameFromPredefinedCharSet(base_encoding, charcode)   // may be None
```

**`CharNameFromPredefinedCharSet`** (`cpdf_fontencoding.cpp:1791-1824`):
```
first = if encoding == PdfDoc { 24 } else { 32 }
if charcode < first { return None }
table[charcode - first]                      // may be a null entry
```
with `first == 32` giving 224-entry tables and `first == 24` giving 232 for
PDFDoc. The seven name tables are `kStandardEncodingNames` (`:253`),
`kAdobeWinAnsiEncodingNames` (`:479`), `kMacRomanEncodingNames` (`:707`),
`kMacExpertEncodingNames` (`:933`), `kPDFDocEncodingNames` (`:1159`),
`kAdobeSymbolEncodingNames` (`:1393`), `kZapfEncodingNames` (`:1619`).
`MsSymbol` and `Builtin` fall through to `None` (the `default:` arm).

**The nine code→Unicode tables**, each `[u16; 256]`, `cpdf_fontencoding.cpp`:
`kMSSymbolEncoding` (`:23`), `kStandardEncoding` (`:57`), `kMacRomanEncoding`
(`:89`), `kAdobeWinAnsiEncoding` (`:120`), `kMacExpertEncoding` (`:151`),
`kAdobeSymbolEncoding` (`:182`), `kZapfEncoding` (`:214`), `kPDFDocEncoding`
(`:24` region). `UnicodesForPredefinedCharSet(encoding)`
(`:1767-1789`) returns the matching slice, and **`Builtin` returns an empty
slice** — the only encoding without a table.

`CPDF_FontEncoding` is `[wchar_t; 256]`, constructed from a predefined table
(empty ⇒ all zeros, `:1669-1679`), with `SetUnicode(code, u)` writes from the
glyph mappers and `UnicodeFromCharCode(code)` reads.
`CharCodeFromUnicode(u)` (`:1660-1667`) is a **linear scan returning the first
index**, or **-1** (not 0) on miss.

`CharCodeFromUnicodeForEncoding(fxge_encoding, unicode)` (`:1741-1761`) is the
reverse over the *raw* tables via `PDF_FindCode` (a linear scan returning 0 on
miss, `:1650-1657`):

| `fxge::FontEncoding` | table |
|---|---|
| `kUnicode` | identity — returns `unicode` |
| `kAdobeStandard` | `kStandardEncoding` |
| `kAdobeExpert` | `kMacExpertEncoding` |
| `kLatin1` | `kAdobeWinAnsiEncoding` |
| `kAppleRoman` | `kMacRomanEncoding` |
| `kAdobeCustom` | `kPDFDocEncoding` |
| `kSymbol` | `kMSSymbolEncoding` |
| anything else | 0 |

`UnicodeFromAppleRomanCharCode(c)` (`:1763-1765`) = `kMacRomanEncoding[c]`.

**These 16 tables are ~1500 lines of pure data.** They are shared by Type1,
TrueType, Type3 and (for the Adobe-name path) CID fonts, so they live in their
own module inside `pdfrum-font`, not in `pdfrum-type1` (§3.2).

**The AGL** — `UnicodeFromAdobeName(name)` / `AdobeNameFromUnicode(unicode)`
(`fx_font.cpp:198-206`) are thin wrappers over FreeType's `pstables.h`
(~56 KB compressed trie, vendored into fxge). Note
`UnicodeFromAdobeName` masks with `& 0x7FFFFFFF`, **stripping FreeType's variant
bit**, so `A.swash` resolves to `A`. `AdobeNameFromUnicode` writes into a
64-byte buffer via an O(table) DFS. Pinned assertions
(`fx_font_unittest.cpp:16-34`):

| name | unicode | | unicode | name |
|---|---|---|---|---|
| `"nonesuch"` | 0x0000 | | 0x0000 | `""` |
| `""` | 0x0000 | | 0x00F7 | `"divide"` |
| `"paragraph"` | 0x00B6 | | 0x0141 | `"Lslash"` |
| `"Oacute"` | 0x00D3 | | 0x0384 | `"tonos"` |
| `"thorn"` | 0x00FE | | 0x0691 | `"afii57513"` |
| `"tonos"` | 0x0384 | | 0x0E5A | `"angkhankhuthai"` |
| `"bullet"` | 0x2022 | | 0x20AC | `"Euro"` |

Also handled by the forward wrapper (`fx_freetype.cpp:126-214`): `uniXXXX`
(exactly 4 uppercase hex) and `uXXXX`–`uXXXXXX` (4–6 hex), plus a `.`-suffix
variant strip. `read-fonts`' `agl` feature provides the same table
(`read_fonts` with `features = ["agl"]` is what PDFium's own skrifa bridge
enables, `core/fxge/skrifa/Cargo.toml`). See D6.

### 1.8 Type1 glyph mapping — `CPDF_Type1Font`

#### 1.8.1 `Load()` — base-14 detection

`cpdf_type1font.cpp:86-114`:
```
base14_font_ = GetStandardFontIndex(base_font_name_)        // §1.11, 89-entry alias table
if base14_font_.is_none() { return LoadCommon() }           // ordinary Type1 font

base_font_name_ = GetCanonicalFontName(base14_font_)        // e.g. ArialMT -> Helvetica

if font_desc exists && font_desc has /Flags { flags_ = /Flags }
else if is_symbolic_base14()                { flags_ = kFontStyleSymbolic }     // 4
else                                        { flags_ = kFontStyleNonSymbolic }  // 32

if is_fixed_base14() { char_width_.fill(600) }              // the four Couriers

if base14 == Symbol       { base_encoding_ = AdobeSymbol }
else if base14 == Dingbats{ base_encoding_ = ZapfDingbats }
else if FontStyleIsNonSymbolic(flags_) { base_encoding_ = Standard }

return LoadCommon()
```
`is_symbolic_base14()` = index is Symbol or Dingbats (`cfx_standardfont.cpp:186-189`);
`is_fixed_base14()` = index is one of the four Couriers (`:192-197`).

The `flags_` written here is **overwritten again** by `LoadFontDescriptor`
inside `LoadCommon` when a descriptor exists (§1.2 sets `flags_` from `/Flags`
first thing). So this assignment only survives for a base-14 font with **no**
`/FontDescriptor` at all. The `char_width_.fill(600)` and the `base_encoding_`
assignment both survive (nothing else writes them before `LoadPDFEncoding`, and
`LoadCommon` step 4 only overwrites `base_encoding_` for non-symbolic fonts,
which is exactly the `Standard` case it would set anyway).

**`IsBase14Font()`** = `base14_font_.is_some()`. Consumed by
`CPDF_Font::IsStandardFont()` (`cpdf_font.cpp:358-366`) which additionally
requires `IsType1Font() && font_file_.is_none()` — **an embedded font is never
"standard"**, even when named `Helvetica`.

#### 1.8.2 `LoadGlyphMap()` — the branches

`cpdf_type1font.cpp:127-323`. Five branches in the C++; **two are Apple-only and
out of scope** (`#if BUILDFLAG(IS_APPLE)`, `:210-266` and the `CalcExtGID`/
`SetExtGID` sprinkles). The oracle is built on Linux (PLAN.md §4), so the
non-Apple paths are normative. The Apple-only ligature remap table
(`kGlyphNameSubsts`, `:36-40`: `ff→uniFB00, ffi→uniFB03, ffl→uniFB04,
fi→uniFB01, fl→uniFB02`) is recorded here only so nobody "completes" the port.

**Guard** (`:128-131`): no face ⇒ return, leaving all 256 `glyph_index_` at
`0xffff`.

**Branch A — non-embedded, non-symbolic, backed by a TrueType face**
(`:148-208`). Condition: `!IsEmbedded() && !is_symbolic_base14() &&
font_.IsTTFont()`. This happens when a Type1 font dict got substituted with a TT
system face.

*A1 — the MS-Symbol prefix probe* (`:149-174`): if a `(3,0)` charmap exists
(`UseTTCharmap(face, kWindowsSymbolCmapId)`), then for each charcode 0..=255,
try the four prefixes `{0x00, 0xF0, 0xF1, 0xF2}` in order:
`glyph_index_[c] = face.char_index(prefix * 256 + c)`, breaking on the first
nonzero. If **any** charcode got a glyph, return.

*A2 — the Unicode path* (`:175-207`): select the Unicode charmap; if
`base_encoding_ == Builtin`, promote it to `Standard`. Then for each charcode:
```
name = GetAdobeCharName(base_encoding_, char_names_, c);  if none { continue }
encoding_.set_unicode(c, UnicodeFromAdobeName(name))
glyph_index_[c] = face.char_index(encoding_.unicode(c))
if glyph_index_[c] == 0 && name == ".notdef" {
    encoding_.set_unicode(c, 0x20)
    glyph_index_[c] = face.char_index(0x20)          // notdef -> space
}
```

**`UseType1Charmap(face)`** (`:53-68`) runs unconditionally at `:209` before the
remaining branches:
```
n = face.charmap_count();  if n == 0 { return false }
first_is_unicode = face.charmap_encoding(0) == Unicode
if n == 1 && first_is_unicode { return false }
face.set_charmap_by_index(if first_is_unicode { 1 } else { 0 })
true
```
A Type1 face typically exposes charmap[0] = Unicode (synthesized from glyph
names via the AGL) and charmap[1] = the font's own encoding vector. This
deliberately picks the **non-Unicode** one, so a later `char_index(c)` means
"look up `c` in the font's built-in encoding".

**Branch B — symbolic** (`:267-292`). Condition `FontStyleIsSymbolic(flags_)`:
```
for c in 0..256 {
    name = GetAdobeCharName(base_encoding_, char_names_, c)
    if let Some(name) = name {
        encoding_.set_unicode(c, UnicodeFromAdobeName(name))
        glyph_index_[c] = face.name_index(name)          // by glyph NAME
    } else {
        glyph_index_[c] = face.char_index(c)             // built-in encoding, raw code
        if glyph_index_[c] != 0 {
            gname = face.glyph_name(glyph_index_[c])
            encoding_.set_unicode(c, if gname.is_empty() { 0 }
                                     else { UnicodeFromAdobeName(gname) })
        }
    }
}
```
The `else` arm is the **built-in-encoding path**: no PDF name for this code, so
consult the font's own encoding vector (selected by `UseType1Charmap`), then
reverse the GID back to a name to recover a unicode. **No `.notdef`/space
fallback here** — a zero result stays zero (which `GlyphFromCharCode` maps to
glyph 0, *not* to -1; see below).

**Branch C — non-symbolic** (`:294-317`):
```
b_unicode = face.select_charmap(Unicode)                 // may fail
for c in 0..256 {
    name = GetAdobeCharName(base_encoding_, char_names_, c);  if none { continue }
    encoding_.set_unicode(c, UnicodeFromAdobeName(name))
    glyph_index_[c] = face.name_index(name)              // NAME IS PRIMARY
    if glyph_index_[c] != 0 { continue }
    if name != ".notdef" && name != "space" {
        glyph_index_[c] = face.char_index(
            if b_unicode { encoding_.unicode(c) } else { c as u32 })
    } else {
        encoding_.set_unicode(c, 0x20)
        glyph_index_[c] = 0xffff                         // explicit "draw nothing"
    }
}
```

**Precedence for Type1 is name-first, charmap-second.** `.notdef` and `space`
are the two names not worth a second lookup: they get unicode U+0020 and the
`0xffff` sentinel. `kNotDef = ".notdef"` and `kSpace = "space"` are at
`cpdf_simplefont.h:32-33`; the comparisons are byte-exact `strcmp`.

#### 1.8.3 `GlyphFromCharCode` for simple face-based fonts

**Not overridden by Type1 or TrueType.** Inherited from
`CPDF_FaceBasedSimpleFont::GlyphFromCharCode` (`cpdf_facebasedsimplefont.cpp:28-44`):
```
*vert_glyph = false
if charcode > 0xff { return -1 }
let index = glyph_index_[charcode]
if index == 0xffff { return -1 }
index as i32
```
All the work happened in `LoadGlyphMap`. **Glyph 0 is a legitimate return
value** and is distinct from -1. `CPDF_SimpleFont::GlyphFromCharCode`
(`cpdf_simplefont.cpp:143-148`) — the Type3 base — always returns **-1**.

### 1.9 TrueType glyph mapping — `CPDF_TrueTypeFont::LoadGlyphMap`

`cpdf_truetypefont.cpp:53-171`. Four rungs, each with an internal "did anything
map?" test.

**Preliminaries.**

`DetermineEncoding()` (`:205-240`) — resolves a WinAnsi/MacRoman `base_encoding_`
against what the embedded face actually supports:
```
if font_file_.is_none() || !FontStyleIsSymbolic(flags_)
   || base_encoding_ not in {WinAnsi, MacRoman} { return base_encoding_ }
n = face.charmap_count();  if n == 0 { return base_encoding_ }
support_win = any charmap with platform_id in {0 (AppleUnicode), 3 (Windows)}
support_mac = any charmap with platform_id == 1 (Mac)
if base_encoding_ == WinAnsi && !support_win {
    return if support_mac { MacRoman } else { Builtin }
}
if base_encoding_ == MacRoman && !support_mac {
    return if support_win { WinAnsi } else { Builtin }
}
base_encoding_
```
The loop breaks early once both flags are true. Platform-id constants:
`kNamePlatformAppleUnicode = 0`, `kNamePlatformMac = 1`,
`kNamePlatformWindows = 3` (`fx_font.h:77-79`).

`DetermineCharmapType()` (`:182-203`) — which charmap to drive, and it **selects**
it as a side effect:
```
if UseTTCharmapUnicode(face) { return MSUnicode }
if FontStyleIsNonSymbolic(flags_) {
    if UseTTCharmap(face, (1,0)) { return MacRoman }
    if UseTTCharmap(face, (3,0)) { return MSSymbol }
} else {
    if UseTTCharmap(face, (3,0)) { return MSSymbol }
    if UseTTCharmap(face, (1,0)) { return MacRoman }
}
Other
```
The symbolic flag flips the (3,0)/(1,0) preference order.

`UseTTCharmapUnicode(face)` (`cpdf_font.cpp:571-596`) — **pinned by
`cpdf_truetypefont_unittest.cpp`'s `AllUnicodeCmapsTreatedEqually`**:
```
unicode_index = 0; unicode_found = false; mssymbol_found = false
for i in 0..face.charmap_count() {
    id = face.charmap_id(i)                             // (platform, encoding)
    if id == (3,1) { face.set_charmap_by_index(i); return true }   // exact win
    if id == (3,0) { mssymbol_found = true; continue }
    if !unicode_found && face.charmap_encoding(i) == Unicode {
        unicode_found = true; unicode_index = i
    }
}
if unicode_found && !mssymbol_found { face.set_charmap_by_index(unicode_index); return true }
false
```
So a `(0,3)` Apple-Unicode charmap counts as Unicode and is used **provided no
`(3,0)` symbol charmap exists**. The unittest builds two 508-byte TTFs
differing only in cmap platform/encoding (`(0,3)` vs `(3,1)`), each mapping
U+002E to glyph 1, and asserts both yield glyph **1** for charcode 0x2E.

`UseTTCharmap(face, id)` (`cpdf_font.cpp:599-608`): first charmap whose
`(platform, encoding)` equals `id`; selects it and returns true.
Ids: `kMacRomanCmapId = (1,0)`, `kWindowsSymbolCmapId = (3,0)`,
`kWindowsUnicodeCmapId = (3,1)` (`cfx_face.h:71-73`).

`GetGlyphIndexForMSSymbol(face, charcode)` (`:20-30`): tries the four prefixes
`kPrefix = {0x00, 0xF0, 0xF1, 0xF2}` (`:18`) as `prefix * 256 + charcode`,
returning the first nonzero `char_index`, else 0.

`HasAnyGlyphIndex()` (`:173-180`): true if any of the 256 entries is nonzero.
**Note `0xffff` is nonzero**, so a table full of the "unset" sentinel counts as
"has glyphs". In practice the rungs that call it have written real values.

**Rung 1 — the name-driven path** (`:60-114`). Condition:
```
(base_encoding in {WinAnsi, MacRoman} && char_names_.is_empty())
|| FontStyleIsNonSymbolic(flags_)
```
where `base_encoding = DetermineEncoding()`.

*1a — the no-charmap shortcut* (`:62-65`): if the face has glyph names **and**
zero charmaps, run `SetGlyphIndicesFromFirstChar()` and return.
`SetGlyphIndicesFromFirstChar` (`:242-252`): read `/FirstChar`; if outside
`0..=255`, return leaving everything `0xffff`; else fill `glyph_index_[0..first]`
with **0** and `glyph_index_[first..256]` with `3, 4, 5, …` (`std::iota` from 3).
The magic 3 is "skip .notdef/.null/CR" in the classic TrueType glyph order.

*1b — the main loop* (`:67-112`):
```
charmap_type = DetermineCharmapType()
has_to_unicode = font_dict has /ToUnicode          // presence only
for c in 0..256 {
    name = GetAdobeCharName(base_encoding, char_names_, c)
    if name.is_none() {
        glyph_index_[c] = if font_file_.is_some() { face.char_index(c) } else { -1 as u16 };
        continue                                    // note: 0xffff via the -1 cast
    }
    encoding_.set_unicode(c, UnicodeFromAdobeName(name))
    match charmap_type {
        MSSymbol  => glyph_index_[c] = GetGlyphIndexForMSSymbol(face, c),
        MSUnicode if encoding_.unicode(c) != 0 =>
                     glyph_index_[c] = face.char_index(encoding_.unicode(c)),
        MacRoman  if encoding_.unicode(c) != 0 => {
            let maccode = CharCodeFromUnicodeForEncoding(AppleRoman, encoding_.unicode(c));
            glyph_index_[c] = if maccode == 0 { face.name_index(name) }
                              else { face.char_index(maccode) };
        }
        _ => {}                                     // Other: leave as-is (0xffff)
    }
    if glyph_index_[c] != 0 && glyph_index_[c] != 0xffff { continue }
    if name == ".notdef" { glyph_index_[c] = face.char_index(32); continue }
    glyph_index_[c] = face.name_index(name)
    if glyph_index_[c] != 0 || !has_to_unicode { continue }
    // last resort: use ToUnicode to find a unicode, then the charmap
    if let Some(u) = self.unicode_from_charcode(c).first() {
        glyph_index_[c] = face.char_index(u)
        encoding_.set_unicode(c, u)
    }
}
return
```
The `|| !name` in the C++'s `if ((glyph_index_[c] != 0 && != 0xffff) || !name)`
guard is dead (a `None` name already `continue`d), but note it: it documents
intent.

**The ToUnicode last-resort rung is Tier-A relevant** — it makes glyph selection
depend on `/ToUnicode`, which in turn makes rendering depend on it. It fires only
for a name that resolved to no glyph by any other route.

**Rung 2 — MS Symbol whole-table** (`:115-136`). If `UseTTCharmap(face, (3,0))`:
fill all 256 from `GetGlyphIndexForMSSymbol`. If `HasAnyGlyphIndex()`:
- if `base_encoding != Builtin`: set `encoding_` unicodes from the Adobe names.
- else if `UseTTCharmap(face, (1,0))`: set `encoding_` unicodes from
  `UnicodeFromAppleRomanCharCode(c)`.
- return.

If nothing mapped, **fall through with `glyph_index_` already overwritten by
zeros** — the next rung reassigns every entry, so this is harmless, but it means
a mid-rung crash would leave a half-written table.

**Rung 3 — MacRoman whole-table** (`:137-145`). If `UseTTCharmap(face, (1,0))`:
`glyph_index_[c] = face.char_index(c)` and
`encoding_.set_unicode(c, UnicodeFromAppleRomanCharCode(c))` for all 256. Return
if `font_file_.is_some() || HasAnyGlyphIndex()` — note an **embedded** font
returns here even if nothing mapped.

**Rung 4 — Unicode charmap** (`:146-167`). If `face.select_charmap(Unicode)`:
```
unicodes = UnicodesForPredefinedCharSet(base_encoding)      // may be empty (Builtin)
for c in 0..256 {
    if font_file_.is_some() {
        encoding_.set_unicode(c, c)                          // identity!
    } else {
        name = GetAdobeCharName(Builtin, char_names_, c)     // ONLY /Differences
        if let Some(name) = name { encoding_.set_unicode(c, UnicodeFromAdobeName(name)) }
        else if !unicodes.is_empty() { encoding_.set_unicode(c, unicodes[c]) }
    }
    glyph_index_[c] = face.char_index(encoding_.unicode(c))
}
if HasAnyGlyphIndex() { return }
```
Note `GetAdobeCharName(Builtin, …)` returns a name **only from `/Differences`** —
the predefined table is skipped because the encoding argument is `Builtin`.

**Rung 5 — identity** (`:168-170`): `glyph_index_[c] = c` for all 256.

### 1.10 CID fonts — `CPDF_CIDFont`

#### 1.10.1 `Load()`

`cpdf_cidfont.cpp:418-541`.

**The GB2312 shortcut** (`:419-422`): if `/Subtype == "TrueType"` (only reachable
via the Chinese-name special case of §1.1), run `LoadGB2312()` and return true.
`LoadGB2312` (`:859-876`): charset = GB1, cmap = the predefined `"GBK-EUC-H"`,
CID2Unicode = GB1's table, load the descriptor, substitute if not embedded,
`CheckFontMetrics()`, and set **`ansi_widths_fixed_ = true`**.

**The main path:**
```
fonts = /DescendantFonts;  if none or len != 1 { return FALSE }
cid_dict = fonts[0] as dict;  if none { return FALSE }
base_font_name_ = cid_dict./BaseFont
if base_font_name_ in {"CourierStd", "CourierStd-Bold",
                       "CourierStd-BoldOblique", "CourierStd-Oblique"}
   && !IsEmbedded() { adobe_courier_std_ = true }

encoding = /Encoding (direct);  if none { return FALSE }
font_type_ = if cid_dict./Subtype == "CIDFontType0" { Type1 } else { TrueType }
if encoding is neither a Name nor a Stream { return FALSE }

cmap_ = if stream { parse_embedded(decoded bytes) }
        else      { predefined(name) }                  // pdfrum-cmap

if cid_dict has /FontDescriptor { LoadFontDescriptor(it) }

charset_ = cmap_.charset()
if charset_ == Unknown && cid_dict has /CIDSystemInfo {
    charset_ = charset_from_ordering(cid_info./Ordering)
}
if charset_ != Unknown { cid2unicode_map_ = registry table for charset_ }

if let Some(face) = font_.face() {
    if font_type_ == Type1 { face.select_charmap(Unicode) }
    else { UseCIDCharmap(face, cmap_.coding()) }
}

default_width_ = cid_dict./DW as integer, default 1000
if let Some(w) = cid_dict./W { LoadMetricsArray(w, &mut width_list_, 1) }
if !IsEmbedded() { LoadSubstFace() }

if let Some(c2g) = cid_dict./CIDToGIDMap (direct) {
    if c2g is a Stream { stream_acc_ = decoded bytes }
    else if font_file_.is_some() && c2g is the Name "Identity" { cid_is_gid_ = true }
}

CheckFontMetrics()
if IsVertWriting() {
    if let Some(w2) = cid_dict./W2  { LoadMetricsArray(w2, &mut vert_metrics_, 3) }
    if let Some(dw2) = cid_dict./DW2 { default_vy_ = dw2[0]; default_w1_ = dw2[1] }
}
if font_type_ == TrueType && IsEmbedded() && face.format() == "TrueType" {
    font_.set_font_type(CIDTrueType)
}
return TRUE
```

**This is the one font type whose `Load()` really can fail**, in four places:
missing/multi-element `/DescendantFonts`, a non-dict first element, a missing
`/Encoding`, and an `/Encoding` that is neither a Name nor a Stream. A `false`
here means `CPDF_Font::Create` returns `nullptr` and the resource is simply
absent — the content-stream interpreter then skips text using that font.

`UseCIDCharmap(face, coding)` (`:201-226`):
```
enc = match coding { GB => GB2312, BIG5 => Big5, JIS => Sjis,
                     KOREA => Johab, _ => Unicode }
if !face.select_charmap(enc) {
    if !face.select_charmap(Unicode) {
        if face.charmap_count() > 0 { face.set_charmap_by_index(0) }
    }
}
```
Note `KOREA → Johab`, not Wansung.

`LoadSubstFace()` (`:845-852`): `font_.LoadSubstFace(base_font_name_,
font_type_ == TrueType, flags_, saturating(stem_v_ * 5) or 400 on overflow,
italic_angle_, kCharsetCodePages[charset_], IsVertWriting())`.
`kCharsetCodePages` (`:57-65`), indexed by `CidSet` ordinal:
```
Unknown -> kDefANSI(0)          GB1     -> kChineseSimplified(936)
CNS1    -> kChineseTraditional(950)     Japan1  -> kShiftJIS(932)
Korea1  -> kHangul(949)                 Unicode -> kUTF16LE(1200)
```

#### 1.10.2 `/W` and `/W2` — `LoadMetricsArray`

`cpdf_cidfont.cpp:228-282`. One function parses both, with `nElements` = 1 for
`/W` and 3 for `/W2`. Output is a flat `Vec<i32>` of records
`[low, high, v1..vN]`, later reinterpreted as `LowHighVal { low, high, val }`
(N=1) or `LowHighValXY { low, high, val, x, y }` (N=3).

```
width_status = 0; cur_element = 0; first_code = 0; last_code = 0
for obj in array (each resolved) {
    if obj is None { continue }                       // dangling refs skipped
    if let Some(inner) = obj.as_array() {
        if width_status != 1 { return }               // ABORT the whole parse
        if first_code > i32::MAX - inner.len() as i32 { width_status = 0; continue }
        for j in (0..inner.len()).step_by(nElements) {
            result.push(first_code); result.push(first_code)
            for k in 0..nElements { result.push(inner.integer_at(j + k)) }
            first_code += 1
        }
        width_status = 0
    } else {
        match width_status {
            0 => { first_code = obj.as_integer(); width_status = 1 }
            1 => { last_code = obj.as_integer(); width_status = 2; cur_element = 0 }
            _ => {
                if cur_element == 0 { result.push(first_code); result.push(last_code) }
                result.push(obj.as_integer())
                cur_element += 1
                if cur_element == nElements { width_status = 0 }
            }
        }
    }
}
```

Behaviors to reproduce:
- The `c [w1 w2 …]` form and the `c_first c_last w` form are distinguished purely
  by whether the current element is an array, and `width_status != 1` when an
  array appears **aborts everything parsed so far is kept, nothing further is
  read** (the `return` exits the function, keeping `result`).
- The overflow guard resets to `width_status = 0` and *continues*, so the array's
  contents are dropped but parsing resumes at the next element.
- In the array form, `j` steps by `nElements`; a trailing partial group reads
  `integer_at` past the end, which yields **0** rather than failing.
- A truncated `c_first c_last w1 w2` group (fewer than `nElements` values before
  the array ends) leaves `width_status == 2` and the partial record **is** in
  `result` — with fewer than `nElements` values pushed after the two codes. The
  final `reinterpret_span` therefore drops the trailing partial record (it
  computes `len / record_size`). Reproduce by chunking exactly.

**`GetCharWidth(charcode)`** (`:573-586`):
```
if charcode < 0x80 && ansi_widths_fixed_ {
    return if (32..127).contains(&charcode) { 500 } else { 0 }
}
cid = CIDFromCharCode(charcode)
for rec in width_list_.chunks_exact(3) {                // LINEAR scan, first match
    if rec.low <= cid && cid <= rec.high { return rec.val }
}
default_width_
```
The scan is **first-match, in file order** — not last, not lowest. `/W` entries
are not sorted and may overlap; the earliest wins.

**`GetVertWidth(cid)`** (`:588-597`): same linear scan over `vert_metrics_`
5-tuples, returning `val`; default `default_w1_`.

**`GetVertOrigin(cid)`** (`:599-617`):
```
for rec in vert_metrics_.chunks_exact(5) {
    if rec.low <= cid && cid <= rec.high { return (rec.x as i16, rec.y as i16) }
}
let mut width = default_width_
for rec in width_list_.chunks_exact(3) {
    if rec.low <= cid && cid <= rec.high { width = rec.val; break }
}
(( width / 2 ) as i16, default_vy_)
```
So absent a `/W2` entry, the vertical origin is (half the horizontal width,
`DW2[0]`). `default_vy_` and `default_w1_` default to **880** and **-1000**
respectively (`cpdf_cidfont.h`, the ISO 32000 `/DW2` default `[880 -1000]`).
Confirm the literals at implementation time; the C++ initializers are in the
header.

`GetCharPosList` (`cpdf_font.cpp:473-480`) applies them:
```
if is_vertical_writing {
    origin = (0.0, origin.x)                        // swap: advance becomes y
    let (vx, vy) = cid_font.vert_origin(cid)
    origin.x -= font_size * vx as f32 / 1000.0
    origin.y -= font_size * vy as f32 / 1000.0
}
```

#### 1.10.3 The Japan1 vertical CID transform table

`kJapan1VerticalCIDs` (`cpdf_cidfont.cpp:67-145`), **150 entries** of
`CIDTransform { cid: u16, a, b, c, d, e, f: u8 }` (`cpdf_cidfont.h:32-39`),
**sorted by cid**, looked up by binary search.

`GetCIDTransform(cid)` (`:878-888`): returns `None` unless
`charset_ == Japan1 && font_file_.is_none()` (i.e. a **non-embedded** Japan1
font); then `lower_bound` by cid with an exact-match check.

`CIDTransformToFloat(ch: u8) -> f32` (`:855-857`):
`(if ch < 128 { ch as i32 } else { ch as i32 - 255 }) as f32 * (1.0 / 127.0)`.
Note `- 255`, not `- 256`: byte 129 maps to `-126/127`, byte 255 maps to `0`.

Applied two ways:
- In `GetCharBBox` (`:554-566`), **only when the glyph is not vertical**: build
  the matrix `(a, b, c, d, e*1000, f*1000)` (each through
  `CIDTransformToFloat`) and transform the rect, taking the outer integer rect.
- In `GetCharPosList` (`cpdf_font.cpp:482-497`), again only when
  `!is_vertical_glyph`: `adjust_matrix = [a*sf, b*sf, c, d]` where `sf` is the
  glyph-spacing scaling factor (§1.14), and
  `origin += (e * font_size, f * font_size)`.

The table's contents (cid, a, b, c, d, e, f) must be ported verbatim. Its
structure: entry 0 is `{97, 129, 0, 0, 127, 55, 0}`; entries 1–2 cover CIDs 7887,
7888 with `{127,0,0,127,…}`; 7889–7917 are `{0,129,127,0,17,·}` (rotations);
7918–7939 are `{127,0,0,127,18,25}`; 8720–8819 are `{0,129,127,0,·,·}`. The
`build.rs`-free approach is a plain `const [CidTransform; 150]` transcribed
directly — it is 150 short rows and belongs in the source, not a blob.

#### 1.10.4 `GlyphFromCharCode` — the CID glyph ladder

`cpdf_cidfont.cpp:657-819`. **The most intricate function in this crate.** Two
top-level halves.

**Half 1 — the non-embedded / substituted path** (`:662-778`). Condition:
```
font_file_.is_none() && (stream_acc_.is_none() || cid2unicode_map_.is_some())
```
i.e. no embedded program, and either no `/CIDToGIDMap` stream or a known
registry.

*1.1 — obtain a unicode* (`:663-693`):
```
cid = CIDFromCharCode(charcode)
unicode: u16 = 0
if cid_is_gid_ {
    return cid                        // non-Apple; Apple has a symbolic/ToUnicode branch
} else {
    if cid != 0 && cid2unicode_map_.is_some() { unicode = unicode_from_cid(cid) }
    if unicode == 0 { unicode = self.unicode_from_charcode_scalar(charcode) }   // §1.10.5
    if unicode == 0 {
        if let Some(u) = self.unicode_from_charcode(charcode).first() { unicode = u }
    }
}
```

*1.2 — the Adobe-CourierStd rescue* (`:694-739`), taken when `unicode == 0`:
```
if !adobe_courier_std_ {
    return if charcode != 0 { charcode as i32 } else { -1 }
}
charcode += 31                                   // !!! the magic offset
let ms_unicode = UseTTCharmapUnicode(face)
let mac_roman  = !ms_unicode && UseTTCharmap(face, (1,0))
let base_encoding = if ms_unicode { WinAnsi } else if mac_roman { MacRoman } else { Standard }
let name = GetAdobeCharName(base_encoding, &[], charcode)
if name.is_none() { return if charcode != 0 { charcode as i32 } else { -1 } }
let nu = UnicodeFromAdobeName(name)
if nu == 0 { return if charcode != 0 { charcode as i32 } else { -1 } }
if base_encoding == Standard { return font_.char_index(nu) }      // early, unchecked
let index = if base_encoding == WinAnsi { font_.char_index(nu) }
            else { let mac = CharCodeFromUnicodeForEncoding(AppleRoman, nu);
                   if mac != 0 { font_.char_index(mac) } else { font_.name_index(name) } };
if index == 0 || index == 0xffff { return if charcode != 0 { charcode as i32 } else { -1 } }
index
```
The `charcode += 31` is a hard-coded rescue for Adobe's CourierStd, whose CIDs
are offset from the standard encoding by 31. It is pinned by
`cpdf_cidfont_unittest.cpp`'s `Bug920636`: a `CPDF_CIDFont` with
`/Encoding /Identity−H` (**note the U+2212 MINUS SIGN, not a hyphen** — a
deliberately malformed name that fails the predefined lookup) and a descendant
`/BaseFont /CourierStd`, asserting:

| charcode | glyph |
|---|---|
| 0 | 31 |
| 256 | 287 |
| 34661 | 34692 |

Each is `charcode + 31`, i.e. every one of these falls all the way through to a
`return charcode` after the `+= 31`. (The test comments that glyph indices are
otherwise machine-dependent.) Reproducing this requires the `Identity−H` name to
fail lookup (it does — `pdfrum-cmap` §1.1 strips the last 2 bytes giving
`"Identity−"`, which matches no row) *and* the CMap to fall back to Identity
(`CidMap::Identity`), *and* `cid2unicode_map_` to be absent
(`charset_ == Unknown`).

*1.3 — the Japan1 backslash/yen fixups* (`:740-748`), when `unicode != 0`:
```
if charset_ == Japan1 {
    if unicode == '\\' { unicode = '/' }
    else if unicode == 0xA5 { unicode = 0x5C }        // ¥ -> backslash (non-Apple only)
}
```

*1.4 — charmap negotiation and lookup* (`:750-777`):
```
let face = match font_.face() { None => return unicode as i32, Some(f) => f };
let n = face.charmap_count()
if !face.select_charmap(Unicode) {
    let mut i = 0;
    while i < n {
        let ret = CharCodeFromUnicodeForEncoding(face.charmap_encoding(i), charcode as u16);
        if ret != 0 { face.set_charmap_by_index(i); unicode = ret as u16; break }
        i += 1
    }
    if i == n && i != 0 { face.set_charmap_by_index(0); unicode = charcode as u16 }
}
if n != 0 {
    let index = self.glyph_index(unicode, vert_glyph)        // §1.10.6
    return if index != 0 { index } else { -1 }
}
unicode as i32
```
Note the reverse-encoding probe uses **`charcode`**, not the `unicode` computed
above — a deliberate-looking inconsistency that must be preserved. And note a
face with **zero charmaps** returns the unicode value itself as a glyph index.

**Half 2 — the embedded path** (`:780-818`):
```
let face = match font_.face() { None => return -1, Some(f) => f };
let cid = CIDFromCharCode(charcode);
if stream_acc_.is_none() {
    if font_type_ == Type1 { return cid }                    // CIDFontType0: CID == GID
    if font_file_.is_some() && cmap_.has_no_direct_table() { return cid }
    if cmap_.coding() == Unknown { return cid }
    let charmap = match face.current_charmap_encoding() { None => return cid, Some(c) => c };
    let code = if charmap == Unicode {
        match self.unicode_from_charcode(charcode).first() {
            None => return -1,
            Some(u) => u as u32,
        }
    } else { charcode };
    return self.glyph_index(code, vert_glyph)
}
// /CIDToGIDMap stream: a big-endian u16 table indexed by CID
let byte_pos = cid as u32 * 2
if byte_pos + 2 > stream_acc_.len() { return -1 }
(stream_acc_[byte_pos] as i32) * 256 + stream_acc_[byte_pos + 1] as i32
```
`has_no_direct_table()` is `pdfrum-cmap`'s
`IsDirectCharcodeToCIDTableIsEmpty()` — true exactly for a *predefined* CMap
(cmap brief §3.2). So an embedded TrueType CID font with a predefined CMap and no
`/CIDToGIDMap` uses CID as GID directly.

#### 1.10.5 CID unicode derivation

**`GetUnicodeFromCharCode(charcode) -> u16`** (`:318-358`) — the *scalar*
variant, distinct from `UnicodeFromCharCode` (which consults ToUnicode first):
```
match cmap_.coding() {
    UCS2 | UTF16 => return charcode as u16,          // charcode IS the unicode
    CID => return if cid2unicode_map is loaded { unicode_from_cid(charcode as u16) }
                  else { 0 },
    _ => {}
}
if cid2unicode_map is loaded && cmap_.is_loaded() {
    return unicode_from_cid(CIDFromCharCode(charcode))
}
// non-Windows tail:
if cmap_.static_map().is_none() { return 0 }
EmbeddedUnicodeFromCharcode(static_map, charset_, charcode)
```
`EmbeddedUnicodeFromCharcode` (`:162-177`): valid only for the four CJK
registries (`IsValidEmbeddedCharcodeFromUnicodeCharset`, `:149-160`); computes
`cid = static_cid_from_charcode(charcode)`, returns 0 if `cid == 0`, else
`registry_table.get(cid).unwrap_or(0)`.

**`UnicodeFromCharCode(charcode)`** (`:309-316`):
`CPDF_Font::UnicodeFromCharCode` (i.e. **ToUnicode first**), and only if that is
empty, `GetUnicodeFromCharCode` wrapped as a 1-char string (empty if 0).

**`CharCodeFromUnicode(unicode)`** (`:360-416`) — the reverse:
```
let c = CPDF_Font::CharCodeFromUnicode(unicode);  if c != 0 { return c }   // ToUnicode reverse
match cmap_.coding() {
    Unknown => return 0,
    UCS2 | UTF16 => return unicode as u32,
    CID => {
        if cid2unicode_map not loaded { return 0 }
        for cid in 0..65536 {                       // O(65536) LINEAR SCAN
            if unicode_from_cid(cid) == unicode { return cid }
        }
        // falls out of the match
    }
    _ => {}
}
if unicode < 0x80 { return unicode as u32 }
if cmap_.coding() == CID { return 0 }
// non-Windows tail:
if let Some(m) = cmap_.static_map() { EmbeddedCharcodeFromUnicode(m, charset_, unicode) }
else { 0 }
```
`EmbeddedCharcodeFromUnicode` (`:179-197`) is the O(table × chain) reverse scan
described in the cmap brief §1.4/D4.

**`IsUnicodeCompatible()`** (`:838-843`): true if
`cid2unicode_map is loaded && cmap_.is_loaded()`, else
`cmap_.coding() != Unknown`. Consumed by `pdfrum-text`.

#### 1.10.6 `GetGlyphIndex` and vertical substitution

`cpdf_cidfont.cpp:619-655`:
```
GetGlyphIndex(unicode, vert_glyph) -> i32 {
    *vert_glyph = false
    let index = font_.char_index(unicode)
    if unicode == 0x2502 { return index }        // BOX DRAWINGS LIGHT VERTICAL: never rotate
    if index == 0 || !IsVertWriting() { return index }
    if ttg_subtable_.is_none() {
        ttg_subtable_ = font_.parse_gsub_table()          // lazy, once
        if ttg_subtable_.is_none() { return index }
    }
    GetVerticalGlyph(index, vert_glyph)
}
GetVerticalGlyph(index, vert_glyph) -> i32 {
    let v = ttg_subtable_.vertical_glyph(index)
    if v == 0 { return index }
    *vert_glyph = true
    v as i32
}
```
`pdfium::unicode::kBoxDrawingsLightVertical` is U+2502.

**`CFX_CTTGSUBTable`** (`cfx_cttgsubtable.cpp`) parses the OpenType `GSUB` table
and resolves single substitutions for the vertical features:
- Header check: `version != 0x00010000` ⇒ fail (`:59-61`). Then read the
  ScriptList / FeatureList / LookupList offsets from bytes 4/6/8 (u16 BE).
- Feature tags of interest: `vrt2` and `vert`
  (`IsVerticalFeatureTag`, `:21-24`).
- **Feature selection** (`:33-53`): first, collect every feature index reachable
  from any script's LangSys records whose tag is vertical. **If that set is
  empty**, fall back to scanning the whole feature list and collecting every
  vertical-tagged feature regardless of script reachability.
- `GetVerticalGlyph(gid)` (`:74-83`): for each selected feature, for each of its
  lookup indices, **only `lookup_type == 1`** (single substitution) is
  considered; per subtable, compute the coverage index and:
  - format 1 (`table_data` is an `i16` delta) — if `coverage_index >= 0`, return
    `gid + delta` (wrapping);
  - format 2 (`table_data` is a `Vec<u16>`) — if `coverage_index` is in bounds,
    return `substitutes[coverage_index]`.
  Return 0 if nothing matched.
- Coverage (`ParseCoverage`, `:275-296`): format 1 = a glyph array (linear scan
  for equality, index is the position); format 2 = range records
  `{start, end, start_coverage_index}` (linear scan; index is
  `start_coverage_index + gid - start`). Any other format ⇒ no coverage,
  index -1.
- **The C++ parser is bounds-unsafe** (raw `subspan` on attacker-controlled
  offsets). Our port must return `Result`/`Option` at every read; a malformed
  GSUB yields "no vertical substitution", never a panic. See D5.

**skrifa/read-fonts covers GSUB parsing** (`read_fonts::tables::gsub`), so this
becomes a thin adapter rather than a from-scratch parser — but the *feature
selection policy* above (script-reachable first, whole-list fallback,
lookup-type-1 only) is PDFium behavior we must reproduce on top of it.

#### 1.10.7 `GetCharBBox` for CID fonts

`cpdf_cidfont.cpp:543-571`: a 256-entry cache (`char_bbox_`, seeded to
`(-1,-1,-1,-1)`) for `charcode < 256` only; otherwise recompute each call. The
Japan1 transform (§1.10.3) is applied when `!vert_glyph`.

### 1.11 Type3 fonts

`cpdf_type3font.cpp`. A Type3 font has no face at all; its glyphs are content
streams.

**`Load()`** (`:54-93`):
```
font_resources_ = /Resources
if let Some(m) = /FontMatrix { font_matrix_ = m; xscale = m.a; yscale = m.d }
else { font_matrix_ = identity; xscale = 1.0; yscale = 1.0 }

if let Some(bb) = /FontBBox {
    let mut box = Rect(bb[0]*xscale, bb[1]*yscale, bb[2]*xscale, bb[3]*yscale)
    TextUnitRectToGlyphUnitRect(&mut box)          // scale by 1000
    font_bbox_ = box.to_fx_rect()
}

let start = /FirstChar as integer
if (0..256).contains(&start) {
    if let Some(w) = /Widths {
        let count = min(min(w.len(), 256), 256 - start)
        for i in 0..count {
            char_width_l_[start + i] = round_f32(TextUnitToGlyphUnit(w.float_at(i) * xscale))
        }
    }
}
char_procs_ = /CharProcs
if /Encoding exists (direct) { LoadPDFEncoding(false, false) }
return TRUE                                        // always
```
`TextUnitToGlyphUnit(v)` = `v * 1000.0` and `TextUnitRectToGlyphUnitRect` scales
all four edges by 1000 (`cpdf_type3char.h`). Note `char_width_l_` is `[i32; 256]`
(**not** the `[u16;256]` of face-based fonts) and defaults to **0**, not `0xffff`.
Note also that `LoadPDFEncoding(embedded=false, truetype=false)` runs **only when
`/Encoding` is present**, so a Type3 font without `/Encoding` keeps
`base_encoding_ == Builtin` and `GetAdobeCharName` therefore returns a name only
from `/Differences` — which is absent too, so **no glyph resolves at all**.

**`LoadChar(charcode)`** (`:99-150`):
```
if char_loading_depth_ >= 4 { return None }        // kMaxType3FormLevel = 4
if let Some(c) = cache_map_.get(charcode) { return Some(c) }
let name = GetAdobeCharName(base_encoding_, char_names_, charcode)?;
let procs = char_procs_.as_ref()?;
let stream = procs.get(name)?.as_stream()?;
let form = create_form(document, font_resources_ ?? page_resources_, stream);
let mut ch = Type3Char::new();
{ depth += 1; form.parse_content_for_type3_char(&mut ch); depth -= 1 }   // may RECURSE
if let Some(c) = cache_map_.get(charcode) { return Some(c) }   // re-check after recursion
ch.transform(&form, font_matrix_)
if form.has_page_objects() { ch.set_form(form) }
cache_map_.insert(charcode, ch);
Some(&cache_map_[charcode])
```
Note `char_names_` is **not** cleared for Type3 (there is no `LoadCommon`), so
`/Differences` names survive for the life of the font.

**`GetCharWidth(charcode)`** (`:152-163`): `>= 256` ⇒ code 0; if
`char_width_l_[c] != 0` return it; else `LoadChar(c).map(|c| c.width()).unwrap_or(0)`.
**`GetCharBBox`** (`:165-172`): `LoadChar(c).map(|c| c.bbox()).unwrap_or_default()`.

`CPDF_Type3Font` inherits `CPDF_SimpleFont::GlyphFromCharCode`, which always
returns **-1** — Type3 has no glyph indices by construction.

`CheckType3FontMetrics()` (`:95-97`) delegates to `CheckFontMetrics`; it is
called by the page layer *after* the first render pass, not during `Load`.

### 1.12 Substitution — `FindSubstFace`, the complete ladder

`cfx_fontmapper.cpp:508-708`. This is `CFX_FontMapper`'s only real job, and per
STYLE.md §1 it decomposes into a pure "resolve a font request to a decision"
function over a small request record, plus a tiny lookup seam for the font
database.

**Signature:** `(name, is_truetype, flags, weight, italic_angle, code_page,
&mut subst_font) -> Option<Face>`.

**Step 0 — weight/attr normalization** (`:515-522`):
```
if weight == 0 { weight = 400 }
if flags & kFontUseExternAttr == 0 { weight = 400; italic_angle = 0 }
```
Without `kFontUseExternAttr` (§1.2), the caller's weight and slant are
**discarded entirely**.

**Step 1 — subst-name normalization**, `GetSubstName(name, is_truetype)`
(`:259-273`):
```
let mut s = name;
if is_truetype && s.starts_with('@') { s.remove(0) }
else { s.retain(|c| c != ' ') }                    // ALL spaces, everywhere
strip_subset_prefix(&mut s)                        // exactly 6 uppercase + '+'
if let Some(idx) = GetStandardFontIndex(&s) { s = GetCanonicalFontName(idx) }
s
```
It is an **either/or**: a TrueType `@`-prefixed vertical-writing name keeps its
spaces; every other name loses all of them.

`strip_subset_prefix` = `MaybeRemoveSubsettedFontPrefix` (`fx_font.cpp:217-223`):
`len > 6 && s[6] == '+' && s[..6].chars().all(is_ascii_uppercase)` ⇒
`s = s[7..]`. Pinned by `cpdf_simplefont_unittest.cpp`'s
`BaseFontNameWithSubsetting`: `/BaseFont /CHEESE+Swiss` with a real `/FontFile`
yields `GetBaseFontName() == "Swiss"`.

**Step 2 — symbolic short-circuits** (`:524-535`):
```
if subst_name == "Symbol" && !is_truetype {
    subst.family = "Chrome Symbol"; subst.charset = Symbol
    return internal_subst(Some(kSymbol), weight, italic_angle, 0, &mut subst)
}
if subst_name == "ZapfDingbats" {                  // note: NO is_truetype condition
    subst.family = "Chrome Dingbats"; subst.charset = Symbol
    return internal_subst(Some(kDingbats), weight, italic_angle, 0, &mut subst)
}
```

**Step 3 — the comma split** (`:536-554`):
```
let (family, style, has_comma, std_font) = match subst_name.find(',') {
    Some(p) => {
        let mut fam = subst_name[..p].to_owned();
        let sf = GetStandardFontIndex(&fam);
        if let Some(i) = sf { fam = GetCanonicalFontName(i) }
        (fam, subst_name[p+1..].to_owned(), true, sf)
    }
    None => (subst_name.clone(), String::new(), false,
             GetStandardFontIndex(&subst_name)),
};
```
Note step 1 already canonicalized the *whole* name when it matched an alias, so
`"Arial,Bold"` became `"Helvetica-Bold"` — which has no comma and takes the
`None` arm.

**Step 4 — style derivation** (`:555-583`):
```
let (mut n_style, pitch_family, mut base_font, mut has_hyphen) =
if let Some(sf) = std_font.filter(|&i| i < kSymbol) {
    (GetStyleFromBaseFont(sf), GetPitchFamilyFromBaseFont(sf), Some(sf), false)
} else {
    let mut style_out = kFontStyleNormal;
    let mut fam = family; let mut sty = style; let mut hyph = false;
    if !has_comma {
        if let Some(p) = fam.rfind('-') {          // LAST hyphen
            sty = fam[p+1..].to_owned(); fam = fam[..p].to_owned(); hyph = true;
        }
    }
    if !hyph {
        if let Some(sr) = GetStyleType(&fam, /*reverse=*/true) {   // SUFFIX match
            fam.truncate(fam.len() - sr.len);
            style_out |= sr.style;
        }
    }
    (style_out, GetPitchFamilyFromFlags(flags), None, hyph)
};
```
**A base-14 font skips all name-based style parsing entirely** and derives both
style and pitch from its index.

`GetStyleFromBaseFont(i)` (`:192-204`): if `i < kSymbol` (12), let `pos = i % 4`;
`ForceBold` if `pos == 1 || pos == 2`; `Italic` if `pos / 2 != 0` (i.e. pos 2 or
3). Matches the enum's Regular/Bold/BoldOblique/Oblique layout.

`GetPitchFamilyFromBaseFont(i)` (`:206-214`): `i < 4` ⇒ `kFontPitchFamilyFixed`
(1); `i >= 8` ⇒ `kFontPitchFamilyRoman` (16); else 0.

`GetPitchFamilyFromFlags(flags)` (`:216-228`): OR of
`Serif→Roman(16)`, `Script→Script(64)`, `FixedPitch→Fixed(1)`.

**`kFontStyles`** (the style-word table, `:92-104`), **exactly 5 entries, order
significant** (first match wins):

| name | len | style bits |
|---|---|---|
| `Regular` | 7 | Normal (0) |
| `Reg` | 3 | Normal (0) |
| `BoldItalic` | 10 | ForceBold \| Italic |
| `Italic` | 6 | Italic |
| `Bold` | 4 | ForceBold |

**There is no `Oblique`, `Light`, `Black`, `Medium`, or `Semibold` entry.**
`GetStyleType(name, reverse)` (`:106-124`): empty ⇒ None; per entry in order,
skip if `entry.len > name.len()`, then compare `name[name.len()-len..]` (reverse)
or `name[..len]` (forward) for **exact, case-sensitive** equality.

**Step 5 — bold weight inference** (`:585-588`):
```
let old_weight = weight;                           // SAVED
if FontStyleIsForceBold(n_style) { weight = 700 }
```
`old_weight` is the pre-inference value. **Every `internal_subst` call passes
`old_weight`; every `external_subst` call passes `weight`.** Getting this
backwards changes both the MM weight axis and the artificial-bolding level.

**Step 6 — `ParseStyles`** (`:590-594`, function at `:126-176`):
```
let mut is_style_available = false;
if parse_styles(&style, &mut is_style_available, &mut weight, &mut n_style) {
    family = subst_name.clone();                   // abort: use the whole name
    base_font = None;                              // and forget the base-14 identity
}
```
`parse_styles` returns `true` meaning **abort**:
```
if style_str.is_empty() { return false }
let mut i = 0; let mut is_first_item = true;
while i < style_str.len() {
    let buf = style_str[i..].split(',').next().unwrap();          // ParseStyle
    let sr = GetStyleType(buf, /*reverse=*/false);                // PREFIX match
    if (i != 0 && !*is_style_available) || (i == 0 && sr.is_none()) { return true }
    let parsed = match sr { Some(s) => { *is_style_available = true; s.style }
                            None => kFontStyleNormal };
    if FontStyleIsForceBold(parsed) {
        if FontStyleIsForceBold(*n_style) { *weight = 900 }        // "double bold"
        else { *weight = 700; *n_style |= ForceBold }
        is_first_item = false
    }
    if FontStyleIsItalic(parsed) && FontStyleIsForceBold(parsed) { *n_style |= Italic }
    else if FontStyleIsItalic(parsed) {
        if !is_first_item { return true }
        *n_style |= Italic; break
    }
    i += buf.len() + 1
}
false
```
Quirks: `"Bold,Italic"` **aborts** (the second token sees `is_first_item ==
false`), while the single token `"BoldItalic"` yields bold+italic at weight 700.
`"Bold,Bold"` yields weight **900**. An unrecognized *first* token aborts; an
unrecognized *later* token is Normal and the loop continues, but only once
`is_style_available` is already true.

**Step 7 — no font database** (`:596-599`):
```
if font_info_.is_none() {
    return internal_subst(base_font, old_weight, italic_angle, pitch_family, &mut subst)
}
```

**Step 8 — charset and CJK** (`:601-608`):
```
let charset = GetCharset(code_page, base_font, flags);
let is_cjk = FX_CharSetIsCJK(charset);
let mut is_italic = FontStyleIsItalic(n_style);
if let Some(f) = GetFontFamily(n_style, &family) { family = f }
```
`GetCharset(cp, base, flags)` (`:247-257`):
```
if cp != kDefANSI { return FX_GetCharsetFromCodePage(cp) }
if FontStyleIsSymbolic(flags) && base.is_none() { return Symbol }
ANSI
```
`FX_CharSetIsCJK` (`fx_codepage.cpp:286-291`) is exactly
`{ChineseSimplified, ChineseTraditional, Hangul, ShiftJIS}` — **Johab and all
`kMAC_*` CJK charsets are excluded.**

`GetFontFamily(n_style, family)` (`:64-84`) — a family rewrite:
```
if family.contains("Script") {
    if FontStyleIsForceBold(n_style) { return Some("ScriptMTBold") }
    if family.contains("Palace")     { return Some("PalaceScriptMT") }
    if family.contains("French")     { return Some("FrenchScriptMT") }
    if family.contains("FreeStyle")  { return Some("FreeStyleScript") }
    return None
}
for (name, fam) in kAltFontFamilies { if family.contains(name) { return Some(fam) } }
None
```
`kAltFontFamilies` (`:33-42`), exactly 3 entries, **substring, case-sensitive**:
`AGaramondPro → "Adobe Garamond Pro"`, `BankGothicBT-Medium → "BankGothic Md BT"`,
`ForteMT → "Forte"`.

**Step 9 — installed-name matching** (`:610-614`):
```
let mut m = match_installed_fonts(&tt_normalize(&family));
if m.is_empty() && family != subst_name && !has_comma && (!has_hyphen || !is_style_available) {
    m = match_installed_fonts(&tt_normalize(&subst_name));
}
```
`tt_normalize` = `TT_NormalizeName` (`:52-62`): remove **all** spaces, then all
`-`, then all `,`; then if a `+` exists at a **non-zero** index, truncate there;
then ASCII-lowercase. Note this is a *second, looser* subset-prefix strip,
different from `strip_subset_prefix`, and it runs after the punctuation removal
so indices shift.

`MatchInstalledFonts(norm)` (`:404-417`): scan `installed_ttfonts_` in **reverse
insertion order** for `tt_normalize(font) == norm`, returning the original name;
then scan `localized_ttfonts_` in reverse comparing `tt_normalize(pair.0)` (a
PostScript name) and returning `pair.1` (the localized original). Empty on miss.

**Step 10 — the two branches** (`:615-654`):

*Branch A* (`m.is_empty() && base_font.is_none()`):
```
if !is_cjk {
    if !check_third_party(&family, &mut pitch_family) {         // family == "MyriadPro"
        is_italic = italic_angle != 0;                          // OVERRIDE
        if !skip_font_enumeration { weight = old_weight }       // see below
    }
    if is_narrow_font_name(&subst_name) { family = kNarrowFamily.into() }
} else {
    subst.subst_cjk = true;
    if n_style != 0 { subst.weight_cjk = weight }
    if FontStyleIsItalic(n_style) { subst.italic_cjk = true }
}
if FontStyleIsItalic(flags) { is_italic = true }                // the PDF /Flags bit
```
`check_third_party` (`:178-184`) returns true only for the exact name
`"MyriadPro"` and clears the Roman bit from `pitch_family`.
`is_narrow_font_name` (`:289-298`): `"Narrow"` or `"Condensed"` found at a
**non-zero** index. `kNarrowFamily` (`:44-50`) is platform-conditional:
`"LiberationSansNarrow"` on Linux/ChromeOS, `"RobotoCondensed"` on Android,
`"ArialNarrow"` elsewhere — **we take the Linux value**, matching the oracle.

The `skip_font_enumeration_` guard is documented in-source (`:619-622`): in
"version 2" mode (a font database rather than GDI-style enumeration) the weight
is **not** reset, so bold/light variants survive into the database query. **Our
`fontdb` backend is exactly that mode**, so we take the `skip_font_enumeration_
== true` branch — see D7.

*Branch B* (otherwise):
```
italic_angle = 0;                                  // no synthetic obliquing
if n_style == kFontStyleNormal { weight = 400 }
if !m.is_empty() { family = m }
if let Some(bf) = base_font {
    base_font = Some(adjust_base_font_for_style(bf, n_style));
    family = GetCanonicalFontName(base_font.unwrap());
}
```
`adjust_base_font_for_style(base, style)` (`:230-245`):
```
if style == 0 || !is_stylable(base) { return base }        // is_stylable: base in {0, 4, 8}
match (bold, italic) { (true,true) => base+2, (true,false) => base+1,
                       (false,true) => base+3, _ => base }
```

**Step 11 — Rung 1: the font database** (`:655-660`):
```
if let Some(h) = font_info.map_font(weight, is_italic, charset, pitch_family, &family) {
    return external_subst(h, &subst_name, weight, is_italic, italic_angle, charset, &mut subst)
}
```

**Step 12 — Rung 2: exact name** (`:662-674`):
```
if is_cjk { is_italic = italic_angle != 0; weight = old_weight }
if !m.is_empty() {
    return match font_info.get_font(&m) {
        None => internal_subst(base_font, old_weight, italic_angle, pitch_family, &mut subst),
        Some(h) => external_subst(h, &subst_name, weight, is_italic, italic_angle, charset, &mut subst),
    }
}
```

**Step 13 — Rung 3: symbolic retry** (`:676-688`):
```
if charset == Symbol {
    // non-Windows only (our target):
    if subst_name == "Symbol" {
        subst.family = "Chrome Symbol"; subst.charset = Symbol
        return internal_subst(Some(kSymbol), old_weight, italic_angle, pitch_family, &mut subst)
    }
    return find_subst_face(&family, is_truetype, flags & !kFontStyleSymbolic,
                           weight, italic_angle, kDefANSI, &mut subst)   // ONE recursion
}
```
The recursion re-enters at step 0 with the *rewritten* family and a non-symbolic
ANSI request. Since `flags` no longer has the symbolic bit, `GetCharset` at step
8 will return ANSI, so the recursion cannot re-enter this rung — **it terminates
after at most one level**. Encode that as a `bool retried` parameter rather than
relying on the reasoning.

**Step 14 — Rung 4: ANSI** (`:690-693`):
`if charset == ANSI { return internal_subst(base_font, old_weight, italic_angle,
pitch_family, &mut subst) }`

**Step 15 — Rung 5: any face with this charset** (`:695-707`):
```
match face_array_.iter().find(|f| f.charset == charset) {          // INSERTION order
    None => internal_subst(base_font, old_weight, italic_angle, pitch_family, &mut subst),
    Some(f) => match font_info.get_font(&f.name) {
        None => None,                                              // the ONE null return
        Some(h) => external_subst(h, &subst_name, weight, is_italic,
                                  italic_angle, charset, &mut subst),
    }
}
```

**`internal_subst(base_font, weight, italic_angle, pitch_family, &mut subst)`**
(`:419-462`) — the terminal rung, itself two-level:
```
if let Some(i) = base_font {
    return standard_faces[i].get_or_init(|| Face::new(kFoxitFonts[i], 0))
    // NOTE: weight, italic_angle, pitch_family and `subst` are ALL IGNORED here
}
subst.set_is_built_in_generic_font()                 // flag_mm_ = true
subst.italic_angle = italic_angle
if weight != 0 { subst.weight = weight }
if FontFamilyIsRoman(pitch_family) {                 // bit 16
    subst.use_chrome_serif()                         // weight = weight*4/5; family="Chrome Serif"
    generic_serif_face.get_or_init(|| Face::new(kFoxitSerifMMFontData, 0))
} else {
    subst.family = "Chrome Sans"
    generic_sans_face.get_or_init(|| Face::new(kFoxitSansMMFontData, 0))
}
```
**This rung always produces a face** (the two MM blobs are always available), so
`FindSubstFace` returns `None` only via step 15's `get_font` failure.

**`external_subst(handle, subst_name, weight, is_italic, italic_angle, charset,
&mut subst)`** (`:464-506`):
```
let got_name = font_info.face_name(handle, &mut face_name);
if charset == Default { font_info.font_charset(handle, &mut charset) }
let ttc_size  = font_info.font_data_size(handle, kTableTTCF)
let font_size = font_info.font_data_size(handle, kTableNone)
if font_size == 0 && ttc_size == 0 { return None }
let face = if ttc_size != 0 { cached_ttc_face(handle, ttc_size, font_size) }
           else { cached_face(handle, &face_name, weight, is_italic, font_size) };
let face = face?;
if !got_name {
    if let Some(n) = font_name_from_face(&face) { face_name = n }
}
subst.configure_external(&face_name, charset, weight, is_italic, italic_angle,
                         face.is_bold(), face.is_italic());
Some(face)
```
`font_name_from_face(face)` (`:275-287`): family; if empty ⇒ empty; else if
`style_name` is non-empty and `!= "Regular"`, append `' '` + style.
Pinned by `cfx_fontmapper_unittest.cpp`'s
`SetSubstFontNameWhenGetFaceNameFails` (`:210-251`): loading
`NotoSansSC-Regular.subset.otf` with `GetFaceName` returning false yields
`subst.family == "Noto Sans SC Regular"`.

**`ConfigureExternalSubst`** (`cfx_substfont.cpp:174-196`):
```
family = face_name; charset = charset
let face_weight = if face_is_bold { 700 } else { 400 };
if weight != face_weight { self.weight = weight }          // 0 stays 0 otherwise!
if is_italic && !face_is_italic {
    if italic_angle == 0 { italic_angle = -12 }
    else if italic_angle.abs() < 5 { italic_angle = 0 }
    self.italic_angle = italic_angle
}
```
**`weight == 0` is a sentinel meaning "the face's natural weight"** — it is
deliberately left 0 when the request matches, which makes
`GetEmboldenLevel*` return 0 (`0 <= 400`) and makes `AdjustVariationParams` use
the axis default.

**The end-to-end substitution test**, `cfx_fontmapper_unittest.cpp:253-310`
(`FindSubstFaceForRegularStandardFontWithBoldWeight`): input
`("Arial-ItalicMT", is_truetype=true, flags=kFontUseExternAttr, weight=700,
italic_angle=0, kDefANSI)` must reach
`MapFont(weight=700, italic=true, charset=ANSI, pitch_family=0,
face="Helvetica-Oblique")`. Trace: alias → `Helvetica-Oblique` →
`std_font = kHelveticaOblique (7)` → `n_style = Italic` (7 % 4 == 3) →
`pitch_family = 0` (4 ≤ 7 < 8) → no ForceBold so weight stays 700 → Branch B
(base_font is Some) → `n_style != Normal` so weight is **not** reset →
`adjust_base_font_for_style(7, Italic)` leaves 7 (7 is not stylable) → family
`"Helvetica-Oblique"`. The test carries an explicit
`TODO(crbug.com/500640684): Should be 400` on the expected weight — **the 700 is
a known upstream bug we reproduce**.

### 1.13 The standard-14 tables (`CFX_StandardFont`)

**The 14 canonical names**, `kBase14FontNames` (`cfx_standardfont.cpp:43-59`),
indexed by `Index { Courier=0, CourierBold, CourierBoldOblique, CourierOblique,
Helvetica=4, HelveticaBold, HelveticaBoldOblique, HelveticaOblique, Times=8,
TimesBold, TimesBoldOblique, TimesOblique, Symbol=12, Dingbats=13 }`:

```
0  "Courier"                 7  "Helvetica-Oblique"
1  "Courier-Bold"            8  "Times-Roman"
2  "Courier-BoldOblique"     9  "Times-Bold"
3  "Courier-Oblique"        10  "Times-BoldItalic"
4  "Helvetica"              11  "Times-Italic"
5  "Helvetica-Bold"         12  "Symbol"
6  "Helvetica-BoldOblique"  13  "ZapfDingbats"
```
Note the asymmetry: Courier and Helvetica use `Oblique`, Times uses `Italic`. The
intra-family order **Regular, Bold, BoldOblique, Oblique** (`+0,+1,+2,+3`) is
load-bearing for `GetStyleFromBaseFont` and `AdjustBaseFontForStyle` (§1.12).

**`kAltFontNames`** (`cfx_standardfont.cpp:66-155`), **89 entries**, sorted for
`FXSYS_stricmp`-ordered `lower_bound`, mapping an alias to an `Index`.
Alias → canonical, complete:

| alias | → | alias | → |
|---|---|---|---|
| `Arial` | Helvetica | `Helvetica` | Helvetica |
| `Arial,Bold` | Helvetica-Bold | `Helvetica,Bold` | Helvetica-Bold |
| `Arial,BoldItalic` | Helvetica-BoldOblique | `Helvetica,BoldItalic` | Helvetica-BoldOblique |
| `Arial,Italic` | Helvetica-Oblique | `Helvetica,Italic` | Helvetica-Oblique |
| `Arial-Bold` | Helvetica-Bold | `Helvetica-Bold` | Helvetica-Bold |
| `Arial-BoldItalic` | Helvetica-BoldOblique | `Helvetica-BoldItalic` | Helvetica-BoldOblique |
| `Arial-BoldItalicMT` | Helvetica-BoldOblique | `Helvetica-BoldOblique` | Helvetica-BoldOblique |
| `Arial-BoldMT` | Helvetica-Bold | `Helvetica-Italic` | Helvetica-Oblique |
| `Arial-Italic` | Helvetica-Oblique | `Helvetica-Oblique` | Helvetica-Oblique |
| `Arial-ItalicMT` | Helvetica-Oblique | `HelveticaBold` | Helvetica-Bold |
| `ArialBold` | Helvetica-Bold | `HelveticaBoldItalic` | Helvetica-BoldOblique |
| `ArialBoldItalic` | Helvetica-BoldOblique | `HelveticaItalic` | Helvetica-Oblique |
| `ArialItalic` | Helvetica-Oblique | `Symbol` | Symbol |
| `ArialMT` | Helvetica | `SymbolMT` | Symbol |
| `ArialMT,Bold` | Helvetica-Bold | `Times-Bold` | Times-Bold |
| `ArialMT,BoldItalic` | Helvetica-BoldOblique | `Times-BoldItalic` | Times-BoldItalic |
| `ArialMT,Italic` | Helvetica-Oblique | `Times-Italic` | Times-Italic |
| `ArialRoundedMTBold` | Helvetica-Bold | `Times-Roman` | Times-Roman |
| `Courier` | Courier | `TimesBold` | Times-Bold |
| `Courier,Bold` | Courier-Bold | `TimesBoldItalic` | Times-BoldItalic |
| `Courier,BoldItalic` | Courier-BoldOblique | `TimesItalic` | Times-Italic |
| `Courier,Italic` | Courier-Oblique | `TimesNewRoman` | Times-Roman |
| `Courier-Bold` | Courier-Bold | `TimesNewRoman,Bold` | Times-Bold |
| `Courier-BoldOblique` | Courier-BoldOblique | `TimesNewRoman,BoldItalic` | Times-BoldItalic |
| `Courier-Oblique` | Courier-Oblique | `TimesNewRoman,Italic` | Times-Italic |
| `CourierBold` | Courier-Bold | `TimesNewRoman-Bold` | Times-Bold |
| `CourierBoldItalic` | Courier-BoldOblique | `TimesNewRoman-BoldItalic` | Times-BoldItalic |
| `CourierItalic` | Courier-Oblique | `TimesNewRoman-Italic` | Times-Italic |
| `CourierNew` | Courier | `TimesNewRomanBold` | Times-Bold |
| `CourierNew,Bold` | Courier-Bold | `TimesNewRomanBoldItalic` | Times-BoldItalic |
| `CourierNew,BoldItalic` | Courier-BoldOblique | `TimesNewRomanItalic` | Times-Italic |
| `CourierNew,Italic` | Courier-Oblique | `TimesNewRomanPS` | Times-Roman |
| `CourierNew-Bold` | Courier-Bold | `TimesNewRomanPS-Bold` | Times-Bold |
| `CourierNew-BoldItalic` | Courier-BoldOblique | `TimesNewRomanPS-BoldItalic` | Times-BoldItalic |
| `CourierNew-Italic` | Courier-Oblique | `TimesNewRomanPS-BoldItalicMT` | Times-BoldItalic |
| `CourierNewBold` | Courier-Bold | `TimesNewRomanPS-BoldMT` | Times-Bold |
| `CourierNewBoldItalic` | Courier-BoldOblique | `TimesNewRomanPS-Italic` | Times-Italic |
| `CourierNewItalic` | Courier-Oblique | `TimesNewRomanPS-ItalicMT` | Times-Italic |
| `CourierNewPS-BoldItalicMT` | Courier-BoldOblique | `TimesNewRomanPSMT` | Times-Roman |
| `CourierNewPS-BoldMT` | Courier-Bold | `TimesNewRomanPSMT,Bold` | Times-Bold |
| `CourierNewPS-ItalicMT` | Courier-Oblique | `TimesNewRomanPSMT,BoldItalic` | Times-BoldItalic |
| `CourierNewPSMT` | Courier | `TimesNewRomanPSMT,Italic` | Times-Italic |
| `CourierStd` | Courier | `ZapfDingbats` | ZapfDingbats |
| `CourierStd-Bold` | Courier-Bold | | |
| `CourierStd-BoldOblique` | Courier-BoldOblique | | |
| `CourierStd-Oblique` | Courier-Oblique | | |

**Lookup is case-insensitive** (`FXSYS_stricmp`, `:161-173`) — pinned by
`cfx_standardfont_unittest.cpp:52` (`"arial"` → Helvetica). The sortedness is an
artifact of the `lower_bound` search; a case-insensitive map is behaviorally
identical and is what we build. Note the sort order that makes the C++ correct:
`,` (0x2C) < `-` (0x2D) < digits < letters, which is why `Arial,Bold` precedes
`Arial-Bold`.

`IsStandardFontName(name)` (`:181-183`) is an **exact, case-sensitive** match
against the 14 canonical names only — *not* the alias table.

**The 16 embedded Foxit blobs** (`fontdata/chromefontdata/chromefontdata.h:14-29`),
with their formats (verified by magic bytes):

| symbol | bytes | slot | format |
|---|---|---|---|
| `kFoxitFixedFontData` | 17 597 | 0 Courier | bare CFF (`01 00 04 02`) |
| `kFoxitFixedBoldFontData` | 18 055 | 1 | bare CFF |
| `kFoxitFixedBoldItalicFontData` | 19 151 | 2 | bare CFF |
| `kFoxitFixedItalicFontData` | 18 746 | 3 | bare CFF |
| `kFoxitSansFontData` | 15 025 | 4 Helvetica | bare CFF |
| `kFoxitSansBoldFontData` | 16 344 | 5 | bare CFF |
| `kFoxitSansBoldItalicFontData` | 16 418 | 6 | bare CFF |
| `kFoxitSansItalicFontData` | 16 339 | 7 | bare CFF |
| `kFoxitSerifFontData` | 19 469 | 8 Times | bare CFF |
| `kFoxitSerifBoldFontData` | 19 395 | 9 | bare CFF |
| `kFoxitSerifBoldItalicFontData` | 20 733 | 10 | bare CFF |
| `kFoxitSerifItalicFontData` | 21 227 | 11 | bare CFF |
| `kFoxitSymbolFontData` | 16 729 | 12 Symbol | bare CFF |
| `kFoxitDingbatsFontData` | 29 513 | 13 ZapfDingbats | bare CFF |
| `kFoxitSerifMMFontData` | 113 417 | "Chrome Serif" | **PFB Type1 MM** (`80 01 … %!PS-Adobe`) |
| `kFoxitSansMMFontData` | 66 919 | "Chrome Sans" | **PFB Type1 MM** |

Total ~365 KiB. **All 14 base-14 blobs are bare CFF**, which `skrifa`/`read-fonts`
reads natively. **The two generic fallbacks are PFB Multiple-Master Type 1** —
the single hardest requirement placed on `pdfrum-type1` (§3.6). They are BSD
licensed (PLAN.md §3) and vendored into `crates/pdfrum-font/fontdata/`.

### 1.14 Glyph rendering surface — what the skrifa integration must reproduce

This subsection is the contract for `GlyphSource`. PDFium drives FreeType; we
drive `skrifa`. **PDFium's own skrifa bridge** (`core/fxge/skrifa/src/main.rs`)
is the normative reference for what a Rust backend must expose, and
`cfx_face.cpp`'s `PDF_ENABLE_SKIA_TYPEFACE_CHECKS` blocks are a ready-made
differential-test specification.

**Face creation** (`cfx_face.cpp:364-397`): `FT_New_Memory_Face(data, index)`
followed by an unconditional `FT_Set_Pixel_Sizes(face, 64, 64)`. This fixed 64px
size is why the render matrix is pre-divided by 64 and why outline coordinates
are divided by `kCoordUnit = 64 * 64.0 = 4096`. **With skrifa we ask for
`unscaled_outline` directly and this whole scale dance disappears** — but the
*resulting coordinates must match*, so see below.

**Outline extraction — the target.** PDFium's Fontations path
(`cfx_face.cpp:917-930`) is:
```
skrifa_font.unscaled_outline(gid, &mut outline) -> ConvertOutline(outline)
```
i.e. **font units, unscaled**, then converted to a path. `ConvertOutline`
(`:178-230`) maps skrifa verbs to path segments, **converting quadratics to
cubics to match FreeType's decomposition**:
```
MoveTo(p)          -> move_to(p)
LineTo(p)          -> line_to(p)
QuadTo(c0, p)      -> curve_to(cur + (c0-cur)*2/3,  c0 + (p-c0)/3,  p)
CurveTo(c0, c1, p) -> curve_to(c0, c1, p)
Close              -> close_path()
```
Our `Font::glyph_path` returns a `kurbo::BezPath` in **1000/em text space**
(SPEC §6), so we apply `Affine::scale(1000.0 / upem as f64)` to the unscaled
outline. PDFium's FreeType path reaches the same place by a different route
(`FT_Set_Pixel_Sizes(0, 64)` then dividing by 4096, which is `1/64` of a 64px em
— i.e. em-relative units, then the caller scales by the font size). The
*Fontations* path is the one to match, because that is where upstream is going.

`Outline_CheckEmptyContour` (`:84-106`) trims degenerate trailing contours before
each `MoveTo` and once at the end: drop the last 2 points if they are
`[Move(open), same-point]`; drop the last 4 if they are
`[Move, Bezier, Bezier, Bezier]` all at the same point. An empty final point list
⇒ **no path at all** (`None`, not an empty path). Reproduce — `kurbo` will
happily hold a degenerate path and the rasterizer's output would differ.

**Hinting flags.** PDFium's choices, which we must translate to skrifa's
`Hinting` setting:

| path | flags | condition |
|---|---|---|
| `RenderGlyph` (`:841-844`) | `NO_BITMAP \| PEDANTIC` | always |
| ″ | `+ NO_HINTING` | if **not** an SFNT face (Type1/bare-CFF) |
| ″ retry (`:849-858`) | drop `PEDANTIC`, add `NO_HINTING` | only if the first load errored and `NO_HINTING` was not already set; otherwise give up |
| `LoadGlyphPath` (`:948-951`) | `NO_BITMAP` | always |
| ″ | `+ NO_HINTING` | if `!is_sfnt \|\| !is_tricky` |
| `GetGlyphWidth` (`:1029`) | `NO_SCALE \| IGNORE_GLOBAL_ADVANCE_WIDTH` | always |
| `LoadGlyph(gid, scale)` (`:1145-1148`) | `IGNORE_GLOBAL_ADVANCE_WIDTH` `[+ NO_SCALE if !scale]` | — |

**We render text as filled outline paths** (SPEC §8), not as FreeType bitmaps, so
the `LoadGlyphPath` row is the one that governs: **hinting is enabled only for a
face that is both SFNT and "tricky"**, i.e. essentially never. Our
`GlyphSource` therefore requests **unhinted** outlines unconditionally, and the
`GlyphCache` key's `hint-flags` component (SPEC §6) degenerates to a constant for
now — keep the field, it costs nothing and documents the axis.

`is_tricky` = `FT_FACE_FLAG_TRICKY`, FreeType's hard-coded list of ~20 CJK fonts
whose glyphs are only correct when hinted (`cfx_face.cpp:448-458`). skrifa has no
equivalent flag. **Escalated as OQ-4.**

**Advance widths.** Two different normalizers (§1.3):
- `CFX_Face::GetGlyphWidth(gid, dest_width, weight, subst)` (`:1020-1055`):
  MM adjust if generic; `FT_Load_Glyph(NO_SCALE | IGNORE_GLOBAL_ADVANCE_WIDTH)`;
  on error **0**; then a range guard
  `if adv < i32::MIN/1000 || adv > i32::MAX/1000 { return 0 }`; then
  `EmAdjust(adv)` = **truncating** `adv * 1000 / upem`.
- `CFX_Face::GetGlyphTTWidth(gid)` (`:988-1018`): asserts the glyph is already
  loaded, uses **`NormalizeFontMetric`** (rounding + saturating). This is the one
  `LoadCharMetrics` (§1.5) calls.

**Glyph bounding boxes.** `CFX_Face::GetGlyphBBox(gid)` (`:1321-1334`), on the
already-loaded glyph:
```
left = clamp(metrics.horiBearingX); top = clamp(metrics.horiBearingY)
FX_RECT( normalize(left, upem), normalize(top, upem),
         normalize(left + metrics.width, upem), normalize(top - metrics.height, upem) )
```
i.e. `(left, top, right, bottom)` with `bottom = top - height` — a y-**down**
rect whose `top` holds the larger y. skrifa's `glyph_bounds` returns
`(x_min, y_min, x_max, y_max)`, which PDFium's own check block (`:1336-1344`)
maps as `(normalize(x_min), normalize(y_max), normalize(x_max), normalize(y_min))`
and asserts agreement **within 2 units**. Take that mapping.

`CFX_Face::GetCharBBox` (`:1258-1319`) adds, on the non-tricky path, a
**+1/64 top expansion**: `if rect.top <= 2114445437 { rect.top += rect.top / 64 }
else { rect.top = i32::MAX }`. `kMaxRectTop = 2114445437` (`:74`).

**Synthetic obliquing (skew).** `CFX_SubstFont::GetSkew()` indexes
`kAngleSkew: [i8; 30]` (`cfx_substfont.cpp:53-56`) by `-italic_angle`:
```
-0,  -2,  -3,  -5,  -7,  -9,  -11, -12, -14, -16, -18, -19, -21, -23, -25,
-27, -29, -31, -32, -34, -36, -38, -40, -42, -45, -47, -49, -51, -53, -55
```
with `GetSkewFromAngle(a)` (`:58-66`) returning **-58** for `a > 0`,
`a == i32::MIN`, or `(-a) >= 30`. `GetSkewCJK()` = `GetSkewFromAngle(if
italic_cjk { -15 } else { 0 })` ⇒ **-27** or **0**.
`GetEffectiveSkew(is_cid_font)` = `if subst_cjk && is_cid_font { GetSkewCJK() }
else { GetSkew() }`.

Application (`cfx_face.cpp:829-833`, `:937-941`): as a shear on the transform,
```
if is_vertical { m.yx += m.yy * skew / 100 } else { m.xy -= m.xx * skew / 100 }
```
In `kurbo` terms with an identity base this is
`Affine::new([1.0, 0.0, -skew as f64/100.0, 1.0, 0.0, 0.0])` for horizontal
(shearing x by y) and the transposed form for vertical. **Verify the sign and
axis against a rendered fixture** — the FreeType matrix convention transposes
`b`/`c` relative to `kurbo`'s.

Pinned by `cfx_substfont_unittest.cpp:9-17`: `SetItalicAngle(-12)` ⇒
`GetEffectiveSkew(false) == -21`; then `subst_cjk = italic_cjk = true` ⇒
`GetEffectiveSkew(true) == -27`.

**Artificial emboldening.** Three `[u8; 100]` tables
(`cfx_substfont.cpp:18-45`), indexed by `(weight - 400) / 10`:

`kWeightPow` (path **load**, non-ShiftJIS):
```
0,   6,   12,  14,  16,  18,  22,  24,  28,  30,  32,  34,  36,  38,  40,
42,  44,  46,  48,  50,  52,  54,  56,  58,  60,  62,  64,  66,  68,  70,
70,  72,  72,  74,  74,  74,  76,  76,  76,  78,  78,  78,  80,  80,  80,
82,  82,  82,  84,  84,  84,  84,  86,  86,  86,  88,  88,  88,  88,  90,
90,  90,  90,  92,  92,  92,  92,  94,  94,  94,  94,  96,  96,  96,  96,
96,  98,  98,  98,  98,  100, 100, 100, 100, 100, 102, 102, 102, 102, 102,
104, 104, 104, 104, 104, 106, 106, 106, 106, 106
```

`kWeightPow11` (**render**, non-ShiftJIS) — **note the three non-monotonic
entries at indices 52, 59, 63 (values 43, 45, 46); reproduce verbatim**:
```
0,  4,  7,  8,  9,  10, 12, 13, 15, 17, 18, 19, 20, 21, 22, 23, 24,
25, 26, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 39, 39, 40, 40, 41,
41, 41, 42, 42, 42, 43, 43, 43, 44, 44, 44, 45, 45, 45, 46, 46, 46,
46, 43, 47, 47, 48, 48, 48, 48, 45, 50, 50, 50, 46, 51, 51, 51, 52,
52, 52, 52, 53, 53, 53, 53, 53, 54, 54, 54, 54, 55, 55, 55, 55, 55,
56, 56, 56, 56, 56, 57, 57, 57, 57, 57, 58, 58, 58, 58, 58
```

`kWeightPowShiftJis` (both paths, when `charset == ShiftJIS`):
```
0,   0,   2,   4,   6,   8,   10,  14,  16,  20,  22,  26,  28,  32,  34,
38,  42,  44,  48,  52,  56,  60,  64,  66,  70,  74,  78,  82,  86,  90,
96,  96,  96,  96,  98,  98,  98,  100, 100, 100, 100, 102, 102, 102, 102,
104, 104, 104, 104, 104, 106, 106, 106, 106, 106, 108, 108, 108, 108, 108,
110, 110, 110, 110, 110, 112, 112, 112, 112, 112, 112, 114, 114, 114, 114,
114, 114, 114, 116, 116, 116, 116, 116, 116, 116, 118, 118, 118, 118, 118,
118, 118, 120, 120, 120, 120, 120, 120, 120, 120
```

```
GetWeightLevel(i)         = if i >= 100 { -1 }                       // FAILS
                            else if shift_jis { kWeightPowShiftJis[i] }
                            else { kWeightPow11[i] }
GetWeightLevelForLoad(i)  = let i = min(i, 99);                      // CLAMPS
                            if shift_jis { kWeightPowShiftJis[i] * 65536 / 36655 }
                            else { kWeightPow[i] }

GetEmboldenLevelForRender(is_cid, m_xx, m_xy) =
    if flag_mm { 0 }
    else { let w = GetEffectiveWeight(is_cid);
           if w <= 400 { 0 } else {
             let lvl = GetWeightLevel(((w - 400) / 10) as usize);
             if lvl < 0 { -1 }                                       // caller aborts
             else { (lvl as i64 * (m_xx.abs() as i64 + m_xy.abs() as i64) / 36655)
                    .try_into().unwrap_or(0) } } }                   // i64 intermediate

GetEmboldenLevelForLoad() =
    if flag_mm { 0 }
    else { let w = GetWeight();                                      // NOT effective
           if w <= 400 { 0 } else { GetWeightLevelForLoad(((w - 400) / 10) as usize) } }
```
Pinned by `cfx_substfont_unittest.cpp:28-44`: weight 700 ⇒ index 30;
`GetEmboldenLevelForRender(false, 1024, 0) == 1` (`39 * 1024 / 36655`);
`GetEmboldenLevelForLoad() == 70`;
`GetEmboldenLevelForRender(false, 30_000_000, 30_000_000) == 63838` (proving the
i64 intermediate: `39 * 60_000_000 = 2.34e9 > i32::MAX`, `/36655 = 63838`); and
after `SetIsBuiltInGenericFont()`, **both return 0**.
`GetEstimatedStemV() == weight / 5` ⇒ 140 for weight 700 (`:46-50`).

Applied via `FT_Outline_Embolden(&outline, level)` — an outline-dilation
operation. **skrifa has no equivalent.** We must implement it or accept a
Tier-B pixel difference on artificially-bolded substituted fonts. See D8/OQ-5.

**Multiple-Master axis solving.** `AdjustVariationParams(gid, dest_width, weight)`
(`cfx_face.cpp:1561-1605`), called only when `subst.IsBuiltInGenericFont()`,
from `RenderGlyph`, `LoadGlyphPath`, and `GetGlyphWidth`:
```
let mm = face.mm_var()?;                     // no MM axes: no-op
let mut coords = [0i64; 2];                  // EXACTLY 2 axes
coords[0] = if weight == 0 { mm.axis_default(0) / 65536 } else { weight as i64 };
if dest_width == 0 {
    coords[1] = mm.axis_default(1) / 65536;
} else {
    let (lo, hi) = (mm.axis_min(1) / 65536, mm.axis_max(1) / 65536);
    coords[1] = lo; face.set_mm_design_coords(&coords);
    face.load_glyph(gid, NO_SCALE | IGNORE_GLOBAL_ADVANCE_WIDTH);
    let min_w = glyph.metrics.horiAdvance * 1000 / upem;
    coords[1] = hi; face.set_mm_design_coords(&coords);
    face.load_glyph(gid, NO_SCALE | IGNORE_GLOBAL_ADVANCE_WIDTH);
    let max_w = glyph.metrics.horiAdvance * 1000 / upem;
    if max_w == min_w { return }             // degenerate: coords stay at the max probe
    coords[1] = lo + (hi - lo) * (dest_width - min_w) / (max_w - min_w);
}
face.set_mm_design_coords(&coords)
```
Axis 0 is weight (the PDF weight used **directly** as a design coordinate); axis
1 is width, solved so the glyph's 1000/em advance equals `dest_width`. The
interpolation is **not clamped**, so an extreme `dest_width` extrapolates. Two
probe glyph loads per width solve.

`dest_width` originates from the PDF's declared width for the charcode
(`CPDF_Font::GetCharWidth`), passed down by the render layer.

**Glyph cache key.** Three separate caches per **face** (`cfx_glyphcache.h:71-74`);
`CFX_FontMgr::GetGlyphCache` keys on the `CFX_Face*` pointer, so two `CFX_Font`s
sharing a face share a cache — which is why the subst parameters appear in every
per-entry key:

| cache | key |
|---|---|
| path | `(gid, dest_width, weight, italic_angle, vertical)` where the last three come from the subst font (`0`/`0`/`false` when there is none) |
| width | `(gid, dest_width, weight)` — `weight` is the **caller's argument**, not the subst font's |
| bitmap | a byte string of `(m.a*10000, m.b*10000, m.c*10000, m.d*10000, dest_width, anti_alias)` as `i32`s, plus `(weight, italic_angle, vertical)` when a subst font exists (`cfx_glyphcache.cpp:32-97`) |

The matrix translation (`e`, `f`) is **not** in the key. SPEC §6's
`(font-id, gid, hint-flags)` key must grow to `(font-id, gid, dest_width, weight,
italic_angle, vertical)` for the path cache — see D9.

`kInvalidGlyphIndex = u32::MAX` (`cfx_glyphcache.cpp:30`) short-circuits every
lookup. Note `LookUpGlyphBitmap` (`:212-240`) **caches a null render result**, so
a failed glyph is memoized as "nothing".

**Charset inference from a unicode** — `CFX_Font::GetCharSetFromUnicode`
(`cfx_font.cpp:83-141`), an ordered first-hit ladder:

| range(s) | charset |
|---|---|
| `< 0x7F` | ANSI ("avoid CJK Font to show ASCII") |
| `4E00..=9FA5`, `E7C7..=E7F3`, `3000..=303F`, `2000..=206F` | ChineseSimplified |
| `3040..=309F`, `30A0..=30FF`, `31F0..=31FF`, `FF00..=FFEF` | ShiftJIS |
| `AC00..=D7AF`, `1100..=11FF`, `3130..=318F` | Hangul |
| `0E00..=0E7F` | Thai |
| `0370..=03FF`, `1F00..=1FFF` | MSWin_Greek |
| `0600..=06FF`, `FB50..=FEFC` | MSWin_Arabic |
| `0590..=05FF` | MSWin_Hebrew |
| `0400..=04FF` | MSWin_Cyrillic |
| `0100..=024F` | MSWin_EasternEuropean |
| `1E00..=1EFF` | MSWin_Vietnamese |
| otherwise | ANSI |

Note General Punctuation `2000..206F` is claimed by ChineseSimplified, and
`0x7F` itself falls through to ANSI.

`kDefaultTTFMap` (`cfx_font.cpp:34-49`) — charset → a default face name:
`ANSI→"Helvetica"`, `ChineseSimplified→"SimSun"`,
`ChineseTraditional→"MingLiU"`, `ShiftJIS→"MS Gothic"`, `Hangul→"Batang"`,
`MSWin_Cyrillic→"Arial"`, `MSWin_EasternEuropean→"Arial"` (Linux),
`MSWin_Arabic→"Arial"`; miss ⇒ `"Arial Unicode MS"`.

### 1.15 `GetCharPosList` — where everything composes

`cpdf_font.cpp:392-501`. The page/render layer calls this per text-showing
operator. It is the single place where glyph selection, fallback fonts, widths,
CID vertical origin and the CID transform all meet, so it is the best
specification of how our `Font::decode` + `CharItem` must behave.

Per charcode (skipping `charcode == u32::MAX`):
```
let unicode = self.unicode_from_charcode(char_code);       // ToUnicode-first
pos.unicode = unicode.first().unwrap_or(char_code as char-ish);
pos.glyph_index = self.glyph_from_charcode(char_code, &mut is_vertical_glyph);
let current_font = if self.should_use_font(pos.glyph_index, has_to_unicode) {
    pos.fallback_font_position = -1;  self.font()
} else {
    let fb = self.fallback_font_from_charcode(char_code);   // always 0
    pos.fallback_font_position = fb;
    pos.glyph_index = self.fallback_glyph_from_charcode(fb, char_code);
    self.font_fallback(fb)
};
pos.font_char_width = if !IsEmbedded() && !IsCIDFont() { self.char_width(char_code) } else { 0 };
pos.origin = (char_pos[i], 0.0);  pos.glyph_adjust = false;
let mut scaling_factor = 1.0;
if self.should_apply_glyph_spacing_heuristic(current_font, is_vertical_writing) {
    let pdf_w  = self.char_width(char_code);
    let font_w = current_font.glyph_width(pos.glyph_index);
    if font_w != 0 && pdf_w > font_w + 1 {
        pos.origin.x += (pdf_w - font_w) as f32 * font_size / 2000.0;
    } else if pdf_w != 0 && font_w != 0 && pdf_w < font_w {
        scaling_factor = pdf_w as f32 / font_w as f32;
        pos.adjust_matrix = [scaling_factor, 0.0, 0.0, 1.0];
        pos.glyph_adjust = true;
    }
}
// ... CID vertical origin (§1.10.2) and CID transform (§1.10.3) ...
```

**`ShouldUseFont(glyph_id, has_to_unicode)`** (`:368-390`):
```
if glyph_id == u32::MAX { return false }
if IsEmbedded() { return true }
if !IsTrueTypeFont() { return true }
glyph_id != 0 || has_to_unicode          // non-embedded TrueType: glyph 0 is suspect
                                         // unless /ToUnicode vouches for it
```
So **only a non-embedded TrueType font ever falls back per-glyph.**

**`FallbackFontFromCharcode`** (`:536-547`): lazily creates **exactly one**
fallback `CFX_Font` via
`LoadSubstFace("Arial", IsTrueTypeFont(), flags_, saturating(stem_v_ * 5) or 400,
italic_angle_, kDefANSI, IsVertWriting())` and always returns index **0**.

**`FallbackGlyphFromCharcode(fb, charcode)`** (`:549-562`): unicode from
`UnicodeFromCharCode` (first char) or the raw charcode; `char_index(unicode)`;
**-1 if 0**.

**`ShouldApplyGlyphSpacingHeuristic(current_font, is_vertical)`** (`:240-264`):
```
if is_vertical || IsEmbedded() || !HasFontWidths() { return false }
let mut n = self.base_font_name().to_lowercase();
if GetStandardFontIndex(&n).is_some() { return false }     // a standard alternate
let subst = current_font.subst_font();
if subst.is_built_in_generic_font() { return false }
!subst.is_actual_font_loaded(&n)
```
`IsActualFontLoaded(base_name)` (`cfx_substfont.cpp:101-111`): lowercase
`family` with **all spaces removed**, then `base_name.find(that) == Some(0)` —
a prefix test. Pinned (`cfx_substfont_unittest.cpp:52-60`): family
`"Times New Roman"` matches `"timesnewroman,bold"` and
`"timesnewromanps-bold"` but not `"arial,bold"`. The C++ comment acknowledges
that `"Book"` would falsely match `"Bookman"`.

The `/2000.0` in the half-excess shift is `font_size / 1000` (text space) halved.

**`GetFontWeight()`** (`cpdf_font.cpp:610-624`):
```
if let Some(w) = font_weight_ { return Some(w) }            // /FontWeight, when > 0
let sv = stem_v_ as i64;
let v = if stem_v_ < 140 { sv * 5 } else { sv * 4 + 140 };
i32::try_from(v).ok()
```
Used by `LoadSubstFont` (`cpdf_facebasedsimplefont.cpp:208-212`), which then
clamps to `[100, 900]` or falls back to 400.

`LoadSubstFont` also **infers fixed-pitch** (`:189-206`): if `!use_font_width_
&& !FontStyleIsFixedPitch(flags_)`, scan all 256 widths skipping `0` and
`0xffff`; if every remaining width is identical and nonzero, set
`flags_ |= kFontStyleFixedPitch`. The loop `break`s on the first mismatch, and
the flag is set only if the loop ran to completion (`i == 256`).

### 1.16 Type1 / CFF font programs — what PDFium actually does

**PDFium does essentially zero Type1 container parsing.** It never touches PFA/PFB
segments, never runs eexec decryption, never parses charstrings. Verified by
exhaustive grep over `core/`: **zero** hits for `eexec`, `PFA`, `charstring`;
`PFB` appears only as a magic-byte sniff in Windows-only device-font enumeration
(`core/fxge/win32/cwin32_platform.cpp:190-191`). `FT_New_Memory_Face` has
**exactly one call site** (`cfx_face.cpp:370`), fed the raw decompressed
`/FontFile` bytes.

So the entire Type1 contract is: **hand a blob to the font library and get back
the API in §1.16.1.** PDFium's *own* Rust bridge already does exactly this
(`core/fxge/skrifa/src/main.rs:221-263`):
```
new_font(data, index) = Sfnt::new(data)  ?? CffFontRef::new(data, index, None)
                                         ?? Type1Font::new(data)
                                         ?? Error
```
using `read_fonts::ps::{cff, charmap, encoding, string, type1}` with the `agl`
feature.

**This is the decisive scoping fact for `pdfrum-type1`** (§3.6, D6, OQ-2): the
functionality exists in the fontations project, but in a module
(`read_fonts::ps`) that PDFium consumes from a **path dependency** on an
unreleased branch (the `Cargo.toml` header cites fontations PR #1820), not from
the crates.io release we pin (`read-fonts = "=0.43.3"`, DEPS.md).

#### 1.16.1 The API a Type1 backend must expose

Taken from PDFium's bridge (`main.rs:112-148`) and its consumers in
`cfx_face.cpp`. This is the complete upward surface — **no charstring or eexec
API is ever exposed**:

```
font_type()      -> {Unknown, TrueType, Type1, Cff}
postscript_name() -> &str          family_name() -> &str        style_name() -> String
units_per_em()   -> i32            ascent()/descent() -> f32    bbox()
num_glyphs()     -> u32            is_fixed_pitch()/is_tricky()/is_scalable() -> bool
is_cid() -> bool                   cid_to_gid(cid: u16) -> u32
unicode_to_gid(u32) -> u32         code_to_gid(code: u8) -> u32     // built-in encoding
encoding() -> {None, Standard, Expert, IsoLatin1, Custom}
has_glyph_names() -> bool          glyph_name(gid) -> String     name_index(&str) -> u32
scaled_outline(gid, ppem, &mut Outline) -> bool
unscaled_outline(gid, &mut Outline) -> bool
has_outline(gid) -> bool           glyph_bounds(gid) -> BoundingBox
get_os2_{code_page_range, unicode_range, panose, fs_type}
get_char_codes_and_indices(max_char) -> Vec<CharCodeAndIndex>
```

Type1-specific dispatch in the bridge:
- `code_to_gid(c)` = `type1.encoding().and_then(|e| e.map(c))` (`main.rs:395-406`)
- `unicode_to_gid(u)` = `type1.unicode_charmap().map(u)` (`:367-375`)
- `encoding()` maps `PredefinedEncoding::{Standard, Expert→Custom, IsoLatin1}`,
  `None → Custom` (`:377-393`)
- `draw(gid, ppem, pen)` for outlines (`:486`)

**The charmap-encoding set that counts as "the Type1 built-in encoding"** is
pinned by PDFium's own differential check (`cfx_face.cpp:1096-1115`): for a
Type1 face with `code <= 0xFF` and an FT charmap encoding in
`{ADOBE_CUSTOM, ADOBE_STANDARD, ADOBE_EXPERT, APPLE_ROMAN}`, it asserts
`FT_Get_Char_Index(code) == skrifa.code_to_gid(code)`. That set is exactly what
`UseType1Charmap` (§1.8.2) is trying to select.

#### 1.16.2 What a from-scratch `pdfrum-type1` must implement

If we write it ourselves rather than depending on `read_fonts::ps` (OQ-2):

1. **Container sniffing.** Three forms, and `/Length1/2/3` cannot be trusted
   (§1.2 — PDFium ignores them and so must we):
   - **PFB**: segments of `[0x80, type, len_le_u32, data...]` where type 1 =
     ASCII, 2 = binary, 3 = EOF. Concatenate types 1 and 2 in order.
   - **PFA**: `%!PS-AdobeFont` or `%!FontType1` ASCII, with the eexec section
     hex-encoded.
   - **Bare**: neither marker; treat as PFA-shaped.
2. **eexec decryption.** The standard Type1 cipher over the binary section:
   `r = 55665`; per byte `c`: `p = c ^ (r >> 8); r = ((c + r) * 52845 + 22719) as u16`.
   Discard the first **4** plaintext bytes. For the hex (PFA) form, hex-decode
   first. `lenIV` (default **4**) from the Private dict governs how many leading
   bytes each *charstring* discards after its own decryption with `r = 4330`.
3. **The PostScript-ish tokenizer** for the cleartext and decrypted portions:
   `/FontName`, `/FontMatrix`, `/FontBBox`, `/Encoding` (either
   `StandardEncoding` or a `dup <code> /<name> put` sequence), `/Subrs`,
   `/CharStrings`, `/lenIV`, `/BlueValues` etc. Note `/Encoding` arrays are
   written as `dup N /name put` lines, terminated by `readonly def`.
4. **Type 1 charstring interpretation** to outlines: the operator set
   `hstem vstem vmoveto rlineto hlineto vlineto rrcurveto closepath callsubr
   return hsbw endchar rmoveto hmoveto vhcurveto hvcurveto`, plus the escape
   `12 x` operators `dotsection vstem3 hstem3 seac sbw div callothersubr pop
   setcurrentpoint`. Notably `seac` (standard-encoding accented character) needs
   the StandardEncoding table (§1.7) and composes two glyphs; `flex` and `hint
   replacement` ride on `callothersubr` 0–3.
5. **Multiple Master** — required for the two Foxit generic fallbacks (§1.13):
   `/BlendAxisTypes`, `/BlendDesignPositions`, `/BlendDesignMap`,
   `/WeightVector`, and `callothersubr` **14** (`$Blend`) which interpolates
   operand tuples by the weight vector. Two axes for these fonts (§1.14).
6. **Glyph names → GID** by CharStrings dict order, plus a synthesized Unicode
   charmap via the AGL (§1.7).

**Item 5 is the load-bearing one.** Without MM, the terminal substitution rung
has no face and every unresolvable font renders nothing. Fallback if MM is
descoped: instantiate the MM fonts at their default axis positions (ignoring
`AdjustVariationParams`) and accept a Tier-B width/weight difference — or
pre-instantiate them offline into two static CFF blobs and ship those instead.
**Escalated as OQ-2.**

**What we do NOT need:** CFF/Type2 charstrings (skrifa/read-fonts reads bare CFF
natively, which covers all 14 base-14 blobs and every `/FontFile3` Type1C),
OpenType wrapping, or any of the `FT_Get_PS_Font_Info` surface (PDFium never
calls it — zero grep hits).

#### 1.16.3 The Type1 fallback ladder, end to end

Five levels, in trigger order:

- **L1 — face creation fails.** `LoadFaceZeroFromSpan` returns false ⇒
  `font_file_` is purged ⇒ `IsEmbedded()` becomes false ⇒ the *later* branch in
  `LoadCommon` (§1.4 step 3) takes `LoadSubstFont()`. Clean degradation.
- **L2 — substitution** (§1.12), which itself is a 5-rung ladder.
- **L3 — internal builtin** (`internal_subst`, §1.12): the exact Foxit base-14
  CFF blob if the name resolved to an index, else Chrome Serif MM (Roman pitch)
  or Chrome Sans MM. **Always produces a face.**
- **L4 — per-charcode fallback** (`GetCharPosList`, §1.15) — but
  `ShouldUseFont` returns true unconditionally for a non-TrueType font, so
  **a Type1 font never reaches L4** unless it was substituted with a TT face.
- **L5 — no glyph.** `glyph_index_[c] == 0xffff` ⇒ `GlyphFromCharCode` returns
  -1 ⇒ the render layer draws nothing. `LoadCharMetrics` still borrows space's
  bbox/width for a non-embedded font (§1.5).

**A Type1 font effectively never fails to construct** (`LoadCommon` always
returns true, §1.4).

**There is no `cpdf_type1font_unittest.cpp`**, and `testing/resources/` contains
no Type1/PFB fixture. `pdfrum-type1` needs its own fixtures; the nearest
in-tree oracle is PDFium's `PDF_ENABLE_SKIA_TYPEFACE_CHECKS` differential
assertions (`cfx_face.cpp:1063-1073`, `:1096-1115`, `:1133-1139`).

---

## 2. Divergences

**D1 — Apple-only code paths are not ported.** `CPDF_Type1Font`'s CoreText
branches (`cpdf_type1font.cpp:210-266`), `ext_gid_`, `SetExtGID`/`CalcExtGID`,
`GlyphFromCharCodeExt`, the `kGlyphNameSubsts` ligature remap, and
`CPDF_CIDFont::GlyphFromCharCode`'s Apple `cid_is_gid_` branch (`:666-679`) are
all `#if BUILDFLAG(IS_APPLE)`. The oracle is built on Linux (PLAN.md §4), so the
non-Apple paths are normative. Recorded so nobody "completes" the port.

**D2 — Windows-only codepage conversion is not ported.**
`CPDF_CIDFont::GetUnicodeFromCharCode` and `CharCodeFromUnicode` have
`#if BUILDFLAG(IS_WIN)` tails using `FX_MultiByteToWideChar` /
`FX_WideCharToMultiByte` over `kCharsetCodePages` (§1.10.1). The non-Windows
branch — the static-table scan through `pdfrum-cmap` — is normative. The
`kFX_CharsetUnicodes` tables (`fx_codepage.cpp:23-217`, eight 128-entry
Windows-codepage upper halves) are consequently **out of scope**. Same for the
`WCHAR_T_IS_32_BIT`-gated ToUnicode assertions (§1.6.6): our `char` is always a
Unicode scalar, so we take the 32-bit branch.

**D3 — unpaired surrogates in ToUnicode.** PDFium stores UTF-16 code units in a
`WideString` whose element is `wchar_t` (32-bit on Linux), so an unpaired
surrogate survives as a lone `0xD800..0xDFFF` value and reaches text output as
such. Our `CharItem.unicode` is `SmallVec<[char; 2]>` and `char` cannot hold a
surrogate. **Decision:** `Lookup` pairs valid high+low surrogates into one
`char`; an **unpaired** surrogate becomes `U+FFFD`. This is a deliberate,
permanent divergence: the alternative (carrying `u16`s to the text layer) would
push UTF-16 into every downstream signature for a case that appears in
malformed files only. (A `DiagKind::ToUnicodeLoneSurrogate` was specified here
and **never recorded**; it was deleted 2026-09-03. `units_to_chars` sits under
`ToUnicode::lookup`, an `&self` per-glyph query with nowhere to put a sink, and
there is no oracle condition to mirror — PDFium's `WideString` stores the lone
surrogate natively rather than recovering from it.) **Conformance must confirm no corpus file trips it**; if
one does, the waiver is documented in `conformance/thresholds.toml` rather than
reverting the design. Note the `NonBmpUnicodeLookup` assertion (§1.6.6) is a
*paired* surrogate and is unaffected.

**D4 — `Lookup` returning a NUL char.** §1.6.6 flags an ambiguity in
`Lookup(0x20)` for the `DestLargeValue` case (value `0x10000` ⇒ `unicode == 0`).
The two candidate behaviors are "empty sequence" and "one `U+0000`". They differ
observably in `--txt` output. **OQ-3** resolves this by measurement, not
reasoning; the brief does not pick one.

**D5 — bounds safety in the GSUB parser.** `CFX_CTTGSUBTable` reads
attacker-controlled offsets with raw `subspan` and no length checks
(`cfx_cttgsubtable.cpp` throughout). Our port returns `Option`/`Result` at every
read; a malformed GSUB yields "no vertical substitution" plus a diagnostic,
never a panic. `#![forbid(unsafe_code)]` and `clippy::indexing_slicing` make this
mandatory. No behavioral divergence on well-formed input. In practice we build
on `read_fonts::tables::gsub`, which is already bounds-checked, and port only the
*feature-selection policy* (§1.10.6).

**D6 — the AGL comes from `read-fonts`, not a vendored FreeType table.**
PDFium compiles FreeType's `pstables.h` into fxge (§1.7). We use
`read-fonts`' `agl` feature — the same choice PDFium's own skrifa bridge makes
(`core/fxge/skrifa/Cargo.toml`). Two consequences: (a) we must confirm the
`uniXXXX` / `uXXXX`–`uXXXXXX` synthetic-name handling and the `.`-variant strip
are present, and implement them ourselves if not (they are ~30 lines,
`fx_freetype.cpp:126-214`); (b) PDFium's **reverse** lookup
(`AdobeNameFromUnicode`) is an O(table) DFS while a Rust implementation is a map
— strictly better, not a divergence. **DEPS.md already lists `read-fonts`; the
`agl` feature must be enabled**, which is a feature addition, not a new
dependency, and therefore not a `[spec]` change. Recorded here for the reviewer.

**D7 — we take the `skip_font_enumeration_ == true` branch.** §1.12 Branch A's
weight reset is guarded on that flag, which PDFium sets in its "version 2"
font-database mode. `fontdb` **is** that mode: there is no `EnumFontList`
callback, we query the database directly. So `weight` is **not** reset to
`old_weight` in Branch A, and bold/light variants survive into the query. This
matches upstream's intended modern behavior and diverges from the *enumeration*
path the oracle's default Linux build takes.

**This is the one divergence with a real Tier-B risk**, because
`pdfium_test --font-dir=third_party/test_fonts` (PLAN.md §4) drives
`CFX_FolderFontInfo`, which uses enumeration. **Mitigation:** implement
`skip_font_enumeration` as a `SubstitutionOptions` field defaulting to the value
that matches the oracle's configuration, determined empirically in M2 by running
both settings against the font-substitution conformance cluster and keeping the
one with fewer Tier-B failures. The field is public so a caller can pick.
**Escalated as OQ-6.**

**D8 — artificial emboldening is deferred.** `FT_Outline_Embolden` has no skrifa
equivalent (§1.14). M2 (fonts + text extraction) does not need it: emboldening
affects pixels, not glyph identity, widths, or Unicode. **Decision:** compute
and store `embolden_level` on the `SubstFont` record exactly as PDFium does
(the tables and formulas are ported and unit-tested against
`cfx_substfont_unittest.cpp`), and have `pdfrum-render` apply it in M3/M5 via a
path-dilation pass. If the dilation proves hard to match, it becomes a
documented Tier-B waiver on the "substituted bold font" cluster. **OQ-5** asks
whether to attempt the dilation at all.

**D9 — the glyph-cache key.** SPEC §6 specifies
`GlyphCache` keyed `(font-id, gid, hint-flags)`. PDFium's path cache key is
`(gid, dest_width, weight, italic_angle, vertical)` per face (§1.14), and
`dest_width` alone changes the *outline* for an MM font. **The SPEC key is
insufficient.** This brief proposes
`(font_id, gid, dest_width, weight, italic_angle, vertical)` with `hint_flags`
folded into `font_id` (it is constant per font under D-hinting, §1.14). Per
SPEC §0 this is a contract correction: the implementing agent makes a `[spec]`
commit editing SPEC.md §6's `GlyphCache` line together with the code. **Flagged
as an escalation, not a silent change.**

**D10 — no global font registry.** `CPDF_FontGlobals` (the predefined-CMap memo,
the CID2Unicode maps, the per-document stock-font array) and `CFX_FontMgr` (the
per-face glyph caches, the FreeType library handle) are process-wide singletons.
Per STYLE.md §1 they are erased: static tables live in `pdfrum-cmap`'s blob and
this crate's `const` arrays; the stock-font memo and the substitution caches move
into a `FontCache` value owned by `Document`; the glyph cache is owned by the
render session (SPEC §6). Non-observable.

**D11 — `CFX_FontMapper` decomposes.** It is a 1000-line god class holding
enumeration state, three caches, and the substitution algorithm (STYLE.md §1
names it explicitly as an anti-pattern). It becomes: a `FontRequest` record, a
pure `resolve(&FontRequest, &SubstitutionOptions) -> Substitution` function, a
`FontDb` seam over `fontdb`, and a `FaceCache` record. Non-observable.

**D12 — `CFX_SubstFont`'s `weight_ == 0` sentinel.** PDFium overloads 0 to mean
"the face's natural weight" (§1.12). We keep it as `Option<i32>` in the record
and map `None ↔ 0` at the boundaries where the arithmetic matters
(`GetEmboldenLevel*`, `AdjustVariationParams`). Behaviorally identical, but a
reader can see the intent. Note `GetEffectiveWeight` returns `weight_cjk_` when
`subst_cjk && is_cid_font`, and `weight_cjk_` has its own 0-default — so the
`Option` must be per-field, not shared.

**D13 — `Limits` for this crate.** PDFium caps nothing here. Candidates:
`max_tounicode_entries` (PDFium's `kOutOfSpecBFLimit = 160000` **is** a cap and
is ported verbatim), `max_cid_width_records` (uncapped — a `/W` array can be
arbitrarily long), `max_type3_depth` (PDFium's `kMaxType3FormLevel = 4`, ported
verbatim), `max_font_file_len` (uncapped). Proposal: add
`Limits.max_cid_width_records = 65_536`, exceeding it records a diagnostic and
drops further records — in the spirit of the accepted `max_decoded_stream_len`
decision (SPEC §4). **Confirm in OQ-7.**

---

## 3. Module plan

```
crates/pdfrum-font/
  fontdata/                       # the 16 Foxit blobs, BSD (PLAN.md §3)
    FoxitFixed.cff  … FoxitDingbats.cff        (14 bare-CFF files)
    FoxitSansMM.pfb  FoxitSerifMM.pfb          (2 PFB Type1 MM files)
    LICENSE.pdfium
  src/
    lib.rs              # public surface (§3.1)
    error.rs
    ids.rs              # Gid, CharCode, Cid, FontId newtypes; FontFlags bitflags
    encoding/
      mod.rs            # FontEncoding enum, resolution (§1.7)
      tables.rs         # the 9 code->unicode + 7 glyph-name tables (§3.2)
      agl.rs            # UnicodeFromAdobeName / AdobeNameFromUnicode (§1.7, D6)
      differences.rs    # /Differences parsing
    tounicode.rs        # the ToUnicode CMap (§1.6) — Tier-A critical
    descriptor.rs       # /FontDescriptor -> FontDescriptor record (§1.2)
    widths.rs           # /Widths /FirstChar /LastChar /MissingWidth (§1.5)
                        # and /W /DW /W2 /DW2 (§1.10.2)
    simple/
      mod.rs            # SimpleFont assembly = LoadCommon (§1.4)
      type1.rs          # the Type1 glyph ladder (§1.8)
      truetype.rs       # the TrueType glyph ladder (§1.9)
    type3.rs            # Type3Font (§1.11)
    cid/
      mod.rs            # Type0Font assembly (§1.10.1)
      glyph.rs          # the CID glyph ladder (§1.10.4)
      transform.rs      # kJapan1VerticalCIDs + CIDTransformToFloat (§1.10.3)
      gsub.rs           # vertical substitution policy over read-fonts (§1.10.6)
    glyphs/
      mod.rs            # GlyphSource enum, glyph_path, advances, bboxes (§1.14)
      skrifa.rs         # the skrifa/read-fonts adapter
      cache.rs          # GlyphCache (§1.14, D9)
    subst/
      mod.rs            # resolve() — the substitution ladder (§1.12)
      standard.rs       # the 14 names, the 89 aliases, the Foxit blobs (§1.13)
      style.rs          # GetSubstName, TT_NormalizeName, ParseStyles, kFontStyles
      substfont.rs      # SubstFont: skew, embolden, MM axes (§1.14)
      charset.rs        # FX_Charset/FX_CodePage, GetCharSetFromUnicode (§1.12, §1.14)
      db.rs             # the FontDb seam over `fontdb`

crates/pdfrum-type1/           # see §3.6
```

### 3.1 `lib.rs` — public surface

```rust
#![forbid(unsafe_code)]

pub use error::Error;
pub use ids::{CharCode, Cid, FontFlags, FontId, Gid};

/// SPEC §6. A loaded PDF font, ready to decode strings and produce glyphs.
#[derive(Debug)]
pub enum Font { Simple(SimpleFont), Type0(Type0Font), Type3(Type3Font) }

/// Build a `Font` from a `/Font` resource dictionary. Never panics; damage goes
/// to `diags`. Returns `None` for the four unrecoverable CID cases of §1.10.1 —
/// every other font type always constructs (§1.4).
pub fn load(
    dict: &Dict, r: &impl Resolve, cache: &FontCache,
    limits: &Limits, diags: &mut Diagnostics,
) -> Option<Font>;

impl Font {
    /// The one text-decoding entry point everything (render, extract) shares.
    pub fn decode<'a>(&'a self, s: &'a [u8]) -> impl Iterator<Item = CharItem> + 'a;
    /// FontUnits scaled to 1000/em text space (§1.14). `None` for a degenerate
    /// or missing outline, and always for Type3.
    pub fn glyph_path(&self, gid: Gid) -> Option<BezPath>;
    #[must_use] pub fn is_vertical(&self) -> bool;
    #[must_use] pub fn is_embedded(&self) -> bool;
    #[must_use] pub fn is_unicode_compatible(&self) -> bool;   // §1.10.5
    #[must_use] pub fn font_bbox(&self) -> Rect;               // §1.3
    #[must_use] pub fn ascent(&self) -> f32;
    #[must_use] pub fn descent(&self) -> f32;
    /// Type3 only; `None` otherwise. The glyph procedures live in `pdfrum-page`.
    #[must_use] pub fn type3(&self) -> Option<&Type3Font>;
}

pub struct CharItem {
    pub code: CharCode,
    pub cid: Option<Cid>,
    pub gid: Gid,
    pub unicode: SmallVec<[char; 2]>,
    pub width: f32,                 // 1000/em text space
    /// Set when GSUB substituted a vertical form (§1.10.6). Suppresses the
    /// Japan1 CID transform (§1.10.3).
    pub vertical_glyph: bool,
}

pub struct SimpleFont {
    pub glyphs: GlyphSource,
    pub encoding: [Option<GlyphName>; 256],
    pub unicodes: [u16; 256],          // CPDF_FontEncoding's parallel table (§1.7)
    pub glyph_index: [u16; 256],       // 0xffff = "no glyph" (§1.8.3)
    pub widths: [f32; 256],
    pub to_unicode: Option<ToUnicode>,
    pub descriptor: FontDescriptor,
    pub subst: Option<SubstFont>,
    pub kind: SimpleKind,              // Type1 { base14: Option<StandardFont> } | TrueType
}

pub struct Type0Font {
    pub cmap: pdfrum_cmap::CMap,
    pub glyphs: GlyphSource,
    pub charset: pdfrum_cmap::CidSet,
    pub cid_to_gid: CidToGid,          // Identity | Stream(Box<[u8]>) | ViaUnicode
    pub widths: CidWidths,             // /W + /DW
    pub vertical: Option<VerticalMetrics>,   // /W2 + /DW2, only when IsVertWriting
    pub to_unicode: Option<ToUnicode>,
    pub descriptor: FontDescriptor,
    pub subst: Option<SubstFont>,
    pub kind: CidFontKind,             // Type0 (CIDFontType0) | TrueType (CIDFontType2)
    pub adobe_courier_std: bool,       // §1.10.4's +31 rescue
}

pub struct Type3Font {
    pub font_matrix: Affine,
    pub char_procs: Dict,
    pub resources: Option<Dict>,
    pub encoding: [Option<GlyphName>; 256],
    pub widths: [i32; 256],            // glyph units, 0 = "ask the CharProc"
    pub font_bbox: Rect,
}

pub enum GlyphSource {
    /// skrifa over embedded bytes, a Foxit blob, or a `fontdb` face.
    Fontations(FontRef<'static>),
    /// Our own PFA/PFB/charstring parser (§3.6).
    Type1(pdfrum_type1::Type1Font),
    /// Type3, or a font whose face failed to load entirely.
    None,
}

/// Per-`Document` caches: parsed faces, resolved substitutions, stock fonts.
/// Replaces `CPDF_FontGlobals` + `CFX_FontMgr` (D10). `Send + Sync`.
#[derive(Default)]
pub struct FontCache { /* … */ }

/// Owned by the render session, not global (SPEC §6, D9).
pub struct GlyphCache { /* … */ }
impl GlyphCache {
    pub fn path(&mut self, f: &Font, key: GlyphKey) -> Option<&BezPath>;
}
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct GlyphKey {
    pub font: FontId, pub gid: Gid, pub dest_width: i32,
    pub weight: i32, pub italic_angle: i32, pub vertical: bool,
}

pub mod tounicode {
    /// Parse a `/ToUnicode` CMap stream's decoded bytes (§1.6).
    #[must_use]
    pub fn parse(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> ToUnicode;
    pub struct ToUnicode { /* … */ }
    impl ToUnicode {
        pub fn lookup(&self, code: CharCode) -> SmallVec<[char; 2]>;
        pub fn reverse(&self, u: char) -> CharCode;
    }
}

pub mod encoding {
    pub enum FontEncoding { Builtin, WinAnsi, MacRoman, MacExpert, Standard,
                            AdobeSymbol, ZapfDingbats, PdfDoc, MsSymbol }
    #[must_use] pub fn unicodes(e: FontEncoding) -> Option<&'static [u16; 256]>;
    #[must_use] pub fn char_name(e: FontEncoding, code: u8) -> Option<&'static str>;
    #[must_use] pub fn unicode_from_adobe_name(name: &str) -> u16;
    #[must_use] pub fn adobe_name_from_unicode(u: u16) -> Option<&'static str>;
}

pub mod subst {
    pub struct FontRequest { pub name: ByteString, pub is_truetype: bool,
                             pub flags: FontFlags, pub weight: i32,
                             pub italic_angle: i32, pub code_page: CodePage }
    pub struct SubstitutionOptions { pub skip_font_enumeration: bool,   // D7
                                     pub font_dirs: Vec<PathBuf> }
    /// The §1.12 ladder as one pure-ish function over the database seam.
    pub fn resolve(req: &FontRequest, db: &FontDb, opts: &SubstitutionOptions,
                   cache: &FontCache, diags: &mut Diagnostics)
        -> (Option<GlyphSource>, SubstFont);
    #[must_use] pub fn standard_font_index(name: &[u8]) -> Option<StandardFont>;
    #[must_use] pub fn canonical_font_name(f: StandardFont) -> &'static str;
    #[must_use] pub fn standard_font_data(f: StandardFont) -> &'static [u8];
}
```

Public types are `Send + Sync` (SPEC §4 / STYLE.md §4). `GlyphSource::Fontations`
holds a `FontRef<'static>` over `Arc<[u8]>`-owned bytes; the `'static` is achieved
by leaking-free self-reference via an owned-bytes wrapper (a `yoke`-style
pattern is **not** available — the dependency set is closed — so the wrapper
stores `Arc<[u8]>` and reconstructs the `FontRef` per use, memoizing the parsed
tables it needs). Measure before optimizing; `FontRef::new` is cheap.

### 3.2 Encoding tables

`encoding/tables.rs` holds 16 `const` arrays transcribed from
`cpdf_fontencoding.cpp`:

- Nine `[u16; 256]` code→Unicode tables: `MS_SYMBOL` (`:23`), `STANDARD` (`:57`),
  `MAC_ROMAN` (`:89`), `WIN_ANSI` (`:120`), `MAC_EXPERT` (`:151`),
  `ADOBE_SYMBOL` (`:182`), `ZAPF` (`:214`), `PDF_DOC` (`:24` region).
  (That is eight; the ninth "table" is `Builtin`, which is `None`.)
- Seven `[Option<&'static str>; N]` glyph-name tables: `STANDARD_NAMES` (224,
  `:253`), `WIN_ANSI_NAMES` (224, `:479`), `MAC_ROMAN_NAMES` (224, `:707`),
  `MAC_EXPERT_NAMES` (224, `:933`), `PDF_DOC_NAMES` (**232**, `:1159`),
  `ADOBE_SYMBOL_NAMES` (224, `:1393`), `ZAPF_NAMES` (224, `:1619`).

These are ~1500 lines of transcribed data. **Transcribe mechanically with a
one-off script, then verify by round-trip**: for every encoding and every code,
assert `unicode_from_adobe_name(char_name(e, c)) == unicodes(e)[c]` wherever both
are defined, and diff the resulting table against the C++ source text. Commit the
extraction script under `scripts/` so a reviewer can re-run it; do **not** make
it a `build.rs` (unlike `pdfrum-cmap`'s 600 KiB blob, 1500 lines of `const` is
perfectly reviewable Rust and belongs in the source).

The `kFontStyles`, `kAltFontNames`, `kAltFontFamilies`, `kAngleSkew`, the three
weight-power tables, `kJapan1VerticalCIDs`, `kCharsetCodePages`,
`kFXCharset2CodePageTable` (31 entries), `kDefaultTTFMap`, and `kChineseFontNames`
follow the same rule: `const` arrays in the source, near the code that uses them.

### 3.3 Data flow

```
/Font dict ─► load() ─┬─► Simple: descriptor ─► widths ─► subst? ─► encoding
                      │                                     ─► glyph ladder (§1.8/§1.9)
                      ├─► Type0:  /Encoding ─► pdfrum-cmap ─► descendant ─► /W /W2
                      │                                     ─► CIDToGIDMap
                      └─► Type3:  /FontMatrix /Widths /CharProcs /Encoding

bytes ─► Font::decode ─► CMap/byte split ─► (code, cid) ─► glyph ladder ─► Gid
                                                        └─► ToUnicode ─► chars
                                                        └─► widths ─► f32
                                        ─► CharItem

Gid + GlyphKey ─► GlyphCache ─► GlyphSource ─► skrifa unscaled_outline
                                            └─► pdfrum-type1 charstrings
                             ─► scale 1000/upem ─► BezPath
```

`pdfrum-font` depends on `pdfrum-object`, `pdfrum-common`, `pdfrum-filters` (to
decode `/ToUnicode`, `/Encoding` and `/FontFile*` streams), `pdfrum-cmap`, and
`pdfrum-type1`. It does **not** depend on `pdfrum-page` or anything above it;
Type3 glyph procedures are exposed as raw `/CharProcs` streams for `pdfrum-page`
to interpret (SPEC §7's `build_page` recursion).

### 3.4 The `FontDb` seam

`SystemFontInfoIface` (§1.12) reduces to four operations we need from `fontdb`:

```rust
pub trait FontDb {
    /// The §1.12 step 11 query.
    fn map_font(&self, weight: i32, italic: bool, charset: Charset,
                pitch_family: PitchFamily, family: &str) -> Option<FaceHandle>;
    /// Exact face-name lookup (§1.12 step 12, step 15).
    fn font_by_name(&self, name: &str) -> Option<FaceHandle>;
    /// Every installed face's (name, charsets) — the `face_array_` of step 15.
    fn faces(&self) -> &[FaceInfo];
    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32 /*index*/)>;
}
```
This is a genuine seam (a real second impl exists: an in-memory test database and
the `--font-dir` hermetic one), so it passes STYLE.md §2b's bar. It is threaded
as `&impl FontDb`, never `dyn`.

`fontdb`'s `Database::query(&Query)` covers `map_font` given a translation of
`(weight, italic, pitch_family)` into `fontdb::{Weight, Style, Stretch, Family}`.
The **charset** dimension has no `fontdb` equivalent — `fontdb` does not expose
`OS/2 ulCodePageRange1`. We read it ourselves from the face's `OS/2` table via
`read-fonts` and cache it in `FaceInfo`, using the same bit→charset mapping
PDFium's folder enumerator uses (`cfx_folderfontinfo.cpp:270-335`):

| bit | charset | | bit | charset |
|---|---|---|---|---|
| 1 | MSWin_EasternEuropean | | 16 | Thai |
| 2 | MSWin_Cyrillic | | 17 | ShiftJIS |
| 3 | MSWin_Greek | | 18 | ChineseSimplified |
| 4 | MSWin_Turkish | | 19 | Hangul |
| 5 | MSWin_Hebrew | | 20 | ChineseTraditional |
| 6 | MSWin_Arabic | | 21 | Johab |
| 7 | MSWin_Baltic | | 30 | OEM |
| 8 | MSWin_Vietnamese | | 31 | Symbol |

plus an **unconditional** ANSI (`:347-348`). The face's display name is
`family` + `" " + style` when style is non-empty and `!= "Regular"` (`:206-361`),
matching `GetFontNameFromFace`.

`FontFaceInfo::SimilarityScore` (`cfx_folderfontinfo.cpp:592-618`) and
`FindFamilyNameMatch` (`:80-99`) are the *folder* implementation's matching
policy, i.e. what `map_font` does on Linux. Since we replace that implementation
with `fontdb`, we do **not** port them — but they are the behavior the oracle
exhibits, so the M2 conformance run on the font cluster is what validates the
substitution. If `fontdb`'s ranking diverges materially, port
`SimilarityScore` (max 68 = 16 bold + 16 italic + 16 serif + 8 script + 8 fixed
+ 4 exact-name) on top of `fontdb`'s enumeration as our own `map_font`.
**Escalated as part of OQ-6.**

### 3.5 Records, not god objects

Following STYLE.md §1, the C++'s three big classes decompose:

| C++ | our records + functions |
|---|---|
| `CPDF_Font` (+4 subclasses, ~2500 lines) | `Font` enum over three small structs; `load()` free function per kind; `decode()` an iterator |
| `CFX_FontMapper` (~1000 lines, 3 caches + enumeration + the ladder) | `FontRequest`, `Substitution`, `SubstFont` records; `subst::resolve()` free function; `FontDb` seam; `FaceCache` |
| `CFX_Face`/`CFX_Font` (~1700 lines) | `GlyphSource` enum; free functions `glyph_path`, `advance`, `bbox`, `char_index`, `name_index` |
| `CPDF_CMapParser` (mutates a `CPDF_CMap` through an `UnownedPtr`, finishing in its **destructor**) | already handled in `pdfrum-cmap` §3.2 |
| `CPDF_ToUnicodeMap` (parser + storage fused) | `tounicode::parse()` free function → `ToUnicode` record |

The glyph ladders (§1.8–§1.10) are the one place where a faithful port *does*
read like the C++, because they are irreducibly a sequence of conditional
lookups. STYLE.md §7's transliteration test still applies to their **shape**:
each ladder is a function returning `Option<Gid>` composed of small named steps
(`try_ms_symbol_prefixes`, `try_name_index`, `try_unicode_charmap`,
`notdef_to_space`), not one 200-line function with flag variables. The *order* of
the steps is the behavior; the steps themselves are named and individually
testable.

### 3.6 `pdfrum-type1` — scope

**What it is for.** Two things, and only two:

1. **Embedded Type 1 font programs** (`/FontFile`, and `/FontFile3` with a
   Type1-shaped payload) that skrifa/read-fonts cannot read: PFB and PFA
   containers, eexec-encrypted charstrings, Type 1 charstring outlines.
2. **The two Foxit Multiple-Master fonts** (`kFoxitSansMMFontData`,
   `kFoxitSerifMMFontData`, §1.13) — PFB Type1 MM, and the **terminal rung of
   the substitution ladder** (§1.12), so every unresolvable font depends on them.

**What it is explicitly NOT for**, because `skrifa`/`read-fonts` already covers
it (§1.16.3, verified by magic-byte inspection of every Foxit blob):

- **Bare CFF** — all 14 base-14 Foxit blobs, and every `/FontFile3 /Type1C`.
  `read-fonts` reads these natively.
- **CFF2**, **OpenType/CFF (`OTTO`)**, **TrueType/glyf**, **TTC**.
- CFF/Type 2 charstrings of any kind.

**The API it must expose** is the Type1 subset of §1.16.1:

```rust
#![forbid(unsafe_code)]
pub struct Type1Font { /* … */ }
pub enum Encoding { Standard, Expert, IsoLatin1, Custom(Box<[Option<GlyphName>; 256]>) }

impl Type1Font {
    /// Sniffs PFB / PFA / bare, decrypts eexec, parses the font program.
    /// Never panics; a partially-parseable font yields whatever it could read.
    pub fn parse(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics)
        -> Result<Self, Error>;

    #[must_use] pub fn units_per_em(&self) -> u16;      // from /FontMatrix
    #[must_use] pub fn font_matrix(&self) -> Affine;
    #[must_use] pub fn bbox(&self) -> Rect;
    #[must_use] pub fn num_glyphs(&self) -> u32;
    #[must_use] pub fn is_fixed_pitch(&self) -> bool;
    #[must_use] pub fn postscript_name(&self) -> Option<&str>;
    #[must_use] pub fn family_name(&self) -> Option<&str>;

    #[must_use] pub fn encoding(&self) -> &Encoding;
    #[must_use] pub fn code_to_gid(&self, code: u8) -> Option<Gid>;   // built-in encoding
    #[must_use] pub fn unicode_to_gid(&self, u: char) -> Option<Gid>; // via AGL
    #[must_use] pub fn name_to_gid(&self, name: &str) -> Option<Gid>;
    #[must_use] pub fn glyph_name(&self, gid: Gid) -> Option<&str>;
    #[must_use] pub fn has_glyph_names(&self) -> bool;                // always true

    /// Unscaled outline in font units, plus the advance width.
    pub fn outline(&self, gid: Gid) -> Option<(BezPath, f32)>;
    #[must_use] pub fn glyph_bounds(&self, gid: Gid) -> Option<Rect>;

    // Multiple Master:
    #[must_use] pub fn mm_axes(&self) -> Option<&[MmAxis]>;
    /// Instantiate at design coordinates. `AdjustVariationParams`'s two probe
    /// loads become two `outline()` calls on two instances (§1.14).
    pub fn instantiate(&self, coords: &[f32]) -> Option<Type1Instance>;
}
pub struct MmAxis { pub min: f32, pub default: f32, pub max: f32, pub kind: AxisKind }
```

**Module plan:**
```
crates/pdfrum-type1/src/
  lib.rs        # the surface above
  error.rs
  container.rs  # PFB segment walk / PFA hex / bare sniffing (§1.16.2 item 1)
  eexec.rs      # the two ciphers, lenIV (item 2)
  postscript.rs # the cleartext/private-dict tokenizer (item 3)
  charstring.rs # the Type 1 interpreter -> BezPath (item 4)
  blend.rs      # Multiple Master: WeightVector, othersubr 14 (item 5)
  encoding.rs   # the built-in /Encoding vector + StandardEncoding for seac
  fuzz targets: type1_parse, type1_outline
```

**Dependencies:** `kurbo` (via `pdfrum-common`), `thiserror`, and the AGL from
`pdfrum-font::encoding` — which would be a **circular dependency**. Resolve it by
putting `unicode_from_adobe_name` in `pdfrum-common` (it is 3 lines over
`read-fonts`' `agl`), or by having `pdfrum-type1` depend on `read-fonts`
directly for the AGL only. **Prefer the latter**: `read-fonts` is already a
workspace dependency and the AGL is squarely its business. `pdfrum-type1`
therefore depends on `read-fonts` (agl feature) + `kurbo` + `thiserror`, and
`pdfrum-font` depends on `pdfrum-type1`. No cycle.

**`hayro-postscript` evaluation** (DEPS.md's standing instruction: "evaluate
`hayro-postscript` during the font design brief before writing an interpreter").
It is a PostScript *calculator/interpreter* for PDF Type 4 functions, not a Type 1
charstring engine — the two languages share syntax but not the operator set, and
a Type 1 charstring is a binary-encoded number/operator stream, not PostScript
source. It is **not applicable** to this crate, and would be the wrong dependency
for `pdfrum-page`'s Type 4 functions too (we write those ourselves per SPEC §7).
**Recorded as evaluated and rejected; DEPS.md needs no change.**

**Sizing.** Container + eexec + tokenizer ≈ 400 lines; the charstring interpreter
≈ 500 lines (the operator set is small and well documented); MM blend ≈ 200 lines.
Roughly 1100 lines plus tests — consistent with PLAN.md §8's "charstring
interpreter is well-trodden ground".

**Or: none of it.** If `read_fonts::ps::type1` reaches a crates.io release before
M2 implementation starts, `pdfrum-type1` collapses to a thin adapter (or
disappears, with `GlyphSource::Fontations` covering Type1 too). **OQ-2** decides.

---

## 4. Test plan

### 4.1 Ported from `cpdf_tounicodemap_unittest.cpp` — **the Tier-A core**

Every one of the 13 tests ports, restated over `tounicode::parse` /
`ToUnicode::lookup` / `::reverse` / a `#[cfg(test)]` `unicode_count(code)`.

| C++ test | line | what it pins |
|---|---|---|
| `StringToCode` | `:13-43` | all 18 rows of §1.6.3's table |
| `StringToWideString` | `:45-64` | all 14 rows of §1.6.4's table |
| `HandleBeginBFCharRejectsInvalidCidValues` | `:66-80` | bad hex and u32 overflow both yield an empty map (2 inputs × 3 assertions) |
| `HandleBeginBFCharBadCount` | `:82-103` | declared 1 with 2 entries, and declared 3 with 2 entries, both commit nothing |
| `HandleBeginBFCharTolerateOutOfSpecCount` | `:105-228` | 112 entries (>100) are honoured; `reverse(0x0001)==9`, `reverse(0x0067)==111`, counts 1 each |
| `HandleBeginBFRangeRejectsInvalidCidValues` | `:230-278` | the 6 high-code-mask cases of §1.6.6 |
| `HandleBeginBFRangeRejectsMismatchedBracket` | `:281-287` | `}` instead of `]` discards the block |
| `HandleBeginBFRangeBadCount` | `:289-314` | declared 1 and declared 3 with 2 ranges both commit nothing (20 unicodes × 2 + 7 counts × 2) |
| `HandleBeginBFRangeGoodCount` | `:316-336` | declared 2 with 2 ranges commits; 8 reverse + 7 count assertions |
| `HandleBeginBFRangeDestLargeValue` | `:338-358` | the U+FFFF indicator collision; **see OQ-3 for the `Lookup(0x20)` row** |
| `InsertIntoMaps` | `:360-393` | the three collision scenarios of §1.6.6 |
| `NonBmpUnicodeLookup` | `:395-404` | a surrogate pair round-trips through `lookup`; `reverse(0x20676) == 0` |

Plus **ours to write**, because Tier-A depends on them and the C++ has no test:
- `StringDataAdd` (§1.6.6) as a table: `[0x0041] → [0x0042]`;
  `[0x00FF] → [0x0100]`; `[0xFFFF] → [0x0001, 0x0000]`;
  `[0x0001, 0xFFFF] → [0x0002, 0x0000]`; `[0xFFFF, 0xFFFF] →
  [0x0001, 0x0000, 0x0000]`. (Derive each from the algorithm; verify against a
  one-off oracle probe for at least the two carry cases.)
- The `/Adobe-*-UCS2` base-map wiring: a stream containing
  `/Adobe-Japan1-UCS2` and no bfchar/bfrange ⇒ `lookup(cid)` returns the Japan1
  registry's Unicode for that CID (`pdfrum-cmap`'s table), and `lookup` on a
  code the registry does not cover returns the registry's 0.
- Multi-char accumulation: two consecutive multi-char `bfchar` entries get
  indicator indices 0 and 1; a third entry colliding on a lower value still
  pushes to `multi_char_vec_`, shifting the fourth entry's index (§1.6.1).
- Whitespace tolerance across the whole grammar: a CMap with `\r\n` between
  every token parses identically to one with single spaces.
- Fuzz-adjacent damage: a `beginbfrange` truncated mid-array; a `beginbfchar`
  whose count token is `"abc"` (parses as 0 ⇒ any entry invalidates it);
  a stream ending inside a `<hex` token.

### 4.2 Ported from the other C++ unittests

**`cpdf_cidfont_unittest.cpp` `Bug920636`** (`:19-51`) — the CourierStd +31
rescue. Build a `Type0Font` with `/Encoding` = the Name `Identity−H` (U+2212
MINUS, encoded as the three bytes `E2 88 92` in the name) and one descendant with
`/BaseFont /CourierStd`, no `/FontDescriptor`, no `/FontFile`. Assert
`glyph_from_charcode(0) == 31`, `(256) == 287`, `(34661) == 34692`. This one test
exercises: the predefined-name lookup failing (`pdfrum-cmap` §1.1), the Identity
CID fallback, `adobe_courier_std_` detection, and the whole §1.10.4 half-1
ladder falling through to `return charcode`.

**`cpdf_simplefont_unittest.cpp` `BaseFontNameWithSubsetting`** (`:39-62`) —
`/BaseFont /CHEESE+Swiss` with a `/FontFile` containing `kFoxitFixedFontData`
yields `base_font_name == "Swiss"`. Pins `strip_subset_prefix` **and** that it
runs only for an embedded font (§1.4 step 3).

**`cpdf_truetypefont_unittest.cpp` `AllUnicodeCmapsTreatedEqually`**
(`:180-260`) — the two 508-byte TTFs differing only in cmap platform/encoding
(`(0,3)` vs `(3,1)`), both mapping U+002E to glyph 1, with
`/Encoding /MacRomanEncoding` and a `/ToUnicode` mapping `<2E>` to U+0001. Assert
both yield glyph **1** for charcode 0x2E. Copy the two byte arrays verbatim into
`tests/fixtures/`. Pins `UseTTCharmapUnicode`'s "any Unicode encoding, provided
no (3,0)" rule (§1.9).

**`cfx_standardfont_unittest.cpp`** (all 5 tests, `:11-84`) — `IsStandardFontName`
(14 true + 4 false), `GetStandardFontIndex` (3 canonical + 4 alias incl. the
lowercase `"arial"` + 1 miss), `GetCanonicalFontName` (3),
`IsSymbolicFont` (2 true + 3 false), `IsFixedFont` (4 true + 3 false).
Plus **ours**: assert the alias table has exactly 89 entries, that every alias
resolves, and that every canonical name round-trips
`canonical(index(canonical(i))) == canonical(i)`.

**`cfx_substfont_unittest.cpp`** (all 5 tests, `:9-60`) — `EffectiveSkew` (2),
`EffectiveWeight` (2), `EmboldenLevels` (5 incl. the i64-overflow case and the
MM zeroing), `EstimatedStemV` (1), `IsActualFontLoaded` (3). These pin the three
weight-power tables and `kAngleSkew` exactly.

**`fx_font_unittest.cpp`** — `UnicodeFromAdobeName` (7) and
`AdobeNameFromUnicode` (7) as §1.7's tables; `GetCodePageRangeFromOS2` (2) and
`GetGlyphCountFromMaxp` (2) if we implement them ourselves rather than via
`read-fonts` (`FindFontTableLocation`'s 6 assertions are `read-fonts`' business,
not ours).

**`cfx_fontmapper_unittest.cpp`** —
`FindSubstFaceForRegularStandardFontWithBoldWeight` (`:253-310`) ports as a test
over `subst::resolve` with a mock `FontDb`, asserting the request that reaches
`map_font` is `(weight=700, italic=true, charset=ANSI, pitch_family=0,
family="Helvetica-Oblique")` — **including the known-buggy 700**, with a comment
citing `crbug.com/500640684`. `SetSubstFontNameWhenGetFaceNameFails` (`:210-251`)
ports as: a face whose name query fails yields
`subst.family == "Noto Sans SC Regular"` (family + style). `AddInstalledFontBasic`
and the three `LoadInstalledFonts*` tests are enumeration lifecycle, which we do
not have (D7) — **skipped, recorded as skipped**.
`GetCachedTTCFaceFailToGetData` and `GetCachedFaceFailToGetData` are
`crbug.com/1372234` regressions against short reads; port them as "a `FontDb`
returning fewer bytes than it promised yields `None`, not a panic".

**`cfx_folderfontinfo_unittest.cpp` `TestFindFont`** (10 assertions, `:74-144`)
— ports **only if** we implement `SimilarityScore`/`FindFamilyNameMatch` on top
of `fontdb` (§3.4). If we rely on `fontdb`'s own ranking, this test is replaced
by the conformance cluster. **Decide in OQ-6.** The most interesting rows to keep
either way: `"Book"` must **not** match `"Bookshelf Symbol 7"` (the
lowercase-next-char rule), while `"Bookshelf"`, `"Tofu"`, `"Lato"`, `"Oxygen"`
and `"Oxygen-Sans"` all must.

### 4.3 Ours to write — the glyph ladders

Each ladder gets a table-driven test over synthetic faces, because the C++ has
no unit coverage for them and they are where Tier-A text extraction breaks.

**Fixtures.** Build four minimal fonts as byte arrays in `tests/fixtures/`:
`tt_unicode_31.ttf` and `tt_unicode_03.ttf` (copied from the C++ unittest),
plus `tt_symbol_30.ttf` (a `(3,0)` charmap mapping `0xF041`→glyph 5) and
`tt_macroman_10.ttf` (a `(1,0)` charmap mapping `0x41`→glyph 7). Generate them
with the same hand-assembled-table approach the C++ test uses; commit the bytes.

**Type1 ladder (§1.8):**
- Branch A1: a non-embedded, non-symbolic base-14 name substituted with
  `tt_symbol_30`; assert the `0xF0` prefix probe finds glyph 5 for charcode 0x41.
- Branch A2 notdef→space: an encoding whose code 0 maps to `.notdef` and a face
  with no `.notdef` glyph ⇒ unicode becomes 0x20 and the glyph is
  `char_index(0x20)`.
- Branch B built-in encoding: a symbolic Type1 with no `/Encoding` and no
  `/Differences` ⇒ `GetAdobeCharName` returns `None` for every code ⇒ every glyph
  comes from `code_to_gid(c)` and every unicode from the reverse glyph name.
- Branch C name-first: a `/Differences` naming `/quotesingle` at code 39, with a
  face whose glyph-name table has it at gid 12 but whose Unicode cmap maps
  U+0027 to gid 99 ⇒ **gid 12 wins**.
- Branch C `space`/`.notdef` sentinel: `glyph_index == 0xffff` ⇒
  `glyph_from_charcode` returns -1, and `unicodes[c] == 0x20`.

**TrueType ladder (§1.9):**
- Rung 1a: `has_glyph_names && charmap_count == 0` ⇒ `SetGlyphIndicesFromFirstChar`
  with `/FirstChar 65` yields `glyph_index[64] == 0` and `glyph_index[65] == 3`,
  `[66] == 4`, `[255] == 193`.
- Rung 1b each charmap type: MSUnicode, MacRoman (both the `maccode != 0` and the
  `name_index` fallback), MSSymbol.
- Rung 1b's **ToUnicode last resort**: a name that resolves to no glyph by any
  route, with `/ToUnicode` mapping the code to a unicode the face **does** have
  ⇒ that glyph is used and `unicodes[c]` is rewritten.
- Rung 2 → 3 → 4 → 5 fall-through: a face with only a `(3,0)` charmap that maps
  nothing ⇒ rung 2 fails `HasAnyGlyphIndex`, rung 3's `(1,0)` is absent, rung 4's
  Unicode charmap is absent ⇒ rung 5 gives `glyph_index[c] == c`.
- `DetermineEncoding`'s four outcomes (§1.9) as a 4-row table over synthetic
  charmap platform-id sets.

**CID ladder (§1.10.4):**
- Half 1 with `cid_is_gid_` (`/CIDToGIDMap /Identity` + embedded) ⇒ returns the
  CID.
- Half 1 unicode derivation order: registry table → `GetUnicodeFromCharCode` →
  `UnicodeFromCharCode` (ToUnicode). Construct three fonts each of which
  succeeds only at one rung.
- The Japan1 `\` → `/` and `0xA5` → `0x5C` fixups.
- The charmap-negotiation loop: a face whose `select_charmap(Unicode)` fails and
  whose charmap 2 reverse-maps the charcode ⇒ charmap 2 is selected.
- Half 2: `CIDFontType0` embedded ⇒ CID as GID; `CIDFontType2` embedded with a
  predefined CMap ⇒ CID as GID; a `/CIDToGIDMap` stream ⇒ the big-endian u16
  lookup, including the `byte_pos + 2 > len` boundary returning -1.
- `Bug920636` (§4.2) covers the CourierStd rescue.

**Vertical (§1.10.2, §1.10.6):**
- `/W2` and `/DW2` parsing over a mixed `c [v1 v2 v3 …]` / `c1 c2 v1 v2 v3` array.
- `GetVertOrigin`'s fallback to `(width/2, DW2[0])` when no `/W2` entry covers
  the CID.
- U+2502 is never vertically substituted even in a vertical font.
- A GSUB `vert` feature reachable from no script still applies (the whole-list
  fallback of §1.10.6).

### 4.4 Widths, encoding, descriptor

- `/Widths` table: the `/LastChar 0` recompute; a `/LastChar` beyond the array;
  a `/LastChar` below it; `/FirstChar 256` dropping everything;
  `/MissingWidth` filling outside the range; a `/Widths` element that is a name
  (⇒ 0).
- `/W` parsing: both forms, the `width_status != 1` abort, the overflow guard's
  continue, a trailing partial group.
- `/Differences`: a null element skipped without advancing; a non-name resetting
  the cursor; a name past 255 dropped while the cursor advances; a negative
  integer.
- `LoadPDFEncoding`'s full decision table (§1.7) as ~14 rows over
  `(has_encoding, is_name_or_dict, base_font_name, symbolic, embedded, truetype,
  prior base_encoding)`.
- `LoadFontDescriptor`: the `descent > 10` sign repair; the
  `kFontUseExternAttr` conjunction (5 rows: each term absent in turn);
  `ItalicAngle >= 0` not setting the flag; `/FontWeight 0` not being stored.
- `CheckFontMetrics`: the deliberate top/bottom swap; the no-face 256-code union;
  the `'A'`/`'g'` ascent/descent derivation.
- `NormalizeFontMetric` vs `EmAdjust` differing (§1.3), as a table proving both
  are implemented.

### 4.5 Substitution

- The `GetSubstName` either/or: `"@MS Mincho"` with `is_truetype` keeps its
  space; `"MS Mincho"` without loses it.
- `TT_NormalizeName` vs `strip_subset_prefix` on `"ABCDEF+Foo Bar-Baz"`.
- `ParseStyles`'s abort cases: `"Bold,Italic"` aborts, `"BoldItalic"` does not;
  `"Bold,Bold"` yields 900; an unrecognized first token aborts; an unrecognized
  later token after a recognized one does not.
- `GetStyleType` reverse/forward with the `BoldItalic` vs `Italic` precedence and
  the `Regular` vs `Reg` precedence.
- `GetFontFamily`'s Script ladder (4 outcomes + None) and the 3
  `kAltFontFamilies` substring rewrites.
- The ladder's rungs 0–5 each reached exactly once, via a mock `FontDb`
  programmed to fail at successive rungs. Assert the resulting
  `(family, charset, weight, italic_angle, flag_mm)` on `SubstFont` at each rung.
- `internal_subst`'s two levels: a base-14 index yields the exact Foxit blob and
  **leaves `SubstFont` untouched**; no index yields Chrome Serif (Roman pitch,
  weight × 4/5) or Chrome Sans.
- `ConfigureExternalSubst`'s three italic-angle cases (0 → -12, |a| < 5 → 0,
  otherwise kept) and the weight-equals-face-weight sentinel.
- The symbolic retry terminating after one recursion.

### 4.6 `pdfrum-type1`

- **Container**: a PFB with three segments round-trips to the same bytes as the
  equivalent PFA; a PFB with a truncated segment length yields what it could
  read plus a diagnostic; a bare font with no marker parses as PFA-shaped.
- **eexec**: decrypt a known 32-byte vector against a hand-computed expectation;
  `lenIV` of 0, 4 (default) and 8 each discard the right number of bytes.
- **Charstrings**: one glyph per operator category (`hsbw`, `rlineto`,
  `rrcurveto`, `hvcurveto`, `closepath`, `callsubr`/`return`, `seac`, `flex` via
  `callothersubr` 0, hint replacement via `callothersubr` 3, `div`,
  `setcurrentpoint`), each asserted as an exact `BezPath`.
- **MM**: `kFoxitSansMMFontData` and `kFoxitSerifMMFontData` parse; report 2
  axes; `instantiate` at the default coordinates produces a non-degenerate
  outline for `A`; two instantiations at the width axis's min and max produce
  **different** advance widths (which is precisely what `AdjustVariationParams`
  needs, §1.14).
- **Fuzz** `type1_parse` and `type1_outline`, seeded from
  `pdfium-c++/testing/fuzzers/` if a Type1 corpus exists, plus every
  `/FontFile` stream extracted from the corpus. 24 h clean before M2 exit.
- **Differential**: for the two MM fonts and any corpus Type1, compare our
  outlines against the oracle's rendered output at the Tier-B threshold. There
  is **no C++ unit test to port** (§1.16.3) — the oracle's pixels are the only
  reference.

### 4.7 Snapshot, fuzz, conformance

- **Snapshot (`insta`)**: for a curated set of ~20 corpus fonts (one per
  interesting ladder path), dump
  `(kind, base_font, flags, encoding, is_embedded, subst.family, subst.weight,
  first 32 (code, gid, unicode, width) tuples)`. This is the highest-value
  regression net for the ladders — a reordered rung shows as a snapshot diff.
- **Fuzz targets**: `tounicode_parse` (the whole §1.6 grammar, then `lookup` and
  `reverse` over every produced code), `font_load` (a `/Font` dict from
  structured fuzz input over a mock resolver), `type1_parse`, `type1_outline`.
  All must run 24 h clean before M2 exit (PLAN.md §6).
- **Conformance clusters** for `--triage`: `tounicode`, `cjk`, `type1`,
  `truetype-symbolic`, `font-subst`, `type3`, `vertical`. M2's exit criterion is
  `--txt` Tier-A byte-exact on ≥ 98% of the corpus (PLAN.md §6), and every
  remaining failure must carry a documented waiver naming the cluster and the
  reason.

---

## 5. Open questions

**OQ-1 (escalated, blocking `pdfrum-cmap`'s parser only).** Inherited from the
cmap brief: `usecmap` in an **embedded** CMap is a no-op in PDFium. The cmap
brief proposes matching that. This brief assumes the same; nothing here changes
if the decision goes the other way.

**OQ-2 (escalated, blocks `pdfrum-type1`'s existence).** `read_fonts::ps::type1`
exists in the fontations project and is what upstream PDFium's own skrifa bridge
uses (`core/fxge/skrifa/src/main.rs:256`, `Cargo.toml` citing fontations PR
#1820) — but from a **path dependency on an unreleased branch**, not from the
crates.io release DEPS.md pins (`read-fonts = "=0.43.3"`). Three options:

1. **Write `pdfrum-type1` as scoped in §3.6** (~1100 lines incl. MM). Safe,
   independent of upstream, and the PLAN.md §3 plan of record.
2. **Bump `read-fonts` to a release that ships `ps::type1`** if one exists by M2,
   and reduce `pdfrum-type1` to a thin adapter or delete it. This is a `[spec]`
   change to DEPS.md.
3. **Hybrid**: use `read_fonts::ps::type1` where available and keep a
   first-party MM/blend layer, since Multiple Master is the part most likely to
   be missing upstream.

**Action for the implementing agent before writing any code:** check whether the
pinned `read-fonts 0.43.3` exposes `ps::type1` and `ps::cff`, and whether any
released version does; report the finding. **The answer changes the M2 crate
list.** If option 1 stands, note that MM (§1.16.2 item 5) is non-negotiable
because the two generic fallback faces are the terminal substitution rung.

**OQ-3 (measurement, not a decision).** Two ToUnicode behaviors are stated in
the C++ but read ambiguously (§1.6.6, D4):
(a) `Lookup(0x20)` for `"1 beginbfrange<0010><00ff><fff0>endbfrange"`, where the
computed value is `0x10000` and `(v & 0xffff) == 0` — does PDFium's
`WideString(wchar_t(0))` compare equal to `L""`?
(b) `StringDataAdd`'s carry cases.
**Action:** the implementing agent builds the oracle (PLAN.md §4) and runs a
two-file probe, recording the answers in code comments and in §4.1's expected
values. No design depends on the outcome; two test expectations do.

**OQ-4 (low stakes, decide at implementation).** `FT_FACE_FLAG_TRICKY` (§1.14)
has no skrifa equivalent. It gates hinting in `LoadGlyphPath`, and since we
render text as filled outlines with hinting effectively off (D8's sibling
observation), the practical answer is "always unhinted". Confirm that skrifa's
`OutlineGlyph::draw` with `Hinting::None` (or `unscaled_outline`, which is
unhinted by construction) is what we call, and that no corpus font regresses.
**Proposal: always unhinted; no `tricky` list.**

**OQ-5 (deferred to M3, recorded now).** Artificial emboldening (D8) has no
skrifa equivalent. `FT_Outline_Embolden` dilates an outline by a level in font
units. Options: (a) implement path dilation in `pdfrum-render`; (b) approximate
with a stroke-and-fill at half the level; (c) skip it and take a Tier-B waiver on
the substituted-bold cluster. M2 needs none of them — the tables and levels are
computed and tested regardless. **Confirm the deferral.**

**OQ-6 (escalated, real Tier-B risk).** Two coupled substitution questions
(D7, §3.4):
(a) Does `skip_font_enumeration` default to `true` (the fontdb-shaped mode, in
which Branch A does **not** reset the weight) or `false` (matching the oracle's
`CFX_FolderFontInfo` enumeration path)? The brief proposes making it a
`SubstitutionOptions` field and choosing the default **empirically** in M2 from
the font-substitution conformance cluster.
(b) Do we port `FontFaceInfo::SimilarityScore` + `FindFamilyNameMatch`
(`cfx_folderfontinfo.cpp:80-99, 373-428, 586-636`) as our own `map_font` on top
of `fontdb`'s enumeration, or do we rely on `fontdb::Database::query`'s ranking?
Porting is ~120 lines and makes the oracle's `--font-dir` behavior reproducible;
relying on `fontdb` is less code but a different ranking function.
**Recommendation: port `SimilarityScore`.** It is the only way Tier-B
substitution results are predictable, and `fontdb` is then used purely as a face
*enumerator*, which is exactly what `SystemFontInfoIface` was. Confirm.

**OQ-7 (confirm the constant).** `Limits` additions for this crate (D13):
`max_cid_width_records = 65_536` (new, ours), with `kOutOfSpecBFLimit = 160_000`
and `kMaxType3FormLevel = 4` ported verbatim from the C++. Confirm the new
constant, or confirm "no cap, match the C++".

**OQ-8 (contract correction, SPEC §0).** Three SPEC §6 shapes need amending; the
implementing agent makes one `[spec]` commit alongside the code:
1. `GlyphCache` keyed `(font-id, gid, hint-flags)` → the six-field key of D9
   (`dest_width` alone changes an MM outline).
2. `SimpleFont`'s `pub widths: [f32; 256]` is listed as complete but the crate
   also needs `unicodes: [u16; 256]` and `glyph_index: [u16; 256]` as first-class
   fields — the *(abridged)* marker covers this, but the brief records the shape
   explicitly (§3.1) so it is reviewed, not discovered.
3. `CharItem` gains `vertical_glyph: bool` (§1.10.6 — it suppresses the Japan1
   transform and is not derivable from the other fields).

**OQ-9 (informational, no action).** `hayro-postscript` was evaluated per
DEPS.md's standing instruction and **rejected**: it is a PostScript
calculator/interpreter for PDF Type 4 functions, not a Type 1 charstring engine
(§3.6). DEPS.md's line "evaluate `hayro-postscript` during the font design brief
before writing an interpreter" is hereby satisfied; no DEPS.md change is needed,
but a reviewer may wish to strike the sentence.
