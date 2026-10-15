# A free-text annotation without an appearance is centred vertically, or not drawn at all when the form has no `/DR`

**Status:** pdfrum diverges deliberately (2026-10-15); not a defect report.

**Confidence:** source read, observed output.

**Repro files:** `freetext_annotation_without_da.pdf` in the checkout's
`testing/resources/` and `testing/resources/pixel/`; for the missing `/DR`
case, a FreeText with no `/AP` added to pdfrum's own `two_signatures.pdf`.

**Source read at** PDFium `main`, `core/fpdfdoc/cpdf_generateap.cpp`,
`GenerateFreeTextAP` (≈line 1048), and Mozilla pdf.js `master`,
`src/core/annotation.js` and `src/core/default_appearance.js` (2026-10-15).

**Tiebreaker:** pdf.js, an independent implementation of the same
specification. pdfrum follows PDFium only where pdf.js agrees.

## What upstream does

`GenerateFreeTextAP` lays `/Contents` out in a multi-line variable-text
box and then shifts the block by half the slack:

```cpp
CFX_PointF offset(0.0f, (content_rect.Height() - body_rect.Height()) / 2.0f);
```

so a short note sits in the vertical middle of its rectangle. Earlier in the
same function, `form_dict->GetMutableDictFor("DR")` returning null makes the
generator return `false`: an interactive form without `/DR` gives the
annotation no appearance, and it draws nothing.

## What pdf.js does

`FreeTextAnnotation` (`annotation.js` ≈4313) builds an appearance with
`FakeUnicodeFont.createAppearance` (`default_appearance.js` ≈395) whenever
the annotation has none. The font is its own, so neither `/DR` nor the
form's `/DA` is needed; the size is `/DA`'s, or 10. The first line is placed
one line height below the **top** of the rectangle
(`getFirstPositionInfo`, ≈375: `coords: [0, h + lineDescent - lineHeight]`)
and each further line steps down by `lineHeight`. poppler and Acrobat also
start the text at the top.

## What pdfrum does

It follows pdf.js on both points. `ap::freetext::free_text` completes a form
without `/DR` (or without `/DR /Font`, or without `/DA`) from the same
synthesized form a document with no form at all already gets, and draws the
text from the top of the box. Files whose FreeText carries its own `/AP` are
unaffected; only generated appearances move, so the rows below score
`diverged`.

## Also: text notes stay upright

A `/Text` annotation (the note icon) is kept upright on a rotated page
whatever its `NoRotate` flag says: pdf.js sets `this.data.noRotate = true`
for every `TextAnnotation` (`annotation.js` ≈4174), and poppler and Acrobat
do the same. Upstream honours `NoRotate` in `AnnotGetMatrix`
(`cpdf_annot.cpp` ≈62) but does not imply it for notes. No corpus row
exercised the difference when this was written.
