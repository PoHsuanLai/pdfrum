# pdfrum-doc

Everything the catalog hangs off the page tree (ISO 32000-1 §12): outline
bookmarks, destinations, links, annotations, the AcroForm data model, page
labels, viewer preferences, the tagged structure tree, XMP metadata, and a
PDF/A conformance report. It reads the object graph; it never rasterizes and
never runs JavaScript.

```rust
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::page_label;
use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

// `/PageLabels` maps a *starting* page index to a rule; page 1 onwards is
// upper-case roman with the prefix `A-` (ISO 32000-1 §12.4.2).
let rule = Dict::from_pairs([
    ("S", Object::from(Name::from("R"))),
    ("P", Object::from(PdfString::literal(b"A-"))),
]);
let labels = Dict::from_pairs([(
    "Nums",
    Object::from(Array::of([Object::from(1), Object::from(rule)])),
)]);
let catalog = Dict::from_pairs([("PageLabels", Object::from(labels))]);

let (limits, mut diags) = (Limits::default(), Diagnostics::default());
let label = page_label(&catalog, 2_u32, 3, &NoResolve, &limits, &mut diags);
assert_eq!(label.as_deref(), Some("A-II"));
```

**Appearance generation returns an [`AnnotOverlay`]; it does not mutate the
store.** Readers take `Option<&AnnotOverlay>`, so the same annotation list can
be read with generated appearances or without, and a caller that never asks
for one pays nothing. [`DocOptions`] gates that generation, and both of its
switches are off by default: the oracle builds every annotation list through
the form-fill environment, which disables the `/NeedAppearances` widget path
outright. Turn them on for the documented viewer behaviour instead.

A page label is a lower-bound lookup plus arithmetic, never a direct hit, and
the two "no label" answers differ — a page index outside the document has no
label at all, while a page inside it with no matching tree entry gets its
one-based index as a decimal.

| module | ISO 32000-1 | what it reads |
|---|---|---|
| [`nav`] | §12.3 | outline, destinations, actions, links, name and number trees |
| [`annot`] | §12.5 | annotation dictionaries, subtypes, flags |
| [`ap`] | §12.5.5 | appearance streams and the generated overlay |
| [`form`] | §12.7 | the AcroForm field tree, as data |
| [`structure`] | §14.7 | the tagged structure tree |
| [`page_label`](mod@page_label) | §12.4.2 | the page numbers a reader sees |
| [`pdfa`] | ISO 19005 | conformance level and failed clauses |

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
