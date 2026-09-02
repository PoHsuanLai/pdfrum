# A text object whose glyph bounding box is empty is dropped, so spaces in their own text object are lost

**Status:** drafted 2026-09-02, not yet filed.

**Confidence:** observed output.

**Repro files:** `whitespace.pdf, bug_444176962.pdf` in `../repro/`.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## a text object whose glyph bounding box is empty is dropped, so spaces in their own text object are lost

**What steps will reproduce the problem?**

1. Build `pdfium_test` (no special flags needed; this is not V8-dependent).
2. Run `pdfium_test --txt testing/resources/whitespace.pdf` and read
   `whitespace.pdf.0.txt`. The file's entire content stream is:
   ```
   BT
   20 100 Td
   /F1 16 Tf
   ( ) Tj
   ET
   ```
   with `/F1` a plain `/Type1 /Helvetica`.
3. Run `pdfium_test --txt testing/resources/bug_444176962.pdf` and read
   `bug_444176962.pdf.0.txt`. Its content stream is nine one-glyph shows, of
   which `<0003>` is the space:
   ```
   BT
   /F7 57 Tf
   13 0 Td <0013> Tj
   12 0 Td <000F> Tj
   26 0 Td <0003> Tj      <- the space, in its own text object
   22 0 Td <000A> Tj
   ...
   ```

**What is the expected output? What do you see instead?**

Expected: `whitespace.pdf` extracts one `U+0020`; `bug_444176962.pdf` extracts
`local act`.

Observed (UTF-32LE, BOM stripped, decoded):

| file | expected | observed |
|---|---|---|
| `whitespace.pdf` | `" "` | `""` — the output is the BOM alone, four bytes total |
| `bug_444176962.pdf` | `"local act"` | `"localact"` |

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28), built from source. Ubuntu 22.04.5 LTS,
x86-64. Not V8-dependent; reproduces in a plain build.

**Please provide any additional information below.**

`core/fpdftext/cpdf_textpage.cpp:881`, in `ProcessTextObject`:
```cpp
if (fabs(text_obj->GetRect().Width()) < kSizeEpsilon) {
  return;
}
```
and the identical test at `:1076` in `ProcessTransformedTextObjects`, which
`continue`s. `kSizeEpsilon` is `0.01f` (`:42`).

`GetRect()` is built from glyph **bounding boxes** —
`core/fpdfapi/page/cpdf_textobject.cpp:305-331` accumulates
`font->GetCharBBox(char_code)` over the object's characters — and a space
glyph's bounding box is legitimately empty. ISO 32000-1 §9.2.2 keeps a glyph's
**displacement** (`w0`) and its **bounding box** distinct precisely because
they differ for exactly this class of glyph: a space advances the text position
and paints nothing. Testing the bounding box therefore discards a text object
that has real content by the specification's own measure.

The suggested predicate is the object's advance rather than its box: an object
whose summed `w0` (through the text matrix) is non-zero is a real object even
when its box is empty. That keeps the gate's original purpose — discarding
genuinely degenerate objects — while retaining spaces.

Why it usually goes unnoticed: most producers put the space inside a larger
`Tj`, where the object's box is non-empty because of the neighbouring glyphs,
and PDFium separately regenerates `U+0020` from inter-object spacing
(`cpdf_textpage.cpp`'s `ProcessTextObject` spacing logic), which masks the loss
in the common case. It only surfaces when the space is alone in its object —
whitespace-only pages, and one-`Tj`-per-glyph producers.

**One consequence a fix must handle.** Once these objects survive, the
character that comes back is whatever the font maps the code to, which for a
number of real files is `U+00A0` rather than `U+0020` — the spacing heuristic
had been substituting the plain space. Mozilla's pdf.js NFKC-normalizes
extracted text and so emits `U+0020` for those glyphs
(`src/shared/util.js:1050-1065`, called from `runBidiTransform` at
`src/core/evaluator.js:2685-2689`); matching that keeps the extracted byte the
same as today for every file where the heuristic was already producing the
right answer. Without it, a corpus-wide `0x20` → `0xA0` change appears at every
such space.
