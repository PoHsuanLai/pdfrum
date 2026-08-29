# pdfrum-doc

**Bookmarks, annotations, AcroForm data model, structure tree.**

The document-level features of ISO 32000 §12, above the page and below any
viewer: the bookmark outline, named destinations, links and actions,
annotations with appearance-stream generation for variable text, the AcroForm
data model for reading and filling fields, the tagged-PDF structure tree, page
labels, viewer preferences, and document metadata including XMP.

No JavaScript. A form field whose displayed value would come from a calculation
script reads here as whatever the file last stored — a scope decision, not a
gap.

**The one design point to understand before reading anything else.** Generating
an annotation's appearance stream conventionally *mutates the document*: a
sticky note's `/Rect` is replaced by a 20×20 box, an ink annotation's is
inflated by half its border width, and every annotation touched gains an
`/AP /N` and a marker key. Those mutations are visible to everything that reads
the file afterwards.

Parsed objects here are values and the object store is immutable, so
`ap::generate_appearances` returns an `AnnotOverlay` instead: a per-annotation
record of the stream it produced and the dictionary edits it implies. Every
reader in this crate takes an `Option<&AnnotOverlay>` and consults it before the
raw dictionary. Forget to thread it and you get the *pre-generation* state,
which is wrong in a way no type will catch.

## Part of pdfrum

`pdfrum-doc` is the document-feature layer of
[pdfrum](https://crates.io/crates/pdfrum), a pure-Rust PDF engine. For a
batteries-included API — `Document`, `Page`, `Form`, with the overlay threaded
for you — use the `pdfrum` facade.

`#![forbid(unsafe_code)]`.

## License

MIT OR Apache-2.0, at your option.
