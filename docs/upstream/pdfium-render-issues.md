# Upstream issue drafts — PDFium rendering and text layout (crbug.com/pdfium)

Six issues, each in the Chromium issue tracker's template and ready to paste.
Everything from the first `## Issue` heading down is issue text; nothing above
the rule is meant to be posted.

**Status:** drafted 2026-09-02, not yet filed.

**How these were produced.** Every cited line was read at commit
`6f2272e1f3aa` (2026-08-28) and is quoted verbatim below. These six were found
by differential audit: a from-scratch reimplementation of the same
specification, run against the same corpus, with each divergence traced to
which side is wrong. Unlike the JavaScript issues filed earlier, several of
these are **arithmetic defects with no cheap black-box repro** — the observable
effect is a small pixel difference on a gradient, or a line-break position — so
each issue states the defect at the line and gives the smallest concrete
trigger known, rather than pretending to a one-command reproduction.

**Versions to quote.** Source read at `6f2272e1f3aa`. Where a repro is given it
was run on Ubuntu 22.04.5 LTS, x86-64.

**Independent implementation used as the tiebreaker.** Mozilla's pdf.js, cited
where its answer differs.

**Ordering.** Issues 1 and 2 (`/TR` transfer functions) are the highest
confidence and the cheapest to fix. Issue 6 (supplementary-character mirroring)
is a one-line fix with a clear repro. Issues 3–5 are arithmetic and layout
defects where the fix needs a decision, not just a correction.

---

## Issue 1 — `/TR` transfer function array is loaded in reverse: `array[2]` drives red, and the fourth function is never read

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render a page whose `/ExtGState` sets `/TR [fR fG fB]` with three
   **different** functions — e.g. `fR` mapping everything to 1.0, `fG` and
   `fB` to 0.0.
3. Compare the rendered colour against the specification's mapping.

**What is the expected output? What do you see instead?**

Expected: `array[0]` is applied to red, `array[1]` to green, `array[2]` to
blue, per ISO 32000-1 table 58. Observed: the channels are transposed —
`array[2]` drives red and `array[0]` drives blue, so the example above renders
blue where it should render red. The defect is invisible whenever the three
functions are identical, which is the common case and why it has survived.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

`core/fpdfapi/render/cpdf_docrenderdata.cpp:90`:
```cpp
for (uint32_t i = 0; i < 3; ++i) {
  pFuncs[2 - i] = CPDF_Function::Load(pArray->GetDirectObjectAt(i));
```
The `2 - i` reverses the array on load. The consumption side was traced end to
end and there is no second reversal that would undo it: `:113-114` builds
`samples` as `{samples_r, samples_g, samples_b}` in that order, and
`samples[0]` is the red channel throughout. So `/TR [fR fG fB]` is applied as
`[fB fG fR]`.

Separately, table 58 specifies **four** functions for `/TR` (the fourth being
the gray/`k` component). PDFium requires `size() >= 3` and never reads
`array[3]`.

---

## Issue 2 — a single-function `/TR` with more than 16 outputs renders all black from an unwritten variable

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render a page whose `/ExtGState` sets `/TR` to a **single** function (not an
   array) whose `OutputCount()` exceeds 16 — e.g. a sampled function with many
   output components.
3. Observe the rendered result.

**What is the expected output? What do you see instead?**

Expected: the identity transfer, which is what the array branch produces for
the identical condition. Observed: an all-black transfer curve, because the
sample value comes from a variable that was never written for this input.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

`core/fpdfapi/render/cpdf_docrenderdata.cpp:102` declares
`float output[kMaxOutputs];` (`kMaxOutputs` is 16, `:32`) and zero-fills it
once before the loop. The **array** branch handles the overflow case correctly
at `:119-122`:
```cpp
if (pFuncs[i]->OutputCount() > kMaxOutputs) {
  samples[i][v] = v;          // identity — correct
  continue;
}
```
The **single-function** branch immediately below, at `:132-137`, guards the
call but not the read:
```cpp
if (pFuncs[0]->OutputCount() <= kMaxOutputs) {
  pFuncs[0]->Call(pdfium::span_from_ref(input), output);
}
size_t o = FXSYS_roundf(output[0] * 255);   // read regardless
```
`OutputCount()` is loop-invariant, so when it exceeds 16 the `Call` never
happens for any `v`, `output[0]` keeps its initial `0.0f` for the whole loop,
and every sample stores 0. That the array branch gets the same condition right
two lines above suggests an oversight rather than a policy. The fix is to
mirror the array branch's `samples[0][v] = v`.

---

## Issue 3 — `/TR` transfer samples are stored unclamped, so a negative output wraps to the top of the byte range

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render a page whose `/ExtGState` sets `/TR` to a function whose `/Range`
   admits negative outputs — a type 4 (PostScript calculator) function is the
   easy case.
3. Inspect the resulting transfer curve, or the rendered page.

**What is the expected output? What do you see instead?**

Expected: outputs clipped to `/Range` per ISO 32000-1 §7.10.1, so a negative
output becomes 0 (black). Observed: the value wraps. A measured example from a
type 4 function: the function returns `-121.26` at input `0xCC`; the stored
sample is `0x87` — the lower half of the range folds onto the **top** of the
byte range, inverting part of the curve.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

`core/fpdfapi/render/cpdf_docrenderdata.cpp:124`:
```cpp
size_t o = FXSYS_roundf(output[0] * 255);
...
samples[i][v] = o;
```
`samples[i]` is `uint8_t`. There is no clamp between the multiply and the
store. Note this is undefined behaviour as well as wrong: converting a negative
float to an unsigned integer type is UB in C++, so the observed `0x87` is one
possible outcome rather than a guaranteed one.

§7.10.1 requires function outputs to be clipped to `/Range`. pdf.js clamps at
`src/core/function.js:265`. A `std::clamp(..., 0.0f, 255.0f)` before the cast
is sufficient.

---

## Issue 4 — axial/radial shading: the colour ramp is filled at `i/256` but sampled at `s*255`, and the radial "decreasing" test truncates a distance to an integer

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render any page with a smooth axial (`/ShadingType 2`) gradient and compare
   the rendered ramp against the specification's `t` parameterisation — the
   skew is one part in 256 and accumulates across the ramp.
3. For the second defect: render a radial (`/ShadingType 3`) shading whose
   geometry has `hypot(dx, dy)` and `dr` both between the same pair of
   integers — e.g. `dx = 1.5, dy = 0, dr = -1.2`.

**What is the expected output? What do you see instead?**

Defect A (ramp skew): expected the ramp evaluated over `[t₀, t₁]` inclusive;
observed a systematic offset because the fill and the lookup disagree about the
divisor — `t_max` is never evaluated at all.

Defect B (radial): expected `bDecreasing` to be false for `dx = 1.5,
dr = -1.2` (since `1.5 < 1.2` is false); observed true, because the distance is
truncated to `1` first. That flag selects between the two roots of the
quadratic, so it changes **which** gradient is drawn, not merely its shading.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

Defect A, `core/fpdfapi/render/cpdf_rendershading.cpp:81` (fill):
```cpp
float input = diff * i / kShadingSteps + t_min;      // divides by 256
```
against `:160` (lookup):
```cpp
int index = static_cast<int32_t>(scale * (kShadingSteps - 1));   // times 255
```
`kShadingSteps` is 256 (`:45`). Filling with `i / 256` means the last entry
corresponds to `t = t_min + 255/256 · diff`, so `t_max` is never evaluated,
while the lookup maps `scale = 1.0` onto index 255. Either the fill should
divide by `kShadingSteps - 1` or the lookup should multiply by `kShadingSteps`
and clamp; they must agree.

Defect B, `:220`:
```cpp
bool bDecreasing = dr < 0 && static_cast<int>(hypotf(dx, dy)) < -dr;
```
The `static_cast<int>` quantises the centre distance to a whole device unit
before comparing it against a float radius delta. Dropping the cast makes the
comparison the geometric one. Neither behaviour has a basis in §8.7.4.5.4.

---

## Issue 5 — `IsPunctuation` classifies all of U+0080–U+0094 as punctuation via a `<=` in a chain of `==` tests

**What steps will reproduce the problem?**

1. Build PDFium with form support.
2. Put text containing a character in U+0080–U+0094 into a variable-text field
   (a form field or free-text annotation) — in cp1252-derived content these are
   characters such as Œ (0x8C), Š (0x8A) and Ž (0x8E).
3. Observe where the line breaks.

**What is the expected output? What do you see instead?**

Expected: Œ, Š and Ž are `Lu` letters in Unicode and should not create a line-
break opportunity. Observed: every code point in U+0080–U+0094 is treated as
punctuation, so line breaks appear inside words containing them.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

`core/fpdfdoc/cpvt_section.cpp:80-86`:
```cpp
if (word >= 0x0080 && word <= 0x00FF) {
  return (word == 0x0082 || word == 0x0084 || word == 0x0085 ||
          word == 0x0091 || word == 0x0092 || word == 0x0093 ||
          word <= 0x0094 || word == 0x0096 || word == 0x00B4 ||
          word == 0x00B8);
}
```
The fourth clause is `<=`, not `==`. Because the enclosing branch has already
established `word >= 0x0080`, `word <= 0x0094` is true for all 21 code points
U+0080–U+0094 — which also makes the six preceding equality tests dead code,
since every one of them is below 0x0094. Almost certainly a typo for
`word == 0x0094`.

Worth noting for whoever fixes it: the code points being tested are the
**cp1252** interpretations (0x82 as a low quote, 0x91-0x93 as curly quotes),
while Unicode assigns U+0080–U+009F to C1 controls. Restoring the `==` makes
the function self-consistent; deciding whether the cp1252 reading is intended
at all is a separate question.

---

## Issue 6 — every character above U+FFFF mirrors to `)`

**What steps will reproduce the problem?**

1. Build PDFium.
2. Call the bidi mirroring path on any supplementary-plane character (above
   U+FFFF) — e.g. lay out right-to-left text containing U+10000 or any emoji.
3. Observe the mirrored character.

**What is the expected output? What do you see instead?**

Expected: a character with no mirror mapping is returned unchanged. Observed:
every character above U+FFFF comes back as `)` (U+0029).

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

A sentinel collision. `core/fxcrt/fx_unicode.cpp` — `GetUnicodeProperties`
returns **`0`** for a character outside its table (`:48`, the fall-through
`return 0;`), but the in-table "this character has no mirror" sentinel is
`kMirrorMax` = `(1 << 9) - 1` = **511** (`:24-27`). `GetMirrorChar` (`:143-150`)
tests only for the sentinel:
```cpp
uint16_t prop = GetUnicodeProperties(wch);
size_t idx = prop >> kMirrorBitPos;
if (idx == kMirrorMax) {
  return wch;                 // no mirror — correct
}
CHECK_LT(idx, std::size(kFXTextLayoutBidiMirror));
return kFXTextLayoutBidiMirror[idx];
```
An out-of-range character yields `prop == 0`, hence `idx == 0`, which is a
**valid table index** rather than the sentinel — and entry 0 of the mirror
table is `)`. The fix is for the out-of-range path to return the sentinel
(or for `GetMirrorChar` to range-check `wch` before consulting the table).
