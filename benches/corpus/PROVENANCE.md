# Benchmark corpus provenance

The 44 PDFs in this directory are copied **verbatim** from the PDFium checkout
that serves as this project's conformance oracle. Nothing is modified: both
sides of every comparison in `docs/status/M12.md` render *these bytes*, so the
criterion column and the `pdfium_test` column describe the same work.

- **Source:** `testing/corpus/` and `testing/resources/` of the PDFium
  repository, at commit `6f2272e1f3aaa141305475b83ef4eac2c1f527b8` (2026-08-28),
  read-only at `$PDFRUM_ORACLE_CHECKOUT` (default `<repo>/../pdfium-c++`).
- **License:** BSD-3-Clause, "Copyright 2014 The PDFium Authors" — see the
  `LICENSE` file at the root of that checkout, and `testing/corpus/LICENSE`,
  which carries the same terms for the corpus half. Redistribution in source
  form is permitted with the copyright notice retained, which this file does.
- **Modifications:** none.
- **Renaming:** each file is stored under a stem that names its class and its
  subject (`text_cjk_page`, `image_jbig2_880920`), because the oracle's own
  names are bug numbers and `example_055.pdf` — unreadable in a benchmark id.
  The `Source` column below is the round trip back to the original.

## Why this corpus replaces the seven in `../fixtures`

`../fixtures` holds the M8 set and is kept: it is what `docs/status/M8.md`'s
tables measured, and deleting it would strand those numbers. But it has a
stated weakness that M12 exists partly to fix — quoting M8.md: *"PDFium's
`testing/resources` are unit-test inputs: the largest is 85 KB, and only two
files exceed two pages. Nothing here resembles a 300-page report."*

This set is chosen against that. The largest file here is 5.0 MB, nine
documents have eight pages or more, and the classes are populated by
*measurement* rather than by file name: every candidate in both of the oracle's
directories (1375 files) was scanned for decompressed operator counts — `Tj`/
`TJ`, path operators, `sh`, `Do` — and then actually rasterized, and the
selection is made on what a file costs and which operator dominates it, not on
what it is called. Two findings from that scan shaped the result and are worth
recording because they contradict the obvious heuristic:

- **File size is nearly uncorrelated with render cost.**
  `FRC_8.5_Screen_Rendition.pdf` is 25 MB and renders in 90 ms — the bulk is an
  embedded video attachment, not page content. `bug_718762.pdf` is **1 KB and
  renders in 1281 ms**: a small JPEG header declaring enormous dimensions. So
  "pick the big files" would have produced a corpus that is slow to clone and
  fast to render, which is exactly backwards. Selection is on measured
  milliseconds.
- **The corpus tops out at sixteen pages.** No file in either directory has
  more, so the rayon scaling curve in M12.md is bounded by a sixteen-page
  document and says so rather than extrapolating.

Two classes are thinner than the rest and this is a limitation, not an
oversight:

- **Pure vector.** Exactly one file in 1375 is path-heavy with no text and no
  images (`fx/path/1751_1.pdf`, 30303 path operators). The rest of the `vector`
  class is *path-dominated* rather than path-pure — embedded-font pages whose
  glyphs are outlines and whose path counts run to 30k — which is the honest
  available shape.
- **CCITT.** Four files in the whole checkout use `CCITTFaxDecode`. Two are
  here; the other two are near-duplicates.

## The set

| File | Class | Pages | Size | `Tj` | paths | imgs | `sh` | widgets | Source |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| `text_bug_1029.pdf` | text | 1 | 53 KB | 1233 | 115 | 0 | 0 | 0 | `resources/bug_1029.pdf` |
| `text_cjk_functions.pdf` | text | 4 | 331 KB | 1011 | 6764 | 0 | 6 | 0 | `corpus/fx/text/zh_function_list.pdf` |
| `text_cjk_page.pdf` | text | 7 | 184 KB | 1227 | 3206 | 0 | 1 | 0 | `corpus/fx/text/zh_page.pdf` |
| `text_cjk_structure.pdf` | text | 7 | 126 KB | 1212 | 1783 | 0 | 1 | 0 | `corpus/fx/text/zh_file_structure.pdf` |
| `text_foxit_products.pdf` | text | 11 | 78 KB | 1075 | 792 | 0 | 1 | 0 | `corpus/fx/text/en_14_foxit_products.pdf` |
| `text_foxittext.pdf` | text | 1 | 61 KB | 64 | 1003 | 0 | 0 | 0 | `corpus/fx/other/foxittext.pdf` |
| `text_quick_start.pdf` | text | 11 | 1.7 MB | 1144 | 25802 | 55 | 7 | 0 | `corpus/fx/text/quick_start_guide.pdf` |
| `text_tcpdf_055.pdf` | text | 14 | 93 KB | 3616 | 8049 | 1 | 0 | 0 | `corpus/third_party/tcpdf/example_055.pdf` |
| `text_tcpdf_063.pdf` | text | 10 | 91 KB | 699 | 2565 | 1 | 0 | 0 | `corpus/third_party/tcpdf/example_063.pdf` |
| `vector_en_system.pdf` | vector | 1 | 624 KB | 26 | 29802 | 62 | 1 | 0 | `corpus/fx/text/en_system.pdf` |
| `vector_en_tem.pdf` | vector | 6 | 629 KB | 489 | 13926 | 2 | 7 | 0 | `corpus/fx/text/en_tem.pdf` |
| `vector_font_feature.pdf` | vector | 10 | 1.5 MB | 934 | 29882 | 0 | 16 | 0 | `corpus/fx/font/font_1_embedded_font_en_feature.pdf` |
| `vector_font_size14.pdf` | vector | 9 | 1.4 MB | 921 | 29571 | 0 | 15 | 0 | `corpus/fx/font/font_2_embedded_font_en_size14.pdf` |
| `vector_paths_1751.pdf` | vector | 1 | 92 KB | 0 | 30303 | 0 | 0 | 0 | `corpus/fx/path/1751_1.pdf` |
| `vector_tcpdf_009.pdf` | vector | 1 | 587 KB | 19 | 11056 | 3 | 4 | 0 | `corpus/third_party/tcpdf/example_009.pdf` |
| `image_bug_583804.pdf` | image | 1 | 123 KB | 0 | 0 | 1 | 0 | 0 | `corpus/pdfium/bug_583804.pdf` |
| `image_bug_718762.pdf` | image | 1 | 2 KB | 0 | 0 | 1 | 0 | 0 | `resources/pixel/bug_718762.pdf` |
| `image_bug_898443.pdf` | image | 1 | 2.1 MB | 0 | 48 | 1 | 0 | 0 | `corpus/pdfium/bug_898443.pdf` |
| `image_ccitt_3bigpreview.pdf` | image | 1 | 318 KB | 525 | 2666 | 8 | 13 | 0 | `corpus/fx/other/3bigpreview.pdf` |
| `image_ccitt_transfer.pdf` | image | 2 | 3 KB | 0 | 3 | 5 | 0 | 0 | `resources/pixel/transfer_function.pdf` |
| `image_en_fqa.pdf` | image | 4 | 964 KB | 43 | 7850 | 552 | 8 | 0 | `corpus/fx/text/en_fqa.pdf` |
| `image_jbig2_1478366.pdf` | image | 1 | 1 KB | 0 | 0 | 1 | 0 | 0 | `resources/pixel/bug_1478366.pdf` |
| `image_jbig2_880920.pdf` | image | 1 | 10 KB | 4 | 3 | 1 | 0 | 0 | `corpus/pdfium/bug_880920.pdf` |
| `image_jpx_123.pdf` | image | 1 | 336 KB | 30 | 7905 | 1 | 5 | 0 | `corpus/fx/action/123.pdf` |
| `shading_axial_radial.pdf` | shading | 1 | 17 KB | 0 | 33 | 0 | 33 | 0 | `resources/pixel/shade.pdf` |
| `shading_coons.pdf` | shading | 1 | 1 KB | 1 | 1 | 0 | 0 | 0 | `corpus/pdfium/bug_481_coons_patch.pdf` |
| `shading_gouraud.pdf` | shading | 1 | 1 KB | 1 | 1 | 0 | 0 | 0 | `corpus/pdfium/bug_481_free_form_gouraud.pdf` |
| `shading_tcpdf_030.pdf` | shading | 2 | 28 KB | 12 | 172 | 1 | 5 | 0 | `corpus/third_party/tcpdf/example_030.pdf` |
| `shading_tcpdf_056.pdf` | shading | 1 | 43 KB | 6 | 566 | 1 | 16 | 0 | `corpus/third_party/tcpdf/example_056.pdf` |
| `shading_tcpdf_058.pdf` | shading | 1 | 79 KB | 9 | 631 | 2 | 24 | 0 | `corpus/third_party/tcpdf/example_058.pdf` |
| `shading_tensor.pdf` | shading | 1 | 7 KB | 0 | 13 | 0 | 13 | 0 | `resources/pixel/shade-tensor.pdf` |
| `shading_type4_5.pdf` | shading | 1 | 1 KB | 0 | 2 | 0 | 1 | 0 | `corpus/fx/shading/2_shading_type5_h.pdf` |
| `forms_combo_box.pdf` | forms | 2 | 464 KB | 230 | 6461 | 0 | 4 | 31 | `corpus/fx/form/combo_box.pdf` |
| `forms_list_box.pdf` | forms | 4 | 632 KB | 330 | 8330 | 0 | 8 | 33 | `corpus/fx/form/list_box.pdf` |
| `forms_number.pdf` | forms | 1 | 94 KB | 26 | 4 | 0 | 0 | 60 | `corpus/fx/form/number.pdf` |
| `forms_push_button.pdf` | forms | 4 | 1.7 MB | 238 | 33111 | 6 | 32 | 28 | `corpus/fx/form/push_button.pdf` |
| `forms_signature.pdf` | forms | 2 | 84 KB | 153 | 825 | 0 | 1 | 50 | `corpus/fx/form/signature.pdf` |
| `forms_text_field.pdf` | forms | 3 | 446 KB | 285 | 5831 | 0 | 5 | 37 | `corpus/fx/form/text_field.pdf` |
| `forms_widgets_407.pdf` | forms | 5 | 3.2 MB | 945 | 1461 | 0 | 0 | 407 | `corpus/fx/js/widget_javascript.pdf` |
| `mixed_en_uicase.pdf` | mixed | 12 | 250 KB | 304 | 5331 | 0 | 7 | 0 | `corpus/fx/text/en_uicase_.pdf` |
| `mixed_formfield.pdf` | mixed | 1 | 4.9 MB | 256 | 64363 | 3 | 63 | 0 | `corpus/fx/form/formfield.pdf` |
| `mixed_tcpdf_006.pdf` | mixed | 6 | 292 KB | 884 | 5327 | 3 | 4 | 0 | `corpus/third_party/tcpdf/example_006.pdf` |
| `mixed_tcpdf_045.pdf` | mixed | 16 | 192 KB | 71 | 4767 | 1 | 2 | 0 | `corpus/third_party/tcpdf/example_045.pdf` |
| `mixed_tcpdf_059.pdf` | mixed | 16 | 51 KB | 70 | 237 | 1 | 0 | 0 | `corpus/third_party/tcpdf/example_059.pdf` |

## What each class is for, and the file that anchors it

- **text** (9 files) — glyph-dominated pages. `text_tcpdf_055` draws 3616 show-
  text operations over 14 pages and is the heaviest; the three `text_cjk_*`
  files carry CJK, where glyph lookup rather than glyph count is what costs;
  `text_foxittext` is carried over from `../fixtures` unchanged so the wave-7b
  glyph-cache numbers in `docs/status/pdfrum-render.md` stay comparable.
- **vector** (6 files) — path-dominated. `vector_paths_1751` is the only
  genuinely pure one in the checkout (30303 path operators, zero text, zero
  images) and is therefore the control for anything touching the scanline
  integrator.
- **image** (9 files) — decode and resample. Deliberately spread across the
  codecs, because they share no code: `image_jpx_123` is JPEG 2000,
  `image_jbig2_*` are JBIG2, `image_ccitt_*` are CCITT G3/G4, and the three
  `image_bug_*` files are the pathological JPEG cases — small files that decode
  enormous images, which is where the resample loop dominates everything else.
- **shading** (8 files) — every shading type that has a distinct evaluator:
  axial and radial (`shading_axial_radial`, 33 of them on one page),
  function-based, free-form Gouraud (`shading_gouraud`), Coons patch
  (`shading_coons`), tensor (`shading_tensor`), and lattice-form
  (`shading_type4_5`).
- **forms** (7 files) — the widget-appearance path, which is a different code
  path from page content: it generates appearance streams and draws them into
  the page graph. `forms_widgets_407` carries 407 widgets across 5 pages and is
  the one that makes that path visible against everything else on the page.
- **mixed** (5 files) — no dominant cost. `mixed_tcpdf_045` and
  `mixed_tcpdf_059` are both sixteen pages, the longest documents available,
  and are what the rayon scaling curve is measured on.
