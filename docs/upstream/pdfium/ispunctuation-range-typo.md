# `IsPunctuation` classifies all of U+0080–U+0094 as punctuation via a `<=` in a chain of `==` tests

**Status:** drafted 2026-09-02, not yet filed.

**Confidence:** code-level, arguable scope.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Re-verified at** `a043bed4a` (2026-09-05): `cpvt_section.cpp:84` still reads `word <= 0x0094`.
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## `IsPunctuation` classifies all of U+0080–U+0094 as punctuation via a `<=` in a chain of `==` tests

**What steps will reproduce the problem?**

1. Build PDFium with form support.
2. Put text containing a character in U+0080–U+0094 into a variable-text field
   (a form field or free-text annotation) — in cp1252-derived content these are
   characters such as Œ (0x8C), Š (0x8A) and Ž (0x8E).
3. Observe where the line breaks.

**What is the expected output? What do you see instead?**

Expected: Œ, Š and Ž are `Lu` letters in Unicode and should not create a line-
break opportunity. Observed: every code point in U+0080–U+0094 is treated as
punctuation, so line breaks appear inside words containing them.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

`core/fpdfdoc/cpvt_section.cpp:80-86`:
```cpp
if (word >= 0x0080 && word <= 0x00FF) {
  return (word == 0x0082 || word == 0x0084 || word == 0x0085 ||
          word == 0x0091 || word == 0x0092 || word == 0x0093 ||
          word <= 0x0094 || word == 0x0096 || word == 0x00B4 ||
          word == 0x00B8);
}
```
The fourth clause is `<=`, not `==`. Because the enclosing branch has already
established `word >= 0x0080`, `word <= 0x0094` is true for all 21 code points
U+0080–U+0094 — which also makes the six preceding equality tests dead code,
since every one of them is below 0x0094. Almost certainly a typo for
`word == 0x0094`.

Worth noting for whoever fixes it: the code points being tested are the
**cp1252** interpretations (0x82 as a low quote, 0x91-0x93 as curly quotes),
while Unicode assigns U+0080–U+009F to C1 controls. Restoring the `==` makes
the function self-consistent; deciding whether the cp1252 reading is intended
at all is a separate question.

---
