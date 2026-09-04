# `util.scand` maps every two-digit year to 2000–2099 (`31/12/85` → 2085)

**Filed** as crbug.com/pdfium/555821586 on 2026-09-02.

**Fixed upstream** in `017295ab7` (2026-09-03), "Implement 50-year pivot for two-digit year parsing", `Fixed: 555821586`; seen at checkout `a043bed4a` on 2026-09-05.

**Confidence:** observed output.

**Repro files:** `yy.pdf, yy.in` in `../repro/`.

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

## `util.scand` maps every two-digit year to 2000–2099 (`31/12/85` → 2085)

**What steps will reproduce the problem?**

1. Build `pdfium_test` with `pdf_enable_v8 = true`.
2. Run `TZ=UTC pdfium_test yy.pdf` (attached). Its `/OpenAction` is:
   ```js
   app.alert("util.scand('dd/mm/yy','31/12/85') = " + util.scand("dd/mm/yy", "31/12/85"));
   app.alert("util.scand('dd/mm/yy','31/12/49') = " + util.scand("dd/mm/yy", "31/12/49"));
   ```
3. Read the two `Alert:` lines.

**What is the expected output? What do you see instead?**

Expected (Acrobat's fifty pivot, as implemented by pdf.js
`src/scripting_api/util.js:372-375`: `n < 50` → 2000s, otherwise 1900s):
```
Alert: util.scand('dd/mm/yy','31/12/85') = Mon Dec 31 1985 …
Alert: util.scand('dd/mm/yy','31/12/49') = Fri Dec 31 2049 …
```
Observed:
```
Alert: util.scand('dd/mm/yy','31/12/85') = Mon Dec 31 2085 20:18:41 GMT+0000 (UTC)
Alert: util.scand('dd/mm/yy','31/12/49') = Fri Dec 31 2049 20:18:41 GMT+0000 (UTC)
```
A birth date, document date or expiry typed into an `AFDate_Keystroke` field
with a `yy` format lands in the 2080s, and `AFDate_Format` prints it back that
way.

**What version of the product are you using? On what operating system?**

PDFium 154.0.8021.0 (pdfium-binaries `chromium/8021`, V8 enabled); source
read at `6f2272e1f3aa`. Ubuntu 22.04.5 LTS x86-64.

**Please provide any additional information below.**

`fxjs/fx_date_helpers.cpp:330`:
```cpp
int nYearSub = 99;  // nYear - 2000;
```
and `:556`:
```cpp
if (nYear >= 0 && nYear <= nYearSub) {
  nYear += 2000;
}
```
Every two-digit year is unconditionally moved into 2000–2099. The commented
expression suggests a window relative to the current year was intended and
never written. Either a fixed pivot at 50 (Acrobat, pdf.js) or a sliding
window would fix it; the current behaviour is the one thing neither does.

---
