# `AFSimple("avg", a, b)` returns the sum: the function name is accepted case-insensitively but the average divide is case-sensitive

**Filed** as crbug.com/pdfium/555848530 on 2026-09-02.

**Confidence:** observed output.

**Repro files:** `avg.pdf, avg.in` in `../repro/`.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

**Binary** for observed output: `bblanchon/pdfium-binaries` release
`chromium/8021` (`VERSION` 154.0.8021.0, `pdf_enable_v8=true`,
`pdf_enable_xfa=true`, linux x64). Each repro PDF's `/OpenAction` runs the
JavaScript shown and reports via `app.alert`, which `pdfium_test` prints to
stdout as `Alert: <message>`; PDFs were expanded from the `.in` templates in
`repro/` with PDFium's own `testing/tools/fixup_pdf_template.py` and run with
`TZ=UTC` through the public C API (`FORM_DoDocumentJSAction` →
`FORM_DoDocumentOpenAction`).

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## `AFSimple("avg", a, b)` returns the sum: the function name is accepted case-insensitively but the average divide is case-sensitive

**What steps will reproduce the problem?**

1. Build `pdfium_test` with `pdf_enable_v8 = true`.
2. Run `TZ=UTC pdfium_test avg.pdf` (attached). Its `/OpenAction` is:
   ```js
   app.alert("AFSimple('AVG', 2, 4) = " + AFSimple("AVG", 2, 4));
   app.alert("AFSimple('avg', 2, 4) = " + AFSimple("avg", 2, 4));
   app.alert("AFSimple('sum', 2, 4) = " + AFSimple("sum", 2, 4));
   ```
3. Read the three `Alert:` lines.

**What is the expected output? What do you see instead?**

Expected: either `avg` is rejected like any unknown function name, or it is
accepted and returns 3 like `AVG`. Observed:
```
Alert: AFSimple('AVG', 2, 4) = 3
Alert: AFSimple('avg', 2, 4) = 6
Alert: AFSimple('sum', 2, 4) = 6
```
`avg` is accepted (`sum` shows lowercase names are honoured) and returns the
sum, 6, with no error. The same split exists in `AFSimple_Calculate`, so a
calculate script written as `AFSimple_Calculate("avg", …)` silently fills the
field with the total of its inputs.

**What version of the product are you using? On what operating system?**

PDFium 154.0.8021.0 (pdfium-binaries `chromium/8021`, V8 enabled); source
read at `6f2272e1f3aa`. Ubuntu 22.04.5 LTS x86-64.

**Please provide any additional information below.**

`fxjs/cjs_publicmethods.cpp:218-225` (`ApplyNamedOperation`) matches the name
with `EqualsASCIINoCase`, treating `AVG` and `SUM` identically (both sum). The
divide that turns the sum into an average is then guarded by a case-sensitive
compare — `:1341` in `AFSimple`:
```cpp
if (sFunction.EqualsASCII("AVG")) {
  dValue /= 2.0;
}
```
and `:1456` in `AFSimple_Calculate`:
```cpp
if (sFunction.EqualsASCII("AVG") && nFieldsCount > 0) {
```
Making both compares `EqualsASCIINoCase` (or rejecting non-uppercase names in
`ApplyNamedOperation`) fixes it; the two halves should agree either way.
