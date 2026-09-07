# Upstream issues

Bugs and requests for other projects, found while building pdfrum.

One file per issue: a header (status, confidence, repros, versions), a
rule, then text ready to paste. Nothing above the rule is posted. Inputs
live in `repro/`. A filed issue keeps its file; the header records the
tracker id.

## PDFium — filed 2026-09-02

Re-checked 2026-09-05 at `a043bed4a`.

| Issue | ID | Upstream |
|---|---|---|
| [`IsLeapYear` century exception inverted](pdfium/leap-year-century-inverted.md) | [555821585](https://crbug.com/555821585) | fixed `c60c84f5b` |
| [Two-digit years map to 2000–2099](pdfium/two-digit-year-window.md) | [555821586](https://crbug.com/555821586) | fixed `017295ab7` |
| [`util.scand` 12-hour parse](pdfium/scand-12-hour-parse.md) | [555848528](https://crbug.com/555848528) | open |
| [`util.printd` weekday always Sunday](pdfium/printd-weekday-always-sunday.md) | [555848529](https://crbug.com/555848529) | fixed `b39aa638b` |
| [`AFSimple("avg")` returns the sum](pdfium/afsimple-avg-case-split.md) | [555848530](https://crbug.com/555848530) | open |
| [U+FFFF+ mirrors to `)`](pdfium/mirror-char-sentinel-collision.md) | [555940408](https://crbug.com/555940408) | open |
| [`/TR` array loaded reversed](pdfium/tr-array-loaded-reversed.md) | [555940409](https://crbug.com/555940409) | fixed `2621636d3` |
| [`/TR` samples unclamped](pdfium/tr-samples-unclamped.md) | [555967331](https://crbug.com/555967331) | fixed `8794e9c27` |

## Drafted, not filed

| Issue | Project |
|---|---|
| [Single-function `/TR` over 16 outputs renders black](pdfium/tr-single-function-unwritten-output.md) | PDFium |
| [Empty-box text gate drops letters](pdfium/text-object-bbox-gate-drops-spaces.md) | PDFium |
| [Shading ramp skew / radial truncation](pdfium/shading-ramp-and-radial-truncation.md) | PDFium |
| [`IsPunctuation` range typo](pdfium/ispunctuation-range-typo.md) | PDFium |
| [`EnableStdConversion` never reaches a pixel](pdfium/std-conversion-never-reaches-a-pixel.md) | PDFium |
| [Bilinear complement darkens transformed images by two levels](pdfium/image-transformer-off-by-one-complement.md) | PDFium |
| [`F32Kernel::pack`/`unpack` are scalar](vello/pack-unpack-simd.md) | [`vello_cpu`](https://crates.io/crates/vello_cpu) |
| [JBIG2 segment bodies read at declared length](hayro/jbig2-segment-lengths.md) | [`hayro-jbig2`](https://crates.io/crates/hayro-jbig2) |
| [`scale_denom` inside the IDCT](zune/scaled-decode.md) | [`zune-jpeg`](https://crates.io/crates/zune-jpeg) (#434 already open) |
