---
name: Bug report
about: Something pdfrum does that it should not
title: ''
labels: bug
assignees: ''
---

## What happened

## What you expected

If the expectation comes from another engine or from the specification, say
which — a difference from PDFium is treated differently from a difference from
ISO 32000.

## The file

A PDF that reproduces it, attached, is worth more than any description. If the
file cannot be shared, say what it contains: the filter chain, the font types,
whether it is encrypted, whether the cross-reference table is damaged.

## Reproducing

```
pdfrum ... file.pdf
```

Include the exact command and the output. If it is a rendering difference,
attach both images.

## Environment

- pdfrum version or commit:
- Rust version (`rustc -V`):
- Platform:
- Features enabled, if not the defaults:
- Render backend, if it matters:
