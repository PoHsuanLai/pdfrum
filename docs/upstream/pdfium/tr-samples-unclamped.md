# `/TR` transfer samples are stored unclamped, so a negative output wraps to the top of the byte range

**Filed** as crbug.com/pdfium/555967331 on 2026-09-02.

**Confidence:** code-level, no runtime repro.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## `/TR` transfer samples are stored unclamped, so a negative output wraps to the top of the byte range

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
