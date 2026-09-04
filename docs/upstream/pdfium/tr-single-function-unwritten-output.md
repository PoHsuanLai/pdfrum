# A single-function `/TR` with more than 16 outputs renders all black from an unwritten variable

**Status:** drafted 2026-09-02, not yet filed.

**Confidence:** observed output (2026-09-05; was code-level).

**Repro files:** `tr_single_function_17_outputs.pdf` (black),
`tr_single_function_16_outputs.pdf` and `tr_array_17_outputs.pdf` (the two
controls, grey) in `../repro/`.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Re-verified at** `a043bed4a` (2026-09-05): two fixes landed in this very
function on 2026-09-03 — `2621636d3` (the reversed array order, crbug
555940409) and `8794e9c27` (the unclamped samples, crbug 555967331) — and
both left the single-function branch as it was; the cited lines `:132-137`
are unchanged. Rendered with `pdfium_test` at `6f2272e1f3aa`.
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
2. Render the attached `tr_single_function_17_outputs.pdf`: a 100×100 page
   drawing one `0.8 g` square under an `/ExtGState` whose `/TR` is a
   **single** sampled function (Type 0, `/Size [2]`, 8-bit) with **17**
   outputs, every output the identity.
3. Render the two controls: `tr_single_function_16_outputs.pdf` (the same
   function with 16 outputs) and `tr_array_17_outputs.pdf` (the 17-output
   function given four times as an array).

**What is the expected output? What do you see instead?**

Expected: all three render the square at grey 204 — the identity transfer,
which is what the array branch produces for the identical condition.
Observed (`pdfium_test --png`, centre pixel):

| file | expected | observed |
|---|---|---|
| `tr_single_function_17_outputs.pdf` | (204, 204, 204) | **(0, 0, 0)** |
| `tr_single_function_16_outputs.pdf` | (204, 204, 204) | (204, 204, 204) |
| `tr_array_17_outputs.pdf` | (204, 204, 204) | (204, 204, 204) |

The single 17-output function turns the page's content black, because the
sample value comes from a variable that was never written for this input.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28), and the branch is unchanged at `a043bed4a` (2026-09-04), after the two adjacent `/TR` fixes of 2026-09-03. Ubuntu 22.04.5 LTS, x86-64.

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
