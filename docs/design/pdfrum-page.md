# Design brief — `pdfrum-page`

Behavior source: `pdfium-c++/core/fpdfapi/page/` in full, plus the pieces of
`core/fxge/` (`cfx_graphstate*`, AGG dash normalization) and
`core/fpdfapi/render/` (`cpdf_docrenderdata.cpp` transfer-function sampling,
`cpdf_renderstatus.cpp` soft-mask/backdrop parsing) that own page-level
*semantics* rather than rasterization. Shape contract: SPEC.md §7 (binding).
All C++ paths are relative to `/mnt/data2/pdfium/pdfium-c++/`.

This is the largest crate in the project. It is the hinge between "bytes that
parsed" and "something you can draw": the content-stream interpreter, the
graphics-state model, colorspaces, PDF functions, patterns/shadings, the image
decode ladder, and the transparency model all live here. Downstream,
`pdfrum-render` consumes `Page` and never re-derives semantics;
`pdfrum-text` consumes `Page` and never touches pixels.

Two standing rules from PLAN §1 and STYLE §7 apply with extra force here,
because the C++ in this directory is the most OOP-shaped in the codebase:

- **Port behavior, erase shape.** `CPDF_StreamContentParser` is a 1798-line
  god class with 73 member-function handlers, a 16-slot ring buffer of
  `std::variant` operands, and a process-global `std::map` opcode table. All
  of that dissolves into an `ops!` table, a `Vec<Op>`, and a fold. The
  *observable* consequences — which operand index feeds which parameter, what a
  wrong operand count does, when a handler silently returns — are the contract.
- **The recovery folklore is the product.** Every clamp, every "if it's
  garbage use 8 instead", every silently-ignored operator is a real file in
  the corpus. They are inventoried below with values, and each one that
  represents a repair emits a `Diagnostic`.

---

## 1. Behavior inventory

### 1.0 The two-layer parse: `CPDF_ContentParser` over `CPDF_StreamContentParser`

C++ splits content parsing into a *staging* driver
(`cpdf_contentparser.cpp`, a resumable state machine for progressive
rendering) and the *interpreter* (`cpdf_streamcontentparser.cpp`). The
staging layer's states (`cpdf_contentparser.h:29`
`enum class Stage { kGetContent, kPrepareContent, kParse, kCheckClip, kComplete }`)
exist only to support `PauseIndicatorIface` pausing. We collapse the pausing
(Divergence D1) but must keep every *data* decision the stages make.

**Stage `kGetContent`** (cpdf_contentparser.cpp:159-174). Only for pages
(`DCHECK(page_object_holder_->IsPage())`). Walks the page's `/Contents`
**array** one element per call, wrapping each element in a `CPDF_StreamAcc`
and calling `LoadAllDataFiltered()` (the full decode pipeline, with the raw
fallback from the filters brief). `ToStream(...)` of a non-stream element
yields a null accumulator, i.e. **an empty segment** — a non-stream element in
`/Contents` contributes zero bytes but still occupies a stream index.

**Constructor dispatch for pages** (:31-59):
- No document → `kComplete` (nothing parsed).
- `/Contents` missing, or resolving to neither a Stream nor a non-empty Array
  → `HandlePageContentFailure()` → `kComplete`. **A page with no content
  parses successfully with zero objects** — never an error.
- `/Contents` is a Stream → single stream, `kPrepareContent`.
- `/Contents` is an Array of size 0 → `HandlePageContentArray` returns false →
  failure path. Size ≥ 1 → `streams_ = size`, `kGetContent`.

**Stage `kPrepareContent`** (:176-210) — the concatenation rule, and it is
load-bearing for content that straddles streams:

- Single stream: data is that stream's decoded span; no copy.
- Multiple streams: build one buffer of
  `sum(stream_i.size() + 1)` bytes. Each stream's bytes are copied in order,
  and **one `0x20` (space) byte is written after each stream** (:203-206).
  This is why `BT /F1 12 Tf` split as `"BT /F1 12"` + `"Tf"` still works.
- `stream_segment_offsets_[i]` records the *start offset of stream i* in the
  merged buffer (:186) — pushed **before** adding that stream's size, so
  offsets are `[0, len0+1, len0+1+len1+1, ...]`. These drive
  `GetCurrentStreamIndex()` (§1.6).
- Overflow of the running `FX_SAFE_UINT32` total, or allocation failure →
  `kComplete` with empty data (:189-199). Port as: a merged size exceeding
  `u32::MAX` yields an empty page + `Diagnostic`.

**Stage `kParse`** (:212-234): constructs the interpreter if absent, and for
the *page* case calls `parser_->GetCurStates()->mutable_color_state().SetDefault()`
(:220) — the initial fill and stroke color are **DeviceGray with colorref 0**
(cpdf_colorstate.cpp:156-163), i.e. opaque black. Forms do **not** get this
call; they inherit the caller's color state (§1.7). Then repeatedly calls
`Parse(data, offset, kParseStepLimit=100, segment_offsets)` (:230-232) — the
`max_cost` budget is *page objects produced*, purely a pause granularity knob,
not observable in the final result (D1).

**Stage `kCheckClip`** (:236-270) — a real, observable post-pass. For every
**active** page object whose clip path: has exactly 1 path, has 0 texts, whose
single path `IsRect()`, and where the object is **not** a shading — if the
rect (built from `path.GetPoint(0)` and `path.GetPoint(2)`) `Contains` the
object's rect, the clip path is **dropped entirely** (`SetNull()`). This is a
redundant-clip elimination that changes rendering only via clip-AA edges, but
it changes `--show-pageinfo`-style dumps, so port it. It also feeds the Type3
char metric initialization (:237-240): `type3_char_->InitializeFromStreamData(colored_, type3_data_)`.

### 1.1 The content tokenizer — `CPDF_StreamParser`

`cpdf_streamparser.cpp` is a *second* PDF tokenizer, deliberately distinct
from `CPDF_SyntaxParser` (parser brief §1.2-1.5). It is simpler, has no
indirect references, and has its own caps. Do not reuse the parser crate's
lexer — the divergences are observable.

**Caps** (all exact):

| Constant | Value | C++ source |
|---|---|---|
| `kMaxWordLength` | 255 (buffer 256) | cpdf_streamparser.h:51,68 |
| `kMaxStringLength` | 32767 | cpdf_streamparser.cpp:43 |
| recursion limit for `ReadNextObject` | `CPDF_SyntaxParser::kParserMaxRecursionDepth` = **64** | cpdf_streamparser.cpp:349 |

`word_buffer_` is `std::array<uint8_t, kMaxWordSize>` with
`kMaxWordSize = kMaxWordLength + 1 = 256`; chars past 255 are consumed but
dropped (:295-296, :478-479, :508-509) — same truncation discipline as the
file lexer, different cap (255 vs 256).

**`ParseNextElement()`** (:254-340) returns
`enum class ElementType { kNumber, kKeyword, kName, kOther, kEndOfData }`
(cpdf_streamparser.h:23):

1. Reset `last_obj_`, `word_size_ = 0`. Out of bounds → `kEndOfData`.
2. Skip whitespace; on `%` skip to a line ending and resume skipping
   (:262-285). Comments are invisible.
3. If the char is a **delimiter and not `/`** → push back, call
   `ReadNextObject(false, false, 0)`, return `kOther` (:287-291). So `(`, `<`,
   `[`, `]`, `>`, `{`, `}`, `)` all route into the object reader. `]`, `)`,
   `>`, `{`, `}` produce a null object (ReadLeafObject falls through) — a
   `kOther` with a null payload, which the interpreter pushes as a null
   operand.
4. Otherwise accumulate until delimiter/whitespace (pushed back), tracking
   `bIsNumber` = every char is in the Numeric class `0-9 + - .` (:293-313).
5. `bIsNumber` → `kNumber`. First char `/` → `kName`. Then the **exact-length**
   keyword checks: `word_size_ == 4` and word is `true` → Bool(true)/`kOther`;
   `word_size_ == 4` and `null` → Null/`kOther`; `word_size_ == 5` and
   `false` → Bool(false)/`kOther` (:324-338). Everything else → `kKeyword`.

Note the asymmetry: `true`/`false`/`null` become *objects*, not keywords, so
they can never be dispatched as operators.

**`GetNextWord()`** (:436-525) is the lower-level scanner used inside
`ReadNextObject`. Same skipping; then:
- Delimiter first char: stored, and
  - `/` → keep appending while class is Other **or** Numeric; stop (push back)
    otherwise. Returns `false` (not-a-number).
  - `<` → if next is `<`, append it (token `<<`); else push back (token `<`).
  - `>` → symmetric (`>>` or `>`).
  - Any other delimiter → 1-char token.
- Non-delimiter: accumulate to delimiter/whitespace; returns is_number.

**`ReadNextObject(allow_nested_array, in_array, recursion_level)`**
(:342-404). Recursion > 64 → null. Grammar:
- number token → `Number` (FX_Number of the word).
- `<<` (first char `<` and `word_size_ > 1`) → Dict: loop `GetNextWord()`;
  `>>` ends it; a word not starting `/` → **the whole dict fails (null)**
  (:367-369) — stricter than the file parser, which skips junk keys; a value
  that fails to parse → **the whole dict fails** (:374-376). Keys are
  `PDF_NameDecode` of `word[1..]`. Values parse with
  `allow_nested_array = true`.
- `[` → Array. **`!allow_nested_array && in_array` → null** (:384-386): a
  bare `[` at top level parses with `allow_nested_array = false`, so a
  *nested* array inside it is rejected and the inner `[` terminates nothing —
  the loop then sees `word_buffer_[0] == '['` (not `]`, not empty) and **spins
  forward consuming elements** until `]` or EOF. Net: `[1 [2] 3]` at operator
  level yields `[1, 3]`, not a nested array. Dict values get
  `allow_nested_array = true`, so dicts *do* nest arrays. Port exactly.
- Array termination (:396-398): a null element ends the loop only when
  `word_size_ == 0` (EOF) or the word starts with `]`. Otherwise it loops
  again — this is the "spin forward" above and is bounded only by EOF.
- Otherwise `ReadLeafObject()` (:406-433): `/` → Name; `(` → literal string;
  `<` (with `word_size_ == 1`) → hex string; `false`/`true`/`null` → those;
  anything else → null.

**`ReadString()`** (:527-618): the same 5-state escape machine as the file
lexer (parser brief §1.3), with one addition — the result is **truncated to
`kMaxStringLength = 32767`** on both the normal close-paren exit (:542) and
the EOF exit (:613). Nested parens tracked by `parlevel`; `\<CR>` and
`\<CR><LF>` are line continuations via state 4; octal up to 3 digits.

**`ReadHexString()`** (:620-656): non-hex chars silently skipped, `>`
terminates, EOF terminates, trailing lone nibble emits `nibble*16`, result
truncated to 32767. Assertion goldens are in
`cpdf_streamparser_unittest.cpp:11-50` (§4).

### 1.2 The operand stack — 16 slots, and how bad operands are tolerated

`CPDF_StreamContentParser` keeps operands in a **fixed ring buffer of 16**
(`kParamBufSize = 16`, cpdf_streamcontentparser.h:80). Note: **16, not 32.**
Each slot is `std::variant<RetainPtr<CPDF_Object>, FX_Number, ByteString>`
(:77-78) — numbers and names are stored unboxed and materialized lazily.

`GetNextParamPos()` (cpdf_streamcontentparser.cpp:434-453): when
`param_count_ == 16`, **the oldest slot is evicted** (`param_start_pos_`
advances, wrapping) and reused; `param_count_` stays at 16. So a content
stream that pushes 20 numbers before an operator leaves the operator seeing
**the last 16**, with indices renumbered. There is no error, no diagnostic in
C++, and no skipped operator.

Accessors index **from the end** (most recent = index 0):
`real_index = param_start_pos_ + param_count_ - index - 1`, wrapped
(:486-489, :512-515, :537-540).

- `GetObject(i)` (:482-505): `i >= param_count_` → null. Materializes an
  `FX_Number` slot into a Number object, a `ByteString` slot into a Name
  object, in place.
- `GetString(i)` (:507-530): out of range → empty. A `ByteString` slot returns
  it directly; an object slot returns `obj->GetString()` (a Name yields its
  text, a String its bytes, a Number its formatted text, others empty).
- `GetNumber(i)` (:532-555): out of range → **`0`**. `FX_Number` slot → its
  float; object slot → `obj->GetNumber()` (Number → value, everything else →
  0).
- `GetNumbers(count)` (:557-563): returns `[GetNumber(count-1), …, GetNumber(0)]`
  — i.e. **source order**, oldest first.
- `GetInteger(i)` (:565-571): `GetNumber(i)` then a range check against
  `int`; out of range → `nullopt`. Callers use `.value_or(0)`.
- `GetPoint(i)` (:573-575): `{GetNumber(i+1), GetNumber(i)}` — x is the
  *older* operand.
- `GetMatrix()` (:577-580): `CFX_Matrix(n5, n4, n3, n2, n1, n0)` where
  `nk = GetNumber(k)`; i.e. `(a,b,c,d,e,f)` read oldest-to-newest.

**The tolerance rule, stated once:** an operator that reads operand index `i`
with fewer than `i+1` operands present gets `0` (numbers), `""` (strings), or
`null` (objects) — never an error, never a skip. Only the handful of
operators listed in §1.3 with an explicit `param_count_ != N` guard actually
bail. This is why `1 2 3 rg` (3 args, correct) and `rg` (0 args) behave
differently — `rg` guards on `param_count_ != 3` — while `1 2 m` and `m`
differ only in that `m` guards on `!= 2`, but `w` (no guard) with zero
operands sets line width to 0.

`ClearAllParams()` runs after **every** keyword dispatch (:1677), including
unrecognized keywords. Operands never survive an operator.

**`OnOperator`** (:600-605): looks the operator up in the global map by
`FXBSTR_ID` (first 4 bytes packed little-endian, zero-padded). Unknown
operator → **nothing happens** (no handler, and params are cleared by the
caller). Operators longer than 4 bytes collide into their first 4 bytes; no
real operator is longer than 3, so this is safe, but a garbage keyword
`"Tjxx"` would hash to a *different* id than `"Tj"` and miss — good.
Our `ops!` table keys on the exact byte string.

**On `ObjectsHeldByParser`:** the C++ lifetime trick (parser-owned
`RetainPtr`s keeping inline objects alive past the parser) is a
reference-counting artifact with **no observable behavior**; Rust ownership
makes it vacuous. Noted and dropped per the task instruction.

### 1.3 Complete operator inventory (71 registered + 2 ignored)

Registered in `InitializeGlobals()` (cpdf_streamcontentparser.cpp:264-382).
**The count is exactly 71** `FXBSTR_ID` entries — verified by
`sed -n '266,381p' … | grep -o 'FXBSTR_ID([^)]*)' | sort -u | wc -l`. The
"~73 operators" figure in SPEC §7 is the informal count that includes `BX`/`EX`
(handled by falling through as unknown keywords, §"Compatibility" below); our
`Op` enum has **73 named variants** plus `Op::Unknown`, which is what SPEC's
number refers to.
The table below is the direct source for the `ops!` macro (STYLE §2b).
"Operands" is what the handler actually *reads*; "Guard" is an explicit
`param_count_` check that makes the operator a no-op when violated. Where no
guard is listed, missing operands read as 0/""/null (§1.2).

Columns: **Op** | **Operands (oldest→newest)** | **Guard** | **Behavior / C++ line**

**Graphics state (11)**

| Op | Operands | Guard | Behavior |
|---|---|---|---|
| `q` | — | — | Push a full clone of the state onto `state_stack_` (:1046-1048). Unbounded depth. |
| `Q` | — | — | Pop; **empty stack → no-op** (:1050-1059). Also refreshes `all_ctms_[current_stream]`. |
| `cm` | 6 numbers | — | `ctm = GetMatrix() * ctm` (**pre**-concat, :730-735); updates `all_ctms_[stream]`; calls `OnChangeTextMatrix`. |
| `w` | 1 number | — | `graph_state.line_width = n` (:1505-1507). No validation; negative and 0 stored as-is. |
| `J` | 1 int | — | `line_cap = GetInteger(0).value_or(0) as LineCap` (:994-997). **Unchecked cast** — values outside 0..2 land as an out-of-range enum; we clamp (D5). |
| `j` | 1 int | — | `line_join = …` same unchecked cast (:989-992). |
| `M` | 1 number | — | `miter_limit = n` (:1036-1038). |
| `d` | array, number | array must be an Array | `SetLineDash(array, phase)` (:757-764); non-array operand 1 → no-op. See §1.4. |
| `ri` | 1 name | — | **No-op** (`Handle_SetRenderIntent`, :1097). Render intent from `cm`-level `ri` is discarded; only ExtGState `/RI` is stored. |
| `i` | 1 number | — | `general_state.flatness = n` (:983-985). Stored, never used for rendering. |
| `gs` | 1 name | dict must resolve | `FindResourceObj("ExtGState", name)` → dict, else no-op; appends the name to `graphics_resource_names_`; then `ProcessExtGS` (§1.5) (:957-969). |

**Path construction (7)**

| Op | Operands | Guard | Behavior |
|---|---|---|---|
| `m` | 2 numbers | `param_count_ != 2` → no-op | `AddPathPoint(GetPoint(0), Move)` then **`ParsePathObject()`** — the fast path sub-loop (§1.6) (:1027-1034). |
| `l` | 2 numbers | `param_count_ != 2` → no-op | `AddPathPoint(GetPoint(0), Line)` (:1019-1025). |
| `c` | 6 numbers | — | Three Bezier points: `GetPoint(4)`, `GetPoint(2)`, `GetPoint(0)` (:724-728). |
| `v` | 4 numbers | — | `path_current_`, `GetPoint(2)`, `GetPoint(0)` — first control point is the current point (:1499-1503). |
| `y` | 4 numbers | — | `GetPoint(2)`, `GetPoint(0)`, `GetPoint(0)` — **last point duplicated** (:1517-1521). |
| `h` | — | — | Close: if `path_points_` empty → no-op; if current ≠ start, append `path_start_` as a closing Line; else set `close_figure_` on the last point (:971-981). |
| `re` | 4 numbers | — | `AddPathRect(n3, n2, n1, n0)` = x,y,w,h (:1061-1067) → Move(x,y), Line(x+w,y), Line(x+w,y+h), Line(x,y+h), Line+close(x,y) (:1069-1075). |

**Path painting (10)** — all funnel into `AddPathObject(fill_type, render_type)` (§1.6).

| Op | Fill rule | Stroke | C++ |
|---|---|---|---|
| `S` | NoFill | yes | :1104-1106 |
| `s` | NoFill | yes | `Handle_ClosePath()` first (:1099-1102) |
| `f` | Winding | no | :933-935 |
| `F` | Winding | no | Legacy alias of `f`; identical handler (:937-939) |
| `f*` | EvenOdd | no | :941-943 |
| `B` | Winding | yes | :612-614 |
| `B*` | EvenOdd | yes | :621-623 |
| `b` | Winding | yes | `Handle_ClosePath()` first (:607-610) |
| `b*` | EvenOdd | yes | **`AddPathPointAndClose(path_start_, Line)`** first — *not* `Handle_ClosePath()`, so it appends unconditionally even when current == start (:616-619). Asymmetry with `b`; port it. |
| `n` | NoFill | no | End path; only the pending clip (if any) survives (:1042-1044). |

**Clipping (2)**

| Op | Behavior |
|---|---|
| `W` | `path_clip_type_ = Winding` (:1509-1511). Deferred until the next painting op. |
| `W*` | `path_clip_type_ = EvenOdd` (:1513-1515). |

**Text objects & positioning (9)**

| Op | Operands | Guard | Behavior |
|---|---|---|---|
| `BT` | — | — | `text_matrix = identity`, `OnChangeTextMatrix()`, `ResetTextPosition()` (:718-722). **Does not clear the text-clip list** — that is `ET`'s job. |
| `ET` | — | — | If `clip_text_list_` non-empty and the current text mode is a clip mode, append the texts to the clip path; then clear the list (:921-931). A clip-mode `BT…ET` with no glyphs adds nothing. |
| `Td` | 2 numbers | — | `text_line_pos += (x, y); text_pos = text_line_pos` (:1192-1194 → cpdf_allstates.cpp:186-189). |
| `TD` | 2 numbers | — | `Handle_MoveTextPoint()` then `text_leading = -GetNumber(0)` — leading is the **negated y** (:1196-1199). |
| `Tm` | 6 numbers | — | `text_matrix = GetMatrix()`, `OnChangeTextMatrix()`, `ResetTextPosition()` (:1452-1456). |
| `T*` | — | — | `text_line_pos.y -= text_leading; text_pos = text_line_pos` (:1495-1497 → allstates:191-194). |
| `TL` | 1 number | — | `text_leading = n` (:1448-1450). |
| `Ts` | 1 number | — | `text_rise = n` (:1479-1481). |
| `Tz` | 1 number | `param_count_ != 1` → no-op | `text_horz_scale = n / 100` — **stored as a fraction, not a percent** (:1487-1493); then `OnChangeTextMatrix()`. |

**Text state (4)**

| Op | Operands | Guard | Behavior |
|---|---|---|---|
| `Tc` | 1 number | — | `text_state.char_space = n` (:1188-1190). |
| `Tw` | 1 number | — | `text_state.word_space = n` (:1483-1485). |
| `Tf` | name, number | — | Font size **always** set to `GetNumber(0)`; font looked up via `FindFont(GetString(1))` and set **only if non-null** (:1201-1207). `FindFont` never returns null when the doc has a stock font, so a bad `/Font` name yields Helvetica (§1.8). |
| `Tr` | 1 int | — | `SetTextRenderingModeFromInt(GetInteger(0).value_or(0))`; **modes outside 0..7 are rejected and the mode is unchanged** (:1472-1477 → cpdf_textstate.cpp:128-134). |

**Text showing (4)**

| Op | Operands | Behavior |
|---|---|---|
| `Tj` | 1 string | Empty string → no-op; else one text object with no kernings (:1389-1394). |
| `'` | 1 string | `T*` then `Tj` (:1523-1526). |
| `"` | aw, ac, string | `word_space = GetNumber(2)`, `char_space = GetNumber(1)`, then `'` (:1528-1532). Note the operand indices: the string is index 0. |
| `TJ` | 1 array | Non-array → no-op. See §1.9 for the exact segmentation math (:1396-1446). |

**Type 3 glyph metrics (2)**

| Op | Operands | Behavior |
|---|---|---|
| `d0` | wx, wy | `type3_data_[0] = GetNumber(1)`, `[1] = GetNumber(0)`, **`colored_ = true`** (:766-770). |
| `d1` | 6 numbers | `type3_data_[i] = GetNumber(5-i)` for i in 0..6, **`colored_ = false`** (:772-777). |

Only the last `d0`/`d1` in the glyph stream wins; there is no
"must be first operator" enforcement.

**Colour (12)**

| Op | Operands | Guard | Behavior |
|---|---|---|---|
| `CS` | 1 name | CS must resolve | `FindColorSpace(GetString(0))`; null → no-op; else `stroke_color.SetColorSpace(cs)` — **components are reset to the space's default color**, not preserved (:747-755 → cpdf_color.cpp:37-44). |
| `cs` | 1 name | ditto | Fill variant (:737-745). |
| `SC` | ≤4 numbers | — | `nargs = min(param_count_, 4)`; `SetStrokeColor(nullptr, GetNumbers(nargs))` — **colorspace unchanged**; if the current color is null, DeviceGray is installed (:1113-1116 → colorstate.cpp:113-131). If `cs->ComponentCount() > values.len()`, the whole set is **discarded** (colorstate:123-125). |
| `sc` | ≤4 numbers | — | Fill variant (:1108-1111). |
| `SCN` | numbers, or numbers + name | last operand must exist | If the last operand is **not** a Name: `SetStrokeColor(nullptr, GetColors())` where `GetColors()` = all `param_count_` values in source order (:894-897). If it **is** a Name: `FindPattern(name)`; null → no-op; else `SetStrokePattern(pattern, GetNamedColors())` where `GetNamedColors()` drops the name (:899-907, :1141-1162). |
| `scn` | ditto | ditto | Fill variant (:1118-1139). |
| `G` | 1 number | — | `SetStrokeColor(stock DeviceGray, GetNumbers(1))` (:951-955). |
| `g` | 1 number | — | Fill variant (:945-949). |
| `RG` | 3 numbers | `param_count_ != 3` → no-op | stock DeviceRGB (:1087-1095). |
| `rg` | 3 numbers | `param_count_ != 3` → no-op | Fill variant (:1077-1085). |
| `K` | 4 numbers | `param_count_ != 4` → no-op | stock DeviceCMYK (:1009-1017). |
| `k` | 4 numbers | `param_count_ != 4` → no-op | Fill variant (:999-1007). |

Note `SC`/`sc` cap at 4 components while `SCN`/`scn` do not — a 6-component
DeviceN set via `sc` silently truncates to 4 and then gets discarded by the
component-count check. Port both halves.

**XObjects & shading (2)**

| Op | Operands | Behavior |
|---|---|---|
| `Do` | 1 name | §1.10. |
| `sh` | 1 name | `FindShading(name)`; null, or `!IsShadingObject()`, or `!Load()` → no-op. Else build a shading object with `matrix = ctm * content_to_user`; bbox = the clip box if a clip exists else the parser's `bbox_`; for **mesh** shadings additionally intersect with `GetShadingBBox()` (§1.11) (:1164-1186). |

**Inline images (3)**

| Op | Behavior |
|---|---|
| `BI` | §1.12. Consumes through `EI` itself. |
| `ID` | **No-op handler** (:987) — only reachable if a stray `ID` appears outside a `BI`. |
| `EI` | **No-op handler** (:911) — same. |

**Marked content (5)**

| Op | Operands | Behavior |
|---|---|---|
| `BMC` | 1 name/string | Clone the top marks, `AddMark(GetString(0))`, push (:711-716). |
| `BDC` | tag, property | `GetObject(0)` null → **no push at all**. If the property is a Name: `FindResourceHolder("Properties")`; if the holder is missing or has no dict under that name → **no push**. If it's a Dictionary: `AddMarkWithDirectDict` (a **clone** of the dict, cpdf_contentmarks.cpp:149-155). Anything else → no push (:625-649). |
| `EMC` | — | Pop, **but never below size 1** — a sentinel `CPDF_ContentMarks` is pushed at construction (:422-423, :913-919). Mismatched `EMC`s are harmless. |
| `MP` | 1 name | **No-op** (:1040). |
| `DP` | name, dict | **No-op** (:909). |

Marks are snapshotted onto each page object at creation
(`SetContentMarks(*content_marks_stack_.top())`, :588). `GetMarkedContentID()`
returns the `/MCID` of the **first** mark that has one, else −1
(cpdf_contentmarks.cpp:129-137).

**Compatibility (2)** — `BX` and `EX` are **not in the table at all**
(:264-382). They fall through `OnOperator` as unknown keywords and are
silently ignored, which is the correct spec behavior by accident. Our `ops!`
table lists them explicitly as `Op::BeginCompat` / `Op::EndCompat` no-ops so
the enum is complete and no `_ =>` arm is needed.

**Count check:** the group subtotals are 11 + 7 + 10 + 2 + 9 + 4 + 4 + 2 + 12
+ 2 + 3 + 5 = **71**, exactly the number of `g_opcodes` entries. Adding
`BX`/`EX` gives our `Op` enum **73** named variants plus
`Op::Unknown(Box<[u8]>)` for diagnostics — which is where SPEC §7's "all ~73
content operators" lands.

### 1.4 Dash arrays — parsing and the normalization ladder

Parsing (`Handle_SetDash`, :757-764 → cpdf_allstates.cpp:34-37): operand 1
must be an Array (else no-op); **every element is read as a float via
`GetFloatAt`** (`ReadArrayElementsToVector`,
fpdf_parser_utility.cpp:151-160) — non-numeric elements become `0.0`. The
array is stored verbatim, phase = `GetNumber(0)`. **No validation happens at
parse time**: negative, zero, NaN, and infinite dash lengths are all stored.

The normalization lives at stroke time, in the AGG driver
(`core/fxge/agg/cfx_agg_devicedriver.cpp:336-386`). It is a rendering
behavior, but it is *fully determined by the dash array*, so we compute it
once in `pdfrum-page` and hand `StrokeParams` a normalized array (D6):

1. Empty array → solid line.
2. Compute `dash_cycle_len = Σ max(0, val)`. **If any element is non-finite
   (NaN or ±∞), the dash pattern is abandoned entirely → solid line**
   (:344-349).
3. `kMinDashCycleThreshold = 0.1f`: if `dash_cycle_len * scale < 0.1`
   (device pixels), → **solid line** (:357-366). `scale` is the device scale
   factor; at scale 1 this means a total cycle under 0.1 user units renders
   solid.
4. Otherwise each element is emitted as
   `fabs(dash_len * scale)`, **after** the substitution
   **`if (dash_len <= 0.000001f) dash_len = 0.1f`** (:371-374). So a `0` entry
   becomes `0.1`, and a **negative** entry becomes `0.1` too (the `<=` catches
   it before the `fabs`) — negatives never survive as their absolute value.
5. Dash start = `dash_phase * scale`, unclamped.
6. Odd-length arrays are used as-is by AGG's `conv_dash` (which cycles), i.e.
   `[3]` means 3 on / 3 off. (The Skia backend explicitly doubles the array to
   achieve the same, fx_skia_device.cpp:535-554 — cosmetic difference only.)

Defaults when no `d` is seen (`CFX_GraphState` getters,
core/fxge/cfx_graphstate.cpp): line width **1.0**, miter limit **10.0**, cap
**Butt(0)**, join **Miter(0)**, dash array empty. Note the oddity that
`GetLineDashPhase()` returns **1.0f** when the state is unset (:44) while a
set-but-default phase is 0.0 — an unreachable path in practice (the state is
always emplaced before use); we default phase to 0.0 and note it.

### 1.5 `gs` — the ExtGState key table, honored vs ignored

`CPDF_AllStates::ProcessExtGS` (cpdf_allstates.cpp:39-174) iterates the
ExtGState dict **in the dict's own order** and switches on `FXBSTR_ID` of the
key (first 4 bytes). Every value is `GetMutableDirect()`-resolved first;
**a null/unresolvable value skips the key entirely** (:43-46).

| Key | Read as | Effect | Line |
|---|---|---|---|
| `/LW` | number | `graph_state.line_width` | :50-52 |
| `/LC` | integer | `line_cap`, unchecked cast | :53-56 |
| `/LJ` | integer | `line_join`, unchecked cast | :57-60 |
| `/ML` | number | `miter_limit` | :61-63 |
| `/D` | array | Must be an Array; **element 0 must itself be an Array** else skip; dash = element 0, phase = `GetFloatAt(1)` | :64-77 |
| `/RI` | string | `render_intent_` as an id: `Abso`→1, `Satu`→2, `Perc`→3, anything else→0 (cpdf_generalstate.cpp:17-32). **Stored, never consumed by rendering.** | :78-80 |
| `/Font` | array | Must be an Array; `font_size = GetFloatAt(1)`, font = `FindFont(GetByteStringAt(0))`. Note: **the name is looked up in `/Font` resources**, not taken as an indirect font dict — a `[<ref> size]` array (the spec form) therefore resolves the ref to a *string* (empty) and yields the fallback font. Port this quirk. | :81-91 |
| `/TR` | object | **Skipped entirely if `/TR2` also exists in the same dict** (:92-96). Otherwise falls through to the TR2 handler. |
| `/TR2` | object | `SetTR(obj)` — **but only if the object is not a Name**; a Name (e.g. `/Default`, `/Identity`) stores **null**, disabling any transfer function (:97-100). |
| `/BM` | name or array | If an Array, take element 0 as a string; else the object's string. `GetBlendTypeInternal` (generalstate.cpp:34-73) maps by 4-byte id; **unknown → Normal**. If the resulting type is `> Multiply`, sets `BackgroundAlphaNeeded` on the holder (:101-109). | |
| `/SMask` | dict | `SetSoftMask(dict)`; if non-null, also records `smask_matrix_ = current ctm` (:110-117). **A non-dict value (including the Name `/None`) sets the mask to null** — `ToDictionary` of a Name is null — which is exactly the spec's `/None` semantics, reached accidentally. | |
| `/CA` | number | `stroke_alpha = clamp(n, 0.0, 1.0)` (:118-121). | |
| `/ca` | number | `fill_alpha = clamp(n, 0.0, 1.0)` (:122-125). | |
| `/OP` | integer | `stroke_op = (n != 0)`; **and also sets `fill_op` unless `/op` exists in the dict** (:126-131). | |
| `/op` | integer | `fill_op` (:132-134). | |
| `/OPM` | integer | `opmode_` (:135-137). | |
| `/BG`, `/BG2` | object | `/BG` skipped if `/BG2` present; stored, **never consumed** (:138-145). | |
| `/UCR`, `/UCR2` | object | Same pattern; stored, never consumed (:146-153). | |
| `/HT` | object | `SetHT(obj)`; **stored, never consumed** — halftones are not implemented (:154-156). | |
| `/FL` | number | `flatness_`; stored, unused (:157-159). | |
| `/SM` | number | `smoothness_`; stored, unused (:160-162). | |
| `/SA` | integer | `stroke_adjust_`; stored, unused (:163-165). | |
| `/AIS` | integer | `alpha_source_`; stored, unused (:166-168). | |
| `/TK` | integer | `text_knockout_`; stored, unused (:169-171). | |

Keys not listed (`/Type`, `/D0`, `/LW2`, anything else) are ignored.

**Honored** (affect output): LW, LC, LJ, ML, D, Font, TR/TR2, BM, SMask, CA,
ca. **Parsed and stored but inert**: RI, OP, op, OPM, BG/BG2, UCR/UCR2, HT,
FL, SM, SA, AIS, TK. We keep the inert ones as fields on `GeneralState`
(so `--show-pageinfo`-style dumps and a future overprint implementation have
them) but must not let them change rendering.

**Transfer functions** (`/TR`, `/TR2`) are sampled to 3×256 `u8` tables at
use time (`cpdf_docrenderdata.cpp:80-149`):
- Array form: **must have ≥ 3 elements**; functions loaded from elements 0,1,2
  and stored **reversed** (`pFuncs[2-i]`), i.e. the array is (R,G,B) and the
  internal order is (B,G,R). Any element failing to load → the whole transfer
  function is **null** (no transfer applied).
- Single-function form: one function applied to all three channels.
- Sampling: for `v` in 0..256, `input = v / 255.0`, evaluate,
  `sample = roundf(output[0] * 255)`. `kChannelSampleSize = 256`
  (cpdf_transferfunc.h). A function whose `OutputCount() > kMaxOutputs` is
  **skipped, and the identity value `v` is used** for that channel in the
  array form (:117-120); in the single form the *stale* `output[0]` from the
  previous iteration is reused (:135-137) — a genuine C++ bug we do **not**
  port (D7); we use identity.
- `bIdentity` is tracked and, when true, the transfer function is a no-op.

### 1.6 Path assembly, `ParsePathObject`, and stream indices

**`AddPathPoint(point, type)`** (:1536-1558) — three repairs in six lines:
1. A `Move` identical to the previous point, where the previous point is also
   an open `Move`, is **dropped** (:1540-1544).
2. A `Move` following an open `Move` **overwrites** that point rather than
   appending (:1547-1553) — consecutive `m m m` collapses to the last one.
3. A non-`Move` point when `path_points_` is **empty is dropped** (:1554-1556)
   — an `l` before any `m` is silently discarded.

**`AddPathPointAndClose`** (:1560-1569) sets `path_current_` first, then
appends only if the list is non-empty.

**`AddPathObject(fill_type, render_type)`** (:1571-1640):
- Takes and clears `path_points_`; takes and resets `path_clip_type_` to
  NoFill. **The pending clip is consumed by whichever painting operator comes
  next, even `n`.**
- Empty points → return (the clip is discarded too).
- **Single point** (:1583-1606): if a clip is pending, append a degenerate
  `rect(0,0,0,0)` clip via `AppendPathWithAutoMerge` and return — this
  produces an **empty clip region**, blanking everything after it. Otherwise:
  unless the point is a `Move` **and** `close_figure_` **and** the line cap is
  **Round**, return (nothing drawn). In that one case, convert it into
  move-then-line-to-itself-and-close so a round dot renders. Butt and
  projecting-square caps draw nothing.
- A trailing open `Move` is **popped** (:1608-1610).
- Build `CPDF_Path`; `matrix = ctm * content_to_user`.
- Emit a `CPDF_PathObject` **only if** stroking or fill_type ≠ NoFill
  (:1624-1632). `n` with no clip produces nothing at all.
- If a clip was pending: transform the path by the matrix (skipped when the
  matrix is identity) and `AppendPathWithAutoMerge` (:1633-1639). **The clip
  path is in device-ish space; the drawn path keeps its matrix separately.**

**`AppendPathWithAutoMerge`** (cpdf_clippath.cpp:83-100): if the previously
pushed clip path `IsRect()` and its rect (from points 0 and 2) **contains**
the new path's bounding box, the old entry is **popped** before appending —
a nesting optimization, not a semantic change, but it changes clip counts in
dumps.

**Text clips** (`AppendTexts`, cpdf_clippath.cpp:102-113):
`kMaxTextObjects = 1024`. If `existing + incoming > 1024` the whole batch is
**dropped silently**; otherwise the texts are appended followed by a **null
sentinel** marking the end of that clip layer. `GetClipBox` (:41-75) unions
each layer's text bboxes and intersects the layers — the sentinel is what
delimits layers.

**`ParsePathObject()`** (:1694-1786) — a fast sub-loop entered from `m` only.
It reads elements directly from the tokenizer, bypassing the operand ring:
- `params[6]` floats, `nParams` counter; **a 7th number is silently dropped**
  (`if (nParams == 6) break;` at :1770-1772, which breaks the *switch*, so the
  loop continues and the token is consumed).
- Recognized 1-char keywords `m l c v y h` and the 2-char `re` update the path
  and reset `nParams = 0`. Note `c`/`v`/`y` here read `params[0..6]` in
  **source order** (:1719-1744) — the same geometry as the ring-buffer
  handlers, expressed forward instead of backward.
- Any other keyword, any non-number element, or `kEndOfData` → **rewind to
  `last_pos`** (the position after the last successfully processed path
  operator) and return, letting the main loop re-read it (:1764-1766,
  :1781-1784). So operands already consumed for an unrecognized operator are
  re-tokenized — no operand loss.
- `v` in this loop uses `path_current_` as the first control point, matching
  the handler; `y` duplicates `params[2..4]`, matching too.

**`GetCurrentStreamIndex()`** (:1383-1387):
`upper_bound(stream_start_offsets_, syntax_->GetPos() + start_parse_offset_) - begin() - 1`.
Every page object records which `/Contents` element it came from. With a
single stream, offsets are `[0]` and every object gets index 0. Objects
created before any offset (impossible in practice) would get −1
(`kNoContentStream`). We keep this as `PageObject::content_stream: i32`
because `pdfrum-edit` needs it.

**`all_ctms_`** (cpdf_streamcontentparser.h:236, a `std::map<int32_t, CFX_Matrix>`):
seeded at construction with `all_ctms_[0] = initial ctm` (:427), and updated
by `cm` (:732) and `Q` (:1057) with the **current stream index**. Consumers
(`GetCTMAtBeginningOfStream`/`GetCTMAtEndOfStream`,
cpdf_pageobjectholder.cpp:123-151) use it for content-stream editing:
beginning-of-stream 0 or an empty map → identity; `kNoContentStream` → the
last entry; otherwise the end-of-previous-stream value, itself
`lower_bound(stream)` or the last entry. Edit-crate concern; we carry the map.

### 1.7 Form XObject recursion, depth guard, and resource inheritance

**The depth guard is not a depth counter.** `Parse()` (:1642-1692) checks:

```
if (recursion_state_->parsed_set.size() > kMaxFormLevel   // 40
    || Contains(recursion_state_->parsed_set, data.data()))
    return data.size();          // consume everything, produce nothing
```

with `kMaxFormLevel = 40` (:55) and `parsed_set` a
`std::set<const uint8_t*>` of **content-buffer start pointers**
(cpdf_form.h:27-32). Two consequences:

- The bound is on the number of *simultaneously in-flight* form parses, and
  the comparison is `>` not `>=`, so **41 nested forms are allowed and the
  42nd is refused**.
- The set is keyed by buffer identity, so **a form that re-invokes itself (or
  any ancestor) is refused even far below the depth limit** — this is the
  real cycle guard, and it triggers on the *same decoded buffer*, meaning two
  distinct `Do` calls on the same XObject at the same nesting level both work
  (sequential, the guard is scoped) but a self-reference does not.
- On refusal the whole stream is skipped and the parse *succeeds* with zero
  objects — never an error.

The set is cleared once per page at the top of the page parse
(`recursion_state_.parsed_set.clear()`, cpdf_contentparser.cpp:214) and a
scoped insertion (`ScopedSetInsertion`, :1659-1660) removes the entry when the
form's parse returns.

**`AddForm`** (:819-842):
1. Snapshot general/graph/color/text state into a fresh `CPDF_AllStates`
   (:821-825) — note the **clip path and the CTM are deliberately not
   copied**; the form gets its own clip from its `/BBox` and its CTM from its
   `/Matrix`.
2. Build a `CPDF_Form` and `ParseContent(&status, nullptr, recursion_state_)`
   — the parent matrix passed is **nullptr**, so the form's own bbox clip is
   not pre-transformed by the parent (see below).
3. `matrix = ctm * content_to_user`; build the form object; if the child form
   needs background alpha, propagate the flag up (:835-838).
4. `CalcBoundingBox()` = the child's `CalcBoundingBox()` (union of its active
   objects' rects, seeded at ±1000000, cpdf_form.cpp:98-118 — an **empty form
   yields an empty rect**, not the ±1e6 sentinel) transformed by the form
   matrix.
5. `SetGraphicStates(obj, color=true, text=true, graph=true)`.

**Form setup** (cpdf_contentparser.cpp:61-121):
- `form_matrix = dict["/Matrix"]` (missing → identity), then
  **`form_matrix.Concat(parent_states.ctm)`** — i.e. `form_matrix * parent_ctm`
  (:72-76).
- `/BBox`: if present, a clip path is built from the rect, transformed by
  `form_matrix`, then by `parent_matrix` if one was supplied; and `form_bbox`
  is transformed the same way (:78-95). **`AddForm` passes `parent_matrix =
  nullptr`**, so for form-inside-content the second transform is skipped; the
  Type3 path and the annotation path do pass one. A **missing `/BBox` means no
  clip at all** — the form is unbounded.
- The interpreter is constructed with `resources = form's /Resources`,
  `parent_resources = holder's resources`, `page_resources`, and the states
  snapshot; then `ctm = form_matrix`, `parent_matrix = form_matrix` (:105-106).
- **Transparency-group reset** (:111-117): if the form's dict declares a
  transparency group, the initial state is forced to `BlendMode::Normal`,
  `stroke_alpha = 1.0`, `fill_alpha = 1.0`, `soft_mask = null`. This is the
  group-isolation rule and is essential for correct compositing.

**Resource resolution** — `ChooseResourcesDict(pResources, pParentResources,
pPageResources)` (cpdf_form.cpp:26-34): the **first non-null** of
form `/Resources`, the parent's resources, the page's resources. Note this is
a whole-dictionary choice, **not** a per-category merge: a form with a
`/Resources` containing only `/Font` cannot see the page's `/XObject`.

Then `FindResourceHolder(type)` (:1209-1225) adds one fallback layer at
lookup time: look in `resources_[type]`; if absent **and** `resources_ !=
page_resources_` **and** `page_resources_` exists, look in
`page_resources_[type]`. So a *category* missing from the chosen resources
does fall back to the page — but only one level, and only to the page. A
category present-but-lacking-the-name does **not** fall back (:1215-1217
returns the first dict found).

`FindResourceObj(type, name)` (:1227-1233) = `holder->GetMutableDirectObjectFor(name)`
— a resolved direct object, so an indirect entry works.

**Page-tree attribute inheritance** (`CPDF_Page::GetPageAttr`,
cpdf_page.cpp:114-127): walk `page_dict`, then `/Parent`, then its `/Parent`…
returning the first **direct** object found for the key, with a
**visited-dict set** as the cycle guard. Applies to `/Resources`,
`/MediaBox`, `/CropBox`, `/Rotate` (and the other boxes). Note `GetDictFor`
is used for `/Parent`, so a `/Parent` that is not a dict ends the walk.

**Box derivation** (`UpdateDimensions`, :266-298):
- `mediabox = GetBox(/MediaBox)`, normalized. **If empty → `(0,0,612,792)`**
  (US Letter) (:267-270). "Empty" means `IsEmpty()`, i.e. width ≤ 0 or
  height ≤ 0 after normalization.
- `bbox = GetBox(/CropBox)`, normalized; **if empty → mediabox; else
  intersected with mediabox** (:272-277). An intersection that comes out
  empty is kept empty (page size 0×0) — `GetDisplayMatrix*` then returns
  identity (:162-164).
- `page_size = (bbox.Width(), bbox.Height())`.
- Rotation (`GetPageRotation`, :227-232):
  `rotation = (GetInteger(/Rotate) / 90) % 4`, then `+4` if negative. So
  `/Rotate 45` → 0, `/Rotate -90` → 3, `/Rotate 450` → 1.
- The page matrix per rotation (:282-297), with width/height **swapped** for
  1 and 3:
  - 0: `(1, 0, 0, 1, -left, -bottom)`
  - 1: `(0, -1, 1, 0, -bottom, right)`
  - 2: `(-1, 0, 0, -1, right, top)`
  - 3: `(0, 1, -1, 0, top, -left)`

**Page dict validation** (:54-71): `IsValidPageDictLoose` — null → false; no
`/Type` → **true** (tolerated); `/Type` present must resolve (through refs) to
the **Name** `Page`. `IsValidPageDict` additionally requires `/Type` to exist.
The constructor `CHECK`s the loose form and then **writes `/Type /Page` into
the dict when missing** (:33-36) — we do not mutate (D3).

**Transparency** (`LoadTransparencyInfo`, cpdf_pageobjectholder.cpp:153-167):
`/Group` must be a dict whose `/S` is the name `Transparency`; then `group =
true`, and `/I` truthy → `isolated = true`. A `CPDF_Page` additionally sets
`isolated` unconditionally at construction (cpdf_page.cpp:47) — **pages are
always isolated**. `/K` (knockout) is **not parsed anywhere in
`core/`** — `CPDF_Transparency` has only `group_` and `isolated_` bools
(cpdf_transparency.h:22-23). Knockout groups are therefore **unimplemented in
the oracle**; we parse `/K` into the model (SPEC §8 wants it) but must render
it as non-knockout to match, until Tier-B says otherwise (Open question Q4).

### 1.8 Font, colorspace, pattern, and shading lookup

**`FindFont(name)`** (:1235-1254): `FindResourceObj("Font", name)` as a dict;
**missing → the stock font `CFX_Font::kDefaultAnsiFontName` (Helvetica)**, so
`Tf` with a bogus name still renders text. Found → cached font; sets the
resource name; for Type3 fonts also installs the page resources and runs
`CheckType3FontMetrics()`.

**`FindColorSpace(name)`** (:1262-1293):
1. `"Pattern"` → the stock Pattern space.
2. `"DeviceGray"` / `"DeviceRGB"` / `"DeviceCMYK"` → build `defname =
   "Default" + name[6..]` (e.g. `DefaultCMYK`) and look **that** up in the
   `/ColorSpace` resources. Absent → the corresponding stock space. Present →
   load it as a colorspace. This is the `/DefaultRGB` mechanism, and it is
   applied **only** for these three literal names, not for `/G` `/RGB` `/CMYK`
   abbreviations.
3. Anything else → `FindResourceObj("ColorSpace", name)`; absent → **null →
   the `cs`/`CS` operator is a no-op**.

There is a **second** `/Default*` path inside
`CPDF_DocPageData::GetColorSpaceInternal` (cpdf_docpagedata.cpp:335-356): when
a *Name* colorspace resolves to a stock device space **and** a `/ColorSpace`
resources dict is supplied, `/DefaultRGB`/`/DefaultGray`/`/DefaultCMYK` is
consulted there too. Since `FindColorSpace` passes `pResources = nullptr`
(:1284-1285, :1291-1292), that path is reached only from the *image* code,
which does pass resources. Net: **default colorspaces apply to both `cs`
operators and image `/ColorSpace` names, by two different routes.**

**Colorspace caching and cycle guards** (cpdf_docpagedata.cpp:284-380):
- Two visited sets: `pVisited` (threaded across the whole load, catching
  `/Indexed` → base → `/Indexed` cycles inside `CPDF_ColorSpace::Load`) and
  `pVisitedInternal` (per-call, catching resource-name cycles) (:291-297).
  A hit on `pVisitedInternal` → null (:308-310).
- A Name resolves via `GetStockCSForName` (colorspace.cpp:481-496), which
  accepts `DeviceRGB`/`RGB`, `DeviceGray`/`G`, `DeviceCMYK`/`CMYK`, `Pattern`
  — **the abbreviations are accepted here**, unlike in `FindColorSpace`.
  Non-stock names are looked up in `pResources["ColorSpace"]` and recursed
  (:317-323).
- An **empty array** or a non-array non-name → null (:358-361).
- **A 1-element array recurses into element 0** (:363-366) — `[/DeviceRGB]`
  works.
- Arrays of size ≥ 2 are cached by array identity in `color_space_map_`
  (:368-379).

**`FindPattern(name)`** (:1295-1303): resource object must be a Dictionary or
a Stream (else null); then `GetPattern(obj, cur_states_->parent_matrix())`.
The **parent matrix**, not the CTM — patterns are anchored to the form/page
coordinate system, not to the current transform. This is a frequent source of
bugs; port it exactly.

**`FindShading(name)`** (:1305-1313): same type check, then
`GetShading(obj, parent_matrix)`.

### 1.9 Text object construction — `TJ` segmentation and position bookkeeping

**`Handle_ShowText_Positioning`** (`TJ`, :1396-1446):
1. Operand must be an Array (else no-op).
2. First pass: count elements that are **direct** and `IsString()` → `nsegs`.
3. **`nsegs == 0`**: the array is pure kerning. For each element,
   `fKerning = GetFloatAt(i)`; if non-zero,
   `text_pos.x -= fKerning * font_size / 1000 * horz_scale`. **Only x moves,
   even for vertical fonts** (:1410-1418) — an inconsistency with the
   `nsegs > 0` path, which does check `IsVertWriting`. Port it.
4. Otherwise: walk the array; null direct objects are **skipped without
   consuming a segment slot**; strings (empty ones skipped, not counted)
   fill `strs[iSegment]` with kerning 0; numbers accumulate into
   `fInitKerning` while `iSegment == 0`, else into `kernings[iSegment-1]`
   (**accumulating**, so `[(A) 5 5 (B)]` gives kerning 10) (:1423-1444).
5. `AddTextObject(strs[0..iSegment], kernings, fInitKerning)`.

Note `nsegs` counts strings including empty ones, but the second pass skips
empty strings, so `strs` may be shorter than `nsegs` — hence the
`.first(iSegment)` at :1445.

**`AddTextObject(strings, kernings, initial_kerning)`** (:1315-1373):
- **No font → return** (:1319-1322). Since `FindFont` always yields something,
  this only fires when `Tf` was never issued.
- `initial_kerning != 0`: advance the text position **before** the object —
  vertical fonts move y by `-GetVerticalTextSize(k)`, horizontal move x by
  `-GetHorizontalTextSize(k)` (:1323-1331), where
  `GetVerticalTextSize(k) = k * font_size / 1000` (:1379-1381) and
  `GetHorizontalTextSize(k) = GetVerticalTextSize(k) * horz_scale` (:1375-1377).
- **Empty `strings` → return** (:1332-1334), *after* the initial kerning has
  already been applied. So `[5] TJ` with `nsegs == 0` goes down path 3, but
  `[(  ) ] TJ` (a string that turns out empty) applies kerning and produces
  nothing.
- Text mode: **Type 3 fonts are forced to `MODE_FILL`** regardless of `Tr`
  (:1335-1337).
- `SetGraphicStates(obj, color=true, text=true, graph=true)`.
- For **stroke** modes, the object's text-state CTM is filled from the current
  CTM as `[a, c, b, d]` (:1342-1350) — the transposed layout the renderer
  expects for stroke-width scaling.
- `SetSegments` then `SetPosition(content_to_user.Transform(GetTransformedTextPosition()))`.
  `GetTransformedTextPosition()` = `ctm.Transform(text_matrix.Transform({x, y + rise}))`
  (cpdf_allstates.cpp:181-184) — **the rise is applied inside the text matrix**,
  not after it.
- `CalcPositionData(horz_scale)` returns the total advance and, for vertical
  writing, returns it as `(0, advance)` instead of `(advance * horz_scale, 0)`
  (cpdf_textobject.cpp:279-286). The parser then increments the text position
  by it (:1357-1358).
- Clip modes push a **clone** of the text object onto `clip_text_list_`
  (:1359-1361).
- After the object, a non-zero **trailing** kerning
  (`kernings.back()`) advances the position again (:1364-1372) — this is the
  kerning that followed the last string in the `TJ` array.

**`OnChangeTextMatrix()`** (:1458-1470) — called by `BT`, `Tm`, `Tz`, `cm`:
builds `[horz_scale 0; 0 1; 0 0] × text_matrix × ctm × content_to_user` and
stores it into the text state as `[a, c, b, d]` (transposed). The *translation*
is dropped here; position is tracked separately.

**Advance math** (`CalcPositionDataInternal`, cpdf_textobject.cpp:288-360),
per char, in text space:
```
current_position += char_width                     // font->GetCharWidth(code) * size/1000
current_position += word_space  if code == ' ' and (not CID or CID char size == 1)
current_position += char_space
current_position -= char_kernings_[i] * size / 1000
```
with `char_width` replaced by `cid_font->GetVertWidth(cid) * size/1000` for
vertical writing (:307-334). The word-space condition
(`GetWordSpaceIfNeeded`, :30-37) is exactly "char code is 0x20 **and** either
the font is not a CID font or `GetCharSize(' ') == 1`" — multi-byte space
codes get no word spacing.

Bounding box: min/max seeded at ±10000; for horizontal writing the y extremes
are in glyph units and scaled by `size/1000` at the end while x is already
scaled; for vertical it is the mirror (:341-347). Stroke modes inflate the
final rect by `line_width / 2` (:351-356).

### 1.10 `Do` — XObject dispatch and the last-image cache

`Handle_ExecuteXObject` (:779-817):
1. **Last-image fast path**: if the name equals `last_image_name_`, and
   `last_image_` exists with a stream that has a **non-zero object number**,
   re-add the same image by objnum without re-reading resources (:781-790).
   Inline images (objnum 0) never take this path.
2. `FindResourceObj("XObject", name)` as a **Stream**; not a stream → no-op.
3. `/Subtype` read as a byte string:
   - `"Form"` → `AddForm` (§1.7).
   - `"Image"` → if the stream is **inline** (objnum 0, i.e. it came from a
     direct object in the resources dict) a **clone** is used; otherwise it is
     fetched by objnum through the document's image cache (:803-807). The
     image name is remembered as `last_image_name_`, and if the image is a
     mask its rect is recorded via `AddImageMaskBoundingBox` (:810-815).
   - Any other subtype (including `"PS"` and missing) → **nothing**.

`AddImageObject` (:881-892): `SetGraphicStates(obj, color = image->IsMask(),
text = false, graph = false)` — **a non-mask image does not carry the color
state**, since it supplies its own colors; a stencil mask does, because it is
painted with the fill color. Then
`SetInitialImageMatrix(ctm * content_to_user)`. The image object's rect is
always the unit square transformed by that matrix
(cpdf_imageobject.cpp:41-45).

### 1.11 `sh` and the mesh bounding box

`Handle_ShadeFill` (:1164-1186) is covered in §1.3. The mesh bbox helper
`GetShadingBBox(shading, matrix)` (:74-151) walks the mesh stream purely to
compute extents, and its loop structure is the same one the real mesh reader
uses:

- Point/colour counts per type: tensor (7) → 16 points, Coons (6) → 12,
  everything else → 1; Coons and tensor → 4 colours, else 1
  (:57-61, :94-109).
- Lattice-form (5) reads **no flag**; every other type reads one per record,
  bailing if `!CanReadFlag()` (:112-118).
- For non-Gouraud types (6, 7) a **non-zero flag reduces the record**:
  `point_count -= 4` and `color_count -= 2` (:120-123) — the shared-edge
  optimization.
- Colours are **skipped, not decoded**: `nBits = components * component_bits *
  color_count`, checked for overflow, then `SkipBits` (:138-145).
- Gouraud types (4, 5) `ByteAlign()` after each record (:146-148).
- The accumulated rect is transformed by the shading object's matrix.

### 1.12 Inline images — `BI … ID … EI`

`Handle_BeginImage` (:651-709) plus `ReadInlineStream`
(cpdf_streamparser.cpp:156-252). This is the single quirkiest area in the
crate.

**Dict scanning** (:652-674):
- Save the position. Loop `ParseNextElement()`:
  - A `kKeyword` element whose word is **not** `"ID"` → **rewind to the saved
    position and abandon the whole `BI`** (:656-660). The `BI` becomes a
    no-op and the stream is re-read from just after `BI` as ordinary content.
  - Any element type other than `kName` → break out of the loop (:662-664).
    Note the ordering: a keyword `"ID"` falls through the first check and then
    breaks here, which is the normal exit.
  - `kName` → key = word[1..]; value = `ReadNextObject(false, false, 0)`.
    **If the value is a non-inline object (objnum ≠ 0) a Reference is stored
    instead** (:668-673) — unreachable for a stream parser (everything it
    builds is inline), noted for completeness.
- **No dict-size cap, no key-count cap.**

**Abbreviation expansion** (`ReplaceAbbr`, :189-259). Key abbreviations
(`kInlineKeyAbbr`, :158-162), exact and case-sensitive, **whole-key match
only** (a prefix does not match):

| Abbr | Full |
|---|---|
| `BPC` | `BitsPerComponent` |
| `CS` | `ColorSpace` |
| `D` | `Decode` |
| `DP` | `DecodeParms` |
| `F` | `Filter` |
| `H` | `Height` |
| `IM` | `ImageMask` |
| `I` | `Interpolate` |
| `W` | `Width` |

Value abbreviations (`kInlineValueAbbr`, :164-171), applied to **Name values
only**, recursively through dicts and arrays:

| Abbr | Full |
|---|---|
| `G` | `DeviceGray` |
| `RGB` | `DeviceRGB` |
| `CMYK` | `DeviceCMYK` |
| `I` | `Indexed` |
| `AHx` | `ASCIIHexDecode` |
| `A85` | `ASCII85Decode` |
| `LZW` | `LZWDecode` |
| `Fl` | `FlateDecode` |
| `RL` | `RunLengthDecode` |
| `CCF` | `CCITTFaxDecode` |
| `DCT` | `DCTDecode` |

Note `I` is ambiguous: as a **key** it means `Interpolate`, as a **value** it
means `Indexed`. The key table is consulted for keys, the value table for
values, so both work. Rewriting is done as a two-phase collect-then-apply
(:191-230) so the dict is not mutated mid-iteration; the **key rename happens
first, then the value replacement is applied under the new key** (:205-206,
:223-229).

**Colorspace resolution for the inline dict** (:676-688): if `/ColorSpace`
exists and its direct value is a Name that is not one of the three device
names, look it up in the page's `/ColorSpace` resources; if found **and
inline**, a **clone** is stored into the dict. Otherwise the name is left in
place (and will fail to resolve later).

Then `/Subtype /Image` is forced into the dict (:689).

**`ReadInlineStream`** (cpdf_streamparser.cpp:156-252) — where the data length
is decided:
1. **Exactly one** leading whitespace byte after `ID` is skipped, if present
   (:165-171). A second whitespace byte becomes image data.
2. Filter: `/Filter` direct object; an **Array** takes element 0 as the
   decoder name and `/DecodeParms` array element 0 as the params; a Name takes
   the name and `/DecodeParms` as a dict (:173-189). **Only the first filter
   of a chain is honored** for length-finding.
3. Expected size: `bpc = 1`, `nComponents = 1` unless a colorspace object was
   supplied, in which case `nComponents = cs->ComponentCount()` (**or 3 if the
   colorspace fails to load**) and `bpc = dict["/BitsPerComponent"]`
   (:190-199). Note: when `/CS` is absent entirely, `pCSObj` is null and the
   defaults 1/1 are used — so a `/BPC 8` grayscale image with no `/CS` is
   sized as 1 bpc.
   `original_size = CalculatePitch8(bpc, nComponents, width) * height`;
   either step overflowing → **null stream, `BI` produces nothing**
   (:200-210).
4. **Unfiltered** (`decoder` empty): `original_size` is **clamped to the
   remaining bytes** (:216) and exactly that many bytes are taken. The parser
   position lands right after them; the trailing `EI` is then found by the
   caller's scan (step 6).
5. **Filtered**: `DecodeInlineStream` (:82-143) actually decodes to learn how
   many source bytes were consumed:
   - `FlateDecode` → `FlateOrLZWDecode(use_lzw=false, …, estimated_size=original_size).bytes_consumed`
   - `LZWDecode` → same with `use_lzw=true, estimated_size=0`
   - `DCTDecode` → build a JPEG scanline decoder (with
     `ColorTransform` from the params, **default 1**) at `scale_denom=1`,
     walk **every** scanline (stopping early on the first empty one), and take
     `GetSrcOffset()` (:49-80, :108-120)
   - `CCITTFaxDecode` → same walk with a fax decoder
   - `ASCII85Decode` / `ASCIIHexDecode` / `RunLengthDecode` → decode, take
     `bytes_consumed`
   - anything else (including `JPXDecode`, `JBIG2Decode`, and any
     abbreviation — the abbreviations were already expanded, hence the
     `DCHECK`s at :89-95) → **`FX_INVALID_OFFSET` (0xFFFFFFFF)**, which fails
     the `IsValueInRangeForNumericType<int>` check at :225 → **null stream**.
     So **inline JPX and JBIG2 images are unsupported and produce nothing.**
   - `DecodeAllScanlines` returns `FX_INVALID_OFFSET` when the decoder is
     null, when width or height is ≤ 0, or when the computed size is 0
     (:49-72).
6. **The EI resync** (:229-249, and again at :692-702 in the handler). This is
   the part that must be exact:
   - Save the position; advance by `actual_stream_size`.
   - Loop: remember the position, `ParseNextElement()`.
     - `kEndOfData` → **the whole inline image fails (null stream)** (:235-237).
     - `kKeyword` with word `"EI"` → **break**.
     - Otherwise **add `pos - saved_position` to `actual_stream_size`** and
       loop (:243).
   - Restore the saved position (the `AutoRestorer`), then take
     `actual_stream_size` bytes and advance past them.

   Net effect: *everything between the end of the decoded data and the next
   standalone `EI` keyword is absorbed into the image data.* Whitespace,
   garbage, and even a `(string)` containing `EI` are swallowed, because the
   tokenizer sees the string as one `kOther` element. An `EI` immediately
   inside a comment is skipped too. Only a real, standalone `EI` token stops
   it.
   - `/Length` is then **written into the dict** as the final size (:250).
7. Back in the handler (:692-702), a **second** scan runs from wherever
   `ReadInlineStream` left off, looking for `EI` again, returning on
   `kEndOfData`. For the unfiltered path this is the scan that finds the
   `EI` (step 4 left the position right after the pixel bytes). For the
   filtered path the position is already past the `EI`'s predecessor, so the
   first element seen is usually `EI` itself.
8. Finally `AddImageFromStream(stream, name="")`; if the image is a mask its
   rect is recorded (:703-708).

Consequences worth pinning as tests: an unfiltered inline image whose
`W*H*bpc*comps` **understates** the real data leaves the surplus to be
tokenized as content (garbage operators, harmlessly ignored); one that
**overstates** it clamps to the remaining stream and then the `EI` scan
consumes the rest of the page. Both are real corpus behaviors.

### 1.13 Optional content — `/OC` visibility

Covered by `cpdf_occontext.*`; the interpreter itself does not filter, so
`/OC` affects rendering, not the object graph. Inventoried in §1.18 with the
rest of the render-adjacent behavior, because `pdfrum-page` must expose the
predicate for `pdfrum-render` and `pdfrum-text` to share.

### 1.14 Colorspaces

#### 1.14.1 Family enum and construction dispatch

`enum class Family` (cpdf_colorspace.h:56-69) with **fixed integer values** that
reach the public API: `kUnknown=0, kDeviceGray=1, kDeviceRGB=2, kDeviceCMYK=3,
kCalGray=4, kCalRGB=5, kLab=6, kICCBased=7, kSeparation=8, kDeviceN=9,
kIndexed=10, kPattern=11`.

`CPDF_ColorSpace::Load(doc, obj, visited)` (cpdf_colorspace.cpp:499-561):

1. Null → null. **Cycle guard**: `visited` contains the object → null (:507-509);
   otherwise a scoped insertion for the duration (:511).
2. **Name** → `GetStockCSForName` (:513-515).
3. **Stream** → iterate the stream's dict; the first value that is a Name and
   resolves via `GetStockCSForName` wins; none → null (:517-531). (Dict
   iteration order is the C++ `std::map`'s key order; we use insertion order —
   see D2. Observably identical for the single-key dicts this path sees.)
4. Non-array or empty array → null (:533-536).
5. Element 0 missing → null; `familyname = elem0.GetString()`.
6. **A 1-element array is `GetStockCSForName(familyname)`** (:544-546) — so
   `[/DeviceRGB]` works but `[/CalRGB]` does not.
7. `AllocateColorSpace(familyname)`; unknown → null.
8. `array_` is set **before** `v_Load` so `HasSameArray` self-reference checks
   work inside it (:554).
9. `components_ = v_Load(...)`; **a return of 0 means failure** → null.

`GetStockCSForName` (:481-496) — exact-match, case-sensitive:
`DeviceRGB`|`RGB` → DeviceRGB; `DeviceGray`|`G` → DeviceGray;
`DeviceCMYK`|`CMYK` → DeviceCMYK; `Pattern` → Pattern; anything else → null.
The one-letter forms are the inline-image abbreviations and are honored
**everywhere**, not just inline.

`AllocateColorSpace` (:564-587) dispatches on `FXBSTR_ID` — **the first four
bytes only**, zero-padded:

| ID prefix | Class | Note |
|---|---|---|
| `CalG` | CalGray | matches `CalGrayAnything` |
| `CalR` | CalRGB | |
| `Lab\0` | Lab | exactly the 3-char `Lab`; `Lab2` does **not** match |
| `ICCB` | ICCBased | |
| `Inde` | Indexed | |
| `I\0\0\0` | Indexed | the inline abbreviation |
| `Sepa` | Separation | |
| `Devi` | DeviceN | **`[/DeviceRGB …]` with ≥2 elements dispatches here**, then fails `v_Load` |
| `Patt` | Pattern | |
| else | — | null |

**There is no numeric colorspace depth cap.** Recursion is bounded only by the
two visited sets (§1.8) and the per-subclass `HasSameArray` self-checks in
Separation (:1110), DeviceN (:1180), Indexed (cpdf_indexedcs.cpp:39), and
Pattern (cpdf_patterncs.cpp:28). `CPDF_ICCBasedCS::FindAlternateProfile` does
**not** self-check; it relies on the shared visited set. Our loader threads an
explicit depth counter as well (D8) — the C++ is protected by its own stack
depth in practice, which is not a contract we can rely on.

`ComponentsForFamily` (:590-601) handles **only** the three device families
(Gray 1, RGB 3, CMYK 4) and is `NOTREACHED()` otherwise; it is used solely by
the `CPDF_DeviceCS` constructor. The real component counts come from `v_Load`:

| Family | Components | Source |
|---|---|---|
| DeviceGray / DeviceRGB / DeviceCMYK | 1 / 3 / 4 | `ComponentsForFamily` |
| CalGray | 1 (fixed) | :716 |
| CalRGB | 3 (fixed) | :777 |
| Lab | 3 (fixed) | :864 |
| ICCBased | `/N` ∈ {1,3,4} | :968 |
| Separation | **1 always** | :1106, :1130 |
| DeviceN | `len(/Names)`, **uncapped** | :1198 |
| Indexed | **1 always** | cpdf_indexedcs.cpp:88 |
| Pattern | stock 1; loaded `base.components + 1`, or 1 with a failed base | cpdf_patterncs.cpp:21,36,47 |

Predicates: `IsSpecial()` = Separation | DeviceN | Indexed | Pattern
(:97-101). `IsNormal()` = DeviceGray | DeviceRGB | DeviceCMYK | CalGray |
CalRGB (:671-676), overridden by ICCBased (:1014-1025).

#### 1.14.2 Shared conversion helpers — exact math

**`GetWhitePoint(dict, out[3])`** (:110-120): requires `/WhitePoint` present
with **exactly 3 elements**, else returns false (which fails the whole
CalGray/CalRGB/Lab load). Then returns
`Xw > 0.0 && Yw == 1.0 && Zw > 0.0` — note the **exact float equality on Yw**;
`[0.9505 0.99999 1.089]` fails.

**`GetBlackPoint(dict, out[3])`** (:93-108): missing or wrong size → all
zeros; **if any component is < 0, all three are reset to 0**. Never fails.

**`RGB_Conversion(c)`** (:365-372) — sRGB gamma encode via a 445-entry table:
```
c = clamp(c, 0.0, 1.0);
scale = max((int)(c * 1023), 0);
if scale < 192 { return SRGB1[scale] / 255.0 }
return SRGB2[scale / 4 - 48] / 255.0
```
`kSRGBSamples1` is 192 bytes (:51-65), `kSRGBSamples2` is 253 bytes (:67-82).
Max index into the second table is `1023/4 - 48 = 207`, in range. **Both
tables are transcribed verbatim into `crates/pdfrum-page/src/color/srgb_table.rs`.**

**`XYZ_to_sRGB(X, Y, Z)`** (:374-379):
```
R1 =  3.2410*X - 1.5374*Y - 0.4986*Z
G1 = -0.9692*X + 1.8760*Y + 0.0416*Z
B1 =  0.0556*X - 0.2040*Y + 1.0570*Z
return (RGB_Conversion(R1), RGB_Conversion(G1), RGB_Conversion(B1))
```

**`XYZ_to_sRGB_WhitePoint(X,Y,Z, Xw,Yw,Zw)`** (:381-408). Chromaticities
(:390-395): `Rx=0.64 Ry=0.33 Gx=0.30 Gy=0.60 Bx=0.15 By=0.06`. Then
```
RGB_xyz = [[Rx, Gx, Bx],
           [Ry, Gy, By],
           [1-Rx-Ry, 1-Gx-Gy, 1-Bx-By]]
S = RGB_xyz.inverse() * (Xw, Yw, Zw)
M = RGB_xyz * diag(S.a, S.b, S.c)
rgb = M.inverse() * (X, Y, Z)
return (RGB_Conversion(rgb.a), RGB_Conversion(rgb.b), RGB_Conversion(rgb.c))
```
`Matrix_3by3::Inverse()` (:329-339): `det = a(ei-fh) - b(id-fg) + c(dh-eg)`;
**if `|det| < f32::EPSILON` it returns the all-zero matrix**, silently
producing black rather than erroring.

#### 1.14.3 DeviceGray / DeviceRGB / DeviceCMYK

`CPDF_DeviceCS::v_Load` is `NOTREACHED()` (cpdf_devicecs.cpp:38-44) — device
spaces are only ever the stock singletons.

`GetRGB` (cpdf_devicecs.cpp:46-79), with `NormalizeChannel(v) = clamp(v,0,1)`:
- **Gray**: `g = clamp(buf[0]); return (g,g,g)` — only component 0 read.
- **RGB**: `(clamp(r), clamp(g), clamp(b))`.
- **CMYK**, two paths selected by `IsStdConversionEnabled()`:
  - **std conversion on** (:65-71), **no input clamping**:
    ```
    r = 1 - min(1, c + k);  g = 1 - min(1, m + k);  b = 1 - min(1, y + k)
    ```
  - **off** (default, :72-74): `AdobeCmykToStandardRgbF(clamp(c),clamp(m),clamp(y),clamp(k))`.

**`IsStdConversionEnabled()`** is `std_conversion_ != 0`, a **saturating
refcount** (`EnableStdConversion`, :663-669) that `CPDF_BasedCS` propagates to
its base (cpdf_basedcs.cpp:13-18). Its **only** callers in the whole tree are
in `cpdf_dib.cpp` (enable :154, disable :245/:323/:858) — i.e. std conversion
is on **exclusively while decoding an image DIB whose caller asked for it**
(`bStdCS`), never for vector fills. We thread it as a plain `bool std_cs`
parameter into the conversion functions (D9); the refcount shape is a C++
artifact of shared ownership.

**`AdobeCmykToStandardRgbF`** (core/fxge/dib/cfx_cmyk_to_srgb.cpp:1740-1782):
```
kRoundingOffset = 0.49999997f   // the float immediately below 0.5
c1 = (int)(c * 255.0 + kRoundingOffset)   // and m1, y1, k1
rgb8 = AdobeCmykToStandardRgb(c1, m1, y1, k1)
return (rgb8.r / 255.0, rgb8.g / 255.0, rgb8.b / 255.0)
```
The odd rounding constant is deliberate: with `0.5f` the result differs from
`FXSYS_roundf` for `0.0019607842`.

**`AdobeCmykToStandardRgb(u8 c,m,y,k)`** (:1670-1738) — a **9×9×9×9 = 6561**
entry table of RGB triples with per-axis linear interpolation in 8.8 fixed
point. `IndexFromCMYK(c,m,y,k) = 729c + 81m + 9y + k` (:1664-1666):
```
fix_c = c << 8 (etc.)
c_index = (fix_c + 4096) >> 13    (etc.)
start = kCMYK[IndexFromCMYK(c_index, m_index, y_index, k_index)]
fix_r = start.r << 8  (etc.)
c1_index = fix_c >> 13
if c1_index == c_index { c1_index = (c1_index == 8) ? c1_index - 1 : c1_index + 1 }
// for each axis independently, against the SAME `start`:
c1 = kCMYK[IndexFromCMYK(c1_index, m_index, y_index, k_index)]
c_rate = (fix_c - (c_index << 13)) * (c_index - c1_index)
fix_r += (start.r - c1.r) * c_rate / 32     (and g, b)
// ... repeat for m, y, k axes ...
return (max(fix_r,0) >> 8, max(fix_g,0) >> 8, max(fix_b,0) >> 8)   // no high clamp
```
The 6561-entry table is transcribed into
`crates/pdfrum-page/src/color/cmyk_table.rs` (generated once by a checked-in
script, verified byte-for-byte against the header).

#### 1.14.4 CalGray and CalRGB — what is honored

**CalGray** `v_Load` (:698-717): element 1 must be a dict (else 0);
`GetWhitePoint` must succeed (else 0); `GetBlackPoint`; `gamma_ =
dict["/Gamma"]`, and `gamma_ == 0` (which includes "key absent") → **1.0**
(`kDefaultGamma`, :141). Returns **1**.

**CalGray `GetRGB` (:719-723) ignores every parsed parameter and does not even
clamp:** `g = buf[0]; return (g, g, g)`. It is a *worse* DeviceGray (which
clamps). `gamma_`, `white_point_`, `black_point_` are dead after load. We keep
the fields (so `--show-*` dumps can report them) but the conversion is a
pass-through.

**CalRGB** `v_Load` (:748-778): element 1 must be a dict; `GetWhitePoint`
must succeed; `GetBlackPoint`. `/Gamma`: **if the key exists at all** (any
length), 3 entries are read positionally with `GetFloatAt` — **no length
validation**, so `/Gamma [2.2]` yields `{2.2, 0, 0}`. `/Matrix` the same, 9
entries. Returns **3**.

**CalRGB `GetRGB` (:780-806) DOES honor gamma, matrix, and white point**:
```
if gamma present: a = a^g0; b = b^g1; c = c^g2
if matrix present:
    X = m0*a + m3*b + m6*c
    Y = m1*a + m4*b + m7*c
    Z = m2*a + m5*b + m8*c
else X,Y,Z = a,b,c
return XYZ_to_sRGB_WhitePoint(X, Y, Z, Xw, Yw, Zw)
```
`black_point_` is parsed and ignored. No input clamping, so `powf(negative,
2.2)` yields NaN which then propagates into `RGB_Conversion`'s clamp.

**But `CalRGB::TranslateImageLine` (:808-816) is a plain red↔blue swap** —
gamma/matrix/white point are **not** applied to image data. This asymmetry
between the scalar and bulk paths is exactly what
`CPDFCalRGBTest.TranslateImageLine` pins, and it must be ported.

#### 1.14.5 Lab

`v_Load` (:844-865): element 1 must be a dict; `GetWhitePoint` must succeed;
`GetBlackPoint`. `/Range`:
```
kDefaultRanges = {-100.0, 100.0, -100.0, 100.0}
for i in 0..4: ranges_[i] = pParam ? pParam.GetFloatAt(i) : kDefaultRanges[i]
```
**The defaults apply only when `/Range` is entirely absent.** A present-but-
short array yields zeros for the missing entries (`/Range []` → `{0,0,0,0}`).
Returns **3**.

`GetDefaultValue` (:822-842) is the only place `ranges_` is read:
- component 0 (L*): value 0, min 0, max **100**.
- components 1/2: if `range_min <= range_max`, min/max from the pair and
  `value = clamp(0, min, max)`; **otherwise fall back to min 0, max 100,
  value 0** (not to ±100).

`GetRGB` (:867-897) — the constants are **not** the textbook CIE ones:
```
M = (L* + 16.0) / 116.0
L = M + a* / 500.0
N = M - b* / 200.0

X = (L < 0.2069) ? 0.957  * 0.12842 * (L - 0.1379) : 0.957  * L*L*L
Y = (M < 0.2069) ?         0.12842 * (M - 0.1379) :          M*M*M
Z = (N < 0.2069) ? 1.0889 * 0.12842 * (N - 0.1379) : 1.0889 * N*N*N

return XYZ_to_sRGB(X, Y, Z)
```
Constants to preserve exactly: `16.0`, `116.0`, `500.0`, `200.0`, threshold
`0.2069`, linear slope `0.12842`, offset `0.1379`, `Xn = 0.957`, `Zn =
1.0889` (Yn implicit 1.0). `white_point_`, `black_point_`, `ranges_` are
**not used in `GetRGB`** — there is no clamping to `/Range` at conversion time.

`TranslateImageLine` (:899-925) uses a **different input encoding**:
`L = byte0 * 100 / 255`, `a = byte1 - 128`, `b = byte2 - 128`, then `GetRGB`,
writing **BGR** with truncation (`(i32)(v * 255)`).

#### 1.14.6 ICCBased — validation and the fallback ladder

`v_Load` (:931-969):
1. Element 1 must be a **Stream** (a dict or name fails) → 0.
2. `n = stream.dict["/N"]`; **`IsValidIccComponents(n)` requires n ∈ {1,3,4}**
   (core/fxcodec/icc/icc_transform.cpp:152-155); a missing `/N` reads as 0 and
   is rejected → 0. Comment at :939-941: *"While some PDF viewers tolerate
   invalid values, Acrobat does not, so be consistent with Acrobat."*
3. `profile_ = GetIccProfile(stream)` (never null in practice).
4. **The ladder** (:958-964):
   ```
   if !profile.is_supported() && !find_alternate_profile(...) {
       base_cs = stock_alternate_profile(n)
   }
   ```
   Read carefully: the alternate is consulted whenever the profile is *not
   supported*, and `IsSupported()` is **false for the sRGB special case** — so
   an sRGB-detected profile also goes looking for `/Alternate`. But `GetRGB`
   checks `IsSRGB()` first, so that `base_cs_` is dead for sRGB.
5. `ranges_ = GetRanges(dict, n)` (:1071-1086): `/Range` with `size >= 2n`
   → its first `2n` floats; otherwise the default `[0,1]` per component.
   **`ranges_` is currently write-only — nothing reads it**
   (`TODO(crbug.com/42271155)` at :966). We store it and likewise do not clamp.
6. Returns `n` (from `/N`, **not** from the profile).

**`FindAlternateProfile`** (:1027-1053) rejects, in order: no `/Alternate`;
`Load` of it fails (shared visited set catches cycles); family is Pattern;
**`components != n`** — the N-vs-alternate mismatch rule, which silently
discards the alternate and falls through to the stock fallback.

**`GetStockAlternateProfile(n)`** (:1056-1068): 1 → DeviceGray, 3 → DeviceRGB,
4 → DeviceCMYK, anything else `NOTREACHED()` (unreachable given step 2).

**sRGB detection** (cpdf_iccprofile.cpp:17-20) — exact:
```
stream length == 3144 bytes  AND  bytes[400 .. 400+17) == b"sRGB IEC61966-2.1"
```
(17 bytes, no NUL). Only checked when `expected_components == 3`
(cpdf_iccprofile.cpp:24-46). When it hits, `src_components_ = 3` and
`transform_` stays null, so `IsSupported()` is false but `IsSRGB()` is true.

A profile is **rejected** (`IsSupported() == false`) when the CMS fails to
build a transform, or when **the profile's own channel count disagrees with
`/N`** (cpdf_iccprofile.cpp:41-45). The C++ CMS is lcms; ours is `moxcms`
(DEPS.md). The *rejection conditions* we must match are:
`cmsOpenProfileFromMem` failure (malformed ICC),
`cmsChannelsOf(srcCS) ∉ {1,3,4}`, and the count mismatch
(icc_transform.cpp:56-117). Everything downstream of a rejection is our
concern, and it is fully specified by the ladder above — so a moxcms
disagreement with lcms about *which* profiles are loadable shows up only as a
different choice between "CMS transform" and "alternate/stock space", which
Tier B measures (Open question Q2).

The transform itself: source format is `PT_Lab` + `double` samples when the
profile's space is Lab, else `PT_ANY` + `u8` samples; **destination is always
`TYPE_BGR_8`, intent always `INTENT_PERCEPTUAL`, flags 0**
(icc_transform.cpp:79-101). `Translate` clamps inputs to `[0,255]`, and
**unswaps BGR→RGB on the way out** (:119-143). `TranslateScanline` writes BGR
directly with no swap, which is why the image path does not post-process.

`GetRGB` (:971-985) — the priority ladder:
```
if profile.is_srgb()      { return (buf[0], buf[1], buf[2]) }   // pass-through, unclamped
if profile.is_supported() { return cms_translate(buf[..n]) }
if base_cs                { return base_cs.get_rgb(buf) }
return (0.0, 0.0, 0.0)    // an ENGAGED value, not "no color"
```
That last line matters: a totally broken ICC space renders **black**, it does
not skip the object.

`IsNormal()` (:1014-1025): sRGB → true; supported → the transform's own
`IsNormal` (true for Gray/RGB/CMYK source spaces); else the base's; else false.

**ICC profile caching** (cpdf_docpagedata.cpp:446-479) is two-level: by stream
pointer, and by `(content digest, expected_components)`. The compound key
exists precisely because the profile's behavior depends on `/N`. Our
equivalent keys on `(ObjRef, n)` with a content-hash fallback.

#### 1.14.7 Indexed

`v_Load` (cpdf_indexedcs.cpp:31-89):
1. **Array size must be ≥ 4** → else 0.
2. Element 1 is the base; `HasSameArray` → 0.
3. `base = GetColorSpaceGuarded(elem1, resources = null, visited)` — **through
   the document cache, with null resources**, so `/DefaultRGB` substitution
   does **not** apply to an Indexed base. Null → 0.
4. **Base family must not be Indexed or Pattern** (:50-55, citing ISO 32000-1
   §8.6.6.3) → 0.
5. Snapshot per base component (:57-65):
   `GetDefaultValue(i, &_, &min, &max)`, then **`max -= min`** — after this
   loop the field named `max` holds the **range**, not the max. Critical for
   the `GetRGB` formula below.
6. **`max_index_ = clamp(array[2], 0, 255)`** (:70) — comment: *"ISO 32000-1
   §8.6.6.3 says the maximum value is 255. Clamp the value to this range, so
   that images with an out-of-range hival value can still be loaded."* A
   negative or >255 `/hival` is **clamped, not rejected**.
7. Element 3 must exist → else 0. The lookup table accepts a **String** (raw
   bytes) or a **Stream** (fully decoded); **anything else leaves the table
   empty without failing the load** — every `GetRGB` then returns "no color".
8. Returns **1**.

`GetRGB` (:91-116):
```
index = buf[0] as i32                          // truncation toward zero
if index < 0 || index > max_index_ { return None }
need = (index + 1) * n_base_components         // checked, overflow -> None
if need > lookup_table.len() { return None }
for i in 0..n: comps[i] = min_i + (range_i * table[index*n + i]) / 255.0
return base.get_rgb(comps)
```
where `range_i = orig_max_i - orig_min_i` per step 5.

Indexed uses the **base** `TranslateImageLine` (cpdf_colorspace.cpp:636-661),
where `divisor` is **1 for Indexed** and 255 for everything else (:649) — i.e.
indexed image bytes reach `GetRGB` as raw indices, not normalized.

#### 1.14.8 Separation

`v_Load` (:1101-1131):
1. **`/None`** (:1104-1107): `if array[1] == "None" { is_none_type_ = true;
   return 1 }` — returns immediately with no alternate and no tint transform.
2. **`/All` has NO special handling.** The string `"All"` does not appear in
   this file at all. An `/All` separation loads as an ordinary named colorant
   with its alternate + tint transform. This diverges from the ISO spec (which
   says `/All` paints every colorant) and we match the C++.
3. Element 2 is the alternate; `HasSameArray` → 0; `Load` (**direct, bypassing
   the document cache**) fails → 0; `IsSpecial()` → 0.
4. Tint transform (:1123-1129) — **optional, failure is non-fatal**:
   element 3, **skipped if it is a Name**; `Function::load` failure, or
   `OutputCount() < base.components`, leaves `func_` null. `v_Load` still
   returns 1.
5. Returns **1**.

`GetDefaultValue` (:1092-1099): **value 1.0**, min 0, max 1.0 — the initial
tint is full colorant, unlike the base default of 0.

`GetRGB` (:1133-1156):
```
if is_none_type { return None }                    // /None paints nothing
if func is None {
    if base is None { return None }
    // broadcast the single tint into EVERY alternate component
    return base.get_rgb(vec![buf[0]; base.components])
}
results = vec![0.0; max(func.output_count(), 16)]  // >=16 scratch, see below
n = func.call(&buf[..1], &mut results)?            // 0 outputs -> None
if base is Some { return base.get_rgb(&results) }
return None
```
The `max(.., 16)` scratch floor (:1146) exists because the alternate space may
read past `OutputCount()`; we size the buffer the same way.

#### 1.14.9 DeviceN

`v_Load` (:1171-1199), stricter than Separation:
1. **Element 1 must be an Array** (`/Names`) → else 0.
2. Element 2 is the alternate; missing or `HasSameArray` → 0.
3. `base = Load(elem2)` (direct) **and** `func = Function::load(elem3)`;
   **either null → 0** — the tint transform is **mandatory**, with no
   `IsName()` pre-filter.
4. `base.IsSpecial()` → 0.
5. **`func.output_count() < base.components` → 0** (a hard failure here, where
   Separation merely drops the function).
6. Returns `len(/Names)` — **there is no cap on `/Names` length**. A zero-length
   `/Names` returns 0 and thus fails.

`GetDefaultValue`: value **1.0**, min 0, max 1.0 (:1162-1169).

`GetRGB` (:1201-1215): same 16-element scratch floor; `func.call(&buf[..n])`;
0 outputs → None; then `base.get_rgb(results)` **unguarded** (safe because
`v_Load` guarantees a base).

#### 1.14.10 Pattern

`kMaxPatternColorComps = 16` (cpdf_colorspace.h:34). The stock `/Pattern`
space reports **1** component (cpdf_patterncs.cpp:20-22).

`v_Load` (cpdf_patterncs.cpp:24-48):
```
base_obj = array[1]
if HasSameArray(base_obj) { return 0 }
base = GetColorSpaceGuarded(base_obj, resources = null, visited)
if base is None { return 1 }                       // SUCCESS with no base
if base.family == Pattern { return 0 }             // no nested patterns
if base.components > 16 { return 0 }
return base.components + 1
```
Line 36 is the notable one: **a missing or unloadable underlying colorspace is
not an error** — it degrades to a 1-component pattern space with no base
(the uncolored-pattern case).

`GetRGB` is `NOTREACHED()` (:50-53). The pattern-safe entry is
`GetPatternRGB(pattern_value)` (:59-66): no base → None; else
`base.get_rgb(value.comps)` where `comps` is the **full 16-float array**
regardless of the base's component count. In Rust the `Pattern` variant has no
`to_rgb` at all — it is a separate function on the pattern value, so the
`NOTREACHED()` is unrepresentable (D10).

#### 1.14.11 Default colors and the colorstate

`CreateBufAndSetDefaultColor()` (:603-614): `DCHECK(family != Pattern)`; a
`components_`-long buffer filled from `GetDefaultValue`. Resulting initial
colors: DeviceGray `[0]`, DeviceRGB `[0,0,0]`, DeviceCMYK `[0,0,0,0]`,
CalGray `[0]`, CalRGB `[0,0,0]`, Lab `[0, clamp(0,amin,amax),
clamp(0,bmin,bmax)]`, ICCBased `[0…]`, Indexed `[0]`, **Separation `[1]`**,
**DeviceN `[1,…,1]`**.

`CPDF_ColorState::SetColor` (cpdf_colorstate.cpp:113-131) — the operand rule
already referenced in §1.3:
```
if colorspace given { color.set_colorspace(cs) }        // RESETS components to defaults
else if color.is_null() { color.set_colorspace(DeviceGray) }
if color.components() > values.len() { return None }    // too few operands: NO CHANGE AT ALL
if !color.is_pattern() { color.set_values(values) }
return color.color_ref().unwrap_or(0xFFFFFFFF)
```
`0xFFFFFFFF` is the "no representable color" sentinel. `SetPattern`
(:133-144) uses the same sentinel except that an **uncolored** pattern whose
`colored()` flag is set falls back to **`0x00BFBFBF`** (mid grey, BGR
191,191,191).

`GetColorRef` (cpdf_color.cpp:103-114): `clamp` each channel to [0,1] then
`FXSYS_roundf(v * 255)` — **round-to-nearest**, in contrast to the truncating
`(i32)(v*255)` used by `TranslateImageLine`. Both must be reproduced in their
respective paths.

`SetValueForPattern` (:52-66) silently returns if `values.len() > 16`.

#### 1.14.12 `TranslateImageLine` — the bulk paths, and where they diverge

The generic base implementation (cpdf_colorspace.cpp:636-661), used by
Separation, DeviceN, Indexed, and Pattern:
```
CHECK(!trans_mask)
divisor = (family != Indexed) ? 255 : 1
for each of `pixels` pixels:
    for j in 0..components: src[j] = src_byte++ as f32 / divisor
    rgb = get_rgb(src).unwrap_or((0,0,0))     // failure renders BLACK
    *dest++ = (i32)(rgb.blue  * 255)
    *dest++ = (i32)(rgb.green * 255)
    *dest++ = (i32)(rgb.red   * 255)
```
Output is **B, G, R**, truncated. Divergences from the scalar path, by class:

| Class | Bulk path | Same as `GetRGB`? |
|---|---|---|
| base (Sep/DeviceN/Indexed/Pattern) | per-pixel `GetRGB`, BGR out, divisor 1 for Indexed | yes (mod byte order + truncation) |
| DeviceGray | replicate the byte, **RGB** out | yes |
| DeviceRGB | `ReverseRGB` (r↔b) | yes |
| DeviceCMYK, `trans_mask` | `((255-c)*(255-k))/255` integer | **a third formula unreachable from `GetRGB`** |
| DeviceCMYK, std conv | `255 - min(255, c+k)` with **c→blue, y→red swapped** | same formula, swapped channel mapping |
| DeviceCMYK, default | `AdobeCmykToStandardRgb` u8 direct, then r↔b swap | yes (skips the float round trip) |
| CalGray | replicate the byte | yes (both identity) |
| **CalRGB** | `ReverseRGB` only | **NO — gamma/matrix/whitepoint ignored** |
| **Lab** | `GetRGB` with `(L*100/255, a-128, b-128)`, BGR out | same math, **different input scaling** |
| ICCBased | sRGB → `ReverseRGB`; unsupported → delegate to base with `trans_mask=false`; else CMS scanline (BGR8, no swap) | yes |

`bTransMask` is honored **only** by DeviceCMYK; every other implementation
starts with a hard `CHECK(!bTransMask)`. There is no `GetRGBArray`; the only
bulk API is `TranslateImageLine`.

### 1.15 PDF functions

#### 1.15.1 Dispatch and the common `Init`

`IntegerToFunctionType` (cpdf_function.cpp:28-38) accepts **only 0, 2, 3, 4**;
anything else (including **type 1**, which PDFium does not implement) →
invalid → null.

`Load(obj, visited)` (:50-87):
1. Null → null. **`visited` contains the object → null** (:57-59), with a
   scoped insertion holding only the *current ancestor chain* — so a function
   reachable twice through disjoint branches loads fine; only true cycles fail.
2. `/FunctionType` read from the stream's dict or the dict itself; an object
   that is neither leaves the type at −1 → null.
3. Dispatch, then `Init(obj, visited)`; failure → null.

Object-type acceptance is per subclass, not at dispatch:
- **Type 0 requires a Stream** (cpdf_sampledfunc.cpp:48-51).
- **Type 4 assumes a Stream** (cpdf_psfunc.cpp:18).
- **Types 2 and 3 accept a dict or a stream** (cpdf_expintfunc.cpp:25,
  cpdf_stitchfunc.cpp:32).

`Init` (cpdf_function.cpp:93-138):
- **`/Domain` is required for every type**; missing → false.
- `inputs_ = len(Domain) / 2` (integer division — an odd-length Domain
  truncates); **`inputs_ == 0` → false** (this is what rejects a 1-element or
  empty Domain).
- `domains_` = the first `2*inputs_` floats; `GetFloatAt` returns 0 out of
  range and coerces non-numeric objects through `GetNumber()`.
- `/Range` optional: `outputs_ = len(Range)/2`, else 0.
- **`/Range` is required only for types 0 and 4** (:114-120).
- After `v_Init`, if `ranges_` is non-empty and `v_Init` **grew** `outputs_`,
  `ranges_` is resized to `2*outputs_` with **zero-filled** new slots
  (:127-136) — this is what degrades multi-input type 2 (§1.15.3).

**There is no cap on `inputs_` or `outputs_` anywhere in the function
subsystem.** The only bounds are overflow checks. Caller-side caps exist:
`kMaxOutputs = 16` in `cpdf_docrenderdata.cpp:32` (transfer functions reject
functions above it) and the `max(OutputCount(), 16)` scratch floors in
colorspace code. We add `Limits::max_function_outputs` (D11) because a
`Vec<f32>` sized from untrusted data is a DoS vector Rust must not accept
silently.

`Call` (:140-174):
- `inputs.len() != inputs_` → **None**.
- Per input: `domain_lo > domain_hi` → **None**; else `clamp(x, lo, hi)`.
- `v_Call` failure → None.
- **Empty `ranges_` skips output clamping entirely**; returns `outputs_`.
- Per output: `range_lo > range_hi` → **None**; else clamp. Note the results
  buffer is written by `v_Call` *before* range validation, and `Call` never
  checks `results.len()` — **the caller must size it to `outputs_`**. In Rust
  the signature enforces this.

`Interpolate` (:177-184), used everywhere:
```
divisor = xmax - xmin
return ymin + (if divisor != 0 { (x - xmin) * (ymax - ymin) / divisor } else { 0 })
```
A degenerate domain yields `ymin`, not NaN.

#### 1.15.2 Type 0 — sampled

**`/BitsPerSample` legal values, exact: 1, 2, 4, 8, 12, 16, 24, 32**
(`IsValidBitsPerSample`, cpdf_sampledfunc.cpp:25-39). Anything else fails.

`v_Init` (:47-109):
1. Must be a Stream.
2. `/Size` required and non-empty.
3. `/BitsPerSample` validated as above.
4. Per input `i`: `size = Size[i]`; **`size <= 0` → false**;
   `nTotalSampleBits *= size`. `/Encode` defaults when absent (:79-83):
   `encode_min = 0`, **`encode_max = (size == 1) ? 1 : size - 1`** — the
   `size == 1` case avoids a zero-width encode range. When `/Encode` is
   present it is read positionally with **no length check**, so a short array
   silently yields zeros.
5. **Size cap** (:85-88): `total_bytes = (bps * outputs * Π sizes + 7) / 8`
   in a checked `u32`; overflow or zero → false. The effective cap is
   `bps * outputs * Π sizes < 2^32` bits.
6. `sample_max_ = 0xFFFFFFFF >> (32 - bps)` = `2^bps - 1` (:90).
7. Stream loaded filtered; **`total_bytes > stream.len()` → false** (:91-95).
8. `/Decode` per output, defaulting to **`ranges_[2i], ranges_[2i+1]`**
   (:97-107) — Range is guaranteed present for type 0.

**`/Order` is never read.** Only linear (Order 1) sampling is implemented;
Order 3 cubic spline is silently ignored.

`v_Call` (:111-192) — the interpolation, exactly:
- `blocksize[0] = 1`, `blocksize[i] = blocksize[i-1] * sizes[i-1]` — **the
  first input varies fastest**.
- `encoded_input[i] = Interpolate(x_i, domain_lo, domain_hi, encode_min, encode_max)`.
- **Index clamping** (:128): `index[i] = clamp((u32)encoded_input[i], 0, sizes[i]-1)`.
  The cast to `u32` happens **before** the clamp, so a negative
  `encoded_input` wraps to a huge unsigned value and clamps to `sizes-1`,
  **the top cell, not 0**. Rust's `as u32` saturates negatives to 0 — a
  faithful port must reproduce the wrap explicitly (D12).
- `pos += index[i] * blocksize[i]`; `bits_to_skip = pos * outputs * bps`,
  negative or overflowing → false.
- Per output `i`: read `sample = bits.get(bps)` sequentially (outputs are
  packed consecutively at the base cell); `encoded = sample`; then per input `j`:
  - `index[j] == sizes[j]-1` (top edge): normally nothing — **except** when
    `index[j] == 0` too (the `sizes[j] == 1` degenerate axis), where it does
    **`encoded = encoded_input[j] * sample`** (:165-168). That *multiplies*
    rather than interpolating, and **overwrites** `encoded`, discarding earlier
    axes' contributions. A genuine quirk; port it.
  - otherwise: read the neighbour sample at
    `((blocksize[j] + pos) * outputs + i) * bps` with a **fresh** bit stream,
    then `encoded += (encoded_input[j] - index[j]) * (sample2 - sample)`.
  So it is a **sum of per-axis gradients against the base sample**, not a true
  tensor-product multilinear interpolation. Exact for 1-D. The fractional
  weight is **not** clamped to [0,1].
- `results[i] = Interpolate(encoded, 0, sample_max_, decode_min, decode_max)`.

`CFX_BitStream::GetBits` returns **0** past the end rather than failing;
`SkipBits` is unchecked. Our bit reader mirrors both.

#### 1.15.3 Type 2 — exponential

`v_Init` (cpdf_expintfunc.cpp:24-59):
- **`/N` is required and must be a Number** → else false. `exponent_ = N`.
  **No constraint on its value** — negative, zero, and fractional all accepted.
- `/C0`: if present **and `outputs_ == 0`** (no `/Range`), `outputs_ = len(C0)`.
  Then `if outputs_ == 0 { outputs_ = 1 }`. So Range wins over C0; the default
  output count is 1.
- `begin_values_` and `end_values_` are each `2*outputs_` long (deliberately
  oversized) and filled positionally for `i < outputs_` with **C0 default 0.0
  and C1 default 1.0**; short arrays yield 0 for the missing entries.
- `orig_outputs_ = outputs_`, then **`outputs_ = outputs_ * inputs_`** (:57).

That last step is the subtle degradation: for a multi-input type 2 the
reported output count becomes `orig_outputs * inputs`, and `Init` then
zero-extends `ranges_`. Since the extension is zeros, `Call` clamps every
extra output to `[0,0]` — **all outputs beyond the original Range entries
become 0**. Type 2 is only meaningful with 1 input; this is the failure mode
when it is not.

`v_Call` (:61-71), exact:
```
results[i * orig_outputs_ + j] =
    C0[j] + powf(inputs[i], N) * (C1[j] - C0[j])
```
Edge cases follow C `powf` unguarded:
- **N < 0 with input 0** → `+inf`. If a `/Range` exists, `Call` clamps it
  finite; without one, the infinity escapes to the caller.
- **Non-integer N with a negative input** → **NaN**, which propagates through
  `clamp` unchanged.
- `N == 0` → 1.0 for every input including 0.
- Inputs are already domain-clamped by `Call`; type 2 does **not** normalize
  the domain onto [0,1], so a `/Domain` other than `[0 1]` feeds raw values
  into `powf`.

#### 1.15.4 Type 3 — stitching

`kRequiredNumInputs = 1` (cpdf_stitchfunc.cpp:19).

`v_Init` (:27-119):
- **`inputs_ != 1` → false.**
- **`/Functions`, `/Bounds`, `/Encode` are all three required**; any missing →
  false. An *empty* `/Bounds` is legal (and required when `nSubs == 1`).
- `nSubs = len(Functions)`; **0 → false**.
- Size checks are **relaxed to allow longer arrays**: `len(Bounds) < nSubs-1`
  → false; `len(Encode) < 2*nSubs` → false. **Mismatched Encode length fails
  only if too short**; extras are ignored.
- Per sub-function: **`sub == self` → false** (self-reference); `Load` failure
  → false; `sub.input_count() != 1` → false; `sub.output_count() == 0` →
  false; **all sub-functions must agree on output count** → false otherwise.
  `outputs_` is taken from the children, **overriding whatever `/Range`
  implied** (and triggering the `ranges_` resize).
- **Bounds vector** (:110-115), length `nSubs + 1`:
  `[domain_lo, Bounds[0], …, Bounds[nSubs-2], domain_hi]`.
- `encode_ = Encode[0 .. 2*nSubs]`.

**There is no monotonicity check on Bounds and no check that they lie within
Domain.** Out-of-order and out-of-domain bounds are accepted.

`v_Call` (:121-135):
```
for i in 0 .. sub_functions.len()-1 { if input < bounds[i+1] { break } }
input = Interpolate(input, bounds[i], bounds[i+1], encode[2i], encode[2i+1])
return sub_functions[i].call(&[input], results)
```
The scan is bounded so `i` maxes at `nSubs-1`, giving half-open
`[bounds[i], bounds[i+1])` selection with the last interval closed on the
right. Input clamping to Domain already happened in `Call`, and
`bounds[0]/bounds[nSubs]` are the domain endpoints by construction, so the
input is always bracketed. With **out-of-order bounds** the scan simply stops
early and `Interpolate` runs on an inverted or degenerate interval — a
degenerate one yields `encode[2i]` exactly, an inverted one extrapolates
outside the Encode range. **No error is raised**; the sub-function's own
domain clamp absorbs it.

**`Function::load` has no depth limit for type-3 nesting** — only the cycle
set. A deeply-but-acyclically nested chain would blow a recursive loader's
stack. We add an explicit depth cap (D11).

#### 1.15.5 Type 4 — the PostScript calculator

**Complete operator list** (`kPsOpNames`, cpdf_psengine.cpp:31-74) — 42 named
operators, **sorted alphabetically** (the lookup is a binary search). The name
field is `char[9]`, so `bitshift` (8 chars) is the longest possible spelling:

`abs add and atan bitshift ceiling copy cos cvi cvr div dup eq exch exp false
floor ge gt idiv if ifelse index le ln log lt mod mul ne neg not or pop roll
round sin sqrt sub true truncate xor`

Plus two unnamed internal ops: `PSOP_PROC` (a `{}` block) and `PSOP_CONST`.

**Unrecognized tokens become numeric constants, not errors** (`AddOperator`,
:190-198): `StringToFloat(word)` on a non-numeric word yields `0.0` and pushes
`PSOP_CONST(0.0)`. The unit test asserts this for `"invalid"`.

**Stack**: `kPSEngineStackSize = 100` (cpdf_psengine.h:125).
- **Push overflow is silently dropped** (:204-208): values beyond 100 vanish,
  no error, no flag.
- **Pop underflow yields 0** (:210-212).
- `PopInt()` = `saturated_cast<int>(Pop())` (:214-216) — **saturating**,
  NaN → 0.

**Parse and nesting** (:218-221, :125-150):
- The program **must** begin with `{`; top level starts at depth 0.
- **`kMaxDepth = 128`** (cpdf_psengine.h:103), checked as `depth > kMaxDepth`,
  so depth 128 is allowed and 129 rejected — **up to 129 nesting levels**
  counting the top-level proc.
- An **empty word (EOF) → false**: an unterminated `{` is a parse failure.
- `}` closes; `{` recurses at `depth+1`; anything else is an operator token.
- Tokenization is `CPDF_SimpleParser::GetWord`: whitespace and `%` comments to
  end of line; `{` and `}` are PDF delimiters and come back as single-char
  words.
- Execution recursion has **no independent limit** — it is bounded only by the
  parse depth, i.e. ≤ 129 frames.

**Procedure semantics** (`CPDF_PSProc::Execute`, :152-184):
- `PSOP_PROC` is **skipped** when encountered — procedures are inert until a
  following `if`/`ifelse` consumes them.
- **`if`** (:164-171): requires `i > 0` **and** `operators_[i-1]` is a
  `PSOP_PROC`, else the whole proc **aborts** (`return false`). The condition
  is popped *after* the structural check; the procedure is located
  **lexically by position**, never popped from the stack.
- **`ifelse`** (:172-178): requires `i >= 2` and both preceding operators are
  procs, else abort. `offset = PopInt() ? 2 : 1` — **true selects `i-2` (the
  first proc), false selects `i-1` (the second)**. The nested proc's own
  return value is **discarded**, so a structural failure inside a branch does
  not abort the parent.
- `Execute()`'s bool is **ignored** by `CPDF_PSFunc::v_Call`, so a malformed
  if/ifelse degrades to "whatever was on the stack" rather than an error.
- `DoOperator` **always returns true**; there is no error channel at all.

**Operator semantics** (:223-462). Note which ops pop `d1` first (commutative)
versus `d2` first (order-sensitive):

| Op | Behavior |
|---|---|
| `add` | `d1=pop; d2=pop; push(d1+d2)` — float |
| `sub` | `d2=pop; d1=pop; push(d1-d2)` — order-sensitive |
| `mul` | `d1=pop; d2=pop; push(d1*d2)` |
| `div` | `d2=pop; d1=pop; push(d2 != 0 ? d1/d2 : 0)` — **÷0 → 0, not inf** |
| `idiv` | **int coercion**; `i2 != 0 ? i1/i2 : 0`; `INT_MIN / -1` overflow → 0 |
| `mod` | int coercion; **mod 0 → 0**; overflow → 0; C sign semantics (follows the dividend) |
| `neg` `abs` `ceiling` `floor` | float `-x`, `fabs`, `ceil`, `floor` |
| `round` | `RoundHalfUp(x)` — see below |
| `truncate` | `push(PopInt())` — a **saturating int round-trip**, not `trunc()` |
| `sqrt` | `sqrt(x)` — **negative → NaN**, unguarded |
| `sin` `cos` | `sin(x * PI / 180)` — **degrees** |
| `atan` | see below |
| `exp` | `d2=pop; d1=pop; push(powf(d1, d2))` — base then exponent |
| `ln` `log` | `log(x)` / `log10(x)` — **`ln(0)` → −inf, `ln(neg)` → NaN**, unguarded |
| `cvi` | `push(PopInt())` — **identical to `truncate`** |
| `cvr` | **a no-op** — no stack change |
| `eq ne gt ge lt le` | pop `d2` then `d1`, compare as **floats**, push 1.0/0.0 |
| `and` `or` `xor` | **bitwise on ints** |
| `not` | `push(!PopInt())` — **logical, yields 0/1**, not bitwise `~` |
| `bitshift` | see below |
| `true` `false` | push 1.0 / 0.0 |
| `pop` `exch` `dup` | discard / swap top two / duplicate top |
| `copy` `index` `roll` | see below |

**Booleans are plain floats** 1.0/0.0, and truthiness goes through `PopInt()`,
which **saturates** — so `0.5` truncates to 0 and reads as **false**.

**`RoundHalfUp`** (:78-86), verbatim:
```
if x.is_nan() { return 0.0 }
if x > f32::MAX - 0.5 { return f32::MAX }
return floor(x + 0.5)
```
Comment: *"half-way numbers always rounded up. Example: −5.5 rounds to −5."*
Both the NaN→0 and the max-float saturation are asserted by tests.

**`atan`** (:308-316), verbatim:
```
d2 = pop; d1 = pop
r = atan2(d1, d2) * 180.0 / PI
if r < 0 { r += 360 }
push(r)
```
So `num den atan`, with the denominator popped first, result in **degrees
normalized to [0, 360)**. `FXSYS_PI = 3.1415926535897932384626433832795`.

**`bitshift`** (:385-397):
```
shift = PopInt(); result = PopInt()
if shift > 0 { result <<= shift }
else { result >>= (-shift).unwrap_or(0) }     // INT_MIN-negation guard
push(result.unwrap_or(0))
```
Positive shifts left, negative shifts right (arithmetic, signed). Every
overflow or invalid-shift case collapses to **0**.

**`copy`** (:418-429): `n = PopInt()`; rejects `n < 0`, `stack_count + n > 100`,
or `n > stack_count` → **silent no-op** (with `n` already consumed). `n == 0`
is a legal no-op. Otherwise duplicates the top `n` entries.

**`index`** (:430-437): `n = PopInt()`; `n < 0 || n >= stack_count` → **silent
no-op that pushes nothing**, so the stack net-shrinks by one. Else push
`stack[stack_count - n - 1]` (0 = top).

**`roll`** (:438-457):
```
j = PopInt(); n = PopInt()
if j == 0 || n == 0 || stack_count == 0 { no-op }
if n < 0 || n > stack_count { no-op }
j %= n; if j > 0 { j -= n }
rotate(stack[stack_count-n .. stack_count], by -j)
```
`j` is normalized into `(-n, 0]`, so positive `j` rolls toward the top
(standard PostScript).

**`CPDF_PSFunc::v_Call`** (cpdf_psfunc.cpp:23-37): reset the engine, push all
inputs in order, execute, then **`if stack_size < outputs_ { return false }`**
and pop into `results[outputs_ - i - 1]` — **outputs come off the top of the
stack in reverse**, so the topmost value is the *last* output. Residue below
the outputs is ignored. The C++ engine is a `mutable` member reused across
calls, making `v_Call` non-reentrant; ours is constructed per call or held
behind `&mut` (D13).

**Every PS error path is silent.** `DoOperator` always succeeds, push overflow
and pop underflow are non-signalling, and the executor's bool is discarded.
A `Result`-returning Rust engine would reject documents PDFium renders — so
our engine's `eval` returns `Ok` in all of these cases and records a
`Diagnostic` instead.

### 1.16 Patterns and shadings

#### 1.16.1 The pattern base — matrix composition and dispatch

`enum PatternType { kTiling = 1, kShading = 2 }` (cpdf_pattern.h:23, marked
*"Values used in PDFs. Do not change."*).

**The matrix rule** (cpdf_pattern.cpp:34-37):
```
pattern_to_form = pattern_dict["/Matrix"] * parent_matrix
```
The pattern's own `/Matrix` is on the **left**. `CFX_Matrix::operator*` is
row-vector convention (`A * B` = "apply A, then B"), matching `kurbo::Affine`'s
`B * A` — so in Rust this is `parent_matrix * pattern_matrix` (D14; the
brief's formulas are written in C++ order, the code inverts the operands).
`/Matrix` **must have exactly 6 elements** or it reads as identity
(cpdf_array.cpp:78-85).

Downstream chains, all in C++ order:
- tiling render: `pattern_to_form * obj_to_device` (cpdf_rendertiling.cpp:90-91)
- shading pattern: `pattern_to_form * obj_to_device` (cpdf_renderstatus.cpp:1203)
- `sh` object: `ctm * content_to_user` at parse, then `matrix * obj_to_device`
  at render (cpdf_renderstatus.cpp:1218)

`CPDF_DocPageData::GetPattern` (cpdf_docpagedata.cpp:383-408) dispatches on
`/PatternType`: **1 → tiling, 2 → shading, anything else → null** (no pattern
object at all). Patterns are **cached per pattern object**, so the same object
shares one instance — **and therefore one `parent_matrix`, taken from the
first requester**. That is observable when the same pattern is used from two
different form nesting levels; we key our cache on
`(ObjRef, parent_matrix_bits)` to avoid the aliasing (D15).

`GetShading` (:410-424) shares the same cache but always constructs with
`bShading = true` and **no `/PatternType` check**.

**Ctor side effect**: the tiling ctor always calls `SetPatternToFormMatrix()`
(cpdf_tilingpattern.cpp:21-28), but the shading ctor calls it **only when
`!bShading`** (cpdf_shadingpattern.cpp:40-42) — so for `sh`-operator shadings
`pattern_to_form` stays identity, correctly ignoring any `/Matrix` on a bare
shading dict.

#### 1.16.2 Shading types and validation

`ShadingType` (cpdf_shadingpattern.h:19-31): `kInvalidShading = 0`,
1 FunctionBased, 2 Axial, 3 Radial, 4 FreeFormGouraud, 5 LatticeGouraud,
6 CoonsPatch, 7 TensorProductPatch, `kMaxShading = 8`.
`ToShadingType(n)` accepts **1..7** and maps everything else to invalid.
`IsMeshShading()` is true for **4, 5, 6, 7**.

`IsShadingObject()` distinguishes the two entry points: **true** when built via
`GetShading` (the `sh` operator's `/Shading` resource — `pattern_obj()` *is*
the shading object); **false** for a `/PatternType 2` pattern, where the
shading object is `pattern_obj().dict["/Shading"]` (cpdf_shadingpattern.cpp:95-98).
`sh` refuses anything where `IsShadingObject()` is false.

`Load()` (cpdf_shadingpattern.cpp:51-93):
1. **Memoized**: `shading_type_ != kInvalidShading` → already loaded, true.
2. No dict on the shading object → false.
3. **`/Function`** (:63-75):
   - Array form: **`functions_.resize(min(len, 4))`** — the **max function
     count is the literal 4**, not a named constant. Individual `Function::load`
     failures leave **null entries**, rejected later by `ValidateFunctions`.
   - Single form: exactly one entry, possibly null.
   - Absent: empty. **Not an error at this stage.**
4. **`/ColorSpace` is required**, and **may not be a Pattern space** (:76-89,
   citing ISO 32000-1 table 78) → false otherwise.
5. `shading_type_ = ToShadingType(dict["/ShadingType"])`.
6. `Validate()`.

`Validate()` (:100-160):
- **A** invalid type → false.
- **B** **mesh types (4-7) require the shading object to be a Stream** (:105-108).
  Types 1-3 may be a dict or a stream.
- **C** colorspace family:
  - types 1, 2, 3: **Indexed is always forbidden**.
  - types 4-7: **Indexed forbidden only when functions are present**.
- **D** function count/arity, with `N = cs.ComponentCount()`:

| Type | Requirement |
|---|---|
| 1 | `ValidateFunctions(1, 2, N)` **or** `ValidateFunctions(N, 2, 1)` — one 2→N function, or N 2→1 functions. **Mandatory.** |
| 2, 3 | `ValidateFunctions(1, 1, N)` **or** `ValidateFunctions(N, 1, 1)`. **Mandatory.** |
| 4-7 | `functions_.empty()` **or** either of the above two shapes. **Optional.** |

Because the array parse caps at 4, a colorspace with `N > 4` can never satisfy
the "N 1-to-1 functions" branch through an array — only the single-function form.

`ValidateFunctions(nExpected, nIn, nOut)` (:162-185): count must match
exactly; **any null entry → false**; every function's `InputCount()` and
`OutputCount()` must match exactly; the summed output count is
overflow-checked.

**When a shading is "unsupported" and silently skipped:**
1. `Load()` false → `sh` builds nothing; a pattern fill draws nothing.
2. No colorspace at draw time.
3. `kInvalidShading` reaching the draw dispatch.
4. **A mesh type whose shading object is a Dictionary** — skipped with the
   comment *"we do not handle the case of dictionary at the moment"*
   (cpdf_rendershading.cpp:1075-1077, 1086-1088, 1098-1100). Redundant with
   Validate step B, but present.
5. Axial/radial/function-based with a **missing `/Coords`**.
6. `GetValidatedOutputsCount(funcs, cs) == 0` — all functions report 0 outputs.

#### 1.16.3 `/Extend`, `/Background`, `/BBox`, `/AntiAlias`

**`/Extend`** (axial + radial only, cpdf_rendershading.cpp:133-135, 203-205):
```
start = array && array.GetBooleanAt(0, false)
end   = array && array.GetBooleanAt(1, false)
```
`GetBooleanAt(i, default)` returns the default for an out-of-range index **and
for a non-Boolean element** (cpdf_array.cpp:139-145). So `/Extend [0 1]`
(integers, not booleans) yields **false, false**, not false/true. Missing
`/Extend` → both false.

**`/Background`** (cpdf_rendershading.cpp:1021-1035):
- **Honored only when `!IsShadingObject()`** — i.e. only for `/PatternType 2`
  shading patterns, **never for the `sh` operator** (which matches the spec).
- Requires `len(array) >= cs.ComponentCount()`; otherwise silently ignored.
- Alpha forced to **255**, independent of the object's fill alpha.
- RGB via `GetRGBOrZerosOnError`, **truncated** (`(i32)(v*255)`) — note the
  shading-steps LUT in the same file uses `roundf`.
- **A background that encodes to exactly ARGB 0 is not applied** (`if
  (background != 0)`, :1053).

**`/BBox`** (:1036-1040): applied for **all** shading types and both entry
points; transformed by the pattern→device matrix, outer-rect-ized, and
intersected with the incoming clip. `GetRectFor` **requires exactly 4
elements**, else an all-zero rect — so a malformed `/BBox` collapses the clip
to the origin and the shading becomes invisible. Element order is
`[left, bottom, right, top]`.

**`/AntiAlias` is never read anywhere in `core/`.** Path smoothing for
Coons/tensor patches comes from a render option instead.

#### 1.16.4 The shading colour LUT

`kShadingSteps = 256` (cpdf_rendershading.cpp:45).

`/Domain` for axial and radial (:126-132, :196-202): two elements, **default
`[0, 1]`**; a shorter array reads 0 for the missing entries. **No validation
that `t_min < t_max`.**

`GetShadingSteps` (:65-100):
```
results_count = GetValidatedOutputsCount(funcs, cs)   // 0 -> abort
diff = t_max - t_min
for i in 0..256:
    input = t_min + diff * i / 256        // NOTE: /256, not /255
    // multiple functions CONCATENATE their outputs into one component vector
    for f in funcs: n = f.call(&[input], &mut span); span = &mut span[n..]
    rgb = cs.to_rgb_or_zeros(&result_array)
    steps[i] = argb(alpha, roundf(r*255), roundf(g*255), roundf(b*255))
```
So `t_max` itself is **never sampled** — the last entry is
`t_min + 255/256 * diff`. `GetValidatedOutputsCount` (:58-63) sizes the buffer
to `max(function_outputs, cs.components)`, leaving trailing zeros when the
functions produce fewer values than the colorspace consumes. `alpha` is
uniform across all 256 entries.

**The systematic off-by-one:** the LUT is *sampled* at `i/256` but *indexed*
at `s*255`. Both halves are reproduced verbatim (D16) — this is a
pixel-visible detail on smooth gradients.

#### 1.16.5 Axial (type 2)

`/Coords` is **4 elements** `[x0 y0 x1 y1]` (cpdf_rendershading.cpp:122-125);
a missing array aborts, a **short array reads zeros with no length check**.

```
x_span = x1 - x0;  y_span = y1 - y0
axis_len_square = x_span² + y_span²
// per device pixel, with matrix = inverse(object_to_bitmap):
pos = matrix * (col, row)
scale = ((pos.x - x0) * x_span + (pos.y - y0) * y_span) / axis_len_square
index = (i32)(scale * 255)
if index < 0    { if !start_extend { skip pixel } ; index = 0 }
if index >= 256 { if !end_extend   { skip pixel } ; index = 255 }
```
`scale` is the normalized parametric position along the axis. **Extend
semantics: a non-extended out-of-range pixel is left completely untouched**
(transparent, background shows through), not clamped. Note the truncation
asymmetry: `scale == 1.0` gives index 255, which is in range, so
`end_extend` is not consulted for exactly 1.0 — it only fires above
`255/256 ≈ 1.0039`.

**`axis_len_square == 0` (a degenerate axis) is not guarded** → division by
zero → ±inf or NaN `scale`, whose truncation is UB in C++ and lands in one of
the out-of-range branches. We treat a zero-length axis as "every pixel is
out of range at the start end" and record a `Diagnostic` (D17).

#### 1.16.6 Radial (type 3)

`/Coords` is **6 elements** `[x0 y0 r0 x1 y1 r1]` (:190-195).

```
dx = x1-x0;  dy = y1-y0;  dr = r1-r0
a = dx² + dy² - dr²
a_is_zero = is_float_zero(a)
decreasing = dr < 0 && (i32)hypot(dx, dy) < -dr    // NOTE the int truncation of hypot
```
Per pixel, `pos = inverse(obj_to_bitmap) * (col, row)`:
```
pdx = pos.x - x0;  pdy = pos.y - y0
b = -2 * (pdx*dx + pdy*dy + r0*dr)
c = pdx² + pdy² - r0²

if is_float_zero(b)      { s = sqrt(-c / a) }          // branch 1: no guards at all
else if a_is_zero        { s = -c / b }                // branch 2: linear
else {
    disc = b² - 4ac
    if disc < 0 { skip pixel }
    root = sqrt(disc)
    s1 = (-b - root) / (2a);  s2 = (-b + root) / (2a)
    if a <= 0 { swap(s1, s2) }                          // ensures s1 <= s2
    if decreasing { s = (s1 >= 0  || start_extend) ? s1 : s2 }
    else          { s = (s2 <= 1.0 || end_extend)  ? s2 : s1 }
    if r0 + s*dr < 0 { skip pixel }                     // negative interpolated radius
}
index = (i32)(s * 255)     // then the same extend clamping as axial
```
Branch 1 has **no discriminant check, no root selection, no radius check, and
no `a == 0` guard** — `-c/a < 0` yields NaN. Branch 2 likewise has no radius
check. Both are ported exactly.

#### 1.16.7 Function-based (type 1)

```
/Domain is 4 elements ordered [xmin xmax ymin ymax]   // NOT [xmin ymin xmax ymax]
default [0 1 0 1]
/Matrix on the SHADING dict (distinct from the pattern's /Matrix), default identity
matrix = inverse(object_to_bitmap) * inverse(domain_to_target)
```
Per device pixel: `pos = matrix * (col, row)`; **outside the inclusive domain
box → pixel untouched**; else evaluate the 2-in-N-out function(s) directly
(**no LUT**), concatenating multi-function outputs, and write
`argb(alpha, (i32)(r*255), (i32)(g*255), (i32)(b*255))` — **truncation here,
unlike the axial/radial LUT's rounding**. There is no `/Extend` for type 1.

#### 1.16.8 Mesh streams

`kMaxComponents = 8` (cpdf_meshstream.h:76).

**Legal bit widths, exact:**
- `/BitsPerCoordinate` ∈ **{1, 2, 4, 8, 12, 16, 24, 32}**
- `/BitsPerComponent` ∈ **{1, 2, 4, 8, 12, 16}**
- `/BitsPerFlag` ∈ **{2, 4, 8}**

`/BitsPerFlag` is validated for types **4, 6, 7 only** — **type 5 (lattice)
has no flags** and its `/BitsPerFlag` is never checked.

`Load()` (cpdf_meshstream.cpp:114-159):
1. Stream loaded filtered into a bit reader.
2. Bit widths read and validated as above.
3. **`cs.ComponentCount() > 8` → false.**
4. **The function override:** `components_ = funcs.is_empty() ? n_cs : 1`
   (:140) — when any function is present, exactly **one** parametric value `t`
   is read per colour and the functions map it.
5. **`/Decode` length must be EXACTLY `4 + 2 * components_`** (`!=`, :142).
   With functions that is exactly **6**. Order:
   `[xmin xmax ymin ymax c0min c0max …]`.
6. `coord_max_ = (coord_bits == 32) ? 0xFFFFFFFF : (1 << coord_bits) - 1`;
   `component_max_ = (1 << component_bits) - 1`.

**Bit reading** (`CFX_BitStream`): `GetBits(n)` returns **0** past the end
rather than failing; `SkipBits` is unchecked; `ByteAlign` rounds **up** to the
next multiple of 8; packing is **MSB-first big-endian**.

Capacity predicates (:173-183) — note the odd formulations, which we preserve:
```
can_read_flag   = remaining >= flag_bits
can_read_coords = remaining / 2 >= coord_bits
can_read_color  = remaining / component_bits >= components   // divides by component_bits!
```
`can_read_color` **divides by `component_bits_`**, which is zero only for
non-mesh types where the validation is skipped — unreachable in practice, but
our version guards it.

**`ReadFlag()`** (:185-188): `GetBits(flag_bits) & 0x03` — **masked to 2 bits**,
so flags are always 0..3 regardless of `/BitsPerFlag`.

**`ReadCoords()`** (:190-206) — the decode interpolation:
```
v = min + raw * (max - min) / coord_max
```
with the **32-bit case forcing `f64` division** to avoid `u32` precision loss,
and the `< 32` case using `f32`. x is fully read before y.

**`ReadColor()`** (:208-221):
```
c_i = color_min[i] + raw * (color_max[i] - color_min[i]) / component_max
if funcs.is_empty() { return cs.to_rgb_or_zeros(&c) }
return (c[0], 0.0, 0.0)      // the parametric t packed into .red
```
With functions the returned "colour" is **not a colour** — it is `t` in the
red slot, converted later by `ComponentToShadingIndex` +
the LUT: `if c_min == c_max { 0 } else { ((c - c_min)/(c_max - c_min)) * 255 }`.

**`ReadVertex()`** (:223-242): flag → coords → colour → **`ByteAlign()`**, in
that order, bailing on any capacity failure.

**`ReadVertexRow(count)`** (:244-264, type 5): no flag; per vertex
coords → colour → **`ByteAlign()`**; **any failure discards the entire row**
(returns empty), which callers treat as end-of-mesh.

**Type 4 — free-form Gouraud** (cpdf_rendershading.cpp:458-517):
- `flag == 0`: start a new triangle and read **two more vertices
  unconditionally** — their flags are read and **discarded**.
- `flag == 1`: `triangle = (old[1], old[2], new)`.
- `flag == 2`: `triangle = (old[0], old[2], new)`.
- **`flag == 3` is reachable** (the mask allows it) and is treated
  **identically to flag 2** — the code only special-cases `flag == 1`.

**Type 5 — lattice** (:519-586): `/VerticesPerRow` must be **≥ 2** (**no
maximum**; an absurd value simply produces an empty first row and aborts).
Two ping-pong rows; each adjacent column pair yields **2 triangles**.

**Types 6/7 — Coons and tensor patches** (:848-1003):
- `point_count` = **16** for tensor (7), **12** for Coons (6). **4 colours**
  per patch either way.
- `flag == 0`: read all points and all 4 colours.
- `flag ∈ {1,2,3}`: **`iStartPoint = 4, iStartColor = 2`** — read only
  `point_count - 4` new pairs (8 for Coons, 12 for tensor) and 2 new colours.
  - Reused points: **`coords[i] = old_coords[(flag*3 + i) % 12]`** for
    `i ∈ 0..4`. Flag 1 → sources 3,4,5,6; flag 2 → 6,7,8,9; flag 3 → 9,10,11,0.
    **The modulo is 12 even in the 16-point tensor case** — the 4 interior
    points are never reused.
  - Reused colours: `colors[0] = old[flag]`, `colors[1] = old[(flag+1) % 4]`.
- The inner capacity `break`s leave the remaining coords/colours at their
  **previous-iteration values** (no reset), and the patch is still drawn.
- **No `ByteAlign()` between patches** for types 6/7, unlike Gouraud's
  per-vertex align.
- Patch control-point layout (:952-969), 4×4 grid, boundary in order
  `c0..c11` then tensor interiors `c12..c15`.
- **Coons interior derivation** (:971-997), from ISO 32000-2 §8.7.4.5.8:
  ```
  p11 = (1/9)(-4·p00 + 6·(p01+p10) - 2·(p03+p30) + 3·(p31+p13) - p33)
  p12 = (1/9)(-4·p03 + 6·(p02+p13) - 2·(p00+p33) + 3·(p32+p10) - p30)
  p21 = (1/9)(-4·p30 + 6·(p31+p20) - 2·(p33+p00) + 3·(p01+p23) - p03)
  p22 = (1/9)(-4·p33 + 6·(p32+p23) - 2·(p30+p03) + 3·(p02+p20) - p00)
  ```
- Subdivision constants: `kBoundaryPathSize = 13`,
  `kCoonColorThreshold = 4`, "is small" = bbox `width < 2 && height < 2`.

**A latent C++ bug we do NOT port** (D18): `GetShadingBBox` in the content
parser mutates the loop-invariant `point_count`/`color_count` inside its loop
(cpdf_streamcontentparser.cpp:120-123), so after the first flagged patch every
subsequent patch under-reads and the bbox is wrong. The bbox only *shrinks*
the `sh` object's rect, and a too-small rect can clip visible output. We
compute the correct bbox and flag any corpus divergence (Open question Q5).

#### 1.16.9 Tiling patterns

`Load(page_obj)` (cpdf_tilingpattern.cpp:36-59):
```
colored_ = (dict["/PaintType"] == 1)          // anything else (incl. 2, 0, missing) is uncolored
xstep_ = fabsf(dict["/XStep"])                // ABSOLUTE VALUE
ystep_ = fabsf(dict["/YStep"])                // ABSOLUTE VALUE
if pattern_obj is not a Stream { return null }  // a tiling pattern MUST be a stream
form = CPDF_Form(doc, page_resources = null, stream)
states = fresh color/graph/text + THE PAGE OBJECT'S general state
form.parse_content(&states, &parent_matrix, recursion = null)
bbox_ = dict["/BBox"]                          // read AFTER parsing
```
Notes: **negative steps become positive at load**; missing steps read as **0**.
`/BBox` requires exactly 4 elements else all zeros. The tile content inherits
the *painting object's* general state (alpha, blend, soft mask) but gets
default colour/graph/text state. `/Resources` is resolved by the form's own
construction. The form matrix passed is `parent_matrix()`, **not**
`pattern_to_form()`.

**The zero/absurd-step ladder lives in the renderer**
(cpdf_rendertiling.cpp), and it is where every quirk actually bites:

1. **Cell sizing** (:90-113): `cell_bbox = pattern_to_device * bbox`; if
   `ceil(width)` or `ceil(height)` does not fit an `i32` → **abort the whole
   pattern**. Then **`width <= 0 → 1` and `height <= 0 → 1`** — a degenerate
   `/BBox` still produces a 1×1 tile.
2. **Step validation** (:116-120), the exact rule:
   ```
   if !x_step.is_finite() || !y_step.is_finite() || x_step == 0.0 || y_step == 0.0 { abort }
   ```
   **A zero or non-finite step silently draws nothing.** Negatives cannot
   reach here (already `fabsf`'d).
3. **Tile index range** (:122-140), in pattern space:
   ```
   min_col = ceil ((clip.left   - bbox.right ) / x_step)
   max_col = floor((clip.right  - bbox.left  ) / x_step)
   min_row = ceil ((clip.bottom - bbox.top   ) / y_step)
   max_row = floor((clip.top    - bbox.bottom) / y_step)
   ```
   each converted through a **checked float→i32**; any failure → **abort the
   whole pattern**. This is what stops an absurdly small step from producing
   an unbounded tile count.
4. **Area overflow guard** (:143-146): `height > i32::MAX / width` → abort.
5. **Slow path** (:151): when the cell is larger than the clip, or the cell
   area exceeds the clip area, each tile is rendered directly rather than
   blitted from a cached cell. Behaviorally identical, performance only.
6. **Aligned fast path** (:186-190): exact float equality of `bbox.left == 0
   && bbox.bottom == 0 && bbox.right == x_step && bbox.top == y_step`, plus a
   scaled-or-90°-rotated matrix.

**Uncolored patterns** (`/PaintType 2`): the cell bitmap is an 8-bit mask, the
render mode is forced to alpha-only, and the colour comes from the **Pattern
colorspace's base space applied to the `scn` operands**. When no colour is
resolvable: **colored tiling → `0x00BFBFBF`, everything else → `0xFFFFFFFF`**
(cpdf_colorstate.cpp:133-146). Operand count is capped at
`kMaxPatternColorComps = 16`.

Tiling composites with `CompositeDIBitmap(..., mask = 0, alpha = 1.0,
BlendMode::Normal, CPDF_Transparency())` — i.e. the tile stack itself is not a
transparency group.

### 1.17 Transparency

`CPDF_Transparency` has **exactly two set-only booleans**: `group_` and
`isolated_` (cpdf_transparency.h:15-23). Both default false; neither can be
cleared.

**Group parsing** (`LoadTransparencyInfo`, cpdf_pageobjectholder.cpp:153-167):
```
group = dict["/Group"]                      // must be a Dictionary
if group is None { return }
if group["/S"] != "Transparency" { return } // exact string, read as a byte string
transparency.set_group()
if group["/I"] != 0 { transparency.set_isolated() }   // any nonzero integer, incl. `true`
```
- **`/K` (knockout) is never read anywhere in `core/`.** `grep Knockout`
  turns up only device-driver plumbing (`SetGroupKnockout`), toggled by the
  render device rather than by PDF content. Knockout groups are therefore
  **unimplemented in the oracle**.
- **`/CS` is not read here** — it is consulted only by the soft-mask backdrop
  path.
- `CPDF_Page`'s constructor calls `SetIsolated()` **unconditionally**
  (cpdf_page.cpp:47), so **every page is an isolated group**.
- Entering a group resets the initial state to
  `BlendMode::Normal, stroke_alpha = 1.0, fill_alpha = 1.0, soft_mask = None`
  (cpdf_contentparser.cpp:111-117).

Query points that matter for compositing:
`IsIsolated()` decides whether the backdrop is grabbed from the device before
rendering the group (cpdf_renderstatus.cpp:674); `IsGroup()` decides whether
the form's fill alpha is applied as a **group-level** alpha (:740).

**Soft-mask dictionary parsing** (`LoadSMask`, cpdf_renderstatus.cpp:1433-1543):

| Key | Rule |
|---|---|
| `/G` | **Must be a Stream**; otherwise the entire soft mask is dropped. Parsed as a form with the **page's** resources. |
| `/S` | **`bLuminosity = (value != "Alpha")` — luminosity is the DEFAULT.** Only the exact string `Alpha` selects alpha mode; a missing `/S`, `/Luminosity`, or garbage all mean luminosity. |
| `/TR` | Accepted **only when it is a Dict or a Stream** — so `/TR /Identity` (a Name) is correctly ignored. Sampled to a **256-entry `u8` LUT**: `input = i / 255.0`, `lut[i] = roundf(out[0] * 255)`; **only output component 0 is used**. Absent → identity. |
| `/BC` | Only consulted in **luminosity** mode; in alpha mode the backdrop is 0. See below. |

The mask matrix is the ExtGState-time CTM translated by `(-clip.left,
-clip.top)`. The result is an 8-bit mask bitmap; in luminosity mode each pixel
is `lut[FXRGB2GRAY(r, g, b)]`, in alpha mode `lut[alpha]`.

**`/BC` backdrop** (`GetBackgroundColor`, :1544-1586):
```
default = ARGB(255, 0, 0, 0)                      // OPAQUE BLACK
bc = smask_dict["/BC"];  if none { return default }
// the colorspace comes from the /G STREAM'S OWN /Group /CS, not the smask dict
cs_obj = group_stream.dict["/Group"]["/CS"]
cs = load_colorspace(cs_obj, resources = null)
if cs is None { return default }
if cs.family == Lab || cs.is_special() || (cs.family == ICCBased && !cs.is_normal()) {
    return default                                 // unsupported group colorspace
}
comps = max(8, cs.components())                    // buffer padded to at least 8
count = min(8, bc.len())                           // AT MOST 8 values read
floats = bc[..count]; floats.resize(comps, 0.0)
rgb = cs.to_rgb_or_zeros(&floats)
return ARGB(255, (i32)(r*255), (i32)(g*255), (i32)(b*255))   // alpha always 255, truncated
```

### 1.18 Optional content (`/OC`)

Owned by `cpdf_occontext.*`. The interpreter never filters; visibility is a
predicate the render and text crates share, so it lives here.

`UsageType { View = 0, Design, Print, Export }` with the strings
`"View"`, `"Design"`, `"Print"`, `"Export"`.

**`CheckOCGDictVisible(dict)`** (:299-310): null → **visible**;
`dict["/Type"]` defaulting to `"OCG"`; `"OCG"` → `GetOCGVisible`, **anything
else → treated as an OCMD**.

**`CheckPageObjectVisible(obj)`** (:189-200): scans the object's content marks
for one named exactly `"OC"` **whose param type is `kPropertiesDict`** — i.e.
`BDC /OC /Name` resolved through the `/Properties` resource. **An inline
`BDC /OC << … >>` (a direct dict) is ignored entirely** and the object stays
visible.

**`GetOCGVisible(dict)`** (:174-187): **null → invisible** (the opposite of
`CheckOCGDictVisible`); memoized per OCG dict. **OCMD results are not cached.**

**`LoadOCGState`** (:149-172):
```
if !has_intent(ocg, "View", "View") { return true }   // /Intent excluding View -> always visible
state = usage_type_string()                            // "View"/"Design"/"Print"/"Export"
usage = ocg["/Usage"]
if usage {
    s = usage[state]                                   // e.g. /Usage /Print << >>
    key = state + "State"                              // "ViewState"/"DesignState"/"PrintState"/"ExportState"
    if s && s.has_key(key) { return s[key] != "OFF" }
    if state != "View" {
        s = usage["/View"]
        if s && s.has_key("ViewState") { return s["ViewState"] != "OFF" }
    }
}
return load_from_config(state, ocg)
```
**Anything other than the exact string `"OFF"` means ON.**

**`HasIntent(dict, element, default)`** (:17-37): absent `/Intent` →
`element == default`; an array → true if any entry is `"All"` or `element`;
a name/string → the same test. Called as `("View", "View")` for OCGs (absent
→ true) and `("View", "")` for configs (absent → **false**, so a config with
no `/Intent` is never selected from `/Configs`).

**`GetConfig`** (:39-71): the catalog's `/OCProperties` and its `/OCGs` array
must exist and **must contain this OCG** (else null → visible). Then **the
first `/Configs` entry whose `/Intent` contains `View` or `All` wins over
`/D`**; otherwise `/D`.

**`LoadOCGStateFromConfig`** (:95-147) — precedence, in order:
1. `/BaseState`, **defaulting to `"ON"`**; only the exact string `"OFF"` turns
   it off.
2. `/ON` array containing this OCG → on.
3. `/OFF` array containing this OCG → off (**OFF wins over ON**).
4. Each matching `/AS` entry in array order (**last wins**): the entry's
   `/Event` (**defaulting to `"View"`**) must equal the usage type, its
   `/OCGs` must contain this OCG, and its `[usage]` sub-dict's `[usage]State`
   is compared against `"OFF"`.

**`LoadOCMDState`** (:253-297):
```
if ocmd["/VE"] is an array { return get_ocg_ve(ve, depth = 0) }   // /VE has ABSOLUTE precedence
p = ocmd["/P"] or "AnyOn"                                          // DEFAULT AnyOn
ocgs = ocmd["/OCGs"]
if none { return true }
if a Dictionary { return get_ocg_visible(it) }                     // single OCG: /P is IGNORED
if not an Array { return true }
state = (p == "AllOn" || p == "AllOff")                            // vacuous-truth default
seen_valid = false
for entry in array {
    let d = entry as Dict else { continue }                        // non-dict entries skipped
    seen_valid = true
    let vis = get_ocg_visible(d)
    if (p == "AnyOn"  &&  vis) || (p == "AnyOff" && !vis) { return true }
    if (p == "AllOn"  && !vis) || (p == "AllOff" &&  vis) { return false }
}
return !seen_valid || state
```
The four policies are exactly `AnyOn` (default), `AllOn`, `AnyOff`, `AllOff`.
**An unrecognized `/P` matches none of the four conditions, so the loop never
short-circuits and the result is `state`, which is `false` — an unknown `/P`
with at least one valid OCG yields INVISIBLE.**

**`GetOCGVE(expr, depth)`** (:202-251):
- **Depth guard: `depth > 32` → false.** The initial call is at depth 0, so
  **33 levels** are allowed.
- Operators, case-sensitive: **`Not`, `Or`, `And`**. Anything else → false
  (invisible).
- `Not` takes element 1: a dict → negated visibility; an array → negated
  recursion; anything else → false.
- `Or`/`And` fold elements 1.., seeding `bValue` on **`i == 1`**. **Null
  elements are `continue`d without advancing the seed logic**, so if element 1
  is missing, element 2 goes through the `else` branch and combines against
  the initial `false` — an `Or` still works, but an **`And` immediately
  yields false**. Non-dict, non-array operands contribute `false`.

Consumers: page objects (`cpdf_renderstatus.cpp:247, 273`), form XObjects
(`form.dict["/OC"]`, :399-403), images (`image.GetOC()`,
cpdf_imagerenderer.cpp:196-199). **No OC context → everything visible.**

### 1.19 Images — the DIB load ladder

#### 1.19.1 Dimension and BPC validation

```
kMaxImageDimension = 0x01FFFF = 131071          (cpdf_dib.cpp:54)
IsValidDimension(v)          = v > 0 && v <= 131071
IsMaybeValidBitsPerComponent = 0 <= bpc <= 16   (:78-79)
IsAllowedBitsPerComponent    = bpc ∈ {1,2,4,8,16}  (:81-84)
```
`/Width` and `/Height` are each validated against `IsValidDimension` (:781).
`/BitsPerComponent` outside `[0, 16]` is a **hard failure** at :336, not a
repair.

**The repair ladder** (`ValidateDictParam`, :982-1005), keyed on the **last**
filter in the pipeline:
```
bpc = bpc_orig
if filter == "JPXDecode"                       { do_bpc_check = false; return true }  // bpc may stay 0
if filter == "CCITTFaxDecode" || "JBIG2Decode" { bpc = 1; components = 1 }             // FORCED
else if filter == "DCTDecode"                  { bpc = 8 }                             // FORCED
// RunLengthDecode is deliberately NOT forced to 8 — comment: "too many documents do not conform"
if bpc ∉ {1,2,4,8,16} { bpc = 0; return false }   // HARD FAILURE
```
So `bpc ∈ {3,5,6,7,9..15}` and `bpc == 0` are **rejected outright, not
repaired to 8**. The only bpc coercions in the whole path are the two
filter-driven ones above, the post-DCT-probe assignment
`bpc = jpeg_info.bits_per_component` (:563, :620), and the unconditional
`bpc = 8` after a successful JPX decode (:713).

**ImageMask forcing** (`LoadColorInfo`, :340-354):
```
image_mask = dict.bool("/ImageMask", false)
if image_mask || !dict.has_key("/ColorSpace") {
    // exception: no /ColorSpace + last filter JPXDecode + not explicitly a mask
    if !image_mask && last_filter == "JPXDecode" { do_bpc_check = false; return true }
    image_mask = true
    bpc = 1; components = 1                    // regardless of /BitsPerComponent
    // /ColorSpace, if present, is IGNORED ENTIRELY
    default_decode = decode.is_none() || decode.int_at(0) == 0
    return true
}
```

**Overflow caps**: `pitch = ceil(bpc * components * width / 8)` and
`pitch * height` are both computed in a checked `u32` (:793-803) — the
effective whole-image cap is **under 4 GiB**. Every later pitch computation
uses the same checked helpers.

#### 1.19.2 The load state machine

`LoadState { Fail, Success, Continue }`. `StartLoadDIBBase(has_mask,
form_resources, page_resources, std_cs, group_family, load_mask,
max_size_required)` (:199-248):
1. **`if !stream.is_inline() { form_resources = None }`** (:212-214) — form
   resources are consulted **only for inline images**.
2. `LoadInternal` (§1.19.1) → Fail.
3. **Resolution skipping** (:220-225):
   ```
   levels = 0
   if max.width != 0 && max.height != 0 {
       levels = log2(max(1, min(width / max.width, height / max.height))) as u8
   }
   ```
   Integer division, then min, then `max(1, ·)`, then a floored `log2`.
4. `CreateDecoder(levels)` → Fail.
5. `ContinueToLoadMask` — enables std conversion on the colorspace when
   `std_cs`, then `ContinueInternal` (format selection + palette).
6. `has_mask ? StartLoadMask() : Success`; either step Continue → **Continue**.
7. Disable std conversion; Success.

`ContinueInternal` (:160-197): a mask forces `1bppMask`; otherwise
`bpp = bpc * components` selects `1bppRgb` (bpp 1), `8bppRgb` (bpp ≤ 8), or
`Bgr` (24bpp). **`bpp == 0` → false.** Then `LoadPalette()`, and if colour-key
masking is active the format switches to `Bgra` with a second line buffer.

`ContinueLoadDIBBase` (:250-326): **`JPXDecode` → always Fail** (JPX is never
resumable); anything but `JBIG2Decode` → Success; JBIG2 pumps
`StartDecode`/`ContinueDecode`, tearing down all state on error.

#### 1.19.3 Colorspace resolution for images

```
cs_obj = dict["/ColorSpace"];  if none { return false }
if form_resources { cs = load_colorspace(cs_obj, form_resources) }
if cs.is_none()   { cs = load_colorspace(cs_obj, page_resources) }
if cs.is_none()   { return false }
components = cs.components();  family = cs.family()
// ICCBased-by-name override:
if family == ICCBased && cs_obj is a Name {
    match name { "DeviceGray" => components = 1, "DeviceRGB" => 3, "DeviceCMYK" => 4, _ => {} }
}
```
`CPDF_DIB` imposes **no family restriction at load** — even Pattern is
accepted, and is filtered downstream (`LoadPalette` early-returns,
`TranslateScanline24bpp` leaves the colour black). The restrictions that *do*
reject are the DCT component-mismatch switch (§1.19.5) and Indexed's own
base-family check.

#### 1.19.4 `/Decode` application

`GetDecodeAndMaskArray` (:402-461), with `max_data = (1 << bpc) - 1`:
```
// with a /Decode array:
for i in 0..components:
    decode_min[i]  = decode[2i]
    decode_step[i] = (decode[2i+1] - decode_min[i]) / max_data
    (def_min, def_max) = cs.default_value_range(i)
    if family == Indexed { def_max = max_data }
    if def_min != decode_min[i] || def_max != decode[2i+1] { default_decode = false }

// without:
for i in 0..components:
    (decode_min[i], m) = cs.default_value_range(i)
    if family == Indexed { m = max_data }
    decode_step[i] = (m - decode_min[i]) / max_data
```
**Short or invalid `/Decode` arrays are not rejected** — out-of-range reads
yield 0.0, so a missing pair makes that component a constant 0 and flips
`default_decode` false. `default_decode` starts true and is only ever cleared
here.

Application, per component: `value = decode_min + decode_step * raw_sample`.

**ImageMask inversion** (`GetScanline`, :1171-1184) when `bpc*components == 1`:
```
if image_mask && default_decode { for each byte: dest = !src }   // BITWISE NOT
else { verbatim copy }
```
So `/Decode [0 1]` (or absent) **inverts**; `/Decode [1 0]` copies.

#### 1.19.5 Filter dispatch and per-filter fallbacks

`CreateDecoder(levels)` (:463-523):
- **No filter → Success** (the raw path; `GetScanline` reads straight out of
  the accumulated stream).
- `do_bpc_check && bpc == 0` → Fail.
- `JPXDecode` → `LoadJpxBitmap(levels)`; null → **Fail, no fallback**.
- `JBIG2Decode` → allocate a 1bpp bitmap, then **return Continue**.
- `CCITTFaxDecode` / `FlateDecode` / `RunLengthDecode` / `DCTDecode` → build a
  scanline decoder; **`DCTDecode` failure → Fail**; the others fall through to
  the generic `!decoder → Fail`.
- Any **other** filter name → no decoder → **Fail**.
- **Pitch reconciliation** (:509-521): `requested = pitch(bpc, components,
  width)`; `provided = pitch(decoder.bpc, decoder.comps, decoder.width)`;
  **`provided < requested` → Fail**.

CCITT defaults (fpdf_parser_decode.cpp:318-342): `K=0, EndOfLine=false,
EncodedByteAlign=false, BlackIs1=false, Columns=1728, Rows=0`, with
`Rows > USHRT_MAX → 0`. Flate/LZW predictor defaults: `predictor=0, Colors=1,
BitsPerComponent=8, Columns=0`.

#### 1.19.6 DCT specifics and the CMYK inversion quirk

`CreateDCTDecoder` (:525-629):
- `scale_denom = 1 << min(levels, 3)` — **capped at 1/8**, libjpeg's limit.
- `/ColorTransform` **defaults to 1**.
- On success the **decoder's dimensions replace the dict's** (:542-544).
- On failure, probe with `LoadInfo`; **the JPEG's own dimensions overwrite the
  dict's** (:554-555). Then `num_components ∈ {1,3,4}` and
  `bits_per_component ∈ {1,2,4,8,16}` must hold, else fail.
- **Components agree** → `bpc = info.bits_per_components`, re-create the
  decoder with the JPEG's own `color_transform`. A null decoder here is
  **not** an error at this level — the caller treats it as Fail.
- **Component mismatch** (:576-628): `components = info.num_components`,
  `comp_data` cleared, then a per-family gate:
  - Device families: `min_comps = ComponentsForFamily(family)`; fail if
    `cs_comps < min_comps || components < min_comps`.
  - Lab: fail unless `components == 3 && cs_comps >= 3`.
  - ICCBased: fail unless both counts are valid ICC counts and
    `cs_comps >= components`.
  - **default (Indexed, Separation, DeviceN, Pattern, CalGray, CalRGB): fail
    unless `cs_comps == components`.**
  - No colorspace: fail only if `family == Lab && components != 3`.
  Then re-derive `/Decode`, set `bpc`, re-create the decoder.

**MCU alignment guard** (libjpeg_scanline_decoder.cpp:140-157): if
`orig_width % (max_h_samp * 8) != 0 || orig_height % (max_v_samp * 8) != 0`,
**`scale_denom` is forced back to 1** — reduced-resolution decoding is refused
for non-MCU-aligned images (crbug 890745). `ScaledJpegSize(dim, denom) =
ceil(dim / denom)` — note this is **ceiling division**, whereas JPX uses a
plain right shift.

**The CMYK inversion quirk** is *not* in the decoder — it is expressed through
`/Decode` and through `TransMask()`:
- `CPDF_Image::InitJPEG` writes **`/Decode [1 0 1 0 1 0 1 0]`** for
  4-component JPEGs (cpdf_image.cpp:119-126), giving `decode_min = 1,
  decode_step = -1/255` per channel — Adobe-inverted CMYK.
- **`TransMask() = load_mask && group_family == DeviceCMYK && family ==
  DeviceCMYK`** (:1306-1309). When set, the conversion uses the naive
  un-inversion:
  ```
  k = 1 - key;  r = (1-c)*k;  g = (1-m)*k;  b = (1-y)*k     // float path
  k = 255 - key;  r = ((255-c)*k)/255;  …                    // 8-bpc integer path
  ```
  instead of the Adobe table.

#### 1.19.7 JPX specifics

`LoadJpxBitmap` (:631-715):
1. `ColorSpaceOption` from the PDF colorspace: none → `kNone`; Indexed →
   `kIndexed`; else `kNormal`. `kIndexed` sets the decoder flag that **ignores
   the codestream's palette/cmap boxes** so raw indices come through.
2. `Create(data, option, levels, strict_mode = true)`; null → null.
3. **`width >>= levels; height >>= levels`** — a **plain right shift** of the
   dict dims, unlike DCT's ceiling division.
4. **`if codestream.width < width || codestream.height < height { return null }`**
   — the codestream must be at least as large as the shifted dict size.
5. `JpxDecodeConversion::Create(info, colorspace)`; none → null.
6. **Colorspace override**: `Some(cs)` replaces it; **`Some(null)` resets it to
   none**; `None` keeps it.
7. Component count: if the conversion supplies one, assert `components == 0`
   and take it (**`<= 0` → null**); else assert `components != 0`.
8. Build the bitmap in the conversion's format, clear to `0xFFFFFFFF`, decode
   with the conversion's `swap_rgb`.
9. `convert_argb_to_rgb` → §1.19.8. Else if the colorspace is Indexed with
   `bpc < 8`, every pixel is `>>= (8 - bpc)`.
10. **`bpc = 8` unconditionally** (:713).

**The conversion table** (jpx_decode_conversion.cpp). With a PDF `/ColorSpace`
(:39-88), where `IsJPXColorSpaceOrUnspecifiedOrUnknown(actual, expected)` also
accepts `UNSPECIFIED` and `UNKNOWN`:

| PDF colorspace | JPX condition | Action |
|---|---|---|
| stock DeviceGray | colorspace ∈ {GRAY, UNSPECIFIED, UNKNOWN} | `UseGray` |
| stock DeviceGray | otherwise | **fail the whole JPX load** |
| stock DeviceRGB | colorspace ∉ {SRGB, UNSPECIFIED, UNKNOWN} | **fail** |
| stock DeviceRGB | channels > 3 | `ConvertArgbToRgb` |
| stock DeviceRGB | channels ≤ 3 | `UseRgb` |
| stock DeviceCMYK | colorspace ∈ {CMYK, UNSPECIFIED, UNKNOWN} | `UseCmyk` |
| stock DeviceCMYK | otherwise | **fail** |
| any other, 3 components | channels == 4 && colorspace == SRGB | `ConvertArgbToRgb` (*"Many PDFs generated by iOS meet this condition"*, crbug 345431077) |
| Indexed, 1 component | — | `UseIndexed` |
| anything else | — | `DoNothing` |

Without a PDF `/ColorSpace` (:90-113):

| codestream colorspace | Action | Component count |
|---|---|---|
| UNKNOWN / UNSPECIFIED | `channels == 3 ? UseRgb : DoNothing` | `channels` |
| SYCC / EYCC | `DoNothing` | 3 |
| SRGB | `channels > 3 ? ConvertArgbToRgb : UseRgb` | 3 |
| GRAY | `UseGray` | 1 |
| CMYK | `UseCmyk` | 4 |

Format (:136-154): `UseGray`/`UseIndexed` → `8bppRgb`; `UseRgb` with 3
channels → `Bgr`, with 4 → `Bgrx`; `ConvertArgbToRgb` → `Bgrx`; anything else
→ **no format**, in which case the fallback is `Bgr` with
**`width = (jpx_width * channels + 2) / 3`** — N channels packed into 3-byte
pixels (:209-218).

Colorspace override and swap per action (:159-221): `UseGray` → stock
DeviceGray; `UseCmyk` → stock DeviceCMYK; **`UseRgb` and `ConvertArgbToRgb`
both set the override to `null` (reset) and `swap_rgb = true`**;
`DoNothing`/`UseIndexed` keep the existing colorspace.

**`/SMaskInData`** (:717-770), reached only on the `ConvertArgbToRgb` path:
- `== 1`: the alpha channel is captured into a side buffer, and the colour is
  **un-premultiplied against white**:
  ```
  na = 255 - alpha
  out.b = (b * alpha + 255 * na) / 255      // and g, r
  ```
- **Anything else (including 2, "premultiplied") takes the drop-alpha path** —
  `/SMaskInData 2` is not distinguished.
- The captured alpha becomes a **synthetic `/DeviceGray` 8-bpc image XObject**
  fed to the mask loader (:813-824).

JPX decoder internals worth reproducing: `kMaxResolutionsToSkip = 32`; input
under 12 bytes rejected; the JP2 signature
`00 00 00 0C 6A 50 20 20 0D 0A 87 0A` selects the JP2 codec, else raw J2K;
a post-decode heuristic **forces SYCC** when the image is 3-component with
matching x/y subsampling on component 0 and `dx != 1` on component 1, and
**forces GRAY** when `numcomps <= 2`; the **ICC profile in the codestream is
discarded** (crbug 346606150). YCC→RGB coefficients: `r = y + 1.402·cr`,
`g = y - (0.344·cb + 0.714·cr)`, `b = y + 1.772·cb`, offset `1 << (prec-1)`.
Sample scaling to 8 bits: `adjust = prec - 8`; negative → left shift;
zero → copy; positive → `(src >> adjust) + ((src >> (adjust-1)) & 1)` then
clamp — i.e. **rounding**, not truncation.

#### 1.19.8 JBIG2

Always 1 bpc, 1 component (forced by `ValidateDictParam`). The bitmap is
allocated up front as `1bppMask` for a mask or `1bppRgb` otherwise.
`/JBIG2Globals` is fetched from the **decode parms** (:271-278) and
**absent or unfetchable globals are silently tolerated** — the decoder is
called with an empty globals span. A decode error tears down the context, the
bitmap, and the globals, and returns Fail.

#### 1.19.9 Masking

**`/SMask` wins over `/Mask` at every level.** `GetDecodeAndMaskArray`
(:438-460) returns early if `/SMask` exists, so **colour-key `/Mask` arrays
are ignored when an `/SMask` is present**. `StartLoadMask` (:826-830) likewise
tries `/SMask` first and only falls back to a `/Mask` **stream**.

**Colour-key masking** (`/Mask` as an array):
```
if array.len() >= components * 2 {
    for i in 0..components:
        color_key_min[i] = max(array[2i], 0) as u32
        color_key_max[i] = clamp(array[2i+1], 0, max_data) as u32
}
color_key = true            // SET EVEN IF THE ARRAY WAS TOO SHORT
```
When the array is too short the C++ leaves `color_key_min/max`
**uninitialized** (`DIB_COMP_DATA` has no default member initializers) — a
genuine bug. We default them to `0/0`, which gives "only exactly-0 samples are
transparent" (D19).

Matching: a pixel is **transparent** iff **every** component is inside its
`[min, max]` range; alpha is `0xFF` when any component is out of range, `0`
otherwise. For 1-bpc, `Get1BitSetValue`/`Get1BitResetValue` (:1317-1329) give
fully-transparent when `color_key_max == 1` / `color_key_min == 0`
respectively.

**`/SMask` and `/Matte`** (:832-843) — four conjoined preconditions, all
required:
```
matte_color = 0xFFFFFFFF        // the "no matte" sentinel
if matte_array && colorspace && family != Pattern
   && matte_array.len() == components          // EXACTLY equal
   && colorspace.components() <= components {
    rgb = colorspace.to_rgb_or_zeros(&matte_array[..components])
    matte_color = ARGB(0, roundf(r*255), roundf(g*255), roundf(b*255))   // alpha byte 0
}
```

**Mask loading** (`StartLoadMaskDIB`, :876-892) — the parameters are
load-bearing:
```
mask.start_load(has_mask = false,           // NO recursive masks (cycle prevention)
                form_resources = None, page_resources = None,   // NO resources at all
                std_cs = true, group_family = Unknown, load_mask = false,
                max_size_required = {0, 0})  // ALWAYS FULL RESOLUTION
```
So a mask never carries a nested mask, never resolves a named colorspace
through resources, and is **never resolution-reduced** — a 400×400 mask stays
400×400 even when its base image decodes at 50×50, and the compositor scales
each independently.

**Asymmetric failure handling:** `StartLoadMaskDIB` on Fail **resets the mask
and returns Success** (:888-891) — a broken mask never fails the base image.
But `ContinueLoadMaskDIB` on Fail **propagates the failure** (:861-864).

#### 1.19.10 Scanline production and pixel conversion

Bit unpacking (:58-75), **MSB-first, big-endian 16-bit**:
```
GetBits8(data, bitpos, nbits):   // nbits ∈ {1,2,4,8,16}, bitpos must be nbits-aligned
    byte = data[bitpos / 8]
    if nbits == 8  { return byte }
    if nbits == 16 { return byte * 256 + data[bitpos/8 + 1] }
    return (byte >> (8 - nbits - (bitpos % 8))) & ((1 << nbits) - 1)
```

`GetScanline(line)` (:1129-1296) source selection, in order:
1. A cached bitmap whose pitch is at least the requested pitch — **the line
   index is clamped to the bitmap's last row**.
2. The scanline decoder.
3. The raw accumulated stream at `line * pitch`; **a short remainder is copied
   into a zero-padded buffer** (:1141-1163) — the truncated-stream fallback.
4. Empty → zero-fill.

Then, by `bpc * components`:
- **== 1**: the mask inversion / verbatim copy / colour-key 32-bit expansion.
- **≤ 8**: `bpc == 8` copies; otherwise components are packed into one byte
  index with **`color_index |= data << (color * bpc)`** (:1203-1207) —
  component `j` occupies bits `[j*bpc, (j+1)*bpc)`. This must match
  `LoadPalette`'s enumeration exactly.
- **> 8**: the colour-key alpha pass (a fast path for 3×8bpc, a generic
  bit-stepping path otherwise), then `TranslateScanline24bpp`.

`TranslateScanline24bppDefaultDecode` (:1056-1127), the fast path, returns
"handled" in two cases where it writes **nothing**: a component-count mismatch
for non-RGB families (:1069-1073), and `components != 3` for RGB families
(:1076-1078). Both leave the line buffer stale. For RGB families:
- **bpc 8**: `dest = (src[2], src[1], src[0])` — RGB→BGR.
- **bpc 16**: `dest = (src[4], src[2], src[0])` — **keep only the high byte of
  each big-endian sample; the low byte is discarded with no rounding.**
- **bpc 1/2/4**: `GetBits8` per component, `min(v, max_data)`, then
  `dest = (B*255/max_data, G*255/max_data, R*255/max_data)` — integer scaling.

The general `TranslateScanline24bpp` (:1007-1054) uses a
`max(components, 16)` scratch buffer, applies the decode formula, then
`TransMask` / `to_rgb_or_zeros` / (Pattern → leave black), clamps to [0,1],
and writes **B, G, R** with **truncation**.

`LoadPalette` (:894-980) — skipped when there is no colorspace, for Pattern,
for `bpc == 0`, when `bpc * components > 8`, and for the identity cases
(1-bit default-decode DeviceGray/DeviceRGB, and 8-bpc default-decode
DeviceGray). For 1-bit it also **skips when `cs.components() > 3`**, and has a
special case: an **Indexed space with `hival == 0` gets `argb1 = 0xFF000000`**
(:928-933), *"to set the range of the color space"*. The general loop
enumerates `1 << bits` entries with the same component packing as `GetScanline`
and uses **`roundf`** (contrast the truncating scanline path).

#### 1.19.11 Image caching

`kHugeImageSize = 60_000_000` (60 MB, cpdf_dib.h:39) —
images under it are **eagerly realized** into a flat bitmap when cached; larger
ones stay lazy. Masks are always realized.

`kCacheSizeLimitBytes = 100 MiB` (cpdf_renderoptions.cpp:11).
`CacheOptimization` (cpdf_pageimagecache.cpp:119-151): sort entries LRU-first,
handle the `time_count_` rollover, then **unconditionally evict everything but
the 15 most recent** (`while (i + 15 < nCount)`, :143-146) **before** any size
check, then evict further until under the byte limit.

**Cache validity** (:347-358):
```
if !cached_at_reduced_resolution { return true }        // full-res cache always usable
if now_wants_full_res { return false }                  // force a re-decode
return cached.width >= wanted.width && cached.height >= wanted.height
```

`max_size_required` at the top of the chain is the **render device's full
width and height** (cpdf_imagerenderer.cpp:73-79).

### 1.20 Consolidated limit and constant table

| Constant | Value | C++ source |
|---|---|---|
| `kMaxFormLevel` (in-flight form parses) | **40**, compared with `>` | cpdf_streamcontentparser.cpp:55 |
| `kParamBufSize` (operand ring) | **16** | cpdf_streamcontentparser.h:80 |
| `kParseStepLimit` (pause granularity) | 100 page objects | cpdf_contentparser.cpp:230 |
| content-tokenizer `kMaxWordLength` | **255** (buffer 256) | cpdf_streamparser.h:51,68 |
| content-tokenizer `kMaxStringLength` | **32767** | cpdf_streamparser.cpp:43 |
| content-tokenizer object recursion | **64** (`kParserMaxRecursionDepth`) | cpdf_streamparser.cpp:349 |
| stream-concat separator | one `0x20` after each stream | cpdf_contentparser.cpp:203-206 |
| `kMaxTextObjects` (text clip) | **1024**, all-or-nothing | cpdf_clippath.cpp:104 |
| content-marks stack cap | **none** | — |
| default line width / miter limit | **1.0** / **10.0** | cfx_graphstate.cpp:50-73 |
| default line cap / join | Butt(0) / Miter(0) | cfx_graphstatedata.h:52-53 |
| dash: non-finite element | abandon the pattern → solid | cfx_agg_devicedriver.cpp:344-349 |
| dash: `kMinDashCycleThreshold` | **0.1** device px → solid | cfx_agg_devicedriver.cpp:357-366 |
| dash: element substitution | `len <= 0.000001 → 0.1` | cfx_agg_devicedriver.cpp:371-374 |
| `CA`/`ca` clamp | `[0.0, 1.0]` | cpdf_allstates.cpp:118-125 |
| text render mode valid range | **0..7**, else unchanged | cpdf_textstate.cpp:128-134 |
| `Tz` semantics | stored as **`n / 100`** | cpdf_streamcontentparser.cpp:1491 |
| transfer-function LUT | **256** entries, `roundf(out*255)` | cpdf_transferfunc.h, cpdf_docrenderdata.cpp:116-146 |
| transfer-function `kMaxOutputs` | **16** | cpdf_docrenderdata.cpp:32 |
| transfer-function array form | **≥ 3 elements**, stored reversed | cpdf_docrenderdata.cpp:85-95 |
| colorspace depth cap | **none** (visited sets only) | cpdf_colorspace.cpp:507-511 |
| `kMaxPatternColorComps` | **16** | cpdf_colorspace.h:34 |
| Indexed `/hival` | `clamp(v, 0, 255)` | cpdf_indexedcs.cpp:70 |
| Indexed minimum array size | **4** | cpdf_indexedcs.cpp:34 |
| DeviceN `/Names` cap | **none** | cpdf_colorspace.cpp:1198 |
| Sep/DeviceN scratch floor | `max(outputs, 16)` | cpdf_colorspace.cpp:1147, 1207 |
| valid ICC component counts | **{1, 3, 4}** | icc_transform.cpp:152-155 |
| sRGB stock profile | length **exactly 3144**, `b"sRGB IEC61966-2.1"` at offset **400** | cpdf_iccprofile.cpp:17-20 |
| sRGB LUT sizes / branch | 192 + 253 bytes; `scale < 192` | cpdf_colorspace.cpp:51-82, 368 |
| sRGB LUT scale | `c * 1023` | cpdf_colorspace.cpp:367 |
| Adobe CMYK table | **9×9×9×9 = 6561** entries; `idx = 729c + 81m + 9y + k` | cfx_cmyk_to_srgb.cpp:1664-1666 |
| Adobe CMYK float rounding offset | **0.49999997** | cfx_cmyk_to_srgb.cpp:1753 |
| Lab default `/Range` | `[-100, 100, -100, 100]` (only when absent) | cpdf_colorspace.cpp:859-860 |
| Lab conversion constants | 16, 116, 500, 200, threshold **0.2069**, slope **0.12842**, offset **0.1379**, Xn **0.957**, Zn **1.0889** | cpdf_colorspace.cpp:867-897 |
| CalGray default gamma | **1.0** | cpdf_colorspace.cpp:141 |
| `/WhitePoint` validity | 3 elements, `Xw>0 && Yw==1.0 && Zw>0` | cpdf_colorspace.cpp:110-120 |
| function types accepted | **0, 2, 3, 4** (no type 1) | cpdf_function.cpp:28-38 |
| function input/output caps | **none** | — |
| function type-3 depth cap | **none** (cycle set only) | cpdf_stitchfunc.cpp:82 |
| sampled `/BitsPerSample` | **{1, 2, 4, 8, 12, 16, 24, 32}** | cpdf_sampledfunc.cpp:25-39 |
| sampled total-bits cap | `bps · outputs · Π sizes < 2^32` | cpdf_sampledfunc.cpp:85-88 |
| sampled `/Order` | **never read** | — |
| stitching `kRequiredNumInputs` | **1** | cpdf_stitchfunc.cpp:19 |
| PS `kPSEngineStackSize` | **100**; overflow drops, underflow → 0 | cpdf_psengine.h:125 |
| PS `kMaxDepth` (`{}` nesting) | **128**, compared with `>` (129 levels) | cpdf_psengine.h:103 |
| PS operator count | **42** named + `PROC` + `CONST` | cpdf_psengine.cpp:31-74 |
| PS `div`/`idiv`/`mod` by zero | **0** | cpdf_psengine.cpp:245-271 |
| PS `atan` | degrees, normalized to **[0, 360)** | cpdf_psengine.cpp:308-316 |
| PS `sin`/`cos` | **degrees** | cpdf_psengine.cpp:300-307 |
| shading types | **1..7** | cpdf_shadingpattern.h:19-31 |
| shading max function count (array) | **4** (a literal) | cpdf_shadingpattern.cpp:68 |
| `kShadingSteps` | **256**; sampled at `i/256`, indexed at `s*255` | cpdf_rendershading.cpp:45 |
| shading device max dpi | 150 | cpdf_rendershading.cpp:1047 |
| `kBoundaryPathSize` / `kCoonColorThreshold` | 13 / 4 | cpdf_rendershading.cpp:589, 739 |
| patch "small" threshold | bbox `w < 2 && h < 2` | cpdf_rendershading.cpp:594 |
| mesh `kMaxComponents` | **8** | cpdf_meshstream.h:76 |
| `/BitsPerCoordinate` | **{1,2,4,8,12,16,24,32}** | cpdf_meshstream.cpp:53-67 |
| mesh `/BitsPerComponent` | **{1,2,4,8,12,16}** | cpdf_meshstream.cpp:38-50 |
| `/BitsPerFlag` | **{2,4,8}**, types 4/6/7 only | cpdf_meshstream.cpp:82-91 |
| mesh `/Decode` length | **exactly `4 + 2·components`** | cpdf_meshstream.cpp:142 |
| mesh flag mask | `& 0x03` | cpdf_meshstream.cpp:187 |
| `/VerticesPerRow` | **≥ 2**, no maximum | cpdf_rendershading.cpp:529 |
| Coons / tensor coord pairs | **12 / 16**; 4 colours | cpdf_rendershading.cpp:894, 924 |
| patch reuse skips | 4 points, 2 colours; `old[(flag*3+i) % 12]` | cpdf_rendershading.cpp:904-913 |
| tiling step rejection | non-finite or **exactly 0.0** → draw nothing | cpdf_rendertiling.cpp:118-120 |
| tiling `/XStep` `/YStep` | **`fabsf`'d at load** | cpdf_tilingpattern.cpp:42-43 |
| tiling cell minimum | clamped to **1×1 px** | cpdf_rendertiling.cpp:108-113 |
| uncolored / colored fallback colorref | `0xFFFFFFFF` / **`0x00BFBFBF`** | cpdf_colorstate.cpp:143 |
| OC `/VE` depth guard | `depth > 32` (33 levels) | cpdf_occontext.cpp:203 |
| OC `/P` default | **`"AnyOn"`** | cpdf_occontext.cpp:259 |
| OC `/BaseState` default | **`"ON"`** | cpdf_occontext.cpp:103 |
| OC `/Type` default | `"OCG"` | cpdf_occontext.cpp:305 |
| OC `/AS` `/Event` default | `"View"` | cpdf_occontext.cpp:126 |
| soft mask `/S` default | **Luminosity** (only `"Alpha"` differs) | cpdf_renderstatus.cpp:1457-1459 |
| soft mask `/BC` default | opaque black `ARGB(255,0,0,0)` | cpdf_renderstatus.cpp:1548 |
| soft mask `/BC` cap / pad | read ≤ 8, pad to `max(8, N)` | cpdf_renderstatus.cpp:1577-1580 |
| soft mask `/TR` LUT | 256 entries, `roundf(out[0]*255)` | cpdf_renderstatus.cpp:1502-1509 |
| transparency `/K` | **never parsed** | — |
| `kMaxImageDimension` | **131071** (`0x01FFFF`), min 1 | cpdf_dib.cpp:53-56 |
| image `/BitsPerComponent` | `[0,16]` gate, then **{1,2,4,8,16}** | cpdf_dib.cpp:78-84 |
| image whole-size cap | `pitch * height` must fit `u32` | cpdf_dib.cpp:793-803 |
| DCT `scale_denom` | `1 << min(levels, 3)` → max 1/8 | cpdf_dib.cpp:531-532 |
| DCT `/ColorTransform` default | **1** | cpdf_dib.cpp:535 |
| valid JPEG components / bpc | {1,3,4} / {1,2,4,8,16} | cpdf_image.cpp:43-50 |
| JPX `kMaxResolutionsToSkip` | **32** | cjpx_decoder.h:31 |
| CCITT `/Columns` / `/Rows` defaults | 1728 / 0 (`>USHRT_MAX → 0`) | fpdf_parser_decode.cpp:327-338 |
| `kHugeImageSize` | **60_000_000** | cpdf_dib.h:39 |
| image cache byte limit | **100 MiB** | cpdf_renderoptions.cpp:11 |
| image cache hard entry cap | **15 most recent**, before any size check | cpdf_pageimagecache.cpp:143-146 |
| default `/MediaBox` | **`(0, 0, 612, 792)`** when empty | cpdf_page.cpp:267-270 |
| `/Rotate` normalization | `((n / 90) % 4 + 4) % 4` | cpdf_page.cpp:227-232 |

`pdfrum-common::Limits` additions from this crate (the field list is
*(abridged)* in SPEC §1): `max_form_depth: 40`, `max_content_operands: 16`,
`max_content_word_len: 255`, `max_content_string_len: 32767`,
`max_text_clip_objects: 1024`, `max_colorspace_depth: 32` (**new**, D8),
`max_function_depth: 32` (**new**, D11),
`max_function_outputs: 1024` (**new**, D11), `max_ps_stack: 100`,
`max_ps_nesting: 128`, `max_shading_functions: 4`, `max_mesh_components: 8`,
`max_oc_ve_depth: 32`, `max_image_dimension: 131071`,
`max_image_bytes: u32::MAX as usize`, `image_cache_entries: 15`,
`image_cache_bytes: 100 << 20`.

### 1.21 Diagnostics mapping

| Event | `DiagKind` | Severity |
|---|---|---|
| unknown content operator | `UnknownOperator` | Suspicious |
| operand ring overflowed (>16 operands) | `OperandsDropped` | Suspicious |
| operator skipped by its arity guard | `OperandCountMismatch` | Suspicious |
| `Q` on an empty state stack | `UnbalancedRestore` | Suspicious |
| `EMC` past the sentinel | `UnbalancedMarkedContent` | Suspicious |
| form recursion refused (depth or repeat buffer) | `FormRecursionRefused` | Recovered |
| `BI` abandoned (non-`ID` keyword) | `InlineImageAbandoned` | Recovered |
| `EI` resync absorbed extra bytes | `InlineImageResync` | Recovered |
| inline image with an unsupported filter (JPX/JBIG2) | `InlineImageUnsupported` | Suspicious |
| `Tr` outside 0..7 ignored | `BadTextRenderMode` | Suspicious |
| dash array abandoned (non-finite / sub-threshold) | `DashPatternDropped` | Recovered |
| dash element substituted (`<= 1e-6 → 0.1`) | `DashElementClamped` | Recovered |
| colorspace failed to load | `ColorSpaceUnsupported` | Suspicious |
| ICC profile rejected → alternate | `IccAlternateUsed` | Recovered |
| ICC profile rejected → stock fallback | `IccStockFallback` | Recovered |
| ICC `/N` vs alternate component mismatch | `IccAlternateMismatch` | Suspicious |
| Indexed `/hival` clamped | `IndexedHivalClamped` | Recovered |
| Separation tint transform dropped | `TintTransformDropped` | Suspicious |
| function failed to load | `FunctionUnsupported` | Suspicious |
| PS stack overflow / underflow | `PostScriptStackAbuse` | Suspicious |
| PS malformed `if`/`ifelse` | `PostScriptMalformedProc` | Suspicious |
| shading validation failed | `ShadingUnsupported` | Suspicious |
| mesh `/Decode` length wrong | `MeshDecodeMalformed` | Suspicious |
| mesh stream truncated mid-record | `MeshTruncated` | Recovered |
| tiling step zero / non-finite → not drawn | `TilingStepInvalid` | Suspicious |
| tiling tile count out of range → not drawn | `TilingRangeOverflow` | Suspicious |
| image BPC rejected | `ImageBadBitDepth` | Suspicious |
| image dimensions rejected | `ImageBadDimensions` | Suspicious |
| DCT dimensions overrode the dict | `ImageDimensionsFromCodec` | Recovered |
| JPX colorspace overridden by the conversion table | `JpxColorSpaceOverride` | Recovered |
| JPX/JBIG2/DCT decode failed | `ImageDecodeFailed` | Suspicious |
| mask failed to load, base image kept | `MaskDropped` | Recovered |
| truncated image stream zero-padded | `ImageStreamTruncated` | Recovered |
| colour-key `/Mask` array too short | `ColorKeyArrayShort` | Suspicious |
| page `/MediaBox` empty → Letter default | `MediaBoxDefaulted` | Recovered |
| unknown `/P` in an OCMD → invisible | `OptionalContentPolicyUnknown` | Suspicious |

`Diagnostic.at` carries the content-stream byte offset where one is known.

---

## 2. Divergences

**D1 — No progressive/pausable parsing.** `CPDF_ContentParser`'s
`Stage`/`PauseIndicatorIface` machinery and the `kParseStepLimit = 100`
budget exist purely for incremental rendering. `parse_content` runs to
completion. Every *data* decision the stages make (the `/Contents` array walk,
the space-separated concatenation, the `kCheckClip` post-pass) is preserved.
Consequence: none observable — the budget only controls when `Continue`
returns, never what is produced.

**D2 — Insertion-ordered dicts.** Inherited from the object crate
(SPEC §2). The only place dict iteration order is observable in this crate is
`CPDF_ColorSpace::Load`'s stream form (§1.14.1 step 3), which scans a stream's
dict for the first Name that is a stock colorspace. Real files have exactly
one such key, so the divergence is unobservable; a crafted file with two
could differ. Documented, accepted.

**D3 — No mutation of parsed objects.** C++ writes `/Type /Page` into a page
dict that lacks it (cpdf_page.cpp:33-36), rewrites inline-image
abbreviations in place, forces `/Subtype /Image`, and writes a computed
`/Length` back into the inline-image dict. Our parsed objects are immutable;
the equivalent normalizations happen in the *constructed* `Page` /
`InlineImage` records. The saved-file consequence is `pdfrum-edit`'s problem
(its brief must re-derive the same normalized values).

**D4 — `ObjectsHeldByParser` erased.** The C++ parser holds `RetainPtr`s to
inline objects so they outlive the parse. Rust ownership makes this vacuous;
noted so nobody looks for it. **No behavioral content.**

**D5 — Line cap and join are clamped, not cast.** C++ does an unchecked
`static_cast` of the `J`/`j` operand and the ExtGState `/LC` `/LJ` values into
a 3-valued enum, so `5 J` produces an out-of-range enum that the AGG driver
then `switch`es on and lands in its `default:` arm (miter/butt). We clamp to
`0..=2` at parse and record a diagnostic. **Observably identical** given every
backend's default arm, and it removes an invalid-enum hazard.

**D6 — Dash normalization moves from the driver to the page layer.** The
non-finite check, the 0.1-cycle threshold, and the `<= 1e-6 → 0.1`
substitution live in the AGG driver (`cfx_agg_devicedriver.cpp:336-386`). They
are pure functions of the dash array and the device scale, so `pdfrum-page`
computes a normalized `StrokeParams` once and both backends consume it
identically. This also fixes a real C++ inconsistency: the Skia backend
implements a *different* normalization (doubling odd-length arrays instead of
letting the dasher cycle), so PDFium's two backends disagree. We pick the AGG
behavior, which is what the oracle produces.

**D7 — The transfer-function stale-output bug is not ported.** When a
single-function `/TR` has `OutputCount() > 16`, C++ skips the call and reuses
the **previous iteration's** `output[0]` (cpdf_docrenderdata.cpp:135-137),
producing a garbage ramp. We use identity for that channel. Affects only
functions with >16 outputs feeding `/TR`, which no real file has; flagged for
conformance.

**D8 — An explicit colorspace depth cap.** The C++ has *no* numeric cap; it
relies on the visited sets plus the native stack. A crafted file with a
10000-deep `/Indexed` chain of distinct arrays would overflow. We add
`Limits::max_colorspace_depth = 32` alongside the visited sets. Justified by
STYLE §3 (no panics) and the fuzz targets. Exceeding it yields "no
colorspace", the same result the C++ reaches by crashing.

**D9 — `std_conversion` is a parameter, not a refcount.** The C++
`EnableStdConversion` refcount with `CPDF_BasedCS` cascading exists because
colorspaces are shared, mutable, retained objects. Our colorspaces are
immutable values; the flag is threaded as a `bool` into `to_rgb` and the image
conversion functions. Same behavior, no shared mutable state (STYLE §1).

**D10 — `Pattern` has no `to_rgb`.** C++ makes `CPDF_PatternCS::GetRGB`
`NOTREACHED()`. In Rust, `ColorSpace::to_rgb` simply has no meaningful
`Pattern` arm; pattern colour resolution is a separate function on
`PatternValue`. The unreachable case is unrepresentable rather than a crash.

**D11 — Function depth and output caps.** The C++ has neither a type-3
nesting depth limit (only a cycle set) nor any cap on `outputs_`. Both are DoS
vectors for a `Vec`-allocating Rust implementation. We add
`max_function_depth = 32` and `max_function_outputs = 1024`. Exceeding either
yields a failed function load — the same outcome as the C++'s stack overflow
or OOM, but survivable.

**D12 — The sampled-function negative-index wrap is reproduced explicitly.**
`(u32)encoded_input` in C++ wraps a negative float to a huge unsigned value
which then clamps to the **top** cell; Rust's `as u32` saturates negatives to
**0**. We write the wrap out (`if x < 0.0 { sizes - 1 } else { (x as u32).min(sizes-1) }`)
because it is Tier-A observable through shading and Separation colour.

**D13 — The PS engine is not shared mutable state.** C++ holds a `mutable`
engine on the function and mutates it from a `const` method. Ours is
constructed per `eval` (the stack is 100 `f32`s — allocation-free on the
Rust stack), making `Function: Sync` honestly.

**D14 — Matrix operand order.** `CFX_Matrix::operator*` is row-vector
("apply left, then right"); `kurbo::Affine`'s `*` is column-vector
("apply right, then left"). Every formula in this brief is written in **C++
order**; the implementation swaps the operands. This is a notation
divergence, not a behavioral one, and is called out at every site because
getting it wrong is silent and catastrophic.

**D15 — Pattern cache keyed on `(ObjRef, parent_matrix)`.** C++ caches a
pattern by object identity alone, so the second user of a pattern silently
inherits the first user's `parent_matrix`. Real files reuse a pattern at one
nesting level, so this is unobservable in practice — but it is a genuine bug
we decline to reproduce. Flagged for conformance (Open question Q6).

**D16 — The shading LUT off-by-one IS reproduced.** Sampling at `i/256` while
indexing at `s*255` is a real, pixel-visible artifact on smooth gradients.
Both halves ported verbatim.

**D17 — Degenerate axial axis handled explicitly.** A zero-length axis divides
by zero in C++, and the subsequent float→int cast of ±inf/NaN is UB. We treat
it as "every pixel is out of range at the start end" (which is what the UB
produces on x86 for NaN) and record a diagnostic.

**D18 — `GetShadingBBox`'s counter-mutation bug is not ported.** The C++ mesh
bbox helper permanently shrinks its `point_count`/`color_count` after the
first flagged patch, under-reading every later record and producing a
too-small bbox that can clip visible output. We compute the correct bbox.
Flagged (Open question Q5) because it can only *grow* the `sh` object's rect
relative to the oracle.

**D19 — Uninitialized colour-key ranges get an explicit default.** When a
`/Mask` array is too short, C++ leaves `color_key_min/max` uninitialized while
still setting `color_key = true`. We default them to `0/0`. Any behavior
difference here is a difference from *undefined* behavior.

**D20 — `/K` (knockout) is parsed but not honored.** SPEC §8 asks for
knockout groups in the model, and the flag is genuinely in the PDF. The oracle
never reads it, so honoring it would *lose* Tier-B points. We parse `/K` into
`Transparency { group, isolated, knockout }` and the render engine ignores it
until the user decides otherwise (Open question Q4).

**D21 — Content-stream tokenizer is separate from the file lexer.** The C++
has two tokenizers with different caps (255 vs 256 word bytes, a 32767 string
cap that the file lexer lacks, a stricter dict grammar, the
`allow_nested_array` rule). We keep them separate for the same reason: the
divergences are observable. `pdfrum-parser`'s lexer is **not** reused here.

**D22 — Image decoding returns owned pixel data, not a lazy scanline
source.** `CPDF_DIB` is a `CFX_DIBBase` that produces scanlines on demand,
with a `line_buf_`/`mask_buf_`/`src_remainder_buf_` triple of mutable
scratch buffers. SPEC §7 pins `decode_image -> ImageData` (owned). Every
per-scanline behavior (the truncated-stream zero pad, the cached-bitmap row
clamp, the palette packing) is preserved; only the laziness is dropped.
Memory cost is bounded by the same `u32` size cap.

---

## 3. Module plan

```
crates/pdfrum-page/src/
  lib.rs            // re-exports: Op, parse_content, Page, PageObject, GraphicsState,
                    // build_page, ColorSpace, Function, Shading, Pattern, ImageData,
                    // decode_image, decode_jbig2, decode_jpx, Error

  ops.rs            // the `ops!` macro invocation (STYLE §2b): generates `enum Op`,
                    // the operand arity/type table, the byte-string -> Op dispatch,
                    // and Debug names, from one declaration. §1.3 is its source.
  tokenize.rs       // ContentLexer: the content-stream tokenizer (§1.1) — Element enum,
                    // word scanner, literal/hex string readers with the 32767 cap,
                    // the object reader with the allow_nested_array rule.
  content.rs        // parse_content: Element stream -> Vec<Op>; the operand ring (§1.2),
                    // the `m`-triggered path fast loop (§1.6), inline-image scanning (§1.12).
  inline_image.rs   // BI/ID/EI: abbreviation tables, dict scan, length inference,
                    // the EI resync loop.

  state/
    mod.rs          // GraphicsState (SPEC §7), StateStack (Vec<GraphicsState>)
    graph.rs        // StrokeParams + the dash normalization ladder (§1.4)
    text.rs         // TextState, TextRenderMode, the text/line matrix bookkeeping (§1.9)
    general.rs      // GeneralState: alphas, blend, soft mask, TR, and the inert fields
    extgstate.rs    // the /gs key table (§1.5) as one `apply_ext_gstate` function
    clip.rs         // ClipStack: paths + text layers, auto-merge, the 1024 cap (§1.6)
    marks.rs        // ContentMarks: the mark stack, sentinel, MCID lookup (§1.3)

  build.rs          // build_page: the fold from &[Op] + Resources -> Page.
                    // Owns resource lookup (§1.8), form recursion (§1.7),
                    // path assembly (§1.6), text object construction (§1.9).
  page.rs           // Page, PageObject, the *Object records, box/rotation derivation (§1.7)
  resources.rs      // Resources: the chosen dict + page fallback (§1.7)

  color/
    mod.rs          // ColorSpace enum, to_rgb, n_components, default_color
    load.rs         // the family dispatch + visited sets + depth cap (§1.14.1)
    device.rs       // Gray/RGB/CMYK incl. the std-conversion switch
    cie.rs          // CalGray, CalRGB, Lab + XYZ/sRGB helpers (§1.14.2, .4, .5)
    icc.rs          // ICCBased: /N validation, the fallback ladder, moxcms glue (§1.14.6)
    indexed.rs      // Indexed (§1.14.7)
    special.rs      // Separation, DeviceN, Pattern (§1.14.8-.10)
    srgb_table.rs   // the two sRGB gamma tables, verbatim
    cmyk_table.rs   // the 6561-entry Adobe CMYK table, verbatim
    value.rs        // ColorValue, PatternValue, the colorstate SetColor rule (§1.14.11)

  function/
    mod.rs          // Function enum + eval + load dispatch (§1.15.1)
    sampled.rs      // type 0 (§1.15.2), incl. the bit reader
    exponential.rs  // type 2 (§1.15.3)
    stitching.rs    // type 3 (§1.15.4)
    postscript/
      mod.rs        // type 4: parse + eval
      op.rs         // the PsOp enum + the sorted name table
      eval.rs       // the stack machine (§1.15.5)

  shading/
    mod.rs          // Shading enum, load + validate (§1.16.2)
    axial.rs        // type 2 parameter math (§1.16.5)
    radial.rs       // type 3 quadratic solve (§1.16.6)
    function.rs     // type 1 (§1.16.7)
    mesh.rs         // MeshStream: bit reading, per-type vertex decoding (§1.16.8)
    steps.rs        // the 256-entry colour LUT (§1.16.4)

  pattern/
    mod.rs          // Pattern enum, the matrix rule, /PatternType dispatch (§1.16.1)
    tiling.rs       // tiling load + the step/tile-range ladder (§1.16.9)

  transparency.rs   // Transparency, GroupAttrs, SoftMask parsing (§1.17)
  optional.rs       // OcContext: the visibility predicate (§1.18)
  transfer.rs       // TransferFunc: the 3x256 LUT (§1.5)

  image/
    mod.rs          // ImageData, decode_image: the load ladder (§1.19.2)
    dict.rs         // dimension/BPC validation + the repair ladder (§1.19.1)
    decode_array.rs // /Decode parsing and application (§1.19.4)
    dct.rs          // zune-jpeg glue, the component-mismatch gate, MCU alignment (§1.19.6)
    jpx.rs          // hayro-jpeg2000 glue + the full conversion table (§1.19.7)
    jbig2.rs        // hayro-jbig2 glue + /JBIG2Globals (§1.19.8)
    mask.rs         // colour-key, stencil, /SMask, /Matte (§1.19.9)
    scanline.rs     // bit unpacking, palette, the per-bpc conversions (§1.19.10)
    cache.rs        // ImageCache: the 15-entry + 100 MiB policy (§1.19.11)

  error.rs          // Error (thiserror)
```

### The `ops!` macro

`ops.rs` is one macro invocation whose rows come straight from §1.3. Shape:

```rust
ops! {
    //  spelling  variant              operands                       guard
    b"q"       => SaveState             ()                             ,
    b"Q"       => RestoreState          ()                             ,
    b"cm"      => Concat                (Affine)                       ,
    b"w"       => SetLineWidth          (f32)                          ,
    b"J"       => SetLineCap            (LineCap)                      ,
    b"d"       => SetDash               (SmallVec<[f32; 4]>, f32)      ,
    b"m"       => MoveTo                (Point)                        exactly 2,
    b"l"       => LineTo                (Point)                        exactly 2,
    b"c"       => CurveTo               (Point, Point, Point)          ,
    b"rg"      => SetFillRgb            (f32, f32, f32)                exactly 3,
    b"k"       => SetFillCmyk           (f32, f32, f32, f32)           exactly 4,
    b"Tz"      => SetHorzScale          (f32)                          exactly 1,
    b"scn"     => SetFillColorN         (SmallVec<[f32; 4]>, Option<Name>) ,
    b"TJ"      => ShowTextAdjusted      (Vec<TextItem>)                ,
    b"BX"      => BeginCompat           ()                             ,
    // … all 73 rows …
}
```
It generates: `pub enum Op` (one variant per row, plus
`Unknown(Box<[u8]>)`); `fn op_for(word: &[u8]) -> Option<OpKind>`; the
per-operator operand extraction from the ring (each row's operand list is a
sequence of `from_operands(&ring, index)` calls in the documented index
order); the guard check; and `Debug` names. Rows with `exactly N` emit the
`param_count != N → skip + diagnostic` check; rows without take whatever is
present with the zero/empty/null defaults of §1.2. **Nothing else in the crate
enumerates operators**, so adding one is a single-line edit and every match
site fails to compile if the enum changes (STYLE §1).

### Key internal types beyond SPEC

- `Element<'a>` — the content tokenizer's output (`Number(f32) | Name(&'a [u8])
  | Keyword(&'a [u8]) | Object(Object) | Eof`), mirroring `ElementType`.
- `OperandRing { slots: [Operand; 16], start: u8, count: u8 }` — a plain record
  with `push`, `get_number(i)`, `get_string(i)`, `get_object(i)`, `clear`. The
  eviction rule of §1.2 lives here and nowhere else.
- `Resources { chosen: Option<Dict>, page: Option<Dict> }` with
  `find(kind, name)` implementing the one-level fallback of §1.7.
- `FormDepth { in_flight: HashSet<*const u8 → ByteSpan range>, depth: u32 }` —
  the C++ pointer set becomes a set of `(ObjRef, byte range)` keys, which is
  the same identity with a safe representation.
- `StrokeParams { width, cap, join, miter, dash: SmallVec<[f32; 4]>, phase }`
  — already normalized per §1.4/D6.
- `TextCursor { matrix: Affine, line_matrix: Affine, pos: Point, line_pos:
  Point, leading, rise, horz_scale }` — the `Td`/`TD`/`T*`/`'`/`"` bookkeeping
  of §1.9 as one small record with five methods.
- `MeshReader<'a>` — the bit reader plus the decode ranges; `read_flag`,
  `read_coords`, `read_color`, `read_vertex`, `read_vertex_row`, `byte_align`.
- `ImageDict` — the validated `(width, height, bpc, components, family,
  image_mask, decode, default_decode)` record produced by `image/dict.rs`,
  so the decoders never re-read the dict.

### Data flow

```
bytes ──parse_content──▶ Vec<Op>          (pure, infallible, diagnostics only)
                             │
Resources + &impl Resolve ───┤
                             ▼
                        build_page  ────▶ Page { objects, boxes, rotation }
                             │
                             ├─▶ colorspace::load  (cached per ObjRef)
                             ├─▶ function::load    (cached per ObjRef)
                             ├─▶ pattern/shading::load
                             ├─▶ decode_image      (cached per (ObjRef, max_size))
                             └─▶ build_page (recursive, for form XObjects)
```
Every stage is a function over values. The only interior mutability is the
three caches, each an owned `HashMap` on a `PageBuildCtx` passed by `&mut`
down the fold — no `Arc<Mutex<…>>`, no globals (STYLE §1).

### Decomposition notes (STYLE §7)

- `CPDF_StreamContentParser` (1798 lines, 73 methods, 24 fields) becomes:
  `tokenize.rs` (bytes → elements), `ops.rs` (the table),
  `content.rs` (elements + ring → `Vec<Op>`), and `build.rs` (ops → objects).
  Nothing holds both "where am I in the bytes" and "what does the page mean".
- `CPDF_ColorSpace` + 9 subclasses become one enum and one module per family;
  `v_Load` returning 0-for-failure becomes `Result<ColorSpace, Error>`.
- `CPDF_DIB` (1329 lines, a `CFX_DIBBase` subclass with 20 fields and three
  scratch buffers) becomes `ImageDict` (validated facts) +
  a decoder-selection function + per-format conversion functions +
  an owned `ImageData`.
- `CPDF_AllStates` / `CPDF_GraphicStates` / the five `SharedCopyOnWrite`
  state classes become one `GraphicsState` record with `Arc`'d heavy fields,
  cloned on `q`.

---

## 4. Test plan

### Ported unittest assertions

`cpdf_streamcontentparser_unittest.cpp:6-44` — the abbreviation tables:
- Key: `BPC → BitsPerComponent`, `W → Width`, `"" → ""`, `NoInList → ""`,
  and **`WW → ""`** (prefixes must not match).
- Value: `G → DeviceGray`, `DCT → DCTDecode`, `"" → ""`, `NoInList → ""`,
  **`II → ""`**.

`cpdf_streamparser_unittest.cpp:11-50` — `ReadHexString`, all five cases with
exact bytes and end positions: position past the end → empty;
`"1A2b>abcd"` → `[0x1a, 0x2b]`, pos 5; `"1A2b"` (no `>`) → `[0x1a, 0x2b]`,
pos 5; `"1A2>asdf"` → **`[0x1a, 0x20]`** (odd nibble padded), pos 4;
`">"` → empty, pos 1.

`cpdf_page_unittest.cpp:18-99` — `IsValidPageDict` / `IsValidPageDictLoose`,
restated over our `PageDict` validation: null → false for both; no `/Type` →
**loose true, strict false**; `/Type /Font` → false; `/Type` as a **String**
`"Page"` → false; `/Type` null → false; `/Type` as a **Reference** to the Name
`Page` → true, to `Pages` → false, to Null → false.

`cpdf_pageobjectholder_unittest.cpp:26-86` — `GraphicsDataAsKey`: the
`FXSYS_SafeLT` total order over floats including min/max/inf/NaN, the exact
72-element permutation table, and map insert/erase round-tripping. Ported as a
property test on our `GraphicsKey` ordering (used by the edit crate's
resource dedup); the NaN ordering is the load-bearing part.

`cpdf_colorspace_unittest.cpp:18-40` — both `TranslateImageLine` tests with
exact bytes:
- **CalGray**: src `[255,0,0,0,255,0,0,0,255,128,128,128]`, 4 pixels →
  `[255,255,255, 0,0,0, 0,0,0, 0,0,0]` (1 byte per pixel, replicated;
  src[4..] unused).
- **CalRGB**: same src, 4 pixels → `[0,0,255, 0,255,0, 255,0,0, 128,128,128]`
  (a pure red↔blue swap — **this is the test that pins "CalRGB's bulk path
  ignores gamma/matrix/whitepoint"**).

`cpdf_devicecs_unittest.cpp:12-129` — every `GetRGB` vector:
- Gray: `[0.43, 0.11, 0.34]` → 0.43 ×3 (only component 0); `0.872` → 0.872 ×3;
  `0.0` → 0; `1.0` → 1; **`-0.01` → 0.0** and **`12.5` → 1.0** (clamping).
- RGB: `(0.13, 1.0, 0.652)` identity; `(0.0, 0.52, 0.78)` identity;
  **`(-10.5, 100.0, 0.78)` → `(0.0, 1.0, 0.78)`**.
- **CMYK, the Adobe table vectors** — the single best conformance set for the
  6561-entry table port:
  - `(0.6, 0.5, 0.3, 0.9)` → `(0.0627451, 0.0627451, 0.10588236)`
  - `(0.15, 0.5, 0.0, 0.9)` → `(0.2, 0.0862745, 0.16470589)`
  - `(0.15, 0.5, 1.0, 0.0)` → `(0.85098046, 0.552941, 0.15686275)`
  - `(0.15, 0.5, 1.5, -0.6)` → **the same as the previous case** (clamping)

  Every value is an exact `n/255`, confirming the float API is a thin wrapper
  over the u8 table.

`cpdf_function_unittest.cpp:13-43` — all four negative cases:
`/FunctionType -2` and `5` → load fails; **no `/Domain`** → fails;
**empty `/Domain`** → fails (`inputs == 0`); **type 0 with no `/Range`** →
fails.

`cpdf_psengine_unittest.cpp:44-289` — the whole file:
- `AddOperator`: the 46-entry table mapping all 42 spellings to their ops,
  plus constants `"55" → 55.0`, `"123.4" → 123.4`, `"-5" → -5.0`, and
  **`"invalid" → 0.0`**.
- `Basic`: `100 200 add → 300`; `100 150 sub → -50`; `5 120 mul → 600`;
  `15 10 div → 1.5`; `15 10 idiv → 1.0`; `15 10 mod → 5.0`;
  `-5 neg → 5.0`; `-5 abs → 5.0`.
- `DivByZero`: `100 0 idiv → 0`, `100 0 mod → 0`, `100 0 div → 0`.
- `Ceiling`, `Floor`, `Round`, `Truncate`: all 13-16 vectors each, including
  `f32::MIN_POSITIVE`, `f32::MAX`, and their negations. The load-bearing ones:
  **round `5.5 → 6.0` and `-5.5 → -5.0`** (half-up asymmetry);
  **truncate `f32(i32::MAX) * -1.5 → -f32(i32::MAX)`** (the saturation case,
  crbug 42270316).
- `Comparisons`: 24 assertions over `(0,0) (0,1) (255,1) (-1,0)` × 6 ops.
- `Logic`: `true→1`, `false→0`; AND/OR/XOR truth tables; `not 0→1`, `not 1→0`.
- `MathFunctions`: `2 sqrt → 1.4142135`; `60 sin → 0.8660254`;
  `60 cos → 0.5`; **`1 1 atan → 45.0`**; `10 3 exp → 1000.0`;
  `1000 log → 3.0`; `10 ln → 2.302585`.

`cpdf_dib_unittest.cpp:56-92` — three default-decode scanline cases on a 2×1
DeviceRGB image (*"Two pixels is enough to catch a wrong stride"*):
- **4 bpc**: `[0x08, 0xff, 0x08]` → `[0xff, 0x88, 0x00, 0x88, 0x00, 0xff]`
  (each nibble scaled by 255/15; BGR order).
- **8 bpc**: `[0x11..0x66]` → `[0x33,0x22,0x11, 0x66,0x55,0x44]` (RGB→BGR).
- **16 bpc**: 12 bytes → `[0x55,0x33,0x11, 0xbb,0x99,0x77]` — **only the high
  byte of each big-endian sample survives**.

`cpdf_pageimagecache_unittest.cpp:27-252` — four resolution/cache cases,
ported as integration tests over `decode_image` + `ImageCache`:
- `RenderBug1924` (`jpx_lzw.pdf`): a 50×50 request followed by a 100×100 one
  must produce a **strictly larger** bitmap the second time.
- `RenderReducedDctThenFullSize` (`jpeg_reduced_size.pdf`, 400×400): 50×50 →
  **exactly 50×50** (1/8); then 100×100 → **exactly 100×100** (1/4), forcing a
  re-decode.
- `RenderReducedDctWithSMask`: base **50×50** but mask **400×400** — the mask
  is always full resolution.
- `NonMcuAlignedDctIsNotReduced` (408×408, 4:2:0): a 50×50 request still
  yields **408×408**, because `408 % 16 == 8`.

### Hand-written tests pinning heuristics with no C++ unittest

**Content parsing**: 20 operands before one operator (the last 16 survive,
renumbered); an operator with too few operands (zeros); `rg` with 2 operands
(skipped) vs `w` with 0 (line width 0); an unknown operator clears operands;
`true`/`false`/`null` never dispatch as operators; a 256-byte name truncates;
a 40000-byte string truncates to 32767; `[1 [2] 3]` at operator level yields
`[1, 3]` but the same inside a dict value nests; `%comment` inside a content
stream; the `m`-fast-loop rewinding on an unknown keyword; a 7th number in the
fast loop dropped; multi-stream concatenation splitting an operator across the
boundary.

**Inline images**: `BI` followed by a non-`ID` keyword abandons and re-parses;
each of the 9 key and 11 value abbreviations; `I` as a key vs as a value;
unfiltered length under-run (surplus tokenized as content) and over-run
(clamped, then `EI` scan); a `(string containing EI)` inside the data being
absorbed; `JPXDecode` and `JBIG2Decode` inline images producing nothing;
exactly one whitespace byte after `ID` consumed, a second one becoming data.

**Forms**: 41 nested forms parse and the 42nd is refused; a self-referential
form is refused immediately; two sequential `Do`s of the same form both work;
a form with no `/BBox` is unclipped; a form whose `/Resources` has only
`/Font` cannot see the page's `/XObject` (the whole-dict rule) but a *missing
category* does fall back one level; a transparency-group form resets blend and
alphas.

**Graphics state**: every ExtGState key from §1.5, including `/TR` suppressed
by `/TR2`, `/OP` setting `fill_op` only when `/op` is absent, `/BG` suppressed
by `/BG2`, `/Font` as `[name size]`, `/SMask /None` clearing the mask,
`/CA 1.5` clamping to 1.0; a `/D` whose element 0 is not an array; dash
normalization for `[0 3]`, `[-2 3]`, `[NaN 3]`, `[0.01 0.01]`, `[3]`.

**Text**: `Tz 150` storing 1.5; `TD 0 -14` setting leading to 14; `T*`
without `TL`; `"` operand order; `TJ` with no strings (pure kerning, x only,
even for vertical fonts); `TJ` with accumulating adjacent numbers; a `TJ`
whose leading number applies before an empty-strings early return; a
clip-mode `BT…ET` with no glyphs; 1025 clip text objects dropping the batch.

**Colorspaces**: the 1-element array form; `[/DeviceRGB 1 2]` dispatching to
DeviceN and failing; `Lab2` not matching Lab; `/WhitePoint [0.95 0.99999
1.08]` failing the load; `/Range []` on Lab yielding zeros; Lab's out-of-order
range falling back to 0/100; Indexed `/hival` of `-5` and `999` clamping;
Indexed with a String vs a Stream table; Indexed with a Pattern base failing;
Separation `/None` painting nothing; Separation `/All` treated as ordinary;
Separation with no function broadcasting the tint; DeviceN with no function
failing; Pattern with an unloadable base succeeding with 1 component; an ICC
profile whose channel count disagrees with `/N`; `/Alternate` with the wrong
component count falling through to the stock space; the exact 3144-byte sRGB
detection; `/DefaultRGB` substitution through both the `cs` and image paths.

**Functions**: `/BitsPerSample 3` rejected, `12` and `24` accepted; a sampled
function with `Size [1]` exercising the `sizes == 1` multiply quirk; a
negative encoded input landing on the **top** cell; type 2 with `N = -1` and
input 0 (inf, clamped by `/Range`); type 2 with 2 inputs zero-clamping the
extra outputs; type 3 with out-of-order bounds; type 3 with a longer-than-
needed `/Encode`; type 3 with a self-referencing sub-function; a 129-level
`{}` nesting rejected and 128 accepted; PS stack overflow at 101 pushes; PS
`if` without a preceding proc aborting the proc; `ifelse` selecting `i-2` for
true; `0.5` truthiness reading as false; `copy`/`index`/`roll` bounds cases;
`bitshift` with `INT_MIN`.

**Shadings and patterns**: each of the 7 types validating and failing; a mesh
type with a dict shading object rejected; 5 functions in the array capped at
4; a null function entry rejecting the shading; `/Extend [0 1]` as integers
yielding both-false; `/Background` on an `sh` shading ignored; a `/BBox` with
3 elements collapsing the clip; the axial LUT sample/index off-by-one on a
known gradient; a degenerate axial axis; each radial branch (b≈0, a≈0,
general) and the decreasing-root selection; mesh `/Decode` of the wrong
length; `/BitsPerFlag 3` rejected for type 4 but ignored for type 5;
flag 3 behaving as flag 2; the `(flag*3+i)%12` patch reuse for each flag;
`/VerticesPerRow 1` rejected; tiling with `/XStep 0`, `-5` (→5), NaN, and
1e-30; a `/PaintType 2` tile taking its colour from `scn`.

**Transparency and optional content**: `/S` missing on a soft mask defaulting
to luminosity; `/TR /Identity` ignored; `/BC` with 12 values reading only 8;
`/BC` with a Lab group colorspace falling back to black; `/K true` parsed but
not honored; each of the four `/P` policies plus an unknown one; `/VE` at
depth 33 rejected; a `/VE` `And` whose element 1 is null yielding false;
a `BDC /OC << >>` direct dict leaving the object visible.

**Images**: BPC 3, 5, 0, 17 rejected; BPC 1 with `CCITTFaxDecode`;
`/ImageMask true` with a `/ColorSpace` ignoring it; missing `/ColorSpace` with
`JPXDecode` not becoming a mask; a `/Decode` array of the wrong length;
`/Decode [1 0]` on a mask not inverting; a DCT whose dimensions differ from
the dict; a DCT with a component mismatch against each colorspace family; the
full JPX conversion table (both with and without a PDF colorspace); JPX
`/SMaskInData 1` un-premultiplying and 2 dropping alpha; a colour-key `/Mask`
array shorter than `2n`; `/SMask` suppressing a colour-key `/Mask`; `/Matte`
with the wrong length ignored; a truncated image stream zero-padding; the
15-entry cache eviction firing before the byte limit.

**Page setup**: `/MediaBox` empty → Letter; `/CropBox` outside `/MediaBox`
intersecting; `/Rotate 45` → 0, `-90` → 3, `450` → 1; `/Resources` inherited
through `/Parent` with a cycle.

### Snapshot tests (`insta`)

For ~25 crafted content streams checked into `tests/content/`, snapshot
`(Vec<Op> debug, diagnostics)`. For ~15 crafted pages, snapshot
`(page-object summary — type, rect, colorspace family, blend, alpha —
plus diagnostics)`. For the colour tables, snapshot 64 sampled conversions
from each family so a table transcription error is caught immediately.

### Fuzz targets

`fuzz_parse_content` (bytes → `Vec<Op>`, must never panic and must terminate);
`fuzz_inline_image` (a `BI`-prefixed corpus, seeded from the oracle's
`testing/fuzzers/` inline-image corpus); `fuzz_colorspace_load`;
`fuzz_function_eval` (a function object plus random inputs);
`fuzz_psengine` (a `{}` program plus a random stack);
`fuzz_mesh_stream`; `fuzz_decode_image` (with `decode_jbig2` and `decode_jpx`
fuzzed separately per SPEC §12, from the upstream corpora).

### Conformance clusters

M3 exit (paths, fills, strokes, clips, device colour, text): the
`bug_*` path/clip files, `text_*`, the `pixel/` regression set.
M4 (images): `--save-images` md5s Tier-A across the image corpus, split by
codec cluster (`jpeg_*`, `jpx_*`, `jbig2_*`, `ccitt_*`, `bmp/indexed`).
M5 (the rest of this crate): shading clusters `shading_type_1` … `type_7`,
`tiling_*`, `smask_*`, `blend_*`, `transparency_group_*`, `type3_*`,
`optional_content_*`.

The colour and function tables get their own non-corpus oracle: a small
harness that drives `pdfium_test` over generated single-object PDFs sweeping
each colorspace family and function type, comparing decoded pixel values
Tier-A. That is cheaper than chasing table typos through SSIM.

---

## 5. Open questions

**Q1 — `Limits` field growth (escalation to the orchestrator).** This brief
adds `max_colorspace_depth`, `max_function_depth`, and
`max_function_outputs` — three caps the C++ does not have (D8, D11). SPEC §1
marks the `Limits` field list *(abridged)*, so growth is sanctioned, but these
are the first fields that are **not** "mirror pdfium's hard limits"; they are
safety caps with no C++ counterpart, in the same spirit as the filters crate's
`max_decoded_stream_len` decision. Requested ruling: accept as proposed
(values 32 / 32 / 1024), or set different values. Conformance will confirm no
corpus file trips them.

**Q2 — moxcms vs lcms profile acceptance.** §1.14.6 pins the *ladder* around a
rejected ICC profile exactly, but "which profiles are rejected" is delegated
to the CMS. `moxcms` and lcms will disagree on some malformed profiles, and
the disagreement is observable: a profile lcms accepts and moxcms rejects
takes the `/Alternate`-or-stock path instead of a real transform, shifting
colour. Action: at M4, sweep every corpus file with an `/ICCBased` space and
diff decoded image bytes; if the divergence set is non-empty and small,
consider a first-party ICC parser for the rejection decision only, keeping
moxcms for the transform. Escalate if the set is large.

**Q3 — `Op` enum size.** With 75 variants carrying `SmallVec` and `Vec`
payloads (`TJ`, `d`, `scn`), `Op` will be roughly 40-56 bytes, and a
content-heavy page produces hundreds of thousands of them. That is a
`Vec<Op>` of tens of MB for a pathological page, versus the C++'s streaming
dispatch which never materializes the list. SPEC §7 pins
`parse_content -> Vec<Op>` and the two-stage purity is a real design win, so
the proposal is: keep `Vec<Op>`, box the three variable-length payloads
(`Box<[f32]>`, `Box<[TextItem]>`) to hold `Op` at 24 bytes, and add
`Limits::max_content_ops` only if a corpus file actually hurts. Confirm in
review.

**Q4 — Knockout groups (D20).** SPEC §8 says "Isolated/knockout groups:
engine-level compositing"; the oracle never parses `/K`. Implementing knockout
would *diverge* from the oracle and lose Tier-B points on any file that uses
it. Proposal: parse `/K` into the model, render as non-knockout to match the
oracle, and revisit only if the user wants correctness-over-parity here. This
is a genuine SPEC tension and needs a ruling.

**Q5 — Two C++ bugs we decline to port (D7, D18).** The transfer-function
stale-output reuse and the mesh-bbox counter mutation are both plainly wrong
and both *can* change output. D18 in particular can only make our `sh` object
rect **larger** than the oracle's, which shows up as extra pixels. Action: at
M5, count corpus files with flagged mesh patches; if any diverge, the fallback
is to port the bug behind a `Limits`-adjacent compatibility flag rather than
diverge. Flagging now so the M5 triage agent knows to look.

**Q6 — Pattern cache aliasing (D15).** C++ caches a pattern by object identity
and therefore reuses the first requester's `parent_matrix`. Our
`(ObjRef, matrix)` key is more correct but produces different output on any
file that uses one pattern from two nesting levels. Action: at M5, scan the
corpus for patterns referenced from more than one resource context; if the set
is empty (expected), the divergence is untestable and we keep the correct
behavior. If not, match the C++.

**Q7 — Type 3 fonts cross the crate boundary.** `d0`/`d1` and the
`colored_`/`type3_data_` handoff (§1.3, §1.0) are produced here but consumed
by `pdfrum-font`'s `Type3Font`, and Type 3 glyph rendering re-enters
`build_page` with the glyph's own content stream. That is a genuine cycle
between `pdfrum-page` and `pdfrum-font` at the *call* level, though not in the
dependency graph (font depends on page for nothing; page calls font). Proposal:
`build_page` takes an optional `&Type3Ctx` carrying the glyph's resources and
the recursion budget, and `pdfrum-font` calls `build_page` for a glyph rather
than the reverse. Confirm the direction in review before M5.

**Q8 — Image decode cache ownership.** SPEC §7 pins
`HashMap<ObjRef, Arc<ImageData>>` "per render/extract session", but §1.19.11's
policy is keyed on `(ObjRef, max_size_required)` and has a 15-entry LRU cap
plus a 100 MiB budget. A plain `HashMap<ObjRef, _>` cannot express the
resolution-dependent invalidation (`IsCacheValid`, §1.19.11) and would return
a 50×50 thumbnail to a full-resolution request. Proposal: keep the SPEC's
ownership story (session-owned, `Arc`'d values) but make the key
`(ObjRef, RequestedSize)` and add the eviction policy. This is a `[spec]`-shaped
refinement of an *(abridged)* line rather than a contract change; flagging so
the orchestrator can decide whether it wants a `[spec]` commit.

**Q9 — The two colour tables are large generated files.** `srgb_table.rs`
(445 bytes of data) is trivial; `cmyk_table.rs` is **6561 RGB triples ≈ 20k
lines** or one `include_bytes!` blob. Proposal: a checked-in
`tools/gen_cmyk_table.rs` that reads the C++ header and emits a
`static CMYK: [[u8; 3]; 6561]` into a generated file, with a test that
re-derives it and compares — same pattern as `pdfrum-cmap`'s `build.rs`
(PLAN §3). Confirm that a generated-but-committed table is acceptable rather
than a `build.rs` that would need the oracle checkout present.
