# Fixture provenance

The PDFs in this directory are copied verbatim from the PDFium
checkout that serves as this project's conformance oracle:

- **Source:** `testing/resources/` of the PDFium repository, at commit
  `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28).
- **License:** BSD-3-Clause, "Copyright 2014 The PDFium Authors" — see the
  `LICENSE` file at the root of that checkout. Redistribution in source form
  is permitted with the copyright notice retained, which this file does.
- **Modifications:** none. The bytes are unchanged, deliberately: they are
  the same inputs the oracle renders, so a doctest's expectations and a
  conformance golden describe the same file.

| File | Size | What it exercises |
|---|---:|---|
| `hello_world.pdf` | 840 B | A 200x200 page, two standard Type1 fonts, two `Tj` strings. The canonical smoke test for loading, rendering and text extraction. Note its content stream deliberately carries **no `/Length`**, so it also exercises the parser's stream-length recovery. |
| `text_form.pdf` | 932 B | A 300x300 page with an AcroForm holding one `/Tx` (text) field named `Text Box`, present both in `/AcroForm /Fields` and as a `/Widget` annotation in the page's `/Annots`. Drives field enumeration, reading, filling and appearance regeneration. |
| `bookmarks.pdf` | 2239 B | Two 612x792 pages and a four-entry outline tree two levels deep, mixing `/Dest` names, an explicit `/XYZ` destination array and a `/URI` action. Drives the outline walk, page-count and multi-page paths. |

Those three were chosen to be the smallest files that between them reach
every read-side facade path, so the doctests stay readable and the crate
stays small.

Four more arrived with page mutation. Each is the file the corresponding
PDFium embedder test uses, so an assertion here and an assertion there are
about the same bytes:

| File | Size | What it exercises |
|---|---:|---|
| `split_streams.pdf` | 3502 B | Nineteen page objects across **three** `/Contents` elements — 15, 3 and 1. The only fixture that distinguishes a content-stream index from an object index, and the one `RemoveAllFromStream` uses to pin the index collapse. |
| `hello_world_split_streams.pdf` | 922 B | Three text objects across **two** elements, 2 and 1. The small case: enough to tell "the element still has a sibling" from "the element is now empty", which are the two sides of the array-shrink rule. |
| `rectangles.pdf` | 633 B | Eight path objects on one page, one element. The visibility and transform tests' fixture: eight distinguishable shapes with no fonts to complicate the resource sweep. |
| `hello_world_2_pages.pdf` | 962 B | Two pages **sharing one content stream and one resource dictionary**. Editing one page must copy rather than rewrite, and this is the file that fails if it does not. |

One more arrived with the decode target (M12b P1). It is the only fixture here
that is **assembled** rather than taken whole from the oracle, because no file
in the oracle's corpus draws one image twice at two very different sizes, and
that is precisely the shape the decode target has to be tested against:

| File | Size | What it exercises |
|---|---:|---|
| `jpx_two_sizes.pdf` | 49 KB | A 600x800 page drawing **one** 1269x1643 JPEG 2000 `XObject` twice — once at 500x640 device points and once as a 40x50 thumbnail. The image is the `/JPXDecode` stream of `corpus/fx/action/123.pdf`, copied byte for byte into a hand-written six-object PDF; the wrapper is ours, the codestream is the oracle's. It is the only fixture that can fail when a reduced decode is handed back to a full-resolution draw, since every other one has either no image or no second size. `pdfium_test` renders it, which is what makes it usable as an oracle comparison rather than only as a self-consistency check. |
| `annotiter.pdf` | 2136 B | Three 612x792 pages that **share one field's four widget kids** and differ only in `/Tabs` — `R`, `C`, `S` — so the same four annotations traverse in three different orders. The kids sit at the corners of a square in an annotation order that is none of the three traversal orders (`Sub_LeftBottom` first, then `RightTop`, `LeftTop`, `RightBottom`), which is what makes a wrong tab implementation unable to agree by accident. It is also the only fixture where focus has to be document-wide rather than per-page, since all three pages list the same annotations. |
| `combobox_form.pdf` | 1738 B | Three combo boxes on one page that differ only in `/Ff`: `Combo_Editable` (393216 — combo plus edit), `Combo1` (131072 — combo, gated, `/V (Banana)` over 26 fruit) and `Combo_ReadOnly` (131073 — the same plus read-only). The three make one point three ways: typing inserts in the first, jumps between options in the second, and cannot reach the third at all, because a read-only widget is not clickable. |
| `listbox_form.pdf` | 3012 B | Six list boxes covering the selection-source matrix — single-select, multi-select, `/I`-only, `/V`-only, `/I` and `/V` disagreeing, and a scrolled `/TI`. It is the fixture the "`/V` wins over `/I`" rule is stated against, and the only one whose lists are longer than their boxes. |
| `click_form.pdf` | 1.1 KB | A read-only check box that starts **checked** (`/AS /Yes`, `/Ff 1`), an ordinary one that starts clear, and a read-only radio group. The read-only pair is the point: it is the only fixture that can show the two halves of read-only pulling in opposite directions — the widget refuses a *click* outright, while a *keystroke* that reaches it by tabbing is consumed and then ignored. |
| `annots_action_handling.pdf` | 2.5 KB | Two widgets followed by four `/Link` annotations, the first carrying `/A << /S /URI /URI (https://cs.chromium.org/) >>`. It is the fixture for actions arriving as **requests** rather than as callbacks, and for the rule that the focus ring admits links only when a caller asks it to. |
| `substituted_da_font.pdf` | 812 B | `testing/resources/pixel/form_textfield_focused_ltr.in` expanded by `testing/tools/fixup_pdf_template.py`, byte for byte. One `/Tx` widget on a 200x100 page, `/Rect [50 40 150 70]`, `/DA (/Arial 12 Tf 0 0 0 rg)` over a `/DR` declaring `/Arial` as a bare `/Type /Font /Subtype /TrueType /BaseFont /Arial` — **nothing embedded**, so which face lays the field out is entirely the substitution's choice. That is what makes it the fixture for the question `FormSession::with_context` exists to answer: under the hermetic `test_fonts` set `/Arial` becomes Arimo (905/−211, a 13.392-unit caret at 12pt) and under a default context it falls through to the built-in base-14 Helvetica (718/−219, 11.244). It is also the fixture for the second face, since its Ansi `/DA` font cannot write a single Hebrew code point. |

One arrived with the `script` feature (WP12). It is the only fixture here that
is not read by a default build: `tests/form_scripts.rs` is behind
`#[cfg(feature = "script")]`, so a `cargo test -p pdfrum` never opens it.

| File | Size | What it exercises |
|---|---:|---|
| `public_methods.pdf` | 30 KB | `testing/resources/javascript/public_methods.pdf`, verbatim. One `/Tx` field named `Text Box` whose `/AA` carries **all four hooks** — `/C`, `/F`, `/K` and `/V` — which makes it the one file in the oracle's corpus that exercises every wire `FormSession::with_scripts` installs. Its `/AA /K` opens with an `app.alert` naming itself, and that line is what the seam test keys on: it is produced by the document's own JavaScript and by nothing else. It is by far the largest fixture here, and that is the trade — every smaller `/AA` fixture in the corpus either carries one hook or needs the `Doc`/`Field` object model M15 has not built (`this.getField(…)`), and a fixture whose script cannot run proves nothing about a seam. |

One arrived with WP1's `Error::WrongPassword`
(`docs/design/idiomatic-api.md` §WP1), because a doctest that shows a caller
matching that variant needs a file that produces it, and until now none of
these was encrypted:

| File | Size | What it exercises |
|---|---:|---|
| `encrypted.pdf` | 10564 B | A one-page linearized 1.6 document under the standard handler at revision 4 (`/V 4`, `/CFM /AESV2`), `/P -3392`, opening with the user password `1234` or the owner password `5678` and refusing everything else. `/P` grants exactly one of ISO 32000-1 table 22's eight named bits — bit 10, accessibility extraction — so it is also the only fixture where [`Document::permissions`] is not `Permissions::ALL`, and the only one where the owner's view and the user's differ. |

One arrived with `SaveOptions::subset_new_fonts` being wired
(`docs/design/pdfrum-edit.md` §1.7). The option only ever fires on fonts a
save writes as **new**, so its test imports a page carrying an embedded CID
TrueType font into `hello_world.pdf`; nothing already in this directory has
one:

| File | Size | What it exercises |
|---|---:|---|
| `roboto.ttf` | 35636 B | `testing/resources/fonts/roboto.ttf`, verbatim. The TrueType program `latin_extended.pdf` embeds; used here as the caller-supplied bytes for `DocEdit::embed_font`, so a test can write "Hello" in a font the page did not already have. |
| `latin_extended.pdf` | 19213 B | `testing/resources/latin_extended.pdf`, verbatim. One 200x200 page drawing Latin Extended through a `/Type0` Identity-H font whose descendant is a `CIDFontType2` `Roboto-Regular` with a 35636-byte embedded `/FontFile2`, a `/W` array, a `/ToUnicode` CMap and `/CIDToGIDMap /Identity`. Every one of those is load-bearing: `/BaseFont` carries **no** subset tag, so a minted one is visible; `/W` and `/ToUnicode` are what the subsetting stage must leave untouched; and the `/Identity` map is what it replaces with the table that absorbs the subsetter's glyph renumbering. Its glyph coverage is also near the worst case for the saving — the page draws most of Roboto's Latin — which is what makes the recorded 35636 → 20424 a floor rather than a flattering number. |

Two arrived with the `FPDFText_LoadFont` embedder-test port
(`crates/pdfrum/tests/load_font.rs`). Both are the regression fonts of the
PDFium bugs they are named for, so the assertion here and the assertion there
are about the same bytes:

| File | Size | What it exercises |
|---|---:|---|
| `bug_2094.ttf` | 1288 B | `testing/resources/fonts/bug_2094.ttf`, verbatim. The smallest font in the corpus that is **degenerate in every direction at once**: it names itself `Test`, its cmap maps nothing, and it declares four glyphs. That combination is the bug — `FPDFEditEmbedderTest.Bug2094` crashed building a CID font from it — and it is why no other fixture substitutes. Because every character encodes to `.notdef`, it is also the only fixture that pins `char_maps`'s empty-cmap fallback and the single-run `/W` (`[0 [1000 0 1000 1000]]`) that follows from it. |
| `bug_377948405.ttf` | 2084 B | `testing/resources/fonts/bug_377948405.ttf`, verbatim. A `NotoSans-Regular` cut whose cmap covers exactly six characters — `A À Ä Å Æ È` — laid out so their advances are `639`, then `639 639 639`, then `881 556`. That run of three equal widths is the fixture's whole point: it is the shortest input on which `create_widths_array`'s run-length form (`5 7 639`) differs from the naive one, which is what `FPDFEditEmbedderTest.Bug377948405` regressed on. The narrow coverage also makes it the second, genuinely *different* face in the two-font subsetting test, where Roboto's near-complete Latin would not distinguish one subset from another. |

Neither is duplicated for the Type 1 cases: `load_font.rs` reaches across to
`crates/pdfrum-type1/tests/fixtures/FoxitSerifMM.pfb` rather than copy a
113 KB program into this directory. Its provenance is recorded beside it.

