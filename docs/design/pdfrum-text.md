# Design brief — `pdfrum-text`

Behavior source: `pdfium-c++/core/fpdftext/` in full (`cpdf_textpage.cpp`,
`cpdf_textpagefind.cpp`, `cpdf_linkextract.cpp`, `unicodenormalizationdata.cpp`),
plus the pieces of `core/fxcrt/` that own text *semantics* rather than string
plumbing (`fx_bidi.cpp`, `fx_unicode.cpp` + `fx_ucddata.inc`), the
`CPDF_TextObject` item model in `core/fpdfapi/page/`, and the public-API layer
`fpdfsdk/fpdf_text.cpp` — which is not "SDK shim" here but the **definition of
what `--txt` emits**. Shape contract: SPEC.md §9 (binding). All C++ paths are
relative to `/mnt/data2/pdfium/pdfium-c++/`.

This crate is **Tier-A byte-exact** (PLAN §5, PLAN §6/M2). Its output is
compared byte-for-byte against the oracle's `--txt` dump for every page of
every corpus file, so *every* heuristic below is load-bearing: a single
misplaced generated space is a whole-file Tier-A failure. There is no
"perceptually close" here and no threshold to ratchet.

Two structural facts dominate everything that follows.

**Fact 1 — there are two texts, and `--txt` emits the one you would not
guess.** `CPDF_TextPage` maintains two parallel outputs: a `WideTextBuffer`
(`text_buf_`) and a `std::vector<CharInfo>` (`char_list_`). They are *not* in
one-to-one correspondence and they do not hold the same characters.
`FPDFText_GetText`/`GetPageText` read `text_buf_` through an index-mapping
table; `WriteText` — the `--txt` path — reads `char_list_[i].unicode()` for
every `i`, with **no filtering at all** (`write.cc:345-371`,
`fpdf_text.cpp:63-77`). Verified empirically against the built oracle:
`bug_781804.pdf` emits `U+0002` at the hyphen position where `GetText` returns
`U+FFFE`, and `control_characters.pdf` emits the raw `U+0002 U+0003` control
characters that `GetPageText` filters out. §1.2 and §1.13 pin this exactly.

**Fact 2 — the pipeline is a *reordering* pipeline, not a streaming one.**
Text objects are collected into `text_objects_`, insertion-sorted by
transformed x, flushed in batches on a y-jump, run through per-object
character emission into a *temporary* buffer, then bidi-segmented and moved
into the final buffer by `CloseTempLine`. Characters can be reversed, merged,
deleted, normalized into several characters, or dropped at four different
stages. Getting the stage ordering wrong changes the output even when every
individual threshold is right.

---

## 1. Behavior inventory

### 1.0 The pipeline, end to end

`CPDF_TextPage::Init()` (cpdf_textpage.cpp:378-403) is the whole entry point:

```
Init()
 ├─ text_buf_.SetAllocStep(10240)                 // allocation only, no behavior
 ├─ ProcessObject()                               // §1.1
 └─ build char_indices_ (the text_buf_ ↔ char_list_ index map)   // §1.13
```

`ProcessObject()` (:742-763):

```
if page->GetActivePageObjectCount() == 0: return       // note: ACTIVE count
textline_dir_ = FindTextlineFlowOrientation()          // §1.4, page-global
for page_obj in page (in content order):
    if !page_obj.IsActive(): continue
    if IsText: ProcessTextObject(obj, Matrix::IDENTITY, page, iter)   // §1.5
    if IsForm: ProcessFormObject(obj, Matrix::IDENTITY)               // §1.3
ProcessTransformedTextObjects()                        // §1.6 — flush the tail
text_objects_.clear()
CloseTempLine()                                        // §1.10 — flush temp buf
```

Note the asymmetry that a naive reading misses: `ProcessTextObject` is a
*collector* with an internal flush; the trailing `ProcessTransformedTextObjects`
+ `CloseTempLine` at the end of `ProcessObject` are what drain the last batch.
Anything still in `temp_char_list_` when `Init` returns is lost — but
`CloseTempLine` is unconditional, so nothing is.

Data members and their roles (cpdf_textpage.h:191-204):

| Member | Role |
|---|---|
| `char_indices_` | `Vec<{index, count}>` mapping text-buffer offsets ↔ char indices (§1.13) |
| `char_list_` | **The `--txt` output.** Final per-character records |
| `temp_char_list_` | Per-line staging; bidi-reordered into `char_list_` by `CloseTempLine` |
| `text_buf_` | Final text buffer for `GetPageText`; filtered differently from `char_list_` |
| `temp_text_buf_` | Per-line staging text buffer, parallel to `temp_char_list_` |
| `prev_text_obj_` / `prev_matrix_` | The previous *emitted* text object + its form matrix |
| `rtl_` | From `/ViewerPreferences /Direction (R2L)` — forces overall bidi direction |
| `display_matrix_` | `page->GetDisplayMatrix()` — page space → device space (§1.4a) |
| `text_objects_` | The pending reorder batch of `{text_obj, form_matrix}` |
| `textline_dir_` | Page-global orientation guess (§1.4) |
| `curline_rect_` | Union of rects of objects on the current output line |

### 1.1 Constants — the complete table

Every numeric constant in `core/fpdftext/`, with value, units, and the
function that owns it. **"fs-rel" = scales with font size; "abs" = absolute
in the coordinate space named.**

| Constant | Value | Units | Site | Meaning |
|---|---|---|---|---|
| `kDefaultFontSize` | `1.0` | text space | :41 | Font size when the text object has no font |
| `kSizeEpsilon` | `0.01` | page space (abs) | :42 | Degenerate-rect threshold; used 4× |
| `NormalizeThreshold` t1/t2/t3 (space) | `300, 500, 700` | glyph units (abs, /1000 em) | :225 | Bucket edges in `CalculateSpaceThreshold` |
| `NormalizeThreshold` t1/t2/t3 (gap) | `400, 700, 800` | glyph units (abs) | :1304 | Bucket edges in `ProcessInsertObject` |
| `NormalizeThreshold` divisors | `2, 4, 5, 6` | ratio | :48-61 | Divisor per bucket |
| char-space "is zero" band | `±0.001` | text space (abs) | :92,:95 | `CalculateBaseSpaceAdjustment` deadband |
| `EndHorizontalLine` min height | `4.5` | page space (abs) | :253 | Below this, never end a line |
| `EndVerticalLine` min width | `0.1 × font_size` | **fs-rel** | :267-268 | Per-object, using *that* object's size |
| newline y-threshold (positive) | `threshold × 2` | prev-text space | :1257 | `pos.y > 2·t` ⇒ candidate newline |
| newline y-threshold (negative) | `threshold × -3` | prev-text space | :1257 | `pos.y < -3·t` ⇒ candidate newline |
| newline y-magnitude floor | `1.0` | prev-text space (abs) | :1258 | `|pos.y| ≥ 1` **or** `|pos.y| > |pos.x|` |
| empty-rect newline height | `5.0` | page space (abs) | :1256 | `rect.IsEmpty() && rect_height > 5` |
| batch y-split threshold | `max(prev_w, this_w)/4 × 2` | device space | :919,:924 | `ProcessTextObject` flush trigger |
| `ProcessInsertObject` threshold | `max(last_w, this_w)/4` | text space | :1240 | Base for the newline y-test |
| `GetTextObjectWritingMode` epsilon | `0.0001` | text space (abs) | :1137 | Below ⇒ orientation unknown |
| `GetTextObjectWritingMode` threshold | `0.0872` | unit vector component | :1144 | ≈ sin(5°); the axis-alignment cone |
| `kTextCharRatioGapDelta` | `0.07` | **fs-rel** | :1439 | Dedup position epsilon, × font size |
| dedup lookback | `7` | count | :1438 | Max preceding chars scanned for a duplicate |
| space-threshold cap | `fontsize_h / 3` | fs-rel | :218 | Above this the space width is discarded |
| space-threshold halving | `/2` | ratio | :221 | Applied when the space width is kept |
| `threshold2` magic band A | `(1.4879, 1.4881)` | text space | :1313 | Exact-float band ⇒ `×1.5` |
| `threshold2` magic band B | `(1.38999, 1.39001)` | text space | :1314 | Exact-float band ⇒ `×1.5` |
| `IsSameTextObject` x tolerance | `0.9 × char_size × font_size/1000` | text space | :1537 | |
| `IsSameTextObject` y tolerance | `max_pre_size / 8` | text space | :1538 | `max_pre_size = max(h, w, font_size)` |
| `IsSameTextObject` width ratio | `current_rect.Width() / 2` | page space | :1502 | |
| `IsSameAsPreTextObject` lookback | `5` | count of **text** objects | :1546 | Non-text objects don't decrement the counter |
| `FindTextlineFlowOrientation` line multiple | `2 × fLineHeight` | page space | :702 | `fLineHeight` = first text object's height |
| `FindTextlineFlowOrientation` fill ratio | `0.8` | ratio | :711 | Horizontal mask fill ⇒ horizontal |
| `GetIndexAtPos` initial diffs | `5000` | page space | :486-487 | Sentinel for nearest-char search |
| `PreMarkedContent` printable band | `0x80 < wc < 0xFFFD` | code point | :986 | Or `wc ≤ 0x80 && isprint(wc)` |
| `ProcessMarkedContent` drop bound | `wc ≥ 0xFFFD` | code point | :1033 | Silently skipped |
| ligature normalization band | `0xFB00 ..= 0xFB06` | code point | :794 | Normalized even when LTR |
| `FXSYS_IsFloatZero` band | `(-0.0001, 0.0001)` | float | fx_system.h:36 | Used by `GetLooseBounds` |
| `kNonBreakingSpace` | `160` | code point | textpagefind:25 | Whitespace for search |
| find-what min length (links) | `> 5` | chars | linkextract:157 | Pre-trim length gate |
| link min length after trim | `> 5` | chars | linkextract:170 | Post-trim gate (`nCount`, not string length!) |
| `FindWebLinkEnding` ASCII bound | `< 0x80` | code point | linkextract:60 | Non-ASCII never trimmed |
| mail domain min span | `host_end - aPos ≥ 3` | chars | linkextract:292 | |

`NormalizeThreshold(threshold, t1, t2, t3)` (:48-61) — the shared bucketer,
called with two *different* bucket sets:

```
threshold < t1  → threshold / 2
threshold < t2  → threshold / 4
threshold < t3  → threshold / 5
otherwise       → threshold / 6
```

Called as `(w, 300, 500, 700)` from `CalculateSpaceThreshold` and
`(max(nLastWidth, nThisWidth), 400, 700, 800)` from `ProcessInsertObject`.
The `DCHECK_LT` ordering assertions are debug-only; the buckets are fixed
constants so there is nothing to validate at runtime.

### 1.2 `CharInfo` — the complete field inventory

`CPDF_TextPage::CharInfo` (cpdf_textpage.h:46-85). Constructed at
:350-365; `loose_char_box_` is **derived in the constructor** from all the
other fields via `GetLooseBounds` (§1.12), so it is not an independent input.

| Field | C++ type | Set by | Meaning / range |
|---|---|---|---|
| `char_type_` | `CharType` | ctor + `set_char_type` | See below |
| `unicode_` | `wchar_t` | ctor + `set_unicode` | **The `--txt` byte.** `0` is a legal value |
| `char_code_` | `uint32_t` | ctor only | The PDF charcode, or `kInvalidCharCode` = `0xFFFF_FFFF` |
| `origin_` | `CFX_PointF` | ctor only | Baseline origin, **already transformed** by `matrix` |
| `char_box_` | `CFX_FloatRect` | ctor only | Tight glyph box in page space |
| `loose_char_box_` | `CFX_FloatRect` | ctor (derived) | §1.12; equals `char_box_` in fallback cases |
| `matrix_` | `CFX_Matrix` | ctor only | For real chars: `text_matrix × form_matrix`. For generated chars: **the bare `form_matrix`** (:1400, :1586) |
| `text_object_` | `CPDF_TextObject*` | ctor only | `nullptr` for `GenerateCharInfo`-produced chars only |

The `matrix_` asymmetry is real and observable: `FPDFText_GetCharAngle` reads
it, and `GetLooseBounds` inverts it. A generated space produced inside
`ProcessTextObjectItems` (:1398-1401) gets `form_matrix`, *not* the composed
`matrix` that the surrounding real characters get.

`CharType` (:37-44) — six variants, and each has a distinct effect:

| Variant | Produced by | `--txt` effect | Other effects |
|---|---|---|---|
| `kNormal` | Normal decoded char (:1407) | unicode emitted | Counts for `IsNormalCharacter` |
| `kGenerated` | Inter-item space (:1399), inter-object space (:1329), `\r`/`\n` (:1335-1336) | unicode emitted | Always counted in `char_indices_` (:390); skipped by `GetRectArray`; excluded from link scanning; zero-area box; `text_object` is `None` for the inter-object and CRLF cases but **`Some`** for the inter-item case (:1401) |
| `kNotUnicode` | Charcode with no unicode mapping (:1410) | **raw charcode as unicode** | `IsNormalCharacter` uses `char_code() != 0` |
| `kHyphen` | `ProcessGenerateCharacter` hyphen path (:1359) | `unicode` forced to **`0x2`** | Exempt from `IsControlChar` |
| `kPiece` | Post-normalization fragments (:808) | each fragment emitted | Feeds `IsHyphen`'s look-back |
| `kActualText` | `/ActualText` synthesis (:1041) | unicode emitted | Suppresses bidi reversal (:851); feeds `IsHyphen` |

**A space that is literally in the content stream is `kNormal`, not
`kGenerated`.** `kGenerated` means "PDFium invented this character because the
geometry implied one". `IsGenerated` on `hello_world.pdf` pins both halves:
`chars[6]` is the space in `"Hello, world!"` and is **not** generated, while
`chars[13..15]` (the `\r\n` between the two lines) are. Any code that treats
"is a space" as "is generated" — or vice versa — breaks link extraction
(§1.15 tests both conditions separately) and the `char_indices_` build (§1.13,
which special-cases `kGenerated` but not `' '`).

Note the header's comment on field order (`unicode_` above `char_code_` "to
potentially pack tighter") is a C++ layout concern with no behavioral content.

### 1.3 Form XObject recursion

`ProcessFormObject` (:765-781) is a plain pre-order walk:

```
actual_form_matrix = form_obj.form_matrix() * form_matrix     // note the order
for page_obj in form_obj.form():
    if !IsActive: continue
    if IsText: ProcessTextObject(obj, actual_form_matrix, holder, iter)
    if IsForm: ProcessFormObject(obj, actual_form_matrix)
```

Three things to port precisely:

1. **No depth guard.** `core/fpdftext/` has none. The guard lives upstream in
   `pdfrum-page` (`build_page`'s form recursion depth cap, page brief §1.7),
   so by the time we see a `Page`, the form tree is already finite. Our walk
   is therefore over an already-materialized `PageObject::Form` tree and is
   structurally non-recursive-on-untrusted-input.
2. **The `obj_list` / `obj_iter` pair passed to `ProcessTextObject` is the
   *form's* holder and iterator**, not the page's. `IsSameAsPreTextObject`
   walks backwards inside the form only.
3. **`text_objects_` is shared across the whole page**, form boundaries
   included. A text object inside a form can be sorted between two page-level
   text objects, and a flush triggered by a page-level object flushes
   form-level objects.

### 1.4 Page-global orientation — `FindTextlineFlowOrientation`

Runs once (:747) before any object is processed; the result (`textline_dir_`)
is the fallback for per-object orientation (§1.7).

```
nPageWidth  = (i32) page->GetPageWidth()          // truncation, not rounding
nPageHeight = (i32) page->GetPageHeight()
if nPageWidth <= 0 || nPageHeight <= 0: return Unknown

horizontal_mask = vec![false; nPageWidth]
vertical_mask   = vec![false; nPageHeight]
fLineHeight = 0
nStartH = nPageWidth; nEndH = 0; nStartV = nPageHeight; nEndV = 0

for page_obj in page:
    if !IsActive || !IsText: continue
    minH = (i32) clamp(rect.left,   0, nPageWidth)
    maxH = (i32) clamp(rect.right,  0, nPageWidth)
    minV = (i32) clamp(rect.bottom, 0, nPageHeight)
    maxV = (i32) clamp(rect.top,    0, nPageHeight)
    if minH >= maxH || minV >= maxV: continue
    mark horizontal_mask[minH..maxH], vertical_mask[minV..maxV]
    nStartH = min(nStartH, minH); nEndH = max(nEndH, maxH)
    nStartV = min(nStartV, minV); nEndV = max(nEndV, maxV)
    if fLineHeight <= 0: fLineHeight = rect.Height()       // FIRST object only

nDoubleLineHeight = (i32)(2 * fLineHeight)                 // truncated to int
if (nEndV - nStartV) < nDoubleLineHeight: return Horizontal
if (nEndH - nStartH) < nDoubleLineHeight: return Vertical
nSumH = filled_ratio(horizontal_mask, nStartH, nEndH)
if nSumH > 0.8: return Horizontal
nSumV = filled_ratio(vertical_mask, nStartV, nEndV)
if nSumH > nSumV: return Horizontal
if nSumH < nSumV: return Vertical
return Unknown
```

Details that matter:

- `fLineHeight` is seeded from the **first text object whose min/max pass**,
  and never updated. It is not a median or an average.
- `nDoubleLineHeight` is an `int32_t` initialized from a `float` expression —
  C++ truncates toward zero. Preserve that (`(2.0 * h) as i32`).
- `MaskPercentFilled(mask, start, end)` (:123-132) returns `0` when
  `start >= end`, else `count_of_true(mask[start..end]) / (end - start)` as a
  **float division** — an `f32` count over an `i32` difference.
- The mask vectors are sized by the truncated page dimensions; a text object
  spanning the whole page marks `[0, nPageWidth)`. The `clamp` upper bound is
  `nPageWidth` (not `nPageWidth - 1`), and `maxH` is exclusive in the fill
  loop, so no out-of-bounds occurs, but a rect whose `right` exactly equals
  the page width still yields `maxH == nPageWidth` and marks the last cell.
- Objects are iterated in **content order**, including form objects — no,
  correction: `for (const auto& page_obj : *page_)` iterates the *page's*
  top-level object list only. Text inside a form XObject is **not** counted
  here, because a `CPDF_FormObject` is not `IsText()`. This is a real quirk:
  a page whose text lives entirely inside forms gets `textline_dir_` from an
  empty scan (`fLineHeight == 0` ⇒ `nDoubleLineHeight == 0` ⇒
  `(nEndV - nStartV) = (0 - nPageHeight) < 0 < 0` is false, then
  `(nEndH - nStartH) = -nPageWidth < 0` false, then `nSumH = 0` (start ≥ end),
  `nSumV = 0`, equal ⇒ **`Unknown`**).

#### 1.4a `display_matrix_`

`page_->GetDisplayMatrix()` (cpdf_page.cpp:222-225) is
`GetDisplayMatrixForFloatRect(Rect(0, 0, page_width, page_height), rotation=0)`,
which for rotation 0 reduces to:

```
x0,y0 = (0, page_height);  x1,y1 = (0, 0);  x2,y2 = (page_width, page_height)
matrix = Matrix((x2-x0)/w, (y2-y0)/w, (x1-x0)/h, (y1-y0)/h, x0, y0)
       = Matrix(1, 0, 0, -1, 0, page_height)
page_matrix_ * matrix
```

so it is `page_matrix_` (the crop/rotate normalizer from
`UpdateDimensions`, cpdf_page.cpp:266-298) composed with a **y-flip**. The
`rotation` argument is always `0` here — the page's own `/Rotate` is baked
into `page_matrix_`, not passed in. `page_size_` is the **cropbox∩mediabox**
size, with width/height swapped for rotations 1 and 3; an empty mediabox
defaults to `(0,0,612,792)`.

`display_matrix_` is used in exactly two places: the batch y-split test
(:920-924) and the newline "table-ish layout" escape hatch (:1264-1265). It
is `Matrix(0,0,0,0,0,0)` when `page_size_` has a zero dimension
(cpdf_page.cpp:162-164) — the `is_newline` escape hatch then never fires
(it requires `a > 0.9`) and the y-split test always sees `this_pos.y ==
prev_pos.y == 0`, so **no batch is ever split on a zero-size page**.

### 1.5 Object collection and the reorder batch — `ProcessTextObject`

`ProcessTextObject(text_obj, form_matrix, obj_list, obj_iter)` (:876-942).
This is the *only* place text objects enter `text_objects_`, and it is where
reading order is decided.

```
1. if |text_obj.GetRect().Width()| < kSizeEpsilon (0.01): DROP the object entirely
2. new_obj = {text_obj, form_matrix}
3. if text_objects_ is empty: push(new_obj); return
4. if IsSameAsPreTextObject(text_obj, obj_list, obj_iter): DROP (dedup, §1.11)
5. prev_obj = text_objects_.back()          // explicit copy — the vector may be cleared
   nItem = prev_obj.text_obj.CountItems()
   if nItem == 0: DROP new_obj              // note: new_obj is lost, not deferred
6. prev_width = GetCharWidth(prev_obj.last_item.char_code, prev_font)
                  * prev_obj.font_size / 1000
   prev_matrix = prev_obj.text_matrix * prev_obj.form_matrix
   prev_width = prev_matrix.TransformDistance(|prev_width|)
7. this_width = GetCharWidth(text_obj.item(0).char_code, this_font)
                  * text_obj.font_size / 1000
   this_width = |this_width|
   this_matrix = text_obj.text_matrix * form_matrix
   this_width = this_matrix.TransformDistance(|this_width|)
8. threshold = max(prev_width, this_width) / 4
   prev_pos = display_matrix.Transform(prev_obj.form_matrix.Transform(prev_obj.GetPos()))
   this_pos = display_matrix.Transform(form_matrix.Transform(text_obj.GetPos()))
9. if |this_pos.y - prev_pos.y| > threshold * 2:
       ProcessTransformedTextObjects()      // flush the whole batch (§1.6)
       text_objects_.clear()
       text_objects_.push(new_obj)
       return
10. // insertion sort by transformed x, scanning backwards
    for i in (1 ..= text_objects_.len()).rev():
        prev = text_objects_[i-1]
        new_prev_pos = display_matrix.Transform(prev.form_matrix.Transform(prev.GetPos()))
        if this_pos.x >= new_prev_pos.x:
            text_objects_.insert(i, new_obj); return
    text_objects_.insert(0, new_obj)
```

Behavioral notes:

- Step 1's `|Width()| < 0.01` uses the **unnormalized** rect width, so a
  rect with `left > right` (possible after a mirrored matrix) has a negative
  width and `fabs` rescues it. `Height` is never checked here.
- Step 5's early return on `nItem == 0` **discards `new_obj`**. A text object
  following a zero-item text object is silently lost. (A zero-item
  `CPDF_TextObject` cannot arise from `SetSegments`, which `CHECK`s
  `char_count`, so this is a defence against a hand-constructed object; port
  the branch anyway — `CountItems()` is `char_codes_.size()` and our
  `TextObject` record may legitimately hold an empty item list from a
  degenerate `TJ`.)
- Step 8's `threshold * 2` is compared against a **device-space** y difference
  while `threshold` is derived from device-space widths (`TransformDistance`
  through the text matrices only, then compared after `display_matrix_`
  transform of the positions). This is dimensionally inconsistent in the C++
  — `prev_width`/`this_width` are *not* pushed through `display_matrix_`. It
  is not a bug we can "fix": the mismatch is the behavior.
- Step 10 is a **stable insertion sort by x within the batch**, scanning from
  the back, inserting *after* the first element whose x is `<=` ours. Because
  it scans backwards and inserts at the first satisfying position, equal-x
  objects keep insertion order. The batch is thus sorted ascending by
  transformed x — this is the reading-order machinery.
- `GetPos()` is the text object's **position** (`pos_`), which for a
  `CPDF_TextObject` built by `pdfrum-page` is
  `content_to_user.Transform(ctm.Transform(text_matrix.Transform({x, y+rise})))`
  (page brief §1.9). Our `TextObject` record must expose it as one point.

### 1.6 Batch flush — `ProcessTransformedTextObjects`

`ProcessTransformedTextObjects()` (:1073-1122) drains `text_objects_` in
(x-sorted) order:

```
for obj in text_objects_:
    if |obj.text_obj.GetRect().Width()| < kSizeEpsilon: continue    // re-checked
    ePreMKC = PreMarkedContent(obj.text_obj)                        // §1.9
    if ePreMKC == Done:
        prev_text_obj_ = text_obj; prev_matrix_ = form_matrix; continue
    if prev_text_obj_ is set:
        type = ProcessInsertObject(text_obj, form_matrix)           // §1.8
        if type == LineBreak: curline_rect_ = text_obj.GetRect()
        else:                 curline_rect_.Union(text_obj.GetRect())
        if !ProcessGenerateCharacter(type, text_obj, form_matrix): continue   // §1.8b
    else:
        curline_rect_ = text_obj.GetRect()
    if ePreMKC == Delay:
        ProcessMarkedContent(obj)                                   // §1.9
        prev_text_obj_ = text_obj; prev_matrix_ = form_matrix; continue
    prev_text_obj_ = text_obj; prev_matrix_ = form_matrix
    matrix = text_obj.GetTextMatrix() * form_matrix
    orig_char_list_index = temp_char_list_.len()
    orig_buf_index       = temp_text_buf_.len()
    if ProcessTextObjectItems(text_obj, form_matrix, matrix):       // §1.7
        ReverseTempTextBufs(orig_char_list_index, orig_buf_index)
```

Notes:

- `prev_text_obj_` and `prev_matrix_` are updated on **every** path except the
  `ProcessGenerateCharacter → false` continue (the hyphen-cancel path), where
  they were *already* updated by the `Delay` branch? No — trace it: the
  `!ProcessGenerateCharacter` continue happens **before** the assignment, so
  `prev_text_obj_` keeps pointing at the object *before* the cancelled one.
  This is deliberate: the cancelled object emitted nothing.
- `curline_rect_` is only touched when `prev_text_obj_` is set, or on the
  first object. A `Done` marked-content object neither resets nor unions it.
- `ReverseTempTextBufs(char_list_index, buf_index)` (:1057-1071) reverses
  `temp_char_list_[char_list_index..]` and `temp_text_buf_[buf_index..]`
  **in place, independently**. Because a normalized character can push several
  entries into `temp_char_list_`… no: normalization happens in
  `AddCharInfo`, *after* `CloseTempLine`, so within the temp buffers the two
  arrays are 1:1 and the two reversals agree. (`ProcessTextObjectItems` pushes
  one `temp_text_buf_` char per `temp_char_list_` entry on every path —
  including the multi-char `unicode` loop at :1460-1464.) Keep them as one
  reversal over a paired structure.
- The guards `if (char_list_index < len)` / `if (buf_index < len)` mean a
  no-op when the object emitted nothing. `DCHECK(!temp_char_list_.empty())`
  is debug-only; an RTL object that emitted nothing at all would trip it in
  debug but is a no-op in release. Our version simply does nothing.

### 1.7 Per-object character emission — `ProcessTextObjectItems`

`ProcessTextObjectItems(text_object, form_matrix, matrix)` (:1367-1477).
Returns "the caller must reverse the temp buffers".

**Base space computation** (:1370-1371):

```
base_space = CalculateBaseSpace(text_object, matrix)
           + CalculateBaseSpaceAdjustment(text_object, matrix)
```

`CalculateBaseSpace` (:63-87):

```
nItems = text_obj.CountItems()
char_space = text_state.GetCharSpace()
if char_space == 0.0 || nItems < 2: return 0.0
has_kerning = false
spacing = matrix.TransformDistance(char_space)
fontsize_h = text_state.GetFontSizeH()
base_space = spacing
for kerning_val in text_obj.GetCharKernings():
    if kerning_val != 0:
        kerning = -fontsize_h * kerning_val / 1000
        base_space = min(base_space, kerning + spacing)
        has_kerning = true
if base_space < 0.0 || (nItems == 2 && has_kerning): return 0.0
return base_space
```

`CalculateBaseSpaceAdjustment` (:89-99):

```
char_space = text_state.GetCharSpace()
if char_space >  0.001: return -matrix.TransformDistance(char_space)
if char_space < -0.001: return  matrix.TransformDistance(|char_space|)
return 0.0
```

Note the sign symmetry: a positive char-space produces a *negative*
adjustment and a negative char-space a *positive* one, both of magnitude
`TransformDistance(|char_space|)`. Combined with `CalculateBaseSpace`
returning `TransformDistance(char_space)` (or a smaller kerning-adjusted
value) for positive char-space, the two mostly cancel — but only mostly, and
only when there is no kerning. This is the single hardest piece of the
space-generation math to get right; it must be transcribed literally.

`GetFontSizeH()` (cpdf_textstate.cpp:124-126) is
`|hypot(matrix_[0], matrix_[2]) * font_size|` where `matrix_` is the text
state's stored **transposed** 4-element matrix `[a, c, b, d]` (set by
`OnChangeTextMatrix`, page brief §1.9), so `matrix_[0] = a`,
`matrix_[2] = b`. It is thus `|hypot(a, b)| × font_size` of the composed
text→device matrix, i.e. "the horizontal scale of the font in device units".
`pdfrum-page`'s `TextObject` must expose this value; see §5/Q2.

**The per-item loop** (:1377-1472). For `i` in `0..nItems`:

```
item = text_object.GetItemInfo(i)

// (a) kerning-derived spacing, from the PREVIOUS gap
if i > 0 && kernings[i-1] != 0:
    str = temp_text_buf_ (or text_buf_ if temp is empty)
    if !str.is_empty() && str.back() != ' ':
        spacing = -text_state.GetFontSizeH() * kernings[i-1] / 1000

// (b) subtract the base space
spacing -= base_space

// (c) generate an inter-word space?
if spacing != 0 && i > 0:
    threshold = CalculateSpaceThreshold(font, GetFontSizeH(), item.char_code)
    if threshold != 0 && spacing != 0 && spacing >= threshold:
        temp_text_buf_.push(' ')
        origin = matrix.Transform(item.origin)
        temp_char_list_.push(CharInfo{
            kGenerated, kInvalidCharCode, ' ', origin,
            Rect(origin.x, origin.y, origin.x, origin.y),   // zero-area
            form_matrix,                                     // NOT `matrix`
            text_object })
spacing = 0

// (d) decode
unicode = font.UnicodeFromCharCode(item.char_code)
char_type = kNormal
if unicode.is_empty() && item.char_code != 0:
    unicode = [item.char_code as wchar_t]        // raw charcode passthrough
    char_type = kNotUnicode

// (e) char box
rect = font.GetCharBBox(item.char_code)          // glyph units
font_size = text_object.GetFontSize() / 1000
char_box = Rect(rect.left  * font_size + item.origin.x,
                rect.bottom* font_size + item.origin.y,
                rect.right * font_size + item.origin.x,
                rect.top   * font_size + item.origin.y)
if |char_box.top - char_box.bottom| < kSizeEpsilon:
    char_box.top = char_box.bottom + font_size            // NOTE: font_size/1000!
if |char_box.right - char_box.left| < kSizeEpsilon:
    char_box.right = char_box.left + text_object.GetCharWidth(item.char_code)
char_box = matrix.TransformRect(char_box)

charinfo = CharInfo{ char_type, item.char_code, 0,
                     matrix.Transform(item.origin), char_box, matrix,
                     text_object }

// (f) unmapped charcode 0 → invisible placeholder
if unicode.is_empty():                            // i.e. char_code == 0 too
    temp_char_list_.push(charinfo)
    temp_text_buf_.push(0xFFFE)                   // buffer gets FFFE, CharInfo keeps 0
    continue

// (g) duplicate suppression (§1.7a)
// (h) emission
if add_unicode:
    for c in unicode:
        charinfo.unicode = c
        temp_text_buf_.push(if c != 0 { c } else { 0xFFFE })
        temp_char_list_.push(charinfo)
else if i == 0:
    // the FIRST item of an object that duplicates a predecessor also eats a
    // preceding generated space
    if !temp_text_buf_.is_empty() && temp_text_buf_.back() == ' ':
        temp_text_buf_.pop()
        temp_char_list_.pop()
```

Then the return value (:1473-1476):

```
is_rtl = IsRightToLeft(*text_object)                    // §1.10a
return is_rtl && (matrix.a*matrix.d - matrix.b*matrix.c) < 0
```

Things that trip a reimplementation:

- **(e)'s degenerate-height rescue uses `font_size` *after* the `/1000`
  division.** `font_size` at that point is `GetFontSize()/1000`, so a
  zero-height box gets a height of `size/1000` — a thousandth of the font
  size, not the font size. It looks like a bug; port it.
- **(e)'s degenerate-width rescue uses `GetCharWidth`, which is already
  scaled** (`cpdf_textobject.cpp:250-260`: `font->GetCharWidth(code) *
  GetFontSize()/1000`, or the CID vertical width). So width and height
  rescues are in the *same* units after all.
- **(f) vs (d):** `unicode.is_empty()` after (d) can only happen when
  `item.char_code == 0` (because (d) fills it from the charcode otherwise).
  Charcode 0 therefore produces a `kNormal` `CharInfo` whose `unicode` is `0`
  — which `--txt` emits as a **NUL code unit**. `IsNormalCharacter` returns
  `false` for it (unicode 0 and char_code 0), so it is excluded from
  `char_indices_` and thus from `GetPageText`.
- **(h)'s multi-char loop pushes the *same* `charinfo`** (same box, same
  origin, same matrix) once per unicode character, differing only in
  `unicode`. A `ToUnicode` entry mapping one code to `"ffi"` yields three
  `CharInfo`s at identical positions. `char_type` stays `kNormal` — the
  `kPiece` retype happens later, in `AddCharInfo` (§1.10b), and only for RTL
  or the `FB00..FB06` ligature band.
- **(h)'s `c != 0 ? c : 0xFFFE`** applies to the *buffer* only; the `CharInfo`
  gets the raw `c` including `0`.
- `GetItemInfo(i)` (cpdf_textobject.cpp:58-79) returns
  `origin = (char_positions_[i], 0)` for horizontal writing, and for a
  **vertical CID font** `origin = (0 - size*vorg.x/1000,
  char_positions_[i] - size*vorg.y/1000)`. Vertical origin handling therefore
  lives in the item, not in this crate.

#### 1.7a Duplicate suppression — the position epsilon

(:1437-1458), executed for every item that produced a unicode:

```
add_unicode = true
count = min(temp_char_list_.len(), 7)
kTextCharRatioGapDelta = 0.07
threshold = charinfo.matrix().TransformXDistance(
                kTextCharRatioGapDelta * text_object.GetFontSize())
for n in (temp_char_list_.len() - count + 1 ..= temp_char_list_.len()).rev():
    candidate = temp_char_list_[n-1]
    if candidate.char_code != charinfo.char_code:      continue
    if candidate.text_object.is_none()
       || candidate.text_object.font != charinfo.text_object.font: continue
    diff = candidate.origin - charinfo.origin
    if |diff.x| < threshold && |diff.y| < threshold:
        add_unicode = false; break
```

- `TransformXDistance(dx)` (fx_coordinates.cpp:467-471) is
  `hypot(a*dx, b*dx)` — the length of the transformed x-unit vector scaled by
  `dx`. Not `TransformDistance`, which is `dx * (GetXUnit()+GetYUnit())/2`.
- The threshold uses **`text_object.GetFontSize()`**, which may be negative;
  a negative font size gives a negative `dx` and `hypot` makes it positive
  again. Fine either way.
- Font comparison is by **pointer identity** in C++
  (`GetFont() != GetFont()` on `RetainPtr`s). In our model this becomes
  `Arc::ptr_eq` on the `Arc<Font>` the page layer attached — which preserves
  the semantics exactly as long as `pdfrum-page` shares one `Arc<Font>` per
  resolved font resource (it does; the font cache is per-`Document`). Record
  this dependency: if the page layer ever clones fonts per text object, dedup
  silently stops firing.
- The loop bound `n > len - count` with `count = min(len, 7)` scans **at most
  the last 7** entries, newest first, and stops at the first match.
- The `else if i == 0` space-eating fallback (:1465-1471) fires only when the
  *first* item of the object was suppressed, and only removes **one** trailing
  space.

#### 1.7b `CalculateSpaceThreshold`

(:210-229) — the width below which an inter-item gap is not a space:

```
space_charcode = font.CharCodeFromUnicode(' ')
threshold = 0
if space_charcode != kInvalidCharCode:
    threshold = fontsize_h * font.GetCharWidth(space_charcode) / 1000
if threshold > fontsize_h / 3: threshold = 0
else:                          threshold /= 2
if threshold == 0:
    threshold = GetCharWidth(char_code, font)               // §1.7c
    threshold = NormalizeThreshold(threshold, 300, 500, 700)
    threshold = fontsize_h * threshold / 1000
return threshold
```

The `> fontsize_h/3` cap means "a space glyph wider than a third of the font
size is not to be trusted"; it falls through to the *current character's*
width bucketed by `NormalizeThreshold`. Note `CharCodeFromUnicode(' ')`
returning `kInvalidCharCode` and returning `0` are handled differently:
`0` is a valid charcode and produces `threshold = fontsize_h *
GetCharWidth(0)/1000`, usually `0`, which then falls through.

#### 1.7c `GetCharWidth` — the local three-rung ladder

(:185-208). Distinct from `CPDF_Font::GetCharWidth`; this is
`core/fpdftext/`'s own fallback chain, in **glyph units (/1000 em)**:

```
if charCode == kInvalidCharCode: return 0
w = font.GetCharWidth(charCode)
if w > 0: return w
str = ""; font.AppendChar(&str, charCode)      // charcode → bytes
w = font.GetStringWidth(str)
if w > 0: return w
rect = font.GetCharBBox(charCode)
if !rect.Valid(): return 0                     // overflow check, FX_RECT::Valid
return max(rect.Width(), 0)
```

`AppendChar` for a simple font is `str += (char)charcode` (one byte,
truncating); for a CID font it is the multi-byte encoding. `GetStringWidth`
then re-decodes through `GetNextChar` and sums `GetCharWidth` — so rung 2 can
differ from rung 1 only when the round trip charcode→bytes→charcode is lossy,
which happens for a simple font with charcode > 255. `FX_RECT::Valid()`
(fx_coordinates.cpp:79-85) is an `i32` overflow check on `right-left` /
`bottom-top`.

### 1.8 Inter-object decisions — `ProcessInsertObject`

`ProcessInsertObject(text_obj, form_matrix) -> GenerateCharacter` (:1194-1320)
decides whether a space, a line break, or a hyphen is generated *between* the
previous emitted object and this one. Four enum outcomes:
`kNone | kSpace | kLineBreak | kHyphen`.

```
FindPreviousTextObject()                        // §1.8a
writing_mode = GetTextObjectWritingMode(text_obj)
if writing_mode == Unknown:
    writing_mode = GetTextObjectWritingMode(prev_text_obj_)

nItem = prev_text_obj_.CountItems()
if nItem == 0: return kNone

prev_item = prev_text_obj_.GetItemInfo(nItem - 1)
item      = text_obj.GetItemInfo(0)
this_rect = text_obj.GetRect()
prev_rect = prev_text_obj_.GetRect()
unicode = text_obj.font.UnicodeFromCharCode(item.char_code)
if unicode.is_empty(): unicode = [item.char_code as wchar_t]
current_char = unicode[0]

// --- line-end tests, per writing mode ---
if writing_mode == Horizontal:
    if EndHorizontalLine(this_rect, prev_rect):
        return IsHyphen(current_char) ? kHyphen : kLineBreak
else if writing_mode == Vertical:
    if EndVerticalLine(this_rect, prev_rect, curline_rect_,
                       text_obj.GetFontSize(), prev_text_obj_.GetFontSize()):
        return IsHyphen(current_char) ? kHyphen : kLineBreak
// writing_mode == Unknown: NEITHER test runs

// --- the newline / space math ---
last_pos    = prev_item.origin.x
nLastWidth  = GetCharWidth(prev_item.char_code, prev_font)     // glyph units
last_width  = |nLastWidth * prev_text_obj_.GetFontSize() / 1000|
nThisWidth  = GetCharWidth(item.char_code, this_font)
this_width  = |nThisWidth * text_obj.GetFontSize() / 1000|
threshold   = max(last_width, this_width) / 4

prev_matrix         = prev_text_obj_.GetTextMatrix() * prev_matrix_
prev_matrix_inverse = prev_matrix.GetInverse()
pos = prev_matrix_inverse.Transform(form_matrix.Transform(text_obj.GetPos()))
if last_width < this_width:
    threshold = prev_matrix_inverse.TransformDistance(threshold)

is_newline = false
if writing_mode == Horizontal:
    rect = prev_text_obj_.GetRect()
    rect_height = rect.Height()               // BEFORE Normalize
    rect.Normalize()
    if (rect.IsEmpty() && rect_height > 5)
       || ((pos.y > threshold*2 || pos.y < threshold*-3)
           && (|pos.y| >= 1 || |pos.y| > |pos.x|)):
        is_newline = true
        if nItem > 1:
            first_item = prev_text_obj_.GetItemInfo(0)
            m = prev_text_obj_.GetTextMatrix()
            if prev_item.origin.x > first_item.origin.x
               && display_matrix_.a >  0.9 && display_matrix_.b <  0.1
               && display_matrix_.c <  0.1 && display_matrix_.d < -0.9
               && m.b < 0.1 && m.c < 0.1:
                re = Rect(0, prev_rect.bottom, 1000, prev_rect.top)
                if re.Contains(text_obj.GetPos()):
                    is_newline = false
                else if Rect(0, this_rect.bottom, 1000, this_rect.top)
                        .Contains(prev_text_obj_.GetPos()):
                    is_newline = false
if is_newline:
    return IsHyphen(current_char) ? kHyphen : kLineBreak

if text_obj.CharCount() == 1 && IsHyphenCode(current_char) && IsHyphen(current_char):
    return kHyphen
if current_char == ' ': return kNone

prev_str = prev_font.UnicodeFromCharCode(prev_item.char_code)
if prev_str.Back() == ' ': return kNone       // Back() on empty is 0, not a space

// --- the space threshold ---
matrix = text_obj.GetTextMatrix() * form_matrix
threshold2 = max(nLastWidth, nThisWidth)                     // GLYPH UNITS, ints
threshold2 = NormalizeThreshold(threshold2, 400, 700, 800)
if nLastWidth >= nThisWidth:
    threshold2 *= |prev_text_obj_.GetFontSize()|
else:
    threshold2 *= |text_obj.GetFontSize()|
    threshold2  = matrix.TransformDistance(threshold2)
    threshold2  = prev_matrix_inverse.TransformDistance(threshold2)
threshold2 /= 1000
if (threshold2 < 1.4881 && threshold2 > 1.4879)
   || (threshold2 < 1.39001 && threshold2 > 1.38999):
    threshold2 *= 1.5
return GenerateSpace(pos, last_pos, this_width, last_width, threshold2)
           ? kSpace : kNone
```

`EndHorizontalLine(this_rect, prev_rect)` (:251-260):

```
if this_rect.Height() <= 4.5 || prev_rect.Height() <= 4.5: return false
top    = min(this_rect.top,    prev_rect.top)
bottom = max(this_rect.bottom, prev_rect.bottom)
return bottom >= top                       // i.e. the two rects do NOT overlap in y
```

`EndVerticalLine(this_rect, prev_rect, curline_rect, this_fs, prev_fs)`
(:262-275):

```
if this_rect.Width() <= this_fs*0.1 || prev_rect.Width() <= prev_fs*0.1: return false
left  = max(this_rect.left,  curline_rect.left)
right = min(this_rect.right, curline_rect.right)
return right <= left                       // no x overlap with the current line
```

Note the asymmetry: the horizontal test compares against `prev_rect`, the
vertical test against the accumulated `curline_rect_`.

`GenerateSpace(pos, last_pos, this_width, last_width, threshold)` (:231-249):

```
if |last_pos + last_width - pos.x| <= threshold: return false
threshold_pos  = threshold + last_width
pos_difference = pos.x - last_pos
if |pos_difference| > threshold_pos:                return true
if pos.x < 0 && -threshold_pos > pos_difference:    return true
return pos_difference > this_width + last_width
```

The middle clause is dead-ish (if `pos_difference < -threshold_pos` then
`|pos_difference| > threshold_pos` already fired) — unless `threshold_pos` is
negative, which happens when `threshold + last_width < 0`. `last_width` is a
`fabs` so it is ≥ 0; `threshold2` can be negative only via a negative
`NormalizeThreshold` input, i.e. a negative `GetCharWidth`, which the ladder
in §1.7c cannot produce. Port the clause anyway — it costs nothing and the
reasoning depends on invariants of *other* crates.

The **magic float bands** at :1313-1314 are exact-value hacks for two
specific font/size combinations that shipped in real files. `1.4880` and
`1.3900` are the sentinel values; the ±0.00001 window is an f32 equality test
spelled as a range. Transcribe the comparison literally, in `f32`.

The `display_matrix_` guard in the `is_newline` escape hatch (`a > 0.9,
b < 0.1, c < 0.1, d < -0.9`) recognizes "unrotated page, y flipped" — the
common case — and then checks whether the two objects overlap vertically in a
0..1000 x-band, i.e. "these look like two cells of a table row, not two
lines". Note the `re` rect uses `prev_text_obj_->GetRect()` freshly (not the
cached `prev_rect`), which is the same value.

`GetTextObjectWritingMode(text_obj)` (:1124-1152):

```
char_count = text_obj.CharCount()
if char_count <= 1: return textline_dir_                     // page-global fallback
first = text_obj.GetCharInfo(0);  last = text_obj.GetCharInfo(char_count - 1)
text_matrix = text_obj.GetTextMatrix()
first.origin = text_matrix.Transform(first.origin)
last.origin  = text_matrix.Transform(last.origin)
dX = |last.origin.x - first.origin.x|
dY = |last.origin.y - first.origin.y|
if dX <= 0.0001 && dY <= 0.0001: return Unknown
v = Vector(dX, dY).normalized()            // no-op if length < 0.0001
is_under_threshold = v.x <= 0.0872
if v.y <= 0.0872: return is_under_threshold ? textline_dir_ : Horizontal
return is_under_threshold ? Vertical : textline_dir_
```

`CFX_VectorF::Normalize()` (fx_coordinates.cpp:69-77) returns *without
normalizing* when the length is `< 0.0001` — but the `dX/dY <= 0.0001` guard
above makes at least one component larger than that, and `hypot` of it is
`>= 0.0001`, so the no-op branch is unreachable here. `0.0872 ≈ sin(5°)`:
both components inside the cone (a diagonal) or both outside is "trust the
page-global guess"; exactly one outside picks that axis.

`ProcessInsertObject` is called with `prev_text_obj_` **assumed non-null** by
its caller, but `GetTextObjectWritingMode(prev_text_obj_)` at :1200 would
dereference null if `prev_text_obj_` were null — the caller guarantees it
isn't. In Rust this is a `&TextObject` parameter, no branch needed.

#### 1.8a `FindPreviousTextObject`

(:1046-1055) — resets `prev_text_obj_` from the *last emitted character*
rather than from the last processed object:

```
prev_char_info = GetPrevCharInfo()          // temp_char_list_.back(), else char_list_.back()
if prev_char_info is None: return
if prev_char_info.text_object is Some: prev_text_obj_ = that
```

So a generated character (whose `text_object` is `None`) leaves
`prev_text_obj_` alone, but a generated *inter-word* space (§1.7 step (c),
whose `text_object` **is** set) can move it back to its own object. And a
`kActualText` char (whose `text_object` is set, :1042) makes the
`/ActualText`-bearing object the "previous" one.

`GetPrevCharInfo()` (:1187-1192): `temp_char_list_.back()` if non-empty, else
`char_list_.back()`, else `None`.

#### 1.8b `ProcessGenerateCharacter` — emitting the decision

(:1322-1365). Returns "continue processing this object".

```
kNone:      return true
kSpace:     AppendGeneratedCharacter(' ', form_matrix, use_temp_buffer=true); return true
kLineBreak: CloseTempLine()                              // flush the line first
            if text_buf_.GetSize() != 0:                 // NOT char_list_!
                AppendGeneratedCharacter('\r', form_matrix, use_temp_buffer=false)
                AppendGeneratedCharacter('\n', form_matrix, use_temp_buffer=false)
            return true
kHyphen:
    if text_object.CharCount() == 1:
        item = text_object.GetCharInfo(0)
        unicode = font.UnicodeFromCharCode(item.char_code)
        if unicode.is_empty(): unicode = [item.char_code as wchar_t]
        if IsHyphenCode(unicode[0]): return false        // CANCEL: skip this object
    while temp_text_buf_.last() == 0x20:
        temp_text_buf_.pop(); temp_char_list_.pop()
    charinfo = temp_char_list_.last_mut()                // CHECK: must be non-empty
    temp_text_buf_.pop()                                 // drop the hyphen char
    charinfo.char_type = kHyphen
    charinfo.unicode   = 0x2                             // <<< the --txt byte
    temp_text_buf_.push(0xFFFE)                          // <<< the GetText byte
    return true
```

This is the origin of Fact 1: `--txt` sees `0x2`, `GetText` sees `0xFFFE`,
and the *same* `CharInfo` carries both meanings. Verified against the built
oracle on `bug_781804.pdf`:
`… 61 00 00 00 | 02 00 00 00 | 73 00 00 00 …` (`a`, `U+0002`, `s`), while
`FPDFText_GetText` returns `…0x0061, 0xFFFE, 0x0073…`.

Three traps:

1. `kLineBreak` guards on `text_buf_.GetSize()` — the *byte* size of the
   final buffer — so no `\r\n` is emitted before any real text has landed in
   `text_buf_`. But `CloseTempLine()` runs **first**, which may itself have
   just populated `text_buf_`. A line break at the very start of a page with
   a non-empty first line therefore does emit. `GetSize()` on
   `WideTextBuffer` is `BinaryBuffer::GetSize()` = bytes; `GetLength()` is
   chars. The check is `!= 0` either way.
2. The `kHyphen` `while` loop pops trailing spaces from **both** temp
   containers and then unconditionally takes `temp_char_list_.back()`. If the
   temp list were empty this is UB in C++ (`CHECK` in the container in debug).
   Reaching it requires `IsHyphen()` to have returned true, which requires a
   non-empty buffer — but `IsHyphen` may have consulted `text_buf_` while
   `temp_text_buf_` is empty (:1155-1158). **This is a genuine reachable
   crash in the C++** for a crafted file; see D4.
3. `AppendGeneratedCharacter` (:725-740) is a no-op when `GenerateCharInfo`
   returns `None`, which happens exactly when there is no previous char at all
   (:1563-1566). So the very first thing on a page can never be a generated
   space or a line break.

`GenerateCharInfo(unicode, form_matrix)` (:1560-1587):

```
prev = GetPrevCharInfo(); if None: return None
pre_width = 0
if prev.text_object is Some && prev.char_code != kInvalidCharCode:
    pre_width = GetCharWidth(prev.char_code, prev.text_object.font)   // §1.7c
font_size = prev.text_object.map(GetFontSize).unwrap_or(prev.char_box.Height())
if font_size == 0: font_size = kDefaultFontSize (1.0)
origin = (prev.origin.x + pre_width * font_size / 1000, prev.origin.y)
return CharInfo{ kGenerated, kInvalidCharCode, unicode, origin,
                 Rect(origin.x, origin.y, origin.x, origin.y),   // zero-area
                 form_matrix, text_object: None }
```

Note `font_size == 0` (exact float equality) falls back to `1.0`, and that a
negative font size is *not* rescued.

#### 1.8c `IsHyphen` — the look-back

(:1154-1185):

```
current_text = temp_text_buf_.as_view()
if current_text.is_empty(): current_text = text_buf_.as_view()
if current_text.is_empty(): return false
iter = current_text.rbegin()
while (iter+1) != rend() && *iter == 0x20: ++iter       // skip trailing spaces
if !IsHyphenCode(*iter): return false
if (iter+1) != rend():
    ++iter
    if FXSYS_iswalpha(*iter) && FXSYS_iswalnum(current_char): return true
prev = GetPrevCharInfo()
return prev.is_some()
       && (prev.char_type == kPiece || prev.char_type == kActualText)
       && IsHyphenCode(prev.unicode)
```

`IsHyphenCode(c)` (:150-152) is `c == 0x2D || c == 0xAD` — ASCII hyphen-minus
and SOFT HYPHEN. **Not** U+2010 HYPHEN, which is why the embeddertest's
`‐` char reports `IsHyphen == 0`.

The space-skipping loop stops one short of the beginning (`(iter+1) != rend()`),
so an all-spaces buffer leaves `iter` at the *first* character, which is a
space and fails `IsHyphenCode`.

`FXSYS_iswalpha`/`FXSYS_iswalnum` are ICU `u_isalpha` / `u_isalnum`, i.e.
general category `L*` and `L*|Nd` respectively (verified in
`third_party/icu/source/common/uchar.cpp:139-156`). See D1 / Q1 — this is a
dependency escalation.

### 1.9 Marked content and `/ActualText`

Two functions, called from `ProcessTransformedTextObjects`.

`PreMarkedContent(text_obj) -> {kPass, kDone, kDelay}` (:944-996):

```
marks = text_obj.GetContentMarks()
if marks.CountItems() == 0: return kPass
actual_text = ""
bExist = false
dict = null
for i in 0..marks.CountItems():
    dict = marks.GetItem(i).GetParam()          // may be null
    if dict is null: continue
    temp = dict.GetStringFor("ActualText")      // NON-resolving, String-typed only
    if temp is Some:
        bExist = true
        actual_text = temp.GetUnicodeText()     // PDFDocEncoding / UTF-16BE BOM detect
if !bExist: return kPass
if prev_text_obj_ is Some:
    prev_marks = prev_text_obj_.GetContentMarks()
    if prev_marks.CountItems() == marks.CountItems()
       && prev_marks.GetItem(count-1).GetParam() == dict:   // POINTER equality
        return kDone
if actual_text.is_empty(): return kPass
bExist = false
for wc in actual_text:
    if (wc > 0x80 && wc < 0xFFFD) || (wc <= 0x80 && isprint(wc)):
        bExist = true; break
if !bExist: return kDone
return kDelay
```

Precise points:

- The loop **overwrites** `dict` and `actual_text` on every mark that has a
  param, so only the *last* param dict and the *last* mark carrying an
  `/ActualText` string win — and `dict` ends up being the last mark's param
  regardless of whether that mark had `/ActualText`. `bExist` is sticky.
- `GetStringFor` (cpdf_dictionary.cpp:233-241) is `ToString(GetObjectFor(key))`
  — **no reference resolution and no type coercion**. An indirect
  `/ActualText` is invisible here. Compare `ProcessMarkedContent`, which uses
  `GetUnicodeTextFor` (:117-123) — one level of resolution and any object
  type. So a file with an indirect `/ActualText` gets `kPass` from
  `PreMarkedContent` (normal text emitted) and `ProcessMarkedContent` never
  runs. This asymmetry is real, exercised, and must be preserved.
- The `kDone` test uses **pointer identity of the param dictionary**, which
  our object model expresses as `Arc::ptr_eq` on the resolved param — or,
  since our marks hold `Dict` values, as identity of the `Arc<Dict>` the page
  layer attached. `pdfrum-page`'s `ContentMark` must therefore carry the param
  as a shared handle, not a cloned `Dict`. Record as an interface requirement
  (§4).
- `isprint(wc)` is the **C locale** `isprint` on a `wchar_t` narrowed by the
  `wc <= 0x80` guard: true for `0x20..=0x7E`. (`wc == 0x80` reaches `isprint`
  with an out-of-`unsigned char` value in C++ — implementation-defined; with
  the guard being `<= 0x80` and the band above starting at `> 0x80`, code
  point `0x80` is tested by `isprint(0x80)` which is false in the C locale on
  glibc. Treat `0x80` as not printable.)
- The overall meaning: `kDone` = "this object is a continuation of a mark we
  already emitted, or its ActualText is unprintable — emit nothing";
  `kDelay` = "replace this object's glyphs with the ActualText";
  `kPass` = "normal processing".

`ProcessMarkedContent(obj)` (:998-1044):

```
actual_text = ""
for n in 0..marks.CountItems():
    dict = marks.GetItem(n).GetParam()
    if dict is Some: actual_text = dict.GetUnicodeTextFor("ActualText")  // RESOLVING
if actual_text.is_empty(): return
is_rtl = IsRightToLeft(*text_obj)                       // §1.10a
matrix = text_obj.GetTextMatrix() * obj.form_matrix
rect = text_obj.GetRect()
if is_rtl: rect.left  = rect.right - rect.Width()/actual_text.len();  step = -rect.Width()
else:      rect.right = rect.left  + rect.Width()/actual_text.len();  step =  rect.Width()
for k, wc in actual_text.enumerate():
    if wc <= 0x80 && !isprint(wc): wc = 0x20            // control → space
    if wc >= 0xFFFD: continue                           // dropped, k still advances
    char_box = rect.translated(k * step, 0)
    temp_text_buf_.push(wc)
    temp_char_list_.push(CharInfo{ kActualText, kInvalidCharCode, wc,
                                   text_obj.GetPos(),      // NOT per-char!
                                   char_box, matrix, text_obj })
```

- The second loop **overwrites `actual_text` unconditionally** for every mark
  with a param — so a later mark with a param but *no* `/ActualText` resets it
  to empty and the function returns early. `PreMarkedContent` (which uses
  `if temp` to guard) and this one disagree. Both are ported as written.
- `rect.Width()` is read **after** `rect.left`/`rect.right` was already
  mutated in the RTL branch, so `step = -rect.Width()` is the *per-character*
  width, negative. In the LTR branch `step = rect.Width()` is likewise the
  per-character width. Same magnitude, opposite sign. The `Width()` before
  mutation is the full object width — the division consumes it. Order matters;
  transcribe literally.
- Every synthesized char shares the **same origin** (`text_obj.GetPos()`) but
  a stepped box. Consequence: `GenerateCharInfo`'s x-advance and
  `GetLooseBounds`'s origin math see a stack of coincident origins.
- Code points `>= 0xFFFD` are skipped but still consume a `k`, leaving a gap
  in the box progression.
- A zero-length `actual_text` is caught by the early return, so the division
  by `actual_text.GetLength()` is safe.

### 1.10 Line closing, bidi, and normalization — `CloseTempLine`

`CloseTempLine()` (:816-874) is where `temp_*` becomes `char_list_`/`text_buf_`.

```
if temp_char_list_.is_empty(): return
str = temp_text_buf_.MakeString()

// (a) collapse runs of spaces, in BOTH containers, keeping the first
prev_char_is_space = false
for i in 0..str.len():
    if str[i] != ' ': prev_char_is_space = false; continue
    if prev_char_is_space:
        temp_text_buf_.Delete(i, 1)
        temp_char_list_.remove(i)
        str.Delete(i)
        i -= 1
    prev_char_is_space = true

// (b) bidi segmentation
bidi = CFX_BidiString(str, auto_order=false)
if rtl_: bidi.SetOverallDirectionRight()
current_direction = bidi.OverallDirection()
for segment in bidi:
    str_span  = str[segment.start .. segment.start+segment.count]
    char_span = temp_char_list_[same range]
    if segment.direction == Right
       || (segment.direction == Neutral && current_direction == Right):
        current_direction = Right
        is_actual_text = !char_span.is_empty()
                         && char_span[0].char_type == kActualText
        if is_actual_text:
            for (c, info) in zip(str_span, char_span): AddCharInfo(c, info, is_rtl=true)
        else:
            for i in (0..segment.count).rev():  AddCharInfo(str_span[i], char_span[i], true)
    else:
        if segment.direction != LeftWeak: current_direction = Left
        for (c, info) in zip(str_span, char_span): AddCharInfo(c, info, is_rtl=false)

temp_char_list_.clear()
temp_text_buf_.clear()
```

Notes on (a): the loop mutates `str` while iterating it by index and
decrements `i` after a deletion, so `prev_char_is_space` is set to `true` at
the end of *every* space iteration including the deleting one. The net effect
is "keep the first space of each run". `WideTextBuffer::Delete` and
`WideString::Delete` bounds-check and no-op on an out-of-range index
(string_template.cpp:104-124), so the pattern is safe. Crucially, this runs
on the **temp** containers, so a run of spaces split across a `CloseTempLine`
boundary is not collapsed.

Notes on (b): `CFX_BidiString` with `auto_order=false` builds segments but
does **not** apply the "more R2L segments than L2R ⇒ flip" heuristic; only
the explicit `rtl_` flag flips it. Segment reversal only reverses within a
segment, never between them (the segment *order* is reversed by
`SetOverallDirectionRight` reversing `order_`, at fx_bidi.cpp:104-109).

#### 1.10a Bidi classification and `IsRightToLeft`

`CFX_BidiChar::AppendChar` (fx_bidi.cpp:18-48) maps `GetBidiClass(wch)` to
four directions:

| Direction | Bidi classes |
|---|---|
| `kLeft` | `L` |
| `kLeftWeak` | `AN, EN, NSM, CS, ES, ET, BN` |
| `kRight` | `R, AL` |
| `kNeutral` | everything else (`ON, S, WS, B, RLO, RLE, LRO, LRE, PDF`) |

Segments are emitted when the direction *changes*; `GetSegmentInfo()` returns
`last_segment_`, and `StartNewSegment` copies `current_ → last_` before
resetting — so the segment pushed on a change is the *completed* one, and
`EndChar()` flushes the final one. Note the very first `AppendChar` also
"changes direction" (from the initial `kNeutral`) unless the first character
is itself neutral, pushing a `{start:0, count:0, kNeutral}` segment. Our
implementation must reproduce that zero-length leading segment, because
`CloseTempLine` iterates segments and a zero-count segment produces empty
spans — harmless, but the *segment indices* of everything after depend on it
being present in the list only for `count` bookkeeping, which it is not
(start/count are absolute). Verified: a leading zero-count segment is a no-op
in `CloseTempLine`.

`CFX_BidiString(str, auto_order=true)` additionally flips to R2L when the
count of `kRight` segments **strictly exceeds** the count of `kLeft` segments
(`kLeftWeak` and `kNeutral` segments count for neither).

`IsRightToLeft(text_obj)` (:165-183) builds a probe string and asks:

```
str = ""
for i in 0..text_obj.CountItems():
    item = text_obj.GetItemInfo(i)
    unicode = font.UnicodeFromCharCode(item.char_code)
    wc = unicode.front()                    // 0 when empty
    if wc == 0: wc = item.char_code as wchar_t
    if wc != 0: str += wc
return CFX_BidiString(str, auto_order=true).OverallDirection() == kRight
```

`GetBidiClass` reads a 65536-entry table (`fx_ucddata.inc`) packed as
`(mirror << 5) | bidi_class`, with 5 bits of bidi class and 9 bits of mirror
index; `GetMirrorChar` indexes `kFXTextLayoutBidiMirror` (a flat
pair-encoded array) unless the index is the `0x1FF` sentinel. Characters
above `0xFFFF` return property `0` = `kON` / no mirror. See D2 for how we
source this table.

#### 1.10b `AddCharInfo` — normalization and the final push

(:783-814):

```
if !IsNormalCharacter(info):
    char_list_.push(info)          // <<< pushed to char_list_ but NOT to text_buf_
    return
if is_rtl: wc = GetMirrorChar(wc)
normalized = if is_rtl || (0xFB00 <= wc <= 0xFB06) { GetUnicodeNormalization(wc) }
             else { empty }
modified = info.clone()
if normalized.is_empty():
    text_buf_.push(wc)
    if is_rtl: modified.unicode = wc          // <<< only RTL updates the CharInfo
    char_list_.push(modified)
    return
modified.char_type = kPiece
for nc in normalized:
    modified.unicode = nc
    text_buf_.push(nc)
    char_list_.push(modified)
```

This is the second half of Fact 1. Three divergences between the two outputs
are created right here:

1. A non-normal character (`IsNormalCharacter == false`) lands in
   `char_list_` — and thus in `--txt` — but **not** in `text_buf_`.
2. An LTR character's `CharInfo.unicode` is **not** updated from `wc`, so if
   the bidi span's `str` character differs from `info.unicode` (it cannot, in
   the current pipeline, since both come from the same push), they would
   diverge. Preserved as written for safety.
3. Normalization multiplies one `CharInfo` into several, all sharing box,
   origin and matrix.

`IsNormalCharacter(info)` (:154-157):
`info.unicode != 0 ? !IsControlChar(info) : info.char_code != 0`.

`IsControlChar(info)` (:134-148): true when `unicode` is one of
`0x02, 0x03, 0x93, 0x94, 0x96, 0x97, 0x98, 0xFFFE` **and**
`char_type != kHyphen`. Everything else is false. Note `0x93..0x98` is the
Windows-1252 smart-quote/dash band interpreted as raw code points — a
historical artifact, ported verbatim.

Because a `kHyphen` char has `unicode == 0x2` but is exempted from
`IsControlChar`, it *is* "normal", so it reaches `text_buf_.push(0x2)` — no,
trace again: `CloseTempLine` passes `str[i]`, which for the hyphen position is
`0xFFFE` (pushed by `ProcessGenerateCharacter`), while `info.unicode` is
`0x2`. `IsNormalCharacter` looks at `info.unicode() == 0x2` and
`char_type == kHyphen` ⇒ not a control char ⇒ normal. So `text_buf_` receives
`wc = 0xFFFE` and `char_list_` receives the info with `unicode = 0x2`
(is_rtl false ⇒ no update). Exactly the observed oracle behavior.

`GetUnicodeNormalization(wch)` (:101-121) — a four-table lookup:

```
wch &= 0xFFFF
wFind = kUnicodeDataNormalization[wch]            // 65536 u16 entries
if wFind == 0: return [wch]                       // identity
if wFind >= 0x8000: return [kUnicodeDataNormalizationMap1[wFind - 0x8000]]
index = wFind & 0x0FFF
table = wFind >> 12                               // 2, 3, or 4
maps = kUnicodeDataNormalizationMaps[table - 2].subspan(index)
if table == 4:
    len = maps[0]; maps = maps[1..]               // table 4 is length-prefixed
else:
    len = table                                   // tables 2 and 3 have fixed length
return maps[..len]
```

Table sizes: `kUnicodeDataNormalization` 65536, `Map1` 5376, `Map2` 1724,
`Map3` 1164, `Map4` 488 (all `u16`). Total ≈ 145 KB as `u16`, which is why
this becomes a build-script-generated binary blob (§3).

Applying it only when `is_rtl || 0xFB00..=0xFB06` means Latin ligatures
`ﬀ ﬁ ﬂ ﬃ ﬄ ﬅ` are decomposed unconditionally, while Arabic presentation
forms are decomposed only in RTL runs.

### 1.11 Object-level deduplication — `IsSameAsPreTextObject`

(:1541-1558):

```
i = 0
while i < 5 && iter != obj_list.begin():
    --iter
    other = *iter
    if other == text_obj || !other.IsText(): continue    // <<< does NOT increment i
    if IsSameTextObject(other.AsText(), text_obj): return true
    ++i
return false
```

The `continue` before `++i` means non-text objects (and the object itself) do
not consume a lookback slot: the loop examines the **5 nearest preceding text
objects**, however many images/paths lie between them. It also means a long
run of non-text objects makes the loop walk the entire list. That is the
behavior; our `Page::objects` slice makes it a bounded backward scan.

`IsSameTextObject(a, b)` (:1479-1539), where `a` is the *candidate* and `b`
the earlier one — note the C++ call site passes `(other, text_obj)`, so
`text_obj1 = other` (earlier) and `text_obj2 = text_obj` (current). The names
inside the function (`prev_obj_rect` from `text_obj2`) are therefore
**inverted** relative to intuition. Transcribe against the parameter names,
not the names:

```
if either is null: return false
prev_obj_rect    = text_obj2.GetRect()          // the CURRENT object
current_obj_rect = text_obj1.GetRect()          // the EARLIER object
if both rects are empty:
    xdiff = |prev_obj_rect.left - current_obj_rect.left|
    if char_list_.len() >= 2:
        dbSpace = char_list_[len-2].char_box().Width()
        if xdiff > dbSpace: return false
if !prev_obj_rect.IsEmpty() || !current_obj_rect.IsEmpty():
    prev_obj_rect.Intersect(current_obj_rect)             // MUTATES prev_obj_rect
    if prev_obj_rect.IsEmpty(): return false
    if |prev_obj_rect.Width() - current_obj_rect.Width()| > current_obj_rect.Width()/2:
        return false
    if text_obj2.GetFontSize() != text_obj1.GetFontSize(): return false   // exact float
nPreCount = text_obj2.CountItems()
if nPreCount != text_obj1.CountItems(): return false
if nPreCount == 0: return true                            // both empty ⇒ same
for i in 0..nPreCount:
    itemPer = text_obj2.GetItemInfo(i)
    itemCur = text_obj1.GetItemInfo(i)
    if itemCur.char_code != itemPer.char_code: return false
diff = text_obj1.GetPos() - text_obj2.GetPos()
font_size = text_obj2.GetFontSize()
char_size = GetCharWidth(itemPer.char_code, text_obj2.font)   // LAST item's code
max_pre_size = max(max(prev_obj_rect.Height(), prev_obj_rect.Width()), font_size)
return |diff.x| <= 0.9 * char_size * font_size / 1000
    && |diff.y| <= max_pre_size / 8
```

Traps:

- `prev_obj_rect` is **intersected in place**, so `max_pre_size` at the end
  uses the *intersection's* height and width, not the original object's.
- `itemPer` at the end is whatever the loop left it as: the **last** item's
  info. If `nPreCount == 0` we already returned, so it is always initialized.
- `char_list_[len-2]` is read while `char_list_` may be much shorter than the
  temp buffers — this reaches into already-closed lines. Deliberate.
- `GetFontSize()` comparison is exact float equality.
- The `both rects empty` branch can only *return false*; falling through it
  reaches the `!IsEmpty() || !IsEmpty()` branch which is then false, so the
  intersection tests are skipped entirely for two empty rects.

### 1.12 `GetLooseBounds` — the loose char box

(:282-339), evaluated in the `CharInfo` constructor. Not observable through
`--txt`, but it is public API (`FPDFText_GetLooseCharBox`) and it is Tier-A
for any future dump, so it is specified in full.

```
if charinfo.char_box().IsEmpty(): return char_box            // covers generated chars
text_object = charinfo.text_object()
font_size = GetFontSize(text_object)          // = text_object&&font ? size : 1.0
if text_object && !FXSYS_IsFloatZero(font_size)
   && charinfo.char_code() != kInvalidCharCode:
    font = text_object.GetFont()
    is_vert_writing = font.IsVertWriting()
    if is_vert_writing && font.IsCIDFont():
        cid = cid_font.CIDFromCharCode(charinfo.char_code())
        vorg = cid_font.GetVertOrigin(cid)
        offsetx = (vorg.x - 500) * font_size / 1000
        offsety =  vorg.y        * font_size / 1000
        vert_width = cid_font.GetVertWidth(cid)              // typically negative
        height = vert_width * font_size / 1000
        left = charinfo.origin().x + offsetx;  right  = left + font_size
        top  = charinfo.origin().y + offsety;  bottom = top  + height
        box = Rect(left, bottom, right, top); box.Union(char_box); return box
    ascent = font.GetTypeAscent(); descent = font.GetTypeDescent()
    bbox = font.GetFontBBox()
    if bbox.top > bbox.bottom:                               // FX_RECT y-down!
        ascent  = min(ascent,  bbox.top)
        descent = max(descent, bbox.bottom)
    if ascent != descent:
        width = text_object.GetCharWidth(charinfo.char_code())    // already scaled
        inverse = charinfo.matrix().GetInverse()
        orig = inverse.Transform(charinfo.origin())
        left  = orig.x
        right = orig.x + (is_vert_writing ? -width : width)
        bottom = orig.y + descent * font_size / 1000
        top    = orig.y + ascent  * font_size / 1000
        box = charinfo.matrix().TransformRect(Rect(left, bottom, right, top))
        box.Union(char_box); return box
return char_box                                              // fallback
```

Note `GetInverse()` returns the **zero matrix** when the determinant is
exactly zero (fx_coordinates.cpp:380-395), which maps every point to
`(0,0)` — a degenerate but non-crashing loose box. Port that.

The double `Union(char_box)` at the end of both branches means the loose box
always contains the tight box. `Bug402562387` asserts exactly this as an
invariant; it is worth checking as a corpus-wide property, not one fixture.

`GetFontSize(text_object)` (:277-280) returns `kDefaultFontSize = 1.0` when
the object is null **or has no font**. Because generated characters carry
`text_object == nullptr` (§1.8b), **every generated character reports font
size 1.0** — pinned by `GetFontSize` on `hello_world.pdf`, where the two CRLF
chars sit between runs of size 12 and 16 and report `1`. The same nullness is
why `FPDFText_GetFontInfo` returns nothing for them, and why
`FPDFText_GetMatrix` reports the identity: a generated char's `matrix_` is
the `form_matrix`, which is `Affine::IDENTITY` for a top-level (non-form)
text object (§1.2). `GetMatrix` on `font_matrix.pdf` pins that literally.

Three empirical consequences worth stating, because they look like bugs:

- A generated character's tight box is **zero-area** by construction
  (`Rect(origin.x, origin.y, origin.x, origin.y)`, §1.7 (c) / §1.8b), so
  `GetLooseBounds`' first line (`char_box().IsEmpty()`) returns it unchanged —
  a generated char's loose box is also 0×0. `Bug399689604` pins this at
  top 100.0, and `SmallType3Glyph` pins two such boxes at top 50.0.
- The loose box is **font-uniform**: its top and height come from the font's
  ascent/descent, not the glyph's, so `Ā` and `Ă` — whose tight boxes differ
  by the accent — have identical loose boxes (`CharBoxForLatinExtendedText`).
- Under a 90° rotation the loose box's width and height **swap**
  (`CharBoxForRotated90DegreesText`, quadrants 1 and 3), because the
  `TransformRect` at the end is an axis-aligned bound of the rotated rect.

### 1.13 `char_indices_` — the two-text index map, and what `--txt` skips

Built at the end of `Init()` (:382-402):

```
count = CountChars()                    // = char_list_.len()
if count != 0: char_indices_.push({index: 0, count: 0})
skipped = false
for i in 0..count:
    ci = char_list_[i]
    if ci.char_type == kGenerated || IsNormalCharacter(ci):
        char_indices_.back().count += 1
        skipped = true
    else:
        if skipped: char_indices_.push({index: i+1, count: 0}); skipped = false
        else:       char_indices_.back().index = i + 1
```

(The local is named `skipped` but means "we have accumulated at least one
printing char since the last break" — the C++ naming is inverted; ignore it.)

`CharIndexFromTextIndex` / `TextIndexFromCharIndex` (:409-431) then walk the
segments. `GetPageText(start, count)` (:590-634) maps char indices to text
indices, scanning forward past leading non-printing chars and backward past
trailing ones, and slices `text_buf_`.

**`--txt` uses none of this.** `WriteText` (`write.cc:345-371`) is:

```
write u32 0x0000FEFF                       // BOM, little-endian → bytes FF FE 00 00
for i in 0 .. FPDFText_CountChars(textpage):     // = char_list_.len()
    write u32 FPDFText_GetUnicode(textpage, i)   // = char_list_[i].unicode() as u32
```

One file per page, named `<pdf_name>.<page>.txt`, opened in **text mode**
(`fopen(..., "w")`) — on POSIX that is identical to binary, and the harness
runs on Linux, so no CRLF translation occurs. There is **no separator between
pages** because each page is a separate file. There is exactly one BOM, at the
start of each per-page file. Every character contributes exactly 4 bytes; a
page with no text yields a 4-byte file containing only the BOM (verified:
`tagged_actual_text.pdf.0.txt` is `ff fe 00 00`).

`FPDFText_GetUnicode` returns `charinfo.unicode()` widened from `wchar_t`
(32-bit on Linux) to `unsigned int` — no surrogate handling, no filtering, no
substitution. Values that reach it in practice: `0` (charcode-0 chars),
`0x2` (hyphens), `0x3`/`0x93`/`0x94`/`0x96`/`0x97`/`0x98` (control chars),
and any `u32` from the `kNotUnicode` charcode passthrough. `0xFFFE` never
reaches `char_list_` from the hyphen path (it goes to `text_buf_`), but a
`kNotUnicode` char whose charcode happens to be `0xFFFE` would.

#### 1.13a The three divergences, verified against the built oracle

Every claim in this section was checked by running
`out/Release/pdfium_test --txt` on the fixture and comparing the bytes with the
`FPDFText_GetText` expectation in `fpdf_text_embeddertest.cpp`. The three
canonical fixtures, and the exact byte evidence:

| Fixture | `--txt` (`chars`) | `GetText` (`text`) | Divergence |
|---|---|---|---|
| `bug_781804.pdf` | `… 61 00 00 00 \| **02** 00 00 00 \| 73 …` (`a`, U+0002, `s`) | `…0x0061, **0xFFFE**, 0x0073…` | Hyphen: `0x2` vs `0xFFFE` (§1.8b) |
| `control_characters.pdf` | `48 65 6c 6c 6f \| **02 03** \| 2c 20 …` — 30 units | `"Hello, world!\r\nGoodbye, world!"` — 30 units, **no** `02 03`; `GetText(17, …)` yields `"Goodbye, world!"` | Control chars in `chars`, absent from `text`; **char indices still count them** (offset 17 = 15 + 2) (§1.10b) |
| `bug_425244539.pdf` | 27 units: **22 × `00 00 00 00`** then `hello` | 5 units: `"hello"` | 22 charcode-0 characters (§1.7 (f)): NUL in `chars`, nothing in `text` |

`bug_431824298.pdf` is a fourth, independent confirmation of the hyphen split:
`--txt` = `2d 68 65 6c 6c 6f 2d 0d 0a 2d 77 6f 72 6c 64 **02** 1f50 6b3e`
(18 units, `0x02` at index 15), while the embeddertest asserts `GetText`
returns `0xfffe` at that position (19 units including the NUL terminator).

`whitespace.pdf` produces a **4-byte file — the BOM alone** — confirming
`FPDFText_CountChars == 0` for a whitespace-only page
(`WhitespaceCharCount`, a pinned upstream bug: `crbug.com/40643656` says it
should be 1).

**Corpus frequency**, measured over the 5783 `--txt` goldens in
`conformance/goldens/`:

| Property | Count | Note |
|---|---|---|
| Total text goldens | 5783 | one per page |
| BOM-only (empty text) | 2808 | 49% of pages have no text at all |
| Containing `U+0000` | 3 | the charcode-0 passthrough |
| Containing `U+0002` | 16 | the hyphen sentinel |
| Containing `U+FFFE` | 0 | confirms `0xFFFE` never reaches `char_list_` here |

So the divergences are rare but real, and each one is a whole-file Tier-A
failure if missed. The harness's transcode step
(`conformance/src/transcode.rs`) already handles `U+0000` — `char::from_u32(0)`
succeeds — and would *error* on a surrogate or `> 0x10FFFF`, which the corpus
does not currently produce. See Q3.

**Consequence for `pdfrum-tool`:** emit the same `u32` stream, unfiltered,
BOM-first, one file per page. Do not route `--txt` through the `text` string.

### 1.14 Search — `CPDF_TextPageFind`

Operates entirely on `GetAllPageText()` = `GetPageText(0, CountChars())`, i.e.
on `text_buf_` filtered through `char_indices_`. **Not** on the `--txt`
stream. Options: `bMatchCase`, `bMatchWholeWord`, `bConsecutive`.

> **`[oracle-bug]` — one deliberate divergence (A42, 2026-09-02).** Everything
> below describes the C++ faithfully and is still accurate as a description of
> it. We diverge in exactly one place: the `U+FFFE` soft-hyphen sentinel is
> **dropped from the haystack** before matching, so a word split across a line
> break is found by a query without the hyphen.
>
> What makes it a bug rather than a trade-off is that **upstream does not match
> itself**: `cpdf_linkextract.cpp:154-155` repairs the very same sentinel
> (`Replace(L"\xfffe", L"-")`, comment "Replace the generated code with the
> hyphen char") for link detection, and `cpdf_textpagefind.cpp:209-211`/`:262`
> does a plain `Find` over the same buffer with no repair — and `U+FFFE` is not
> in its separator set (`:250`, `:284`). There is no coherent upstream
> behaviour to be faithful to. `crbug.com/431824298` is open. pdf.js joins
> across the break at query time with a reversible index map
> (`pdf_find_controller.js:131`, `:290-307`).
>
> The shape is pdf.js's: `find::search` builds the haystack without the
> sentinel and carries an `origins` map from haystack index back to text index,
> so **yielded ranges are still text offsets** and a match spanning a dropped
> sentinel still covers it. Measured cost: 0 rows — no board pass calls find.
>
> **Scope.** Only the search corpus changes. The `U+0002`/`U+FFFE`
> representation in the char list and the text buffer is untouched — that is
> A41, declined at 12 rows (see `docs/status/reopened-declines.md` §2.9), and
> the tables in §5.2 below still pin it exactly.

**Construction** (:191-217): the needle is lowercased (if not match-case) and
split by `ExtractFindWhat`; the haystack is `GetAllPageText()` lowercased the
same way. `find_next_start_ = startPos`; `find_pre_start_ =
startPos.value_or(len - 1)`. Both stay `nullopt`/unset when the page text is
empty.

`GetStringCase(s, match_case)` (:91-99): `match_case ? s : s.MakeLower()`.
`MakeLower` is `FXSYS_wcslwr` → `FXSYS_towlower` → ICU `u_tolower`, i.e.
**Unicode simple lowercase**, not ASCII and not full case folding. See D1/Q1.

`ExtractFindWhat(findwhat)` (:128-186) splits the needle into an array of
sub-needles:

```
if the needle is all spaces (or empty): return [needle]        // single element
index = 0
loop:
    word = ExtractSubString(needle, index)     // the index-th space-delimited token
    if word is None: break
    if word.is_empty(): push(""); index += 1; continue
    pos = 0
    while pos < word.len():
        curStr = word[pos..pos+1]; curChar = word[pos]
        if IsIgnoreSpaceCharacter(curChar):
            if pos > 0 && curChar == 0x2019 (RIGHT SINGLE QUOTATION MARK):
                pos += 1; continue                              // not a split point
            if pos > 0: push(word[..pos])
            push(curStr)
            if pos == word.len()-1: word.clear(); break
            word = word[pos+1..]; pos = 0; continue
        pos += 1
    if !word.is_empty(): push(word)
    index += 1
```

`ExtractSubString(full, i)` (:101-126) walks `i` space-delimited tokens,
skipping *runs* of spaces after each, and returns the next token (up to the
next space or end). Returns `None` when a `wcschr(.., ' ')` fails — i.e. when
there are fewer than `i` spaces left. Consequence: for `"a b"`, index 0 →
`"a"`, index 1 → `"b"`, index 2 → `None`. For `"a  b"` (two spaces), index 1
→ `"b"` (the run-skip). For a trailing space `"a "`, index 1 → `""` (empty
token), index 2 → `None`.

`IsIgnoreSpaceCharacter(c)` (:27-39) — returns **false** (i.e. "do not
split") for:

```
c < 255
0x0600..=0x06FF   (Arabic)
0xFE70..=0xFEFF   (Arabic Presentation Forms-B)
0xFB50..=0xFDFF   (Arabic Presentation Forms-A)
0x0400..=0x04FF   (Cyrillic)
0x0500..=0x052F   (Cyrillic Supplement)
0xA640..=0xA69F   (Cyrillic Extended-B)
0x2DE0..=0x2DFF   (Cyrillic Extended-A)
c == 8467         (0x2113 SCRIPT SMALL L)
0x2000..=0x206F   (General Punctuation)
```

and **true** (split) for everything else — CJK, Hangul, Devanagari, etc. The
name is backwards: returning true means "this character is a standalone
searchable unit, split around it". Note `c < 255` (not `<= 255`).

**`FindNext()`** (:229-323) — the match loop:

```
if page text empty or no start: return false
strLen = str_text_.len(); nStartPos = find_next_start_
if nStartPos >= strLen: return false
nCount = find_what_array_.len()
nResultPos = Some(0); bSpaceStart = false
for iWord in 0..nCount:                       // iWord is MUTATED inside the loop
    csWord = find_what_array_[iWord]
    if csWord.is_empty():
        if iWord == nCount - 1:
            if nStartPos >= strLen: return false
            c = str_text_[nStartPos]
            if c in {'\n', ' ', '\r', 0x00A0}: nResultPos = nStartPos + 1; break
            iWord = -1                        // restart the whole word loop
        else if iWord == 0:
            bSpaceStart = true
        continue
    nResultPos = str_text_.find(csWord, nStartPos)
    if nResultPos is None: return false
    endIndex = nResultPos + csWord.len() - 1
    if iWord == 0: res_start_ = nResultPos
    bMatch = true
    if iWord != 0 && !bSpaceStart:
        curChar  = csWord[0]
        lastChar = find_what_array_[iWord-1].back()
        if nStartPos == nResultPos
           && !(IsIgnoreSpaceCharacter(lastChar) || IsIgnoreSpaceCharacter(curChar)):
            bMatch = false
        for d in nStartPos..nResultPos:
            if str_text_[d] not in {'\n', ' ', '\r', 0x00A0}: bMatch = false; break
    else if bSpaceStart:
        if nResultPos > 0:
            c = str_text_[nResultPos - 1]
            if c not in {'\n',' ','\r',0x00A0}: bMatch = false; res_start_ = nResultPos
            else:                               res_start_ = nResultPos - 1
    if options.bMatchWholeWord && bMatch:
        bMatch = IsMatchWholeWord(str_text_, nResultPos, endIndex)
    if bMatch: nStartPos = endIndex + 1
    else:
        iWord = -1                             // restart
        index = if bSpaceStart {1} else {0}
        nStartPos = res_start_ + find_what_array_[index].len()
res_end_ = nResultPos + find_what_array_.back().len() - 1
if options.bConsecutive: find_next_start_ = res_start_ + 1; find_pre_start_ = res_end_ - 1
else:                    find_next_start_ = res_end_ + 1;   find_pre_start_ = res_start_ - 1
return true
```

The quirks worth naming:

- **Space tolerance:** consecutive sub-needles may be separated in the
  haystack only by `\n`, `\r`, `' '`, or U+00A0. A zero-gap join
  (`nStartPos == nResultPos`) is *rejected* unless one of the two joining
  characters is an "ignore-space" character — this is what lets CJK needles
  match without spaces while Latin ones require a separator.
- **`iWord = -1` restart** is a `for`-loop hack: the `++iWord` makes it `0`,
  restarting the scan from the new `nStartPos`. Infinite looping is prevented
  only by `nStartPos` advancing (`res_start_ + len(array[index])`) — which it
  does when `res_start_` was set, i.e. after `iWord == 0` ran once. A
  find-what array whose first element is empty and second element never
  matches loops until `Find` returns `None`. Reproduce with a bounded loop
  plus the same advance rule.
- **`res_end_` uses `nResultPos` from the last iteration** plus the length of
  the **last** array element — which is only correct if the loop ended
  normally. On the `break` from the empty-trailing-word path, `nResultPos` is
  `nStartPos + 1` and the last element is empty, so `res_end_ = nStartPos + 1
  + 0 - 1 = nStartPos` — a length-0-ish match. Ported as-is.
- **`find_pre_start_ = res_start_ - 1`** underflows to `SIZE_MAX` when
  `res_start_ == 0`. It is `std::optional<size_t>` and stays engaged, so
  `FindPrev` then compares against `SIZE_MAX + 1`. In Rust this is a
  `usize::wrapping_sub`, or better: an explicit `Option<usize>` that we
  document as saturating. Behavior difference only for `FindPrev` from a
  match at offset 0, which returns nothing either way.

`IsMatchWholeWord(text, startPos, endPos)` (:41-89):

```
if startPos > endPos: return false
char_count = endPos - startPos + 1
if char_count == 0: return false                 // unreachable given the above
if char_count == 1 && text[startPos] > 255: return true
char_left  = startPos >= 1 ? text[startPos-1] : 0
char_right = startPos + char_count < text.len() ? text[startPos+char_count] : 0
if (char_left  > 'A' && char_left  < 'a')        // note: EXCLUSIVE bounds
   || (char_left  > 'a' && char_left  < 'z')
   || (char_left  > 0xFB00 && char_left  < 0xFB06)
   || IsDecimalDigit(char_left)
   || (char_right > 'A' && char_right < 'a')
   || (char_right > 'a' && char_right < 'z')
   || (char_right > 0xFB00 && char_right < 0xFB06)
   || IsDecimalDigit(char_right):
    return false
if !(('A' > char_left  || char_left  > 'Z') && ('a' > char_left  || char_left  > 'z')
  && ('A' > char_right || char_right > 'Z') && ('a' > char_right || char_right > 'z')):
    return false
if IsDecimalDigit(char_left)  && IsDecimalDigit(text[startPos]): return false
if IsDecimalDigit(char_right) && IsDecimalDigit(text[endPos]):   return false
return true
```

The first block's ranges are **strictly exclusive on both ends**, so `'A'`,
`'a'`, `'z'`, `0xFB00` and `0xFB06` themselves pass it; the *second* block
then rejects any ASCII letter properly. `'Z'` is caught only by the second
block. The overlapping tests are redundant but harmless — transcribe both,
because the `0xFB00..0xFB06` exclusive band appears only in the first.
`FXSYS_IsDecimalDigit(wchar_t)` is `!((c & 0xFFFFFF80) || !iswdigit(c))`,
i.e. **ASCII `0`-`9` only**.

**`FindPrev()`** (:325-362) is a linear re-scan: it constructs a fresh engine
from position 0 and calls `FindNext()` repeatedly, keeping the last match
whose `GetCurOrder() + GetMatchedCount()` is `<= find_pre_start_ + 1`. The
result indices are re-derived through `TextIndexFromCharIndex`. `GetCurOrder()`
is `CharIndexFromTextIndex(res_start_)` and `GetMatchedCount()` is
`CharIndexFromTextIndex(res_end_) - CharIndexFromTextIndex(res_start_) + 1` —
both in **char-list** indices, while `res_start_`/`res_end_` are text-buffer
indices. `FPDFText_GetSchResultIndex` returns `GetCurOrder()`.

### 1.15 Link extraction — `CPDF_LinkExtract`

`ExtractLinks()` (:117-182) segments the page text into candidate strings at
generated/space/end boundaries, then tries web then mail on each:

```
start = 0; pos = 0; bAfterHyphen = false; bLineBreak = false
nTotalChar = text_page.CountChars()              // char_list_ length
page_text = text_page.GetAllPageText()           // text_buf_ via char_indices_ (!)
while pos < nTotalChar:
    ci = text_page.GetCharInfo(pos)
    if ci.char_type != kGenerated && ci.unicode != ' ' && pos != nTotalChar-1:
        bAfterHyphen = (ci.char_type == kHyphen)
                    || (ci.char_type == kNormal && ci.unicode == '-')
        pos += 1; continue
    nCount = pos - start
    if pos == nTotalChar - 1: nCount += 1
    else if bAfterHyphen && (ci.unicode == '\n' || ci.unicode == '\r'):
        bLineBreak = true; pos += 1; continue
    strBeCheck = page_text.Substr(start, nCount)
    if bLineBreak: strBeCheck.Remove('\n'); strBeCheck.Remove('\r'); bLineBreak = false
    strBeCheck.Replace("\xfffe", "-")
    if strBeCheck.len() > 5:
        while !strBeCheck.is_empty():
            ch = strBeCheck.back()
            if ch not in {')', ',', '>', '.'}: break
            strBeCheck = strBeCheck[..len-1]; nCount -= 1
        if nCount > 5:
            if let Some(link) = CheckWebLink(&strBeCheck):
                link.start += start; link_array_.push(link)
            else if CheckMailLink(&mut strBeCheck):
                link_array_.push(Link{ start, nCount, url: strBeCheck })
    start = ++pos
```

The critical, easily-missed defect: **`page_text` is indexed by `text_buf_`
offsets, while `pos`/`start`/`nCount` count `char_list_` entries.** These
coincide only when every character is "normal" and no normalization
multiplied anything. On a page with control characters or ligature
normalization the substring is misaligned. This is upstream behavior, it is
observable through `FPDFLink_GetTextRange`, and it is ported as-is (D5).

Also note `Substr(first, count)` returns an **empty string** when
`first + count - 1` is out of range (string_view_template.h:226-244) — it does
not clamp. And the post-trim gate is `nCount > 5`, where `nCount` was
decremented by the trim loop but `strBeCheck.len()` was too; they track.

`CheckWebLink(strBeCheck)` (:184-233):

```
str = strBeCheck.to_lower()                      // ICU simple lowercase
// 1. scheme form
start = str.find("http")
if start is Some:
    off = start + 4
    if str.len() > off + 4:                      // at least "://<char>"
        if str[off] == 's': off += 1
        if str[off]==':' && str[off+1]=='/' && str[off+2]=='/':
            off += 3
            end = FindWebLinkEnding(str, off,
                     TrimExternalBracketsFromWebLink(str, start, str.len()-1))
            if end > off:
                return Link{ start, end-start+1, strBeCheck.substr(start, end-start+1) }
// 2. www form
start = str.find("www.")
if start is Some:
    off = start + 4
    if str.len() > off:
        end = FindWebLinkEnding(str, start,
                 TrimExternalBracketsFromWebLink(str, start, str.len()-1))
        if end > off:
            return Link{ start, end-start+1, "http://" + strBeCheck.substr(...) }
return None
```

Note the URL text in the returned `Link` is taken from the **original-case**
`strBeCheck`, while the offsets come from the lowercased copy. `MakeLower`
is a per-character simple mapping so lengths always match — except for
characters whose simple lowercase differs in length, which
`u_tolower`/`FXSYS_wcslwr` never produces (it is `wchar_t`-to-`wchar_t`).

The `str.len() > off + 4` gate on the scheme form means `"http://a"`
(len 8, off 4, needs > 8) fails but `"http://ab"` (len 9) passes — this is
what makes the unittest's `L"http://example"` succeed and short ones fail.
And the `www.` form's `start` is passed to `FindWebLinkEnding` (so the scan
starts at `w`), while the scheme form passes `off` (after `//`).

`FindWebLinkEnding(str, start, end)` (:24-68), `str` lowercased:

```
if str.contains('/', start): return end          // path present: no sanitizing
if str[start] == '[':                            // IPv6 reference
    if let Some(close) = str.find(']', start+1):
        end = close
        if end > start + 1:                      // non-empty brackets
            off = end + 1
            if off < len && str[off] == ':':
                off += 1
                while off < len && IsDecimalDigit(str[off]): off += 1
                if off > end + 2 && off <= len: end = off - 1   // ≥1 port digit
    return end
// host name: RFC1123 — alnum, hyphen, period; hyphen not at the end
while end > start && str[end] < 0x80:
    if IsDecimalDigit(str[end]) || ('a' <= str[end] <= 'z') || str[end] == '.': break
    end -= 1
return end
```

Note the host-name trim loop's condition `str[end] < 0x80` — a non-ASCII
trailing character **stops the loop immediately** and is kept, which is why
`L"www.测试。net。"` yields a URL ending in the ideographic full stop.
Uppercase ASCII is not in the keep-set, but the string is lowercased, so it
never appears.

`TrimExternalBracketsFromWebLink(str, start, end)` (:89-108):

```
for pos in 0..start:                             // characters BEFORE the URL
    match str[pos]:
        '(' => TrimBackwardsToChar(str, ')', start, &mut end)
        '[' => TrimBackwardsToChar(str, ']', start, &mut end)
        '{' => TrimBackwardsToChar(str, '}', start, &mut end)
        '<' => TrimBackwardsToChar(str, '>', start, &mut end)
        '"' => TrimBackwardsToChar(str, '"', start, &mut end)
        '\''=> TrimBackwardsToChar(str, '\'', start, &mut end)
return end
```

`TrimBackwardsToChar(str, ch, start, end)` (:73-83) scans **backwards** from
`*end` down to `start`, and on the first hit sets `*end = pos - 1`. The loop
condition is `pos >= start` with `pos--` on a `size_t`: when `start == 0`,
`pos` underflows to `SIZE_MAX` and the loop reads `str[SIZE_MAX]` — **an
out-of-bounds read**. It is unreachable in practice because the caller's
`for pos in 0..start` body only runs when `start > 0`. Our version uses a
`(start..=*end).rev()` range and cannot underflow (D6).

`CheckMailLink(str)` (:235-307) — mutates `str` into the mailto URL:

```
aPos = str.find('@')
if aPos is None || aPos == 0 || aPos == str.len()-1: return false
// --- local part, scanning backwards from '@' ---
pPos = aPos
for i in (1..=aPos).rev():                        // i from aPos down to 1
    ch = str[i-1]
    if ch == '_' || ch == '-' || iswalnum(ch): continue
    if ch != '.' || i == pPos || i == 1:
        if i == aPos: return false                // '.' or junk immediately before '@'
        removed_len = if i == pPos { i + 1 } else { i }
        str = str[removed_len..]
        break
    pPos = i - 1                                  // a valid '.'
// --- domain part ---
aPos = str.find('@')
if aPos is None || aPos == 0: return false
str.trim_back('.')
ePos = str.find('.', aPos + 1)
if ePos is None || ePos == aPos + 1: return false  // need a dot, not right after '@'
nLen = str.len(); pPos = 0
for i in (aPos+1)..nLen:
    wch = str[i]
    if wch == '-' || iswalnum(wch): continue
    if wch != '.' || i == pPos + 1:
        host_end = if i == pPos + 1 { i - 2 } else { i - 1 }
        if pPos > 0 && host_end - aPos >= 3:
            str = str[..host_end+1]; break
        return false
    pPos = i
if !str.contains("mailto:"): str = "mailto:" + str
return true
```

Traps: `pPos` is reused with two different meanings (index of the last valid
`.`+1 in the local part, index of the last `.` in the domain part) and is
re-initialized to `0` between them. The `pPos > 0` guard in the domain loop is
"we have seen at least one dot" — but `pPos = i` where `i > aPos ≥ 0`, so any
dot makes it positive. `host_end - aPos >= 3` is unsigned arithmetic on
`size_t`; `host_end` can be `i - 2` with `i == aPos + 1` giving
`host_end = aPos - 1` and the subtraction underflowing to a huge number —
which then passes the `>= 3` test. Reachable when `str[aPos+1] == '.'`… but
that case already returned via the `ePos == aPos + 1` check. Use checked
arithmetic and document the reasoning (D6).

`str.contains("mailto:")` is a substring search anywhere, not a prefix test —
so `"x@mailto:y.com"` would not get the prefix. Ported.

`GetRects(index)` (:313-320) returns `text_page.GetRectArray(start, count)`;
`GetTextRange` returns the `{start, count}` pair.

### 1.16 `GetRectArray`, `GetIndexAtPos`, `GetTextByRect`, `GetTextByObject`

Selection helpers, public API, not `--txt`-observable. Specified for
completeness since SPEC §9 exposes `TextPage::chars` and a future dump may
use them.

`GetRectArray(start, count)` (:433-481): clamps `count`, then walks
`char_list_[start..start+count]`, skipping `kGenerated` chars and chars whose
box is narrower or shorter than `kSizeEpsilon`, unioning consecutive chars
that share a `text_object` pointer and starting a new rect on a change.
`rect.Normalize()` is applied only to the *first* char of each run. A final
`rects.push_back(rect)` happens unconditionally — including when every char
was skipped, in which case a default-constructed (all-zero) rect is pushed.
`CountRects` returns `-1` for `start < 0` and otherwise caches into
`sel_rects_`.

`GetIndexAtPos(point, tolerance)` (:483-521): exact-containment scan first
(returns the first containing char), else a nearest-char search minimizing
`|dx| + |dy|` to the nearest edge, over boxes expanded by `tolerance/2` on
each side. Sentinels start at `5000`. Returns `-1` if nothing qualifies.

`GetTextByPredicate(pred)` (:523-555) — the shared body for `GetTextByRect`
and `GetTextByObject`. Inserts `\r\n` between chars whose `origin.y` changed
while the previous char failed the predicate and a line feed is pending;
tracks `IsContainPreChar` / `IsAddLineFeed` flags. `GetTextByRect` uses
"rect intersects char_box" (via `IsRectIntersect`, :159-163, which intersects
copies and asks `!IsEmpty()`); `GetTextByObject` uses pointer equality on
`text_object`.

### 1.17 Diagnostics — what damage looks like here

`core/fpdftext/` performs **no recovery and records no errors**: it has no
error channel at all. Every "damaged" input is handled by a silent skip or a
default value. The complete list of silent drops, each of which becomes a
`Diagnostic` in our port (STYLE §3: "never silently swallow a recovery"):

| Situation | C++ behavior | Our `DiagKind` |
|---|---|---|
| Text object with `|rect.Width()| < 0.01` | dropped (:881, :1076) | `TextObjectDegenerate` |
| Text object after a zero-item object | dropped (:900) | `TextObjectDropped` |
| Object matching a predecessor | dropped (:892) | `TextObjectDuplicate` |
| Charcode with no unicode | raw charcode as unicode | `TextCharcodeUnmapped` |
| Charcode 0 | `unicode = 0` emitted | `TextCharcodeZero` |
| Duplicate char at same position | unicode suppressed | `TextCharDeduplicated` |
| `/ActualText` present but unprintable | object emits nothing | `TextActualTextUnprintable` |
| `/ActualText` char `>= 0xFFFD` | skipped | `TextActualTextCharDropped` |
| `GetInverse` on a singular matrix | zero matrix | `TextMatrixSingular` |
| Zero page size ⇒ zero display matrix | no batch splits | `TextPageSizeZero` |
| Hyphen path with an empty temp list | **C++ crashes** | `TextHyphenNoPrevChar` (we skip) |

`Diagnostics` is bounded (SPEC §1), so a pathological page cannot turn the
per-character diagnostics into an OOM. Per-character kinds (`Deduplicated`,
`CharcodeUnmapped`) are common enough on real files that they would flood the
sink; they are recorded at `Severity::Recovered` and the bound handles the
rest. See Q4 for whether to demote them further.

---

## 2. Divergences

**D1 — Unicode character properties come from tables we generate, not ICU.**
The C++ reaches ICU for four predicates: `u_isalpha` (`IsHyphen`),
`u_isalnum` (`IsHyphen`, `CheckMailLink`), `u_tolower` (search case folding,
`CheckWebLink`), and its own 65536-entry `fx_ucddata.inc` table for bidi class
and mirroring. DEPS.md's closed set contains `unicode-bidi` (which gives
*bidi algorithm*, not the raw class/mirror pair we need in PDFium's exact
5-class-bucket form) and nothing for general category or case mapping.

Resolution: **`pdfrum-text` ships a `build.rs`-generated table**, exactly as
`pdfrum-cmap` does for CJK CMaps (SPEC §6) — transcribed from the oracle's own
`core/fxcrt/fx_ucddata.inc` for bidi class + mirror index, and from the
oracle's `third_party/icu` property data for `isalpha`/`isalnum`/`tolower`.
Sourcing the predicates from the *oracle's own ICU version* is the only way to
be byte-exact: a newer Unicode revision would classify some characters
differently. This adds **no dependency** (build scripts are code, not crates)
and matches the precedent the cmap crate already set. See Q1 for the
escalation: this is a design decision with a real size cost (§3).

We do **not** use `unicode-bidi` in this crate. PDFium's bidi is not the UBA:
it is a four-way bucket (`kLeft`/`kLeftWeak`/`kRight`/`kNeutral`) over raw
bidi classes with a segment-run reversal, no embedding levels, no paragraph
resolution, no mirroring pass beyond a single lookup. Feeding it through a
real UBA implementation would produce different output. `unicode-bidi` stays
in DEPS.md for whoever needs it (the render side's text shaping, if ever);
this crate does not link it. **This is a DEPS.md observation, not a change.**

**D2 — Two outputs, one type.** The C++ keeps `char_list_` and `text_buf_` as
separate members and lets the callers pick. We keep the same split but name it:
`TextPage { chars: Vec<CharBox>, text: String, index: CharIndex }`, where
`chars` is the `--txt` source and `text` is the search/`GetPageText` source.
SPEC §9's `TextPage { chars, runs: … }` sketch is honored — `runs` becomes the
`CharIndex` segment table (§1.13) plus the derived `text`. Making the two
outputs *visibly* different types in the public API is the single best defence
against Fact 1 being lost in a refactor.

**D3 — `Vec<CharBox>` instead of a reordering `std::vector`.** The C++ does
`erase(begin()+i)` and `pop_back()` on `temp_char_list_` mid-loop. We use the
same operations on a `Vec` — this is one of the rare places where the C++
shape is also the right Rust shape, because the algorithm genuinely is
"build a line, then splice it". No divergence, recorded so a reviewer does not
flag it as a transliteration.

**D4 — The hyphen path's empty-list dereference is fixed, not ported.**
`ProcessGenerateCharacter`'s `kHyphen` arm takes `temp_char_list_.back()`
after popping trailing spaces (§1.8b trap 2). Reaching it with an empty list
is possible when `IsHyphen` consulted `text_buf_` while `temp_text_buf_` was
empty. In C++ that is a `CHECK` failure (crash) in debug and UB in release;
`clippy::indexing_slicing` and STYLE §3 forbid the equivalent. We return
without emitting the hyphen and record `TextHyphenNoPrevChar`. **This can
diverge from the oracle** — but only on an input where the oracle crashes, and
a crashing oracle produces no golden, so no Tier-A comparison exists. Flagged
in Q5 in case a release-mode oracle silently produces something.

**Relabelled 2026-09-02 (oracle-divergence audit, A50): an oracle bug, so
declining it is obligatory rather than our choice, and Q5 no longer needs an
answer.** Verified at the line: `cpdf_textpage.cpp:1357` is
`CharInfo& charinfo = temp_char_list_.back();` with **no** emptiness guard,
directly after the `while` at `:1352-1356` that pops trailing spaces from
`temp_char_list_` and `temp_text_buf_` together. Nothing in §9.10 asks a text
extractor to crash, and pdf.js has no staging list of this shape to
dereference — its soft hyphen is normalised to `-` (`unicode.js:57-58`) and
rejoined at query time (`pdf_find_controller.js:290-307`), so there is no
sentinel and no back-reference. Q5 asked what a release-mode oracle produces
instead of crashing; PLAN.md §212–229 settles the case without that answer,
since a crashing oracle writes no golden either way. `pipeline.rs` carries
`// [oracle-bug]`. No code change; 0 rows.

**D5 — Link extraction's index mismatch is ported.** §1.15's
`page_text` (text-buffer indexed) vs `pos`/`start` (char-list indexed)
mismatch is a genuine upstream bug that changes which substring is checked.
It is observable through `FPDFLink_GetTextRange` and, transitively, through
which links are found. We port it exactly: `web_links()` slices the `text`
string by char-list-derived offsets, with a saturating/`get()`-based slice
that yields an empty candidate instead of panicking when the offsets run past
the text (matching `Substr`'s empty-on-out-of-range, §1.15).

**D6 — Two unsigned underflows are made explicit.**
`TrimBackwardsToChar`'s `pos--` at `start == 0` (unreachable, §1.15) and
`CheckMailLink`'s `host_end - aPos` (§1.15) both rely on `size_t` wrapping.
We use ranges and `checked_sub`, preserving the *reachable* semantics and
eliminating the unreachable UB. No behavioral difference on any input the
C++ does not already crash on.

**D7 — `FindPrev` is not a separate engine.** The C++ constructs a whole
second `CPDF_TextPageFind` and replays `FindNext` from zero. We expose
`find(needle, opts) -> impl Iterator<Item = Range<usize>>` (SPEC §9) as the
primitive and derive "previous" by collecting or by a reverse scan over the
same iterator. Same results, no second engine. The `bConsecutive` option
changes the *iterator's step* (advance by 1 from `res_start_` instead of past
`res_end_`), which the iterator models directly.

**D8 — No `GetIndexAtPos`/`CountRects` statefulness.** The C++ caches
`sel_rects_` in the object so `GetRect(i)` can read it after `CountRects`.
Ours returns `Vec<Rect>` from one call. Pure API shape, zero behavior.

**D9 — Diagnostics where the C++ has none.** §1.17. Every silent drop becomes
a `Diagnostic`. This is additive; it changes no output.

**D10 — `rtl_` is an explicit option, not a document lookup.** The C++
constructor takes `bool rtl` filled by `CPDF_ViewerPreferences::IsDirectionR2L()`
(`/Root /ViewerPreferences /Direction` == the **name-typed-as-bytestring**
`R2L`) at the SDK layer. We take it in `ExtractOptions { rtl: bool }` with a
`pdfrum-doc`-side helper to compute it, keeping this crate free of catalog
walking. `pdfrum-tool` must set it the same way the oracle does, or every
R2L-preference file diverges. **`--txt` is affected**, so this is a
`pdfrum-tool` requirement, recorded in §4.

---

## 3. Module plan

```
crates/pdfrum-text/
├── build.rs                  # generates unicode tables → OUT_DIR blob (D1)
├── tables/                   # inputs to build.rs, transcribed from the oracle
│   ├── ucd.rs                # (bidi class, mirror idx) per BMP code point + mirror pairs
│   ├── category.rs           # is_alpha / is_alnum bitsets
│   ├── lowercase.rs          # simple lowercase mapping deltas
│   └── normalization.rs      # the four kUnicodeDataNormalization* tables
└── src/
    ├── lib.rs                # public surface (below)
    ├── error.rs              # Error enum (thiserror)
    ├── charinfo.rs           # CharBox record + CharType + loose-bounds derivation (§1.2, §1.12)
    ├── unicode.rs            # table accessors: bidi_class, mirror, normalize,
    │                         #   is_alpha, is_alnum, to_lower, is_control_char (§1.10a, §1.10b)
    ├── bidi.rs               # the four-bucket segmenter + overall direction (§1.10a)
    ├── orientation.rs        # FindTextlineFlowOrientation + GetTextObjectWritingMode (§1.4, §1.8)
    ├── collect.rs            # ProcessObject / form walk / the reorder batch (§1.1, §1.3, §1.5)
    ├── emit.rs               # per-object item emission + dedup + space threshold (§1.7)
    ├── insert.rs             # ProcessInsertObject + GenerateSpace + line-end tests (§1.8)
    ├── marked.rs             # /ActualText (§1.9)
    ├── line.rs               # CloseTempLine + AddCharInfo + normalization (§1.10)
    ├── dedup.rs              # IsSameTextObject / IsSameAsPreTextObject (§1.11)
    ├── index.rs              # char_indices_, page_text, text↔char mapping (§1.13)
    ├── find.rs               # search (§1.14)
    ├── links.rs              # web + mail extraction (§1.15)
    └── select.rs             # rect arrays, index-at-pos, text-by-rect/object (§1.16)
```

### 3.1 `lib.rs` — public surface

```rust
#![forbid(unsafe_code)]

pub use error::Error;

/// SPEC §9. The extracted text of one page: the character stream the oracle's
/// `--txt` emits, the filtered text string search and selection operate on,
/// and the map between them.
#[derive(Debug, Clone)]
pub struct TextPage {
    /// One entry per emitted character, in reading order. **This is the
    /// `--txt` stream**: `chars[i].unicode` is exactly what the oracle
    /// writes as the i-th UTF-32LE code unit (§1.13). Includes control
    /// characters, `\0`, and hyphen sentinels.
    pub chars: Vec<CharBox>,
    /// The filtered text `find`, `page_text` and `web_links` operate on —
    /// **not** the same character sequence as `chars` (§1.10b).
    pub text: String,
    /// Segment table mapping `text` offsets ↔ `chars` indices (§1.13).
    pub runs: CharIndex,
}

/// One extracted character. Mirrors `CPDF_TextPage::CharInfo` (§1.2).
#[derive(Debug, Clone, Copy)]
pub struct CharBox {
    pub char_type: CharType,
    /// The `--txt` code unit. `0` is legal; not necessarily a Unicode scalar.
    pub unicode: u32,
    /// `None` where the C++ uses `kInvalidCharCode`.
    pub code: Option<CharCode>,
    pub origin: Point,
    pub char_box: Rect,
    pub loose_char_box: Rect,
    /// Composed text×form matrix for real chars; the bare form matrix for
    /// generated ones (§1.2).
    pub matrix: Affine,
    /// Index into `Page::objects` of the owning text object; `None` for
    /// `GenerateCharInfo`-produced characters (§1.8b).
    pub object: Option<ObjectIndex>,
    pub font_size: f32,
    /// `atan2(matrix.c, matrix.a)`, normalized to `[0, 2π)`.
    pub angle: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharType { Normal, Generated, NotUnicode, Hyphen, Piece, ActualText }

/// The `char_indices_` segment table (§1.13).
#[derive(Debug, Clone, Default)]
pub struct CharIndex { segments: Vec<CharSegment> }
#[derive(Debug, Clone, Copy)]
pub struct CharSegment { pub index: u32, pub count: u32 }
impl CharIndex {
    #[must_use] pub fn char_index(&self, text_index: usize) -> Option<usize>;
    #[must_use] pub fn text_index(&self, char_index: usize) -> Option<usize>;
}

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// `/Root /ViewerPreferences /Direction (R2L)` — forces overall bidi
    /// direction (§1.10, D10). `pdfrum-tool` must set this to match the
    /// oracle or every R2L-preference file diverges.
    pub rtl: bool,
}

/// SPEC §9. Pure derivation over an already-built page. Never renders,
/// never panics; damage goes to `diags`.
#[must_use]
pub fn extract(
    page: &Page, r: &impl Resolve, opts: &ExtractOptions,
    limits: &Limits, diags: &mut Diagnostics,
) -> TextPage;

impl TextPage {
    /// `text[start..]`-style slice by **char** index, with the C++'s
    /// scan-forward/scan-back over non-printing characters (§1.13).
    #[must_use] pub fn page_text(&self, start: usize, count: usize) -> &str;
    /// SPEC §9. Match ranges are **text** offsets (§1.14).
    pub fn find<'a>(&'a self, needle: &str, opts: &FindOptions)
        -> impl Iterator<Item = Range<usize>> + 'a;
    /// SPEC §9. §1.15.
    #[must_use] pub fn web_links(&self) -> Vec<WebLink>;
    #[must_use] pub fn rects(&self, start: usize, count: usize) -> Vec<Rect>;
    #[must_use] pub fn index_at(&self, p: Point, tolerance: Size) -> Option<usize>;
    #[must_use] pub fn text_in_rect(&self, rect: Rect) -> String;
    #[must_use] pub fn text_of_object(&self, obj: ObjectIndex) -> String;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FindOptions {
    pub match_case: bool, pub match_whole_word: bool, pub consecutive: bool,
}

#[derive(Debug, Clone)]
pub struct WebLink { pub url: String, pub range: Range<usize>, pub rects: Vec<Rect> }
```

`TextPage` is `Send + Sync + Clone` (STYLE §4). No trait is defined here; the
only trait crossing the boundary is `Resolve` (SPEC §2), needed to read
`/ActualText` param dictionaries.

### 3.2 Internal types

The pipeline is a fold over a small mutable state record, built once and
consumed:

```rust
/// The in-progress extraction. Not public; `extract` builds one, drives it,
/// and destructures it into a `TextPage`. Small on purpose: the C++'s
/// 14-member god object splits into this plus the pure helpers below.
struct Builder<'a> {
    chars: Vec<CharBox>,          // char_list_
    text: String,                 // text_buf_
    line: Line,                   // temp_char_list_ + temp_text_buf_, paired
    prev: Option<PrevObject>,     // prev_text_obj_ + prev_matrix_
    batch: Vec<Pending<'a>>,      // text_objects_
    line_dir: Orientation,        // textline_dir_
    curline_rect: Rect,
    display: Affine,
    rtl: bool,
}

/// The paired staging buffers. Keeping them in one type makes the 1:1
/// invariant (§1.6) a type-level fact instead of a comment.
#[derive(Default)]
struct Line { chars: Vec<CharBox>, text: Vec<u32> }
impl Line {
    fn push(&mut self, unit: u32, info: CharBox);
    fn pop(&mut self);
    fn last(&self) -> Option<(&u32, &CharBox)>;
    fn reverse_from(&mut self, index: usize);
    fn collapse_spaces(&mut self);          // §1.10 (a)
    fn take(&mut self) -> (Vec<u32>, Vec<CharBox>);
}

struct Pending<'a> { obj: &'a TextObject, form: Affine }
struct PrevObject { obj_index: ObjectIndex, form: Affine }

#[derive(Copy, Clone, PartialEq, Eq)]
enum Orientation { Unknown, Horizontal, Vertical }
#[derive(Copy, Clone, PartialEq, Eq)]
enum Generate { None, Space, LineBreak, Hyphen }
#[derive(Copy, Clone, PartialEq, Eq)]
enum MarkState { Pass, Done, Delay }
```

Note `Line::text` is `Vec<u32>`, not `String`: the staging buffer legitimately
holds `0xFFFE` and lone `0` values that are not what `String` will finally
contain. The conversion to `String` happens in `AddCharInfo`'s equivalent,
where non-scalar units are already impossible (mirroring/normalization only
produce BMP scalars, and the `0xFFFE` sentinel is a valid scalar). See Q3.

Everything else is a free function taking what it needs:
`orientation::page_flow(&Page) -> Orientation`,
`insert::decide(prev, this, ctx) -> Generate`,
`emit::items(obj, form, matrix, &mut Line, diags) -> bool`,
`line::close(&mut Builder)`, `dedup::same_object(a, b, chars) -> bool`. No
method takes `&mut self` on `Builder` unless it genuinely mutates the whole
pipeline state (STYLE §1).

### 3.3 The unicode tables (`build.rs`)

Four table groups, all generated at build time into one `OUT_DIR` blob
loaded with `include_bytes!` — the `pdfrum-cmap` pattern (SPEC §6):

| Table | Source | Encoded size |
|---|---|---|
| bidi class + mirror index | `core/fxcrt/fx_ucddata.inc` (65536 rows, 5+9 bits) | 128 KiB raw; ~6 KiB run-length encoded (long constant runs) |
| mirror pairs | `kFXTextLayoutBidiMirror` in `fx_unicode.cpp` | ~1 KiB |
| `is_alpha` / `is_alnum` | oracle ICU property data, materialized as two BMP bitsets | 16 KiB (8 KiB each), or ~2 KiB as ranges |
| simple lowercase | oracle ICU, materialized as sorted `(from, to)` deltas over the BMP | ~8 KiB as ranges |
| normalization | the four `kUnicodeDataNormalization*` arrays | 145 KiB raw; ~40 KiB with the main table run-length encoded |

The `tables/` inputs are checked in as Rust source (transcribed once, by a
script, from the oracle checkout) so the build does not depend on the oracle
being present — the same rule `pdfrum-cmap` follows. `build.rs` compacts them;
`unicode.rs` decodes at lookup time with a two-level index. Total binary cost
after compaction: **~60 KiB**, versus 290 KiB naive. Above the BMP, all
lookups return the C++'s defaults (`kON`, no mirror, no normalization) because
the tables are `wchar_t & 0xFFFF`-indexed — `GetUnicodeNormalization` masks
explicitly (:102) and `GetUnicodeProperties` bounds-checks (fx_unicode.cpp:41-48).

`is_alpha`/`is_alnum`/`to_lower` above the BMP: the C++ passes a 32-bit
`wchar_t` straight to ICU, which *does* handle supplementary planes. Our
tables must too, or a supplementary-plane character in `IsHyphen` or
`CheckMailLink` diverges. Resolution: store BMP as bitsets and supplementary
planes as sorted ranges (the alphabetic/alnum sets above U+FFFF are large but
contiguous; ~200 ranges covers them). Lowercase above the BMP has ~1500
mappings (Deseret, Adlam, Warang Citi, …); stored as ranges with a delta.

### 3.4 Data flow

```
Page (pdfrum-page)  ──►  collect::process_page
                            │  orientation::page_flow          (§1.4)
                            │  for each object, in content order:
                            │     Text ──► collect::process_text_object   (§1.5)
                            │              ├─ dedup::same_as_predecessor  (§1.11)
                            │              ├─ flush on y-jump ──► drain   (§1.6)
                            │              └─ insertion sort by x
                            │     Form ──► recurse with composed matrix   (§1.3)
                            └─ drain the tail                             (§1.6)
                                 │  for each pending object, x-sorted:
                                 │     marked::pre                        (§1.9)
                                 │     insert::decide ──► Generate        (§1.8)
                                 │     insert::apply  (space/CRLF/hyphen)  (§1.8b)
                                 │     marked::process  (Delay only)      (§1.9)
                                 │     emit::items ──► Line               (§1.7)
                                 │        └─ maybe reverse_from
                                 └─ line::close                           (§1.10)
                                      ├─ collapse_spaces
                                      ├─ bidi::segments
                                      └─ add_char_info ──► chars + text   (§1.10b)
                            ──►  index::build                             (§1.13)
                            ──►  TextPage { chars, text, runs }

TextPage ──► find::search   (over `text`)                                 (§1.14)
         ──► links::extract (over `chars` indices × `text` bytes)         (§1.15)
         ──► select::*      (over `chars`)                                (§1.16)
```

Every stage is a function over values; the only mutable state is `Builder`,
threaded as `&mut` through the collection half and dropped before the query
half runs. `extract` is pure w.r.t. its inputs (STYLE §2).

### 3.5 Decomposition notes (STYLE §7)

`CPDF_TextPage` is a 1588-line class with 14 members and 30 methods. The
decomposition above is not cosmetic — it is what makes the two-output
invariant checkable:

- **`Line` exists so the 1:1 invariant is a type.** In C++ every
  `temp_text_buf_.AppendChar` must be manually paired with a
  `temp_char_list_.push_back`; three sites do it, two sites pop both, one
  reverses both. A single `Line::push(unit, info)` makes desync impossible,
  and `Line::reverse_from` replaces `ReverseTempTextBufs`' two independent
  reversals with one.
- **The per-object emission (`emit`) is separated from the inter-object
  decision (`insert`).** In C++ these interleave through `prev_text_obj_`;
  separating them makes the "what does `prev` mean at this point" question
  answerable, which is where §1.8a's subtlety lives.
- **`orientation` is a leaf module with no state.** Both orientation
  functions are pure over a `&Page` / `&TextObject` plus the page-global
  guess.
- **No `Rc<RefCell<>>`, no back-pointers.** `CharInfo::text_object_` is an
  `UnownedPtr` in C++; ours is an `ObjectIndex` into `Page::objects`
  (STYLE §2: "cross-references stay as ids"). The two places that compare
  text-object identity (`GetTextByObject`, the dedup font check) become index
  comparison and `Arc::ptr_eq` on the font respectively.

---

## 4. Interface required from `pdfrum-font` and `pdfrum-page`

This is the crate's dependency contract. Everything here is *consumed*; none
of it is re-derived locally.

### 4.1 From `pdfrum-font`

The font brief (§3.1) exposes `Font::decode -> Iterator<CharItem>` and a
handful of predicates. Text extraction needs **more than `decode` gives**,
because the C++ calls per-charcode accessors at points where it does not have
a decoded string. Required additions, all of which the font brief's internal
data already contains:

| Needed | C++ counterpart | Why extraction needs it |
|---|---|---|
| `fn unicode(&self, code: CharCode) -> SmallVec<[char; 2]>` | `CPDF_Font::UnicodeFromCharCode` | §1.7 (d), §1.8, §1.10a — called on a **single charcode**, not a string. Must implement the full ladder: `/ToUnicode` first, then simple-font encoding table (`CPDF_SimpleFont`:123-133) or CID coding/`cid2unicode` (`CPDF_CIDFont`:309-334). Empty result is meaningful (⇒ `kNotUnicode`). |
| `fn char_code_from_unicode(&self, u: char) -> Option<CharCode>` | `CPDF_Font::CharCodeFromUnicode` | §1.7b — `CharCodeFromUnicode(' ')`. `/ToUnicode` reverse lookup first, then encoding reverse (simple) or the CID coding switch incl. the **65536-iteration linear CID scan** (`CPDF_CIDFont`:372-386). `None` ⇔ `kInvalidCharCode`. |
| `fn char_width(&self, code: CharCode) -> i32` | `CPDF_Font::GetCharWidth` | §1.7c rung 1, glyph units. |
| `fn string_width(&self, bytes: &[u8]) -> i32` | `CPDF_Font::GetStringWidth` | §1.7c rung 2. |
| `fn append_char(&self, out: &mut Vec<u8>, code: CharCode)` | `CPDF_Font::AppendChar` | §1.7c rung 2's input. Simple: one truncated byte. CID: the cmap's byte encoding. |
| `fn char_bbox(&self, code: CharCode) -> Option<Rect>` | `CPDF_Font::GetCharBBox` | §1.7 (e), §1.7c rung 3. Glyph units, y-up as `FX_RECT` is y-**down** — see below. |
| `fn type_ascent(&self) -> i32` / `type_descent(&self) -> i32` | `GetTypeAscent`/`GetTypeDescent` | §1.12 |
| `fn font_bbox(&self) -> Rect` | `GetFontBBox` | §1.12 (already in the font brief) |
| `fn is_vertical(&self) -> bool` | `IsVertWriting` | §1.12 (already in the font brief) |
| `fn cid_from_char_code(&self, code) -> Option<Cid>` | `CPDF_CIDFont::CIDFromCharCode` | §1.12 vertical branch |
| `fn vert_origin(&self, cid: Cid) -> (i16, i16)` | `GetVertOrigin` | §1.12 vertical branch |
| `fn vert_width(&self, cid: Cid) -> i16` | `GetVertWidth` | §1.12 vertical branch |

**`char_bbox` coordinate convention.** `FX_RECT` is y-**down** (`IsEmpty()` is
`right <= left || bottom <= top`), but `GetCharBBox` for a simple font returns
glyph-space values where `top > bottom` in the y-**up** sense, and §1.7 (e)
uses `rect.bottom * fs + origin.y` as the box's *bottom*. In practice the
C++'s `FX_RECT` here holds y-up values in a y-down type. Our `Rect` (kurbo)
must receive them y-up: `Rect::new(left, bottom, right, top)` with
`bottom < top`. The font crate must state which convention it returns; §1.12's
`if font_bbox.top > font_bbox.bottom` test only makes sense with the y-up
reading. **Action: `pdfrum-font` must document `char_bbox`/`font_bbox` as
y-up glyph units**; this brief assumes that.

Every one of these is a `&self` method on `Font` returning a plain value —
no `&mut self` caching (the C++ memoizes `char_bbox_[256]` inside the font;
ours is either recomputed or memoized behind the font's own interior
mutability, which is the font crate's business).

### 4.2 From `pdfrum-page`

`Page::objects: Vec<PageObject>` with, for `PageObject::Text(TextObject)`:

| Needed | C++ counterpart | Notes |
|---|---|---|
| `font: Arc<Font>` | `text_state().GetFont()` | **Must be shared per resolved resource** — dedup compares font identity (§1.7a). |
| `font_size: f32` | `GetFontSize()` | May be negative; never rescued. |
| `font_size_h: f32` | `text_state().GetFontSizeH()` | `|hypot(a,b)| × font_size` of the composed matrix; §1.7. **Not currently in the page brief's `GraphicsState`** — must be exposed or recomputable. |
| `char_space: f32`, `word_space: f32` | `GetCharSpace`/`GetWordSpace` | §1.7 base-space math. |
| `codes: &[CharCode]` | `GetCharCodes()` | `CountItems()` = `codes.len()`; `CharCount()` is the same value. |
| `kernings: &[f32]` | `GetCharKernings()` | Same length as `codes`; last is always 0. |
| `positions: &[f32]` | `GetCharPositions()` | Same length; first is always 0. |
| `fn item(&self, i) -> Item { code, origin }` | `GetItemInfo` | Includes the **vertical CID origin offset** (cpdf_textobject.cpp:58-79). |
| `fn char_width(&self, code) -> f32` | `CPDF_TextObject::GetCharWidth` | Already scaled by `font_size/1000`; vertical CID uses `GetVertWidth`. |
| `pos: Point` | `GetPos()` | The object's text-space origin in user space. |
| `text_matrix: Affine` | `GetTextMatrix()` | `Matrix(m[0], m[2], m[1], m[3], pos.x, pos.y)` — the transposed storage unpacked. |
| `rect: Rect` | `GetRect()` | The object's bounding rect, **already stroke-inflated** where applicable. |
| `original_rect: Rect` | `GetOriginalRect()` | Not used by this crate; listed so nobody confuses the two. |
| `marks: &[ContentMark]` | `GetContentMarks()` | With `param: Option<Arc<Dict>>` — §1.9 needs **`Arc` identity** for the `kDone` test, and the `raw`-vs-resolved distinction for `/ActualText` (§1.9). |
| `is_active: bool` | `IsActive()` | §1.1, §1.4. |

Plus, on `Page`: `media_box`/`crop_box`/`rotate` sufficient to compute
`page_size` and `display_matrix` per §1.4a, or the `display_matrix` itself.

**Two gaps against the page brief as written**, both to be raised with that
crate's owner: `font_size_h` is not in the `GraphicsState` sketch, and the
content-mark param must be an `Arc<Dict>` (identity-comparable) plus carry
whether the `/ActualText` entry was direct or indirect (§1.9's `GetStringFor`
vs `GetUnicodeTextFor` asymmetry). Recorded in Q6.

### 4.3 To `pdfrum-tool`

`--txt` must emit, per page, to `<pdf>.<page>.txt`:

```rust
out.write_all(&0x0000_FEFFu32.to_le_bytes())?;
for c in &text_page.chars { out.write_all(&c.unicode.to_le_bytes())?; }
```

No filtering, no separator, one BOM per file. `ExtractOptions::rtl` set from
`/Root /ViewerPreferences /Direction == /R2L` (via `pdfrum-doc`), matching
`FPDFText_LoadPage`.

---

## 5. Test plan

### 5.1 Ported from `cpdf_linkextract_unittest.cpp` — the only C++ unittest here

`core/fpdftext/BUILD.gn` registers exactly one unittest source set:
`cpdf_linkextract_unittest.cpp`. **There is no `cpdf_textpage_unittest.cpp`** —
the text-page heuristics have no unit-level coverage upstream at all; their
only assertions live in `fpdfsdk/fpdf_text_embeddertest.cpp` (§5.2) and in the
corpus. That absence is itself a finding: it is why this brief transcribes so
literally.

`links.rs` tests, ported one-for-one (`CPDFLinkExtractTest.CheckMailLink`,
`CheckWebLink`):

- **Mail, rejected:** `""`, `"peter.pan"`, `"abc@server"`, `"abc.@gmail.com"`,
  `"abc@xyz&q.org"`, `"abc@.xyz.org"`, `"fan@g..com"`.
- **Mail, accepted** (all prefixed `mailto:`): `peter@abc.d`,
  `red.teddy.b@abc.com`, `abc_@gmail.com`, `dummy-hi@gmail.com`,
  `a..df@gmail.com → df@gmail.com`, `.john@yahoo.com → john@yahoo.com`,
  `abc@xyz.org?/ → abc@xyz.org`, `fan{abc@xyz.org → abc@xyz.org`,
  `fan@g.com.. → fan@g.com`, `CAP.cap@Gmail.Com` (case preserved).
- **Web, rejected:** `""`, `"http"`, `"www."`, `"https-and-www"`,
  `"http:/abc.com"`, `"http://((()),"`, `"ftp://example.com"`,
  `"http:example.com"`, `"http//[example.com"`,
  `"http//[00:00:00:00:00:00"`, `"http//[]"`, `"abc.example.com"`.
- **Web, accepted** — all 38 `kValidCases` rows, each asserting the extracted
  URL **and** `start_offset` **and** `count`. The load-bearing ones:
  `http////www.server → http://www.server` at offset 8 count 10;
  `test:www.abc.com → http://www.abc.com` at 5/11;
  `0(http://www.abc.com)0 → http://www.abc.com` at 2/18;
  `http://www.abc.com)0` **not** trimmed (unopened bracket), 0/20;
  `[http://www.abc.com/z(1)] → http://www.abc.com/z(1)` at 1/23;
  `www.g.com.. → http://www.g.com..` at 0/11 (periods kept — the trim in
  `ExtractLinks` is upstream of `CheckWebLink`, so the unit test sees them);
  `http://[aa]:12abc → http://[aa]:12` at 0/14;
  `http://[aa]: → http://[aa]` at 0/11;
  `www.测试。net。 → http://www.测试。net。` at 0/11 (non-ASCII not trimmed);
  `http://a.com/1/2/3/4\5\6` kept whole (path ⇒ no sanitizing).

### 5.2 Ported from `fpdf_text_embeddertest.cpp` — the heuristic assertions

61 tests, restated over our types (`TextPage::chars`, `TextPage::text`,
`TextPage::find`, `TextPage::web_links`). With no `cpdf_textpage_unittest.cpp`
upstream (§5.1), **these are the only assertions that pin the text-page
heuristics**, so every one of them ports. Exact values are reproduced here so
the porting agent does not have to re-derive them.

Two shared fixtures dominate: `hello_world.pdf` yields
`"Hello, world!\r\nGoodbye, world!"` (30 chars; indices 13,14 are the
generated CRLF; chars 0-12 are Times-Roman at size 12, chars 15-29 Helvetica
at size 16), and `rotated_text.pdf` / `rotated_text_90.pdf` place the same
text at four rotations with quadrant first-char indices **0, 7, 16, 25** and
**0, 9, 18, 27** respectively.

#### The character-model tests — highest value, port first

| Test | Fixture | Assertion |
|---|---|---|
| `IsGenerated` | `hello_world.pdf` | `chars[0] = 'H'`, not generated; `chars[6] = ' '`, **not** generated (a real space in the stream); `chars[13] = '\r'` and `chars[14] = '\n'`, **both generated** |
| `IsHyphen` | `bug_781804.pdf` | `chars[0]='V'` not hyphen; `chars[6].unicode == 0x0002`, `CharType::Hyphen`; `chars[14]='U'` not hyphen; `chars[18] == U+2010`, **not** `CharType::Hyphen` |
| `GetTextWithHyphen` | `bug_781804.pdf` | `text[0..12] == "Verita\u{FFFE}serum"` — **the Fact-1 test**, paired with `IsHyphen`'s `0x0002`. Then `text[14..30] == "User\u{2010}\r\ngenerated"`: a **hard** hyphen is preserved *and* followed by a generated CRLF |
| `IsInvalidUnicode` | `bug_1388_2.pdf` | 5 chars; `chars[0]='X'` normal; `chars[1]=' '` normal; `chars[2].unicode == 31` with `CharType::NotUnicode` |
| `ControlCharacters` | `control_characters.pdf` | `text` == the 30-char hello/goodbye string with **no** `0x02 0x03`; but `page_text(17, …) == "Goodbye, world!"` — **the char index space still counts the 2 stripped control chars** (15 + 2 = 17) |
| `Bug1139` | `bug_1139.pdf` | Leading control char: `chars.len() == 31` (30 + 1) while `text` is the plain 30-char string |
| `ToUnicode` | `bug_583.pdf` | 1 char, `unicode == 0` — an unmappable char is still counted (§1.7 (f)) |
| `WhitespaceCharCount` | `whitespace.pdf` | `chars.len() == 0` for a whitespace-only page. Pinned upstream **bug** (`crbug.com/40643656` wants 1); verified: the golden is a 4-byte BOM-only file |
| `Bug425244539` | `bug_425244539.pdf` | `text == "hello"` (5) while `chars.len() == 27` (22 charcode-0 NULs + `hello`) — verified against the oracle. Also: `find("hello")` reports result index **22**, not 0, **before** the first step, and count 5 after |
| `Bug431824298` | `bug_431824298.pdf` | `text` = `- h e l l o - \r \n - w o r l d 0xFFFE 0x501F 0x6B3E` (18 + NUL); `chars[15].unicode == 0x02`. `find("-world-")` finds **nothing** — *corrected 2026-09-02 (A42)*: this used to be annotated "a pinned upstream bug, `crbug.com/431824298`", which is no longer why. The bug is fixed; the zero here is now correct, because `find` drops the sentinel and the joined text holds no literal hyphen for this needle's `-` to match. `find("world\u{501F}")` finds **one** — the word the break split, joined |
| `Bug1029` | `bug_1029.pdf` | `0x0002` replaces a **hard** hyphen when the word has non-ASCII chars. `text[171..227]` == `"METADATA table. When the split has committed, it noti\u{2}fi"` |
| `SmallType3Glyph` | `bug_1591.pdf` | 5 chars `'1' ' ' '2' ' ' '1'` — **two generated spaces**. Exact boxes: `chars[1]` and `chars[3]` are **zero-area** (`left==right`, `top==bottom==50`); `chars[2]` (the small Type3 glyph) is `{86.0, 50.240001678466797, 88.400001525878906, 50.0}` |
| `Bug444176962` | `bug_444176962.pdf` | `text == "localact"` — a space that **should** be generated is not (pinned bug `crbug.com/444176962`) |
| `Bug1769` | `bug_1769.pdf` | `text == "wo d wo d"` — chars dropped by the dedup/overlap logic (pinned bug `crbug.com/42270780`) |
| `TextObjectSetIsActive` | `hello_world.pdf` | Deactivating object 0 and re-extracting yields 16 chars `"Goodbye, world!"` — **the generated CRLF disappears with it** |
| `StreamLengthPastEndOfFile` | `bug_57.pdf` | 13 chars despite a `/Length` past EOF (damage tolerance) |
| `Bug921` | `bug_921.pdf` | 268 chars; `chars[238..262]` == the Cyrillic run `1095,1077,1083,1086,1074,1077,1095,1077,1089,1082,1086,1077,32,1089,1090,1088,1072,1076,1072,1085,1080,1077,46,32` |
| `Bug642` | `bug_642.pdf` | 4 chars, `"ABCD"` |
| `Bug384770169` | `bug_384770169.pdf` | `"What is my favorite food?"` |
| `Bug420508260` | `bug_420508260.pdf` | `"What is 我的 favorite 食物?"` — pins space generation around CJK↔Latin transitions |
| `Bug491161396` / `Bug491516663` | resp. fixtures | Both `"Hello, world!"` |
| `Bug782596` | `bug_782596.pdf` | No assertions — an **ASAN-only** regression guard. Our equivalent is the fuzz target (§5.5). Verified `--txt` output is a single `-` |

#### Reading order, bidi, and `/ActualText`

| Test | Fixture | Assertion |
|---|---|---|
| `TextHebrewMirrored` | `hebrew_mirrored.pdf` | 10 chars, **logical** order: `05D1 05E0 05D9 05DE 05D9 05DF` (בנימין), `000D 000A`, `05DF 05D1`. Pins §1.10b's `GetMirrorChar` + RTL normalization |
| `ActualTextRtl` | `actual_text_rtl.pdf` | The single richest assertion in the file: ~110 code units covering pure-RTL, predominantly-RTL, tie-breaker, pure-LTR-in-`/ActualText`, RTL→LTR, LTR→RTL, RTL/LTR/RTL and LTR/RTL/LTR `/ActualText` cases, each emitted in **logical** order. Line 3 emits `05DD ']' '('` — source `[`/`)` **mirrored**. The **final** case (literal RTL followed by RTL `/ActualText`) comes out **visually reversed** (`05DD 05D5 05DC 05E9` instead of `05E9 05DC 05D5 05DD`) — a pinned upstream bug, `crbug.com/525087036`. Port the whole array verbatim; it is the acceptance test for §1.9 + §1.10 together |
| `BigtableTextExtraction` | `bigtable_mini.pdf` | 65 chars: `"{fay,jeff,sanjay,wilsonh,kerr,m3b,tushar,\x02k es,gruber}@google.com"` — a `0x02` hyphen sentinel **mid-string** followed by a generated space. Verified against the oracle (264-byte golden = 65 chars) |
| `BigtableTextRects` | `bigtable_mini.pdf` | `rects(0, 65).len() == 12` with 12 exact rects. Upstream `TODO(crbug.com/40448046)` says it *should* be 4 — port the 12 |
| `CroppedText` | `cropped_text.pdf` | 4 pages; **full** extraction is the same 30-char string on all four (cropping does not affect `chars`), but `text_in_rect` differs per page: `" world!\r\ndbye, world!"` (pages 0,1) and `"bye, world!"` (pages 2,3). Note the generated `\r\n` **is** included in the bounded output |
| `GetTextShouldNotGetInvisibleSpaces` | `hello_world_with_invisible_spaces.pdf` | 5 text objects; the first three yield the **empty** string from `text_of_object`, not spaces. (Verified: `--txt` for this file is the plain 30-char string) |

#### Search — §1.14

All on `hello_world.pdf` unless noted. Ranges are `text` offsets.

| Test | Needle / flags | Result |
|---|---|---|
| `TextSearch` | `"nope"` | no match |
| | `"world"` | `7..12`, then `24..29`; reverse gives `7..12` then nothing |
| | `"world"` + match-case + whole-word | `7..12` |
| | `"WORLD"`, no flags | `7..12` (**default is case-insensitive**) |
| | `"WORLD"` + match-case | no match |
| | `"orld"`, no flags | `8..12` (substring allowed) |
| | `"orld"` + whole-word | no match |
| `TextSearchConsecutive` | `"aaaa"` in `"aaaaaaaaaa"`, no flags | **2** matches: `0..4`, `4..8` |
| | same + `consecutive` | **7** matches: `0..4`, `1..5`, … `6..10` |
| `TextSearchTermAtEnd` | `"world!"` | `7..13`, `24..30` |
| `TextSearchLeadingSpace` | `" Good"` | `14..19` — **the leading space matched the `\n` at index 14**, so the match starts at the `\n`, not at `'G'` (15) |
| `TextSearchTrailingSpace` | `"ld! "` | `10..14` — the trailing space matched the `\r` at 13; count is 4 |
| `TextSearchSpaceInSearchTerm` | `"ld! G"` (5 chars) | `10..16` — **count 6**: a single space in the needle consumed the two-char `\r\n` run (§1.14's space tolerance) |
| `TextSearchLatinExtended` | `latin_extended.pdf`, `"Ă"` (U+0102) and `"ă"` (U+0103) | Both give `2..3` then `3..4` — case-insensitive matching across the Latin Extended-A pair. **`DISABLED_` on Windows** (`crbug.com/42270374`); we have no platform variance, so we run it |
| `Bug425244539` | `"hello"` | result index **22** *before* the first step (not 0), count 0; after stepping, index 22 count 5 |
| `Bug431824298` | `"-world-"` | no match — *corrected 2026-09-02 (A42)*: not a pinned bug any more. The needle's literal hyphens are absent from the joined haystack. The bug this row used to pin is now covered by `find::tests::a_word_split_across_a_line_break_is_found_joined` and by `"world\u{501F}"` on this fixture, which matches |

#### Web links — §1.15

| Test | Fixture | Assertion |
|---|---|---|
| `WebLinks` | `weblinks.pdf` | 2 links; link 0 URL `"http://example.com?q=foo"` (24 chars); `rects(0).len() == 1`, rect `{50.828, 108.700, 187.904, 97.516}` |
| `WebLinksCharRanges` | `weblinks.pdf` | link 0 range = start **35**, count **24** — the D5 index-space test |
| `WebLinksAcrossLines` | `weblinks_across_lines.pdf` | 6 links, and the rules they encode: `www.` is stripped; a trailing `?` or `/` before a break **terminates** the URL (`"http://www.example.com?\r\nfoo"` → `http://example.com`); a trailing `-` before **one** `\r\n` **joins** (`test-\r\nfoo` → `test-foo`); a trailing `-` before **two** `\r\n` still joins; a trailing `/` is dropped on the last (`http://www.abc.com`) |
| `WebLinksAcrossLinesBug` | `bug_650.pdf` | 2 links; link 1 == `"http://tutorial45.com/learn-autocad-basics-day-166/"` (51 chars) |

#### Geometry — `CharBox` fields (§1.7 (e), §1.12)

| Test | Fixture | Assertion |
|---|---|---|
| `Text` | `hello_world.pdf` | `chars[4].char_box == {41.12, 55.652, 46.208, 49.892}` (±0.001); `loose_char_box == {40.664001, 60.692001, 46.664001, 47.419998}` (exact f32); `origin == (40.664, 50.000)`; `rects(0,30).len() == 2`, rect 1 `{20.800, 111.600, 135.040, 96.688}` |
| `TextVertical` | `vertical_text.pdf` | Origins move **down** and slightly right: `chars[1].origin == (6.664, 171.508)`, `chars[2].origin == (8.668, 160.492)`. Loose boxes are **vertically contiguous** (both share y=170.308) and horizontally fixed at left=4, right=16 — the §1.12 vertical-CID branch |
| `CharBox` | `font_matrix.pdf` | Chars 0, 4, 8 are all `'A'` with **identical** tight (8.460 × 6.600) and loose (8.664 × 11.060005) dims **despite three different font matrices** |
| `CharBoxForRotated45DegreesText` | `rotated_text.pdf` | Tight boxes are square (45°): 11.192, 10.055, 11.209, 10.055 per quadrant; loose is **15.511 × 15.511 in all four** |
| `CharBoxForRotated90DegreesText` | `rotated_text_90.pdf` | Tight (w×h) per quadrant: 7.968×7.86, 5.616×8.604, 7.8×8.052, 5.616×8.604. Loose: quadrants 0,2 are 8.664×13.272; quadrants **1,3 are transposed** to 13.272×8.664 |
| `CharBoxForLatinExtendedText` | `latin_extended.pdf` | `Ā` (U+0100) tight 7.512×10.488 top 750.238; `Ă` (U+0102) tight 7.512×**10.74** top **750.49** — the tight box tracks the accent, while **both** loose boxes are 7.824×14.052 top 750.874 (font-uniform) |
| `Bug402562387` | `bug_402562387.pdf` | Invariant for all 4 chars: the loose box **contains** the tight box on every side. Assert this as a property over the whole corpus, not just this file |
| `Bug399689604` | `bug_399689604.pdf` | `chars[5]` is generated and its tight **and** loose boxes are both exactly 0×0 at top 100.0 |
| `Bug488948351` | `bug_488948351.pdf` | 2 chars, both tight 4.944×7.668 top 107.584, loose 6.948×14.064 top 111.4 |
| `Bug502757960` | `text_large_ascent_descent.pdf` | Large ascent/descent clamping (§1.12's `font_bbox` min/max): char 0 tight 5.172×8.256 loose 6.672×14.676; char 1 tight 8.184×7.860 loose 8.664×14.376 |
| `GetCharAngle` | `rotated_text.pdf` | 31 chars; angles at the quadrant indices are exactly **π/4, 3π/4, 5π/4, 7π/4** — `atan2(matrix.c, matrix.a)` normalized to `[0, 2π)` |
| `GetMatrix` | `font_matrix.pdf` | Text `"A1\r\nA2\r\nA3"`; per-char matrices `{12,0,0,10,66,90}` (0,1), **`{1,0,0,1,0,0}` identity for the generated `\r\n` at 2,3 and 6,7**, `{12,0,0,10,38,60}` (4,5), `{1,0,0,0.833333,60,130}` (8,9). Confirms §1.2's generated-char matrix rule |
| `GetFontSize` | `hello_world.pdf` | Sizes `[12×13, **1, 1**, 16×15]` — **the generated CRLF chars have font size 1**, i.e. `kDefaultFontSize` via `GetFontSize(nullptr)` (§1.12) |
| `GetFontInfo` | `hello_world.pdf` | Chars 0-12 `"Times-Roman"`, 15-29 `"Helvetica"`, both non-symbolic; chars **13, 14 return nothing** — generated chars carry no `object` (§1.8b) |
| `CountRects` | `hello_world.pdf` | Full table over `start ∈ [0,13)`, `[16,30)`, `[30,100)` × `count ∈ {-1,0,1,2,15,500}` — pins `GetRectArray`'s clamping and the object-change split (§1.16) |
| `GetTextRenderMode`, `GetFillColor`, `GetStrokeColor`, `GetFontWeight` | `text_render_mode.pdf`, `text_color.pdf`, `font_weight.pdf` | `CharBox → object` plumbing. Fill `(0xff,0,0,0xff)`, stroke `(0,0xff,0,0xff)`; font weight from `/StemV × 5` when `/FontWeight` is absent |

#### API-shape assertions we deliberately do **not** port

Roughly a third of the file asserts C-ABI buffer semantics: "returns the
required size when the buffer is too small **without modifying it**", "never
writes past the stated length", "failure paths leave out-parameters
unchanged" (except `FPDFText_GetRect`, which zeroes them), and the
`nullptr`/`-1`/`out-of-range` guard matrix on every accessor. These are
`fpdfsdk` marshalling contracts that Rust's `&str`/`Vec`/`Option` return
types make vacuous (STYLE §7: port behavior, not shape). They are listed here
so a reviewer can confirm the omission is deliberate, not an oversight. The
one piece of *behavior* hiding among them is `FPDFText_GetText(page, 0, 0,
buf)` returning 1 with `buf[0] == 0` — a zero-length request yields the empty
string, which `page_text(start, 0) -> ""` reproduces.

Where a test asserts an exact float, port the `EXPECT_NEAR` tolerance with it
(three decimal places for the `_Three_Places` comparators, exact for
`EXPECT_FLOAT_EQ` / `CompareFS_RECT_DOUBLE`); where it asserts an exact
string, port the string including escapes.

### 5.3 Ours to write — heuristics with no upstream test

The following have **no** upstream assertion and are pinned by hand-built
fixtures plus corpus conformance. Each is a numbered constant or branch from
§1 that a refactor could silently break:

1. **`NormalizeThreshold` bucket edges** — a table test over both call sites'
   bucket sets, at and around each boundary (299/300/301, 499/500/501, …).
2. **The two magic float bands** (§1.8, `1.4880` / `1.3900`) — construct a
   `threshold2` landing inside and just outside each band; assert the `×1.5`.
3. **`CalculateBaseSpace` + `CalculateBaseSpaceAdjustment` interaction** —
   char-space in `{0, ±0.0005, ±0.002, ±1}` × kernings in `{none, one
   non-zero, all non-zero}` × `nItems` in `{1, 2, 3}`, asserting the
   `nItems == 2 && has_kerning ⇒ 0` and `base_space < 0 ⇒ 0` short-circuits.
4. **`GenerateSpace`'s three clauses** — including the `pos.x < 0` clause,
   which no fixture exercises.
5. **The dedup epsilon** (`0.07 × font_size` through `TransformXDistance`) —
   two identical charcodes at exactly the threshold, just inside, just
   outside, with the same and with different fonts, and at lookback distances
   6, 7, 8.
6. **`IsSameAsPreTextObject`'s non-text-object skip** — a text object
   preceded by 20 image objects and then 5 text objects; assert the 5th text
   object back is examined.
7. **`IsSameTextObject`'s in-place intersection** feeding `max_pre_size` —
   two overlapping objects where the intersection's height differs from the
   object's, at the `/8` boundary.
8. **`EndHorizontalLine` at exactly 4.5** and `EndVerticalLine` at exactly
   `0.1 × font_size`.
9. **`GetTextObjectWritingMode`'s 0.0872 cone** — vectors at 4°, 5°, 6°, 85°,
   86°, and exactly on the boundary, with `textline_dir_` set to each of the
   three values.
10. **`FindTextlineFlowOrientation`'s `fLineHeight` from the first object
    only** — three objects with heights 10, 100, 100; assert the threshold
    uses 10.
11. **The zero-size-page display matrix** (§1.4a) — assert no batch splits.
12. **The form-only page** (§1.4) — text entirely inside a form XObject;
    assert `Orientation::Unknown`.
13. **`ProcessGenerateCharacter`'s `text_buf_` emptiness guard** — a line
    break as the very first decision on a page; assert no `\r\n`.
14. **The hyphen-cancel path** (`return false`) — a single-char object whose
    char is itself a hyphen; assert the object emits nothing and
    `prev_text_obj_` is unchanged.
15. **`GenerateCharInfo`'s `None` at page start** — assert a leading space
    decision emits nothing.
16. **`PreMarkedContent`'s direct-vs-indirect `/ActualText`** — the same file
    with `/ActualText (x)` and `/ActualText 5 0 R`; assert the first
    suppresses glyphs and the second does not.
17. **`ProcessMarkedContent`'s last-mark-wins reset** — two marks, the second
    with a param but no `/ActualText`; assert nothing is emitted.
18. **`/ActualText` chars `>= 0xFFFD`** — assert they are skipped but still
    advance the box step.
19. **The charcode-0 path** (§1.7 (f)) — assert `chars` gets `unicode == 0`
    and `text` gets nothing.
20. **Multi-char `/ToUnicode` expansion** — one code mapping to `"ffi"`;
    assert three `CharBox`es at identical origins, `CharType::Normal`.
21. **Ligature normalization** (`U+FB01` LTR) — assert `CharType::Piece` and
    two output characters.
22. **`CloseTempLine`'s space collapsing across a line boundary** — assert
    spaces split by a `CloseTempLine` are *not* collapsed.
23. **Bidi segment reversal with a leading neutral** — assert the zero-count
    leading segment is a no-op.
24. **`Substr` out-of-range in link extraction** (D5) — a page where the
    char-index/text-index mismatch pushes the slice past the end; assert an
    empty candidate rather than a panic.
25. **`FindNext`'s `iWord = -1` restart termination** — a needle whose first
    element is empty and whose second never matches; assert termination.
26. **`IsMatchWholeWord`'s exclusive `'A' < c < 'a'` band** — assert `'Z'`,
    `'['`, `'_'`, `'`'` behave as the C++ does.
27. **The unicode tables round-trip** — for every BMP code point, assert our
    `bidi_class`/`mirror`/`normalize`/`is_alpha`/`is_alnum`/`to_lower` agree
    with the checked-in transcription of the oracle's tables (a generated
    exhaustive test, run once in CI, not per-commit).

### 5.4 Snapshot tests (`insta`)

- `TextPage` debug dump (char index, type, unicode as `U+XXXX`, box, origin)
  for one representative file per cluster: simple Latin, CJK vertical, Hebrew
  RTL, `/ActualText`, hyphenated, rotated 45°, multi-column, form-nested,
  Type3.
- The `CharIndex` segment table for a page with control characters, where the
  segments are non-trivial.
- The find-what split (`ExtractFindWhat`) for a dozen needles spanning the
  `IsIgnoreSpaceCharacter` script ranges.

### 5.5 Fuzz targets

This crate consumes no untrusted bytes directly — it consumes a `Page`, which
is already the product of fuzzed parsing. Two targets nonetheless:

- `fuzz_text_extract`: bytes → `pdfrum_parser::load` → `build_page` →
  `extract`. Catches panics from the arithmetic in §1.7/§1.8 on degenerate
  geometry (NaN/infinite matrices, zero font sizes, empty rects). Seeded from
  `testing/fuzzers/` corpora plus the corpus's text-heavy files.
- `fuzz_text_find`: a `TextPage` built from a fixed page plus a fuzzed needle,
  exercising §1.14's restart loop and `IsMatchWholeWord`'s index arithmetic.

`fuzz_link_extract` is folded into `fuzz_text_extract` (links are derived from
the same `TextPage`), plus a direct `check_web_link`/`check_mail_link` target
over fuzzed `&str` since those are pure string functions with the trickiest
index math in the crate.

### 5.6 Conformance clusters

Tier A, `--txt`, per PLAN §5. Triage tags this crate owns:

`text-basic`, `text-cjk`, `text-vertical`, `text-rtl`, `text-actualtext`,
`text-hyphen`, `text-control-chars`, `text-rotated`, `text-multicolumn`,
`text-form-nested`, `text-type3`, `text-tounicode`, `text-dedup`,
`text-spacing`.

M2's exit criterion is `--txt` Tier-A byte-exact on **≥ 98%** of the corpus.

The golden store holds **5783** per-page text dumps, of which **2808 (49%)
are BOM-only** — pages with no text at all. That is a comfort and a trap: half
the Tier-A text score is available from `extract` returning an empty
`TextPage` for a page with no active text objects, which means a naive
implementation can post ~49% and look like it is halfway there while every
heuristic is still wrong. **The scoreboard must report text Tier-A over
non-empty goldens separately**, or the burn-down loop will optimize the wrong
number. Recommended: add a `text-nonempty` aggregate to `--triage` alongside
the raw percentage.

The rare-value pages are the canaries: 16 goldens contain `U+0002`
(hyphen sentinel) and 3 contain `U+0000` (charcode-0 passthrough). Those 19
files exercise §1.8b and §1.7 (f) and should be in the M2 smoke set, not left
to the full-corpus run.

Expected waiver sources, in descending likelihood: (a) font substitution
differences changing `GetCharWidth` and therefore space thresholds (a
`pdfrum-font` problem surfacing here — see Q6), (b) the ICU
property/case-mapping table version (D1/Q1), (c) files where the oracle
crashes (D4).

---

## 6. Open questions

**Q1 — Unicode tables: build-script generation, escalation (D1).** This brief
proposes generating five table groups at build time from data transcribed from
the *oracle's own* `fx_ucddata.inc` and bundled ICU, adding ~60 KiB to the
binary and one `build.rs`. The alternative — a new dependency for general
category and case mapping (`icu_properties` / `unicode-case-mapping`) — is
both a DEPS.md change (SPEC §0) and a **fidelity risk**: any crate tracking a
newer Unicode revision than the oracle's ICU will disagree on some code
points, and each disagreement is a Tier-A failure. Requested ruling: accept
the build-script approach (the `pdfrum-cmap` precedent), or name a dependency
and accept the version-skew risk. **This is the one question that blocks
implementation.**

**Q2 — `font_size_h` and the content-mark param shape (interface gap).**
§4.2 needs two things the page brief does not currently promise:
`text_state().GetFontSizeH()` (`|hypot(a,b)| × font_size` of the composed
matrix) exposed on `TextObject`, and content-mark params carried as
identity-comparable `Arc<Dict>` **plus** a flag for whether `/ActualText` was
a direct String (§1.9's `GetStringFor`-vs-`GetUnicodeTextFor` asymmetry). Both
are small additions to `pdfrum-page`'s records. Requested: confirm with that
crate's owner, or accept that this crate recomputes `font_size_h` from the
matrix and re-reads the mark dict through `Resolve` (which it can, given
`raw()` from SPEC §2 — the non-resolving accessor exists precisely for this).
Leaning: **recompute locally**, since SPEC §2's `Dict::raw` + `Dict::text`
give us both halves of the asymmetry without touching the page crate.

**Q3 — Non-scalar `unicode` values and the harness (SPEC tension).**
SPEC §9 gives `TextPage { chars: Vec<CharBox> /* unicode, ... */ }`, implying
a `char`. But §1.7 (d)'s raw-charcode passthrough can produce any `u32`,
including surrogates and values `> 0x10FFFF` (a 4-byte CID charcode). The
oracle writes them raw; our harness's `utf32le_to_utf8` (`transcode.rs:33`)
**errors** on a non-scalar, which would fail *golden generation*, not our
comparison. The golden store proves the tension is not hypothetical but is
currently benign: **3 goldens hold `U+0000`** (which `char::from_u32`
accepts, so they transcoded fine) and none holds a surrogate or an
out-of-range value — the store generated cleanly over all 5783 pages. A
future corpus file with a 4-byte CID charcode passthrough would break golden
generation on that page. This brief types
`CharBox::unicode` as `u32` rather than `char` — a deliberate deviation from
the SPEC sketch's implication, and the only way to be byte-exact. Requested
ruling: confirm `u32`, and confirm whether the harness should transcode
non-scalars lossily (to `U+FFFD`) or keep erroring. Leaning: **`u32`, and the
harness keeps erroring** — a corpus file that trips it is a finding, not noise.

**Q4 — Per-character diagnostics volume.** §1.17 records `TextCharcodeUnmapped`
and `TextCharDeduplicated` per character. On a CJK page with a broken
`/ToUnicode` that is tens of thousands of entries, hitting the 4096 bound
immediately and hiding every *other* diagnostic on the page. Options: (a) keep
as-is and rely on `Diagnostics`' bound + `dropped()` count; (b) record these
two kinds once per text object with a count; (c) demote them out of
`Diagnostics` entirely into a `TextPage` statistics field. Proposal: **(b)** —
one diagnostic per object carrying the count, which preserves "never silently
swallow" without flooding. Confirm.

**Q5 — The hyphen crash (D4).** The C++'s `ProcessGenerateCharacter` `kHyphen`
arm can dereference an empty `temp_char_list_`. We decline to port the crash.
If the *release-mode* oracle happens not to crash but to produce garbage on
such a file, our output diverges and we would need to reproduce the garbage.
Action for M2 triage: if any corpus file produces a golden whose text is
inexplicable near a hyphen, check this path first. Flagged, not blocking.

**Q6 — Reading-order divergence risk from font substitution.** Every space
and line-break threshold in §1.7/§1.8 is a function of `GetCharWidth`, which
for a non-embedded font comes from `/Widths` (deterministic) but falls back
through §1.7c to `GetCharBBox` (substituted-face-dependent). A different
substitute face than the oracle's hermetic `test_fonts` set changes bbox
widths, which changes thresholds, which changes generated spaces — a Tier-A
failure with no local cause. `pdfrum-tool` must reproduce the oracle's
`--font-dir=third_party/test_fonts --croscore-font-names` invocation exactly
(PLAN §4). Recorded here because when M2 stalls at 95%, this is the first
place to look, and the fix is in `pdfrum-font`, not this crate.
