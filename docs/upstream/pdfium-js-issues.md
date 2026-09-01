# Upstream issue drafts — PDFium (crbug.com/pdfium)

Five issues in PDFium's JavaScript date/`AF*` support, each in the Chromium
issue tracker's template and ready to paste. Everything from the first `## Issue`
heading down is issue text; nothing above the rule is meant to be posted.

**Status:** drafted 2026-09-02, not yet filed.

**How these were produced.** Every "What do you see instead" line below is
observed output, not a reading of the source. Each issue has a one-page PDF in
`docs/upstream/pdfium-repro/` whose `/OpenAction` runs the JavaScript shown and
reports through `app.alert`; `pdfium_test` prints each alert to stdout as
`Alert: <message>`. The PDFs were expanded from the `.in` templates beside
them with PDFium's own `testing/tools/fixup_pdf_template.py`, and run through
the public C API (`FORM_DoDocumentJSAction` → `FORM_DoDocumentOpenAction`,
the same sequence as `pdfium_test`) against a V8-enabled PDFium binary with
`TZ=UTC`. Source line numbers are from commit `6f2272e1f3aa` (2026-08-28).

**Versions to quote.** Binary: `bblanchon/pdfium-binaries` release
`chromium/8021` (`VERSION` 154.0.8021.0, `pdf_enable_v8=true`,
`pdf_enable_xfa=true`, linux x64). Source read at `6f2272e1f3aa`. Host:
Ubuntu 22.04.5 LTS, x86-64.

**Where the earlier draft was wrong.** `docs/reviews/claude-review-af-library.md`
§6 gave `FX_ParseDateUsingFormat(L"29/02/2000", …) == kBadDate` as the
leap-year repro. Measured, that call *succeeds*: the day-of-month check is a
range check, and January and February have the same offset in both month
tables, so the inverted leap flag is invisible until March. The observable
damage is the one-day shift from March onward and the `00/00/2000` output for
31 December — recorded below. The §6 paragraph is annotated in place, not
rewritten.

**Independent implementation used as the tiebreaker.** Mozilla's pdf.js
implements the same Adobe-specified library from the specification;
`src/scripting_api/util.js` is cited where its answer differs from PDFium's.

---

## Issue 1 — `IsLeapYear` has its century exception inverted: 2000 is not a leap year, 1900 is

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

## Issue 2 — `util.scand` maps every two-digit year to 2000–2099 (`31/12/85` → 2085)

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

## Issue 3 — `util.scand` mis-parses 12-hour times: `12:30 pm` becomes 00:30 next day, `12:30 am` becomes 12:30, uppercase `PM` is ignored

**What steps will reproduce the problem?**

1. Build `pdfium_test` with `pdf_enable_v8 = true`.
2. Run `TZ=UTC pdfium_test ampm.pdf` (attached). Its `/OpenAction` is:
   ```js
   app.alert("util.scand('HH:MM tt','12:30 pm') = " + util.scand("HH:MM tt", "12:30 pm"));
   app.alert("util.scand('HH:MM tt','12:30 am') = " + util.scand("HH:MM tt", "12:30 am"));
   app.alert("util.scand('h:MM tt','1:30 PM') = " + util.scand("h:MM tt", "1:30 PM"));
   app.alert("util.scand('h:MM tt','1:30 pm') = " + util.scand("h:MM tt", "1:30 pm"));
   ```
3. Read the four `Alert:` lines (the date part is "today"; only the time and
   the day-of-month roll-over matter).

**What is the expected output? What do you see instead?**

Expected (`hours = hours % 12 + (am ? 0 : 12)`, marker matched
case-insensitively — pdf.js `util.js:578-584` and `:644`):
```
… '12:30 pm' → 12:30 the same day
… '12:30 am' → 00:30 the same day
… '1:30 PM'  → 13:30
… '1:30 pm'  → 13:30
```
Observed (run on 1 September 2026, 20:18 UTC):
```
Alert: util.scand('HH:MM tt','12:30 pm') = Wed Sep 02 2026 00:30:35 GMT+0000 (UTC)
Alert: util.scand('HH:MM tt','12:30 am') = Tue Sep 01 2026 12:30:35 GMT+0000 (UTC)
Alert: util.scand('h:MM tt','1:30 PM') = Tue Sep 01 2026 01:30:35 GMT+0000 (UTC)
Alert: util.scand('h:MM tt','1:30 pm') = Tue Sep 01 2026 13:30:35 GMT+0000 (UTC)
```
`12:30 pm` rolls over to half past midnight *tomorrow*; `12:30 am` is read as
noon; `PM` in capitals is silently treated as AM. The `printd` direction is
correct (`h:MM tt` prints 12:30 as `12:30 pm` and 00:30 as `12:30 am`), so a
value formatted by PDFium and parsed back by PDFium changes by twelve hours or
a day.

**What version of the product are you using? On what operating system?**

PDFium 154.0.8021.0 (pdfium-binaries `chromium/8021`, V8 enabled); source
read at `6f2272e1f3aa`. Ubuntu 22.04.5 LTS x86-64.

**Please provide any additional information below.**

`fxjs/fx_date_helpers.cpp:402` and `:445` set
`bPm = (… value[j] == 'p' …)` — lowercase only — and `:552`:
```cpp
if (bPm) {
  nHour += 12;
}
```
adds twelve without reducing the hour modulo 12 first, so `12 pm` becomes 24
(the next day) and `12 am` is left at 12. The fix is
`nHour = (nHour % 12) + (bPm ? 12 : 0)` when a `t`/`tt` marker was consumed,
and accepting `A`/`P` as well as `a`/`p`.

---

## Issue 4 — `util.printd` prints every weekday as Sunday (`dddd` / `ddd`)

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

## Issue 5 — `AFSimple("avg", a, b)` returns the sum: the function name is accepted case-insensitively but the average divide is case-sensitive

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
