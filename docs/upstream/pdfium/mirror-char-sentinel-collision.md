# Every character above U+FFFF mirrors to `)`

**Filed** as crbug.com/pdfium/555940408 on 2026-09-02.

**Confidence:** code-level, clear repro.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## every character above U+FFFF mirrors to `)`

**What steps will reproduce the problem?**

1. Build PDFium.
2. Call the bidi mirroring path on any supplementary-plane character (above
   U+FFFF) — e.g. lay out right-to-left text containing U+10000 or any emoji.
3. Observe the mirrored character.

**What is the expected output? What do you see instead?**

Expected: a character with no mirror mapping is returned unchanged. Observed:
every character above U+FFFF comes back as `)` (U+0029).

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

A sentinel collision. `core/fxcrt/fx_unicode.cpp` — `GetUnicodeProperties`
returns **`0`** for a character outside its table (`:48`, the fall-through
`return 0;`), but the in-table "this character has no mirror" sentinel is
`kMirrorMax` = `(1 << 9) - 1` = **511** (`:24-27`). `GetMirrorChar` (`:143-150`)
tests only for the sentinel:
```cpp
uint16_t prop = GetUnicodeProperties(wch);
size_t idx = prop >> kMirrorBitPos;
if (idx == kMirrorMax) {
  return wch;                 // no mirror — correct
}
CHECK_LT(idx, std::size(kFXTextLayoutBidiMirror));
return kFXTextLayoutBidiMirror[idx];
```
An out-of-range character yields `prop == 0`, hence `idx == 0`, which is a
**valid table index** rather than the sentinel — and entry 0 of the mirror
table is `)`. The fix is for the out-of-range path to return the sentinel
(or for `GetMirrorChar` to range-check `wch` before consulting the table).
