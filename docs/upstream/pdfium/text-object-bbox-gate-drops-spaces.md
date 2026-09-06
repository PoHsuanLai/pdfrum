# A text object whose glyph bounding box is empty is dropped, so spaces in their own text object are lost

**Status:** drafted 2026-09-02, not yet filed.

**Confidence:** observed output.

**Repro files:** `whitespace.pdf, bug_444176962.pdf` in `../repro/`.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Re-verified at** `a043bed4a` (2026-09-05): the gate at `cpdf_textpage.cpp:881` and `:1076` and `kSizeEpsilon` at `:42` are unchanged.
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

**How often the mask holds, measured (2026-09-06).** pdfrum implemented the
suggested advance-based predicate and measured it against `pdfium_test` over
44 benchmark documents and a 1759-file conformance corpus. The result bounds
this report rather than withdrawing it, and it also strengthens one half of
it.

*Where the mask holds.* On pages that contain another text object, keeping
these objects made the extracted text **differ** from PDFium's on 20 of the
44 files and on 131 corpus files, and in every case the difference was a
**duplicated** separator, never a recovered one — the inter-object heuristic
had already emitted it. So the loss described above is genuinely not
observable whenever a neighbour exists, which is the overwhelming majority
of real content.

*Where it does not.* When the spaces-only object is the page's **only** text
object there is no neighbour for the heuristic to span, and the loss is
total: `whitespace.pdf` extracts as the empty string. pdfrum diverges from
PDFium for exactly this shape and keeps the space.

*A stronger case than spaces.* The same gate drops objects that draw
**letters**, not only spaces. On `testing/resources/bug_921.pdf`,
`pdfium_test --txt` begins mid-sentence — "разве не выражает глубокого
челове…" — where the page draws "И разве не выражает…". Five characters of
running Russian prose are lost: an `И`, an em dash, a `в`, a `я` and a
second `И`. Their glyph boxes are empty while their `w0` is 5.3–11.3, so
this is the same defect, and here it silently corrupts extracted prose
rather than dropping whitespace. **This is probably the strongest repro in
the report and it does not depend on the space argument at all.**

*A predicate that works, measured.* An earlier draft of this note said the
recovered letters could not be told apart from the C0 control runs the gate
*correctly* discards in `testing/resources/text_tcpdf_055.pdf` (codes 0..=31
in one-glyph objects) and `bug_651304.pdf` (a lone `U+0001`), because both
families have empty boxes and no usable `ToUnicode`. That is not so. The
mapping is empty for both, but the **character codes** separate them, and
`CPDF_TextPage` already falls back to the code where the mapping is empty —
`cpdf_textpage.cpp:1213-1215`, `unicode += static_cast<wchar_t>(char_code_)`.

pdfrum now keeps an empty-box object when the object's summed `w0` clears
the same epsilon the box is tested against **and** at least one code it
shows resolves (mapping first, code as fallback) to a scalar that is neither
whitespace nor a C0/C1 control. Scored over 44 benchmark documents and a
1759-file conformance corpus, that recovers `bug_921.pdf`'s five characters
and `bug_665467.pdf`'s `Л` while leaving every control run and every
whitespace-only object dropped exactly as today.

Two details are worth passing on, because both cost a corpus-wide regression
when we got them wrong:

- The whitespace test must be Unicode whitespace, not `U+0020`. Fifty
  `testing/corpus/pdfium/annots/annotation_*.pdf` fixtures draw `U+00A0` in
  an empty-box object; treating that as a character appends a trailing space
  to every one of them.
- The `w0` test is what excludes genuinely zero-width glyphs — `U+200B` in
  `bug_491516663.pdf`, and the hairline pair in `bug_491161396.pdf` at `w0`
  0.006 — which are degenerate on both §9.2.2's and §9.4.3's measures and
  have nothing to recover.

Any change here still needs to be scored against the extracted text of a
large corpus, because the gate and the spacing heuristic are load-bearing
together: the same measurement showed that keeping spaces-only objects
*generally* duplicates separators `ProcessInsertObject` already generates,
which is why the predicate above excludes them.
