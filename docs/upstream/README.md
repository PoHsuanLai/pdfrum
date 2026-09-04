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

Re-checked 2026-09-05 against upstream `a043bed4a`: five of the eight were
fixed on 2026-09-03, each commit carrying the tracker id; the three others
are open with their cited code unchanged.

| Issue | ID | Confidence | Upstream |
|---|---|---|---|
| [`IsLeapYear` century exception inverted](pdfium/leap-year-century-inverted.md) | [555821585](https://crbug.com/pdfium/555821585) | observed output | fixed `c60c84f5b` |
| [Two-digit years all map to 2000–2099](pdfium/two-digit-year-window.md) | [555821586](https://crbug.com/pdfium/555821586) | observed output | fixed `017295ab7` |
| [`util.scand` mis-parses 12-hour times](pdfium/scand-12-hour-parse.md) | [555848528](https://crbug.com/pdfium/555848528) | observed output | open |
| [`util.printd` prints every weekday as Sunday](pdfium/printd-weekday-always-sunday.md) | [555848529](https://crbug.com/pdfium/555848529) | observed output | fixed `b39aa638b` |
| [`AFSimple("avg", …)` returns the sum](pdfium/afsimple-avg-case-split.md) | [555848530](https://crbug.com/pdfium/555848530) | observed output | open |
| [Every character above U+FFFF mirrors to `)`](pdfium/mirror-char-sentinel-collision.md) | [555940408](https://crbug.com/pdfium/555940408) | code-level, clear repro | open |
| [`/TR` array loaded in reverse](pdfium/tr-array-loaded-reversed.md) | [555940409](https://crbug.com/pdfium/555940409) | code-level | fixed `2621636d3` |
| [`/TR` samples stored unclamped](pdfium/tr-samples-unclamped.md) | [555967331](https://crbug.com/pdfium/555967331) | code-level | fixed `8794e9c27` |

## PDFium — drafted, not filed

Every draft re-verified at `a043bed4a` on 2026-09-05 (the cited code is
unchanged; line shifts are noted in each file's header).

| Issue | Confidence | Note |
|---|---|---|
| [Single-function `/TR` over 16 outputs renders all black](pdfium/tr-single-function-unwritten-output.md) | **observed output** (repro files added 2026-09-05; was code-level) | The two neighbouring `/TR` fixes of 2026-09-03 left this branch as it was; the strongest of the five |
| [Text-object bbox gate drops standalone spaces](pdfium/text-object-bbox-gate-drops-spaces.md) | observed output | Root cause behind crbug.com/40643656 and crbug.com/444176962; link them |
| [Shading ramp skew and radial truncation](pdfium/shading-ramp-and-radial-truncation.md) | code-level, contrived trigger | Real but low-salience: one part in 256 |
| [`IsPunctuation` range typo](pdfium/ispunctuation-range-typo.md) | code-level, arguable scope | The `<=` is a typo; the cp1252-vs-Unicode question underneath is not |
| [`EnableStdConversion` never reaches a pixel](pdfium/std-conversion-never-reaches-a-pixel.md) | code-level, null result over 1757 renders | Dead mechanism, not a defect; low priority |

## Other projects — drafted, not filed

| Issue | Project | Re-checked 2026-09-05 |
|---|---|---|
| [Truncated JBIG2: segment bodies read at declared length](hayro/jbig2-segment-lengths.md) | `hayro-jbig2` | 0.3.0 still latest; no upstream issue covers it — file as drafted |
| [`scale_denom`: decode at 1/2, 1/4 or 1/8 inside the IDCT](zune/scaled-decode.md) | `zune-jpeg` | **Already asked for** as zune-image #434 (2026-08-18, open); post ours as a comment there with the measurements. PDFium itself moved to zune-jpeg on 2026-09-03 and box-averages after a full decode, which the draft now says |

## Withdrawn

Nothing withdrawn yet. One claim was **removed before filing**: the divergence
audit asserted that PDFium treats any Adobe marker as "transformed" and ignores
the transform byte. That is a misreading — the byte is honoured one layer down
in libjpeg (`jdapimin.c:185-198`), and pdf.js and `zune-jpeg` agree. It never
reached a draft; recorded here so it is not rediscovered and refiled.
