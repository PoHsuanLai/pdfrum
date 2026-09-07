# `util.scand` mis-parses 12-hour times: `12:30 pm` becomes 00:30 next day, `12:30 am` becomes 12:30, uppercase `PM` is ignored

**Filed** as crbug.com/555848528 on 2026-09-02.

**Still open** at checkout `a043bed4a` (2026-09-05): `bPm` is still set only on a lowercase `p` (`fx_date_helpers.cpp:406`, `:449`) and `nHour += 12` at `:557` still has no modulo — four lines later than cited, after the two-digit-year fix above it.

**Confidence:** observed output.

**Repro files:** `ampm.pdf, ampm.in` in `../repro/`.

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

## `util.scand` mis-parses 12-hour times: `12:30 pm` becomes 00:30 next day, `12:30 am` becomes 12:30, uppercase `PM` is ignored

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
