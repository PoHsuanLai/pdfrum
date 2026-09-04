# `/TR` transfer function array is loaded in reverse: `array[2]` drives red, and the fourth function is never read

**Filed** as crbug.com/pdfium/555940409 on 2026-09-02.

**Fixed upstream** in `2621636d3` (2026-09-03), "Fix reversed channel order when loading transfer function array", `Fixed: 555940409`; seen at checkout `a043bed4a` on 2026-09-05.

**Confidence:** code-level, no runtime repro.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## `/TR` transfer function array is loaded in reverse: `array[2]` drives red, and the fourth function is never read

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
