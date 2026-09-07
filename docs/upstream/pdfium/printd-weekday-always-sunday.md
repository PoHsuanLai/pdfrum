# `util.printd` prints every weekday as Sunday (`dddd` / `ddd`)

**Filed** as crbug.com/555848529 on 2026-09-02.

**Fixed upstream** in `b39aa638b` (2026-09-03), "Fix weekday formatting in CJS_Util::printd()", `Fixed: 555848529`; seen at checkout `a043bed4a` on 2026-09-05.

**Confidence:** observed output.

**Repro files:** `weekday.pdf, weekday.in` in `../repro/`.

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

## `util.printd` prints every weekday as Sunday (`dddd` / `ddd`)

**What steps will reproduce the problem?**

1. Build `pdfium_test` with `pdf_enable_v8 = true`.
2. Run `TZ=UTC pdfium_test weekday.pdf` (attached). Its `/OpenAction` is:
   ```js
   app.alert("util.printd('dddd, d mmmm yyyy', 2024-01-03 Wednesday) = " + util.printd("dddd, d mmmm yyyy", new Date(Date.UTC(2024, 0, 3, 12))));
   app.alert("util.printd('ddd', 2024-01-05 Friday) = " + util.printd("ddd", new Date(Date.UTC(2024, 0, 5, 12))));
   ```
3. Read the two `Alert:` lines.

**What is the expected output? What do you see instead?**

Expected:
```
Alert: util.printd('dddd, d mmmm yyyy', 2024-01-03 Wednesday) = Wednesday, 3 January 2024
Alert: util.printd('ddd', 2024-01-05 Friday) = Fri
```
Observed:
```
Alert: util.printd('dddd, d mmmm yyyy', 2024-01-03 Wednesday) = Sunday, 3 January 2024
Alert: util.printd('ddd', 2024-01-05 Friday) = Sun
```

**What version of the product are you using? On what operating system?**

PDFium 154.0.8021.0 (pdfium-binaries `chromium/8021`, V8 enabled); source
read at `6f2272e1f3aa`. Ubuntu 22.04.5 LTS x86-64.

**Please provide any additional information below.**

`fxjs/cjs_util.cpp:52` maps `dddd` → `%A` and `ddd` → `%a`, which `wcsftime`
renders from `tm_wday`. `cjs_util.cpp:267-273` builds the `struct tm` as
```cpp
struct tm time = {};
time.tm_year = year - 1900;
time.tm_mon = month - 1;
time.tm_mday = day;
time.tm_hour = hour; time.tm_min = min; time.tm_sec = sec;
```
and never sets `tm_wday` (or `tm_yday`), so the zero-initialised value —
Sunday — is printed for every date. Either compute `tm_wday` from the date
(the `FX_` helpers already know the day number) or run the struct through
`mktime`/`timegm` before formatting. pdf.js (`util.js:232`, `:254`) takes the
weekday from `Date.prototype.getDay`.

---
