# `IsLeapYear` has its century exception inverted: 2000 is not a leap year, 1900 is

**Filed** as crbug.com/pdfium/555821585 on 2026-09-02.

**Confidence:** observed output.

**Repro files:** `leap.pdf, leap.in` in `../repro/`.

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

## `IsLeapYear` has its century exception inverted: 2000 is not a leap year, 1900 is

**What steps will reproduce the problem?**

1. Build `pdfium_test` with `pdf_enable_v8 = true` (or use a V8-enabled
   prebuilt such as pdfium-binaries `chromium/8021`).
2. Run `TZ=UTC pdfium_test leap.pdf` (attached). Its `/OpenAction` is:
   ```js
   app.alert("util.scand('dd/mm/yyyy','01/03/2000') = " + util.scand("dd/mm/yyyy", "01/03/2000"));
   app.alert("util.printd('dd/mm/yyyy', 2000-03-01) = " + util.printd("dd/mm/yyyy", new Date(Date.UTC(2000, 2, 1, 12))));
   app.alert("util.printd('dd/mm/yyyy', 2000-12-31) = " + util.printd("dd/mm/yyyy", new Date(Date.UTC(2000, 11, 31, 12))));
   app.alert("util.scand('dd/mm/yyyy','01/03/1900') = " + util.scand("dd/mm/yyyy", "01/03/1900"));
   app.alert("util.printd('dd/mm/yyyy', 1900-03-01) = " + util.printd("dd/mm/yyyy", new Date(Date.UTC(1900, 2, 1, 12))));
   ```
3. Read the five `Alert:` lines on stdout.

**What is the expected output? What do you see instead?**

Expected (2000 is a leap year, 1900 is not):
```
Alert: util.scand('dd/mm/yyyy','01/03/2000') = Wed Mar 01 2000 …
Alert: util.printd('dd/mm/yyyy', 2000-03-01) = 01/03/2000
Alert: util.printd('dd/mm/yyyy', 2000-12-31) = 31/12/2000
Alert: util.scand('dd/mm/yyyy','01/03/1900') = Thu Mar 01 1900 …
Alert: util.printd('dd/mm/yyyy', 1900-03-01) = 01/03/1900
```
Observed:
```
Alert: util.scand('dd/mm/yyyy','01/03/2000') = Tue Feb 29 2000 20:18:41 GMT+0000 (UTC)
Alert: util.printd('dd/mm/yyyy', 2000-03-01) = 02/03/2000
Alert: util.printd('dd/mm/yyyy', 2000-12-31) = 00/00/2000
Alert: util.scand('dd/mm/yyyy','01/03/1900') = Fri Mar 02 1900 20:18:41 GMT+0000 (UTC)
Alert: util.printd('dd/mm/yyyy', 1900-03-01) = 29/02/1900
```
Every date from 1 March to 31 December of a year divisible by 400 parses one
day early and prints one day late; 31 December 2000 prints as `00/00/2000`
(`mmmm` prints an empty string for it). Years divisible by 100 but not 400
have the opposite error. All other years are unaffected.

**What version of the product are you using? On what operating system?**

PDFium 154.0.8021.0 (pdfium-binaries `chromium/8021`, V8 enabled); source
read at `6f2272e1f3aa`. Ubuntu 22.04.5 LTS x86-64.

**Please provide any additional information below.**

`fxjs/fx_date_helpers.cpp:71`:
```cpp
return (year % 4 == 0) && ((year % 100 != 0) || (year % 400 != 0));
```
The last clause should be `year % 400 == 0`. As written it is true for every
year not divisible by 400, so the `% 100` exception never fires and the `% 400`
exception fires backwards. `DayFromYear` three functions above counts leap
days with the correct `/4 − /100 + /400` cadence, so for a century year the
two disagree about the year's length; `TimeFromYearMonth` (`:84`),
`MonthFromTime` (`:123`) and `DateFromTime` (`:142`) consume both, which is
where the one-day shift and the `00/00` come from. The bug survives because
the only leap year `fx_date_helpers_unittest.cpp` exercises is 1972, and its
year-2000 cases (`:131-144`) are all in January, February or November — the
`% 100` and `% 400` boundaries are never tested. A unit-test repro without V8:
`EXPECT_TRUE(IsLeapYear(2000))` and `EXPECT_FALSE(IsLeapYear(1900))` both fail.

---
