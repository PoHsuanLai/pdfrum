# Upstream issues

Defects and feature requests that belong to a project other than this one,
found while building pdfrum against the same specification.

**One file per issue.** Each file is self-contained: a short header (status,
confidence, repro files, versions to quote), then a horizontal rule, then the
issue text ready to paste. Nothing above the rule is meant to be posted.
Reproduction inputs live in `repro/`, shared across issues.

**Layout.** `pdfium/` for PDFium (crbug.com/pdfium), `zune/` for `zune-jpeg`,
`hayro/` for `hayro-jbig2` — one directory per upstream project.

**A filed issue keeps its file.** The header records the tracker ID and the
date; the text stays as filed so a later reader can see what was reported
without opening the tracker. Corrections go in the file, in place — never by
deletion.

## PDFium — filed 2026-09-02

| Issue | ID | Confidence |
|---|---|---|
| [`IsLeapYear` century exception inverted](pdfium/leap-year-century-inverted.md) | [555821585](https://crbug.com/pdfium/555821585) | observed output |
| [Two-digit years all map to 2000–2099](pdfium/two-digit-year-window.md) | [555821586](https://crbug.com/pdfium/555821586) | observed output |
| [`util.scand` mis-parses 12-hour times](pdfium/scand-12-hour-parse.md) | [555848528](https://crbug.com/pdfium/555848528) | observed output |
| [`util.printd` prints every weekday as Sunday](pdfium/printd-weekday-always-sunday.md) | [555848529](https://crbug.com/pdfium/555848529) | observed output |
| [`AFSimple("avg", …)` returns the sum](pdfium/afsimple-avg-case-split.md) | [555848530](https://crbug.com/pdfium/555848530) | observed output |
| [Every character above U+FFFF mirrors to `)`](pdfium/mirror-char-sentinel-collision.md) | [555940408](https://crbug.com/pdfium/555940408) | code-level, clear repro |
| [`/TR` array loaded in reverse](pdfium/tr-array-loaded-reversed.md) | [555940409](https://crbug.com/pdfium/555940409) | code-level |
| [`/TR` samples stored unclamped](pdfium/tr-samples-unclamped.md) | [555967331](https://crbug.com/pdfium/555967331) | code-level |

## PDFium — drafted, not filed

| Issue | Confidence | Note |
|---|---|---|
| [Single-function `/TR` over 16 outputs renders all black](pdfium/tr-single-function-unwritten-output.md) | code-level | Third of the three `/TR` defects; the other two are filed |
| [Text-object bbox gate drops standalone spaces](pdfium/text-object-bbox-gate-drops-spaces.md) | observed output | Root cause behind crbug.com/40643656 and crbug.com/444176962; link them |
| [Shading ramp skew and radial truncation](pdfium/shading-ramp-and-radial-truncation.md) | code-level, contrived trigger | Real but low-salience: one part in 256 |
| [`IsPunctuation` range typo](pdfium/ispunctuation-range-typo.md) | code-level, arguable scope | The `<=` is a typo; the cp1252-vs-Unicode question underneath is not |
| [`EnableStdConversion` never reaches a pixel](pdfium/std-conversion-never-reaches-a-pixel.md) | code-level, null result over 1757 renders | Dead mechanism, not a defect; low priority |

## Other projects — drafted, not filed

| Issue | Project |
|---|---|
| [Truncated JBIG2: segment bodies read at declared length](hayro/jbig2-segment-lengths.md) | `hayro-jbig2` |
| [`scale_denom`: decode at 1/2, 1/4 or 1/8 inside the IDCT](zune/scaled-decode.md) | `zune-jpeg` |

## Withdrawn

Nothing withdrawn yet. One claim was **removed before filing**: the divergence
audit asserted that PDFium treats any Adobe marker as "transformed" and ignores
the transform byte. That is a misreading — the byte is honoured one layer down
in libjpeg (`jdapimin.c:185-198`), and pdf.js and `zune-jpeg` agree. It never
reached a draft; recorded here so it is not rediscovered and refiled.
