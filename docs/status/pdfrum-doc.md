# `pdfrum-doc` status

**Updated:** 2026-08-29 · **State:** navigation, annotations, structure tree
and shape-based appearance generation implemented; the variable-text engine
and the form-field tree are not yet built.

Contract: SPEC.md §10 (including the 2026-08-29 rulings); behavior:
`docs/design/pdfrum-doc.md`.

## Conformance

| Metric | Before | After |
|---|---|---|
| `--annot` Tier-A | 3/1832 (0.2%) | **1974/2055 (96.1%)** |
| `--show-structure` Tier-A | 1408/1468 (95.9%) | **1674/1675 (99.9%)** |
| Files passing outright | 47/1468 | **1085/1675** |

Both dumps are wired through `pdfrum-tool`. metadata and pageinfo stay at
100%, text is unchanged at 99.0% (97.9% non-empty), and pixel is unaffected
at 66.0% ≥ 0.99 — `conformance run --check-regressions` reports **no
regressions**.

The corpus grew from 1468 files to 1675 during this work, so the "before"
column is the smaller set; the rates are what compare, not the counts. The
annot denominator is 2055 rather than 2061 because six artifacts are now
excluded as crash goldens (see E3 below).

Neither tier reaches M6's ≥ 98% exit criterion for `--annot` yet. The 81
remaining mismatches are almost entirely `Number of objects` lines where the
oracle counts text objects that a laid-out field body produced — which is the
work named under "what is not built yet", not a defect in what is here.

The structure tier went from 60 mismatched files to one, and that one —
`bug_717.pdf` — is blocked below this crate: its whole structure tree lives
in an object stream whose objects currently resolve as free, so the catalog's
`/StructTreeRoot` reads as absent. Its test asserts the right answer and skips
while the tree is empty, so it starts passing on its own when the objects do.

## What landed

| module | contents |
|---|---|
| `geom` | the PDF rectangle vocabulary: normalize, inflate/deflate, union, intersect, contains, centre square, `match_rect`, and the epsilon comparisons |
| `color` | the colour-array reader and the **two different** CMYK-to-byte formulas, named apart |
| `nav/outline` | the pre-order bookmark walk with its cycle guard, the title's control-character clamp, the colour's range test |
| `nav/name_tree` | the linear leaf scan, the objnum cycle set with its array side effect, the two-rung named-destination ladder |
| `nav/number_tree` | `find` and the backward-scanning `lower_bound`, with the re-descent that can report a key and no value |
| `nav/dest` | the eight view modes, the strict `XYZ` reader, unvalidated page indices |
| `nav/action` | the eighteen action types, the per-type accessors, the additional-action table with its key collision, the `/Next` chain |
| `nav/filespec` | the five-key precedence, the two different decodings, the embedded-file stream |
| `nav/link` | the null-preserving per-page list and the backward hit test |
| `annot` | the subtype table, the flag word, quadpoint arithmetic, the `/AP` + `/AS` ladder, the annotation list with pop-up synthesis |
| `annot_dump` | the `--annot` emitter, including round-half-to-even fixed-point formatting |
| `ap/fmt` | both float writers, pinned against a shared reference table |
| `ap/emit` | the content sink that makes the writer choice explicit at every call site |
| `ap/border` | the five border styles and the two different width lookups |
| `ap/da` | the `/DA` tokenizer — a third one, deliberately not shared — and the font and colour lookups |
| `ap/markup` | the nine shape generators |
| `ap/shapes` | the six checkbox and radio glyph outlines |
| `ap/widget` | widget chrome for a control whose appearance does not resolve |
| `structure` | the per-page bottom-up build and the `--show-structure` emitter |
| `form/attr` | the inherited-attribute walk and the fully-qualified name |
| `page_label`, `prefs`, `metadata` | the remaining catalog readers |

## What is not built yet

- **The variable-text layout engine** (`vt/`). Only its character classifier
  is in place. Without it the two text-bearing appearance generators — free
  text and pop-ups — produce nothing, and a widget's appearance carries its
  chrome but no text body.
- **The AcroForm field tree** (`form/`). Only the inherited-attribute walk and
  the fully-qualified name are in place; field discovery, values, the
  checkbox and radio group semantics and the `/I`-versus-`/V` tie-break are
  not.

Between them they are what the remaining `--annot` gap is made of.

## Errata against the design brief

Five, each found by a fixture rather than by reading.

**§1.7.1 — a Boolean `/MarkInfo /Marked` does not disable the tree.** The
brief says a Boolean reads as zero there. It does not: the integer reading of
a Boolean is its flag, so the spec-conforming `/Marked true` works. Believing
the brief would have made every tagged document untagged.

**§1.7.1 — the page's parent-tree key is `/StructParents`, not
`/StructParent`.** The plural is a page's; the singular is an annotation's.
With the wrong one nothing resolves and every tree comes out empty.

**§1.7.3 item 5 — a present-but-empty `/ID` prints `ID: `, not nothing.** The
length the emitter tests is a UTF-16 **byte** count including the terminator,
so an empty string measures two and passes the `> 0` gate. Same for `Lang:`
and `Parent ID:`.

**§4.3 — the shortest float writer gives `123456790` for `123456789.0`.** The
brief's expected column holds `123456792`, which is the exact `f32` value.
Both dragonbox and `ryu` answer with the shortest decimal that round-trips,
which is shorter than the exact one.

**§1.7.3 / D10 — wide-string output stops at the first NUL as well.** The
brief names only the unpaired-surrogate truncation. `marked_content_id.pdf`
writes `/Alt (Hello!\000)` and the oracle prints `Hello!`: the conversion ends
at the first zero code unit, which is a code point a PDF string may
legitimately contain.

## Escalations, resolved

**E1 / OQ-4 — the widget appearance path is reachable, and far more widely
than the brief estimated.** The brief scopes `cpdfsdk_appstream` to the five
corpus files that set `/NeedAppearances`, on the grounds that the wider
regeneration path is gated on that flag. It is — but a second, ungated call
site is not: a widget whose appearance does not resolve gets one built when
its page is opened, whatever the form says. That reaches **every widget in
the corpus without a usable `/AP`**, and it is directly `--annot`-visible: the
two colour lines fail whenever an appearance exists.

The ruling to ship the shape tables and skip the second layout engine still
holds, and this crate follows it: `ap/shapes` draws the six checkbox and
radio glyphs, `ap/widget` draws the background and border, and the text body
is left out. That is exact for a widget with no `/MK` colours and no value —
which is the common case, and matches byte for byte — and short by the text
objects for the rest.

**E2 — dict keys are sorted at the emission site.** Done, in
`structure/dump`. Confirmed against the goldens: `tagged_table.pdf` writes
`Scope` before `O` and the golden prints them alphabetically.

**E3 — the Unknown/Redact `--annot` goldens are crash artifacts.** Confirmed:
no golden in the store contains either spelling, and `redact_annot`'s
manifest records `oracle_failures: ["Annot"]` with a zero-byte dump. The
harness now **excludes an artifact whose own oracle pass failed** rather than
comparing against it — a golden written by an aborting process is not an
answer, and pinning it would make the crash the contract.

**E4 — `%.3f` tie-breaking does matter, and is implemented.** `annot_dump`
carries a round-half-to-even fixed-point formatter rather than relying on
Rust's round-half-away-from-zero, so a value landing exactly on a half-milli
boundary agrees with the C library.

**E5 — the bidi dependency is pinned and unused so far.** `unicode-bidi` is a
declared dependency of this crate for the layout engine's line ordering. The
six pinned orderings will be validated before `vt/place` is written, per the
ruling.

## Divergences

The design brief's numbered divergences all hold as written, with these
notes:

- **D1 (the overlay) is load-bearing and works.** `ap::generate_appearances`
  returns an `AnnotOverlay` keyed by `/Annots` index; the dump consults it
  before the dictionary. The sticky-note and ink rectangle rewrites reach the
  golden through it, which is what took `--annot` off the floor.
- **D16 and D17** (the auto-font-size off-by-one and the `IsPunctuation`
  `<= 0x0094` bug) are transcribed in `vt/classify`; the first waits on the
  layout engine.
- One divergence the brief does not name: `ap/widget`'s regeneration test is
  **narrowed to radio buttons** for the "has an `/AP` that resolves to
  nothing" case. A checkbox in the same shape is left alone, because an
  ungrouped checkbox never becomes a form control and the appearance path
  runs per control. Both halves are pinned by fixtures.
