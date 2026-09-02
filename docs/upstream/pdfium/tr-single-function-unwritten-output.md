# A single-function `/TR` with more than 16 outputs renders all black from an unwritten variable

**Status:** drafted 2026-09-02, not yet filed.

**Confidence:** code-level, no runtime repro.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## a single-function `/TR` with more than 16 outputs renders all black from an unwritten variable

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
