# `CPDF_ColorSpace::EnableStdConversion` can no longer affect any rendered pixel

**Status:** drafted 2026-09-03, not yet filed. Low priority — a dead-code
finding, not a rendering defect.

**Confidence:** code-level, verified by a null result: a port that wired the
flag through to its consumer moved zero of 1757 corpus renders.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js has no equivalent mode; images inside
transparency groups convert colour the same way as at top level.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## `CPDF_ColorSpace::EnableStdConversion` can no longer affect any rendered pixel

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render any page containing a `DeviceCMYK` image inside a transparency
   group, soft mask, or pattern cell (any path where a `CPDF_RenderStatus`
   has `SetStdCS(true)`).
3. Compare with a build where `CPDF_DeviceCS::GetRGB` and
   `TranslateImageLine` have their `IsStdConversionEnabled()` branches
   removed.

**What is the expected output? What do you see instead?**

Expected, if the mechanism were live: images drawn offscreen would use the
"standard" (unclamped, naive) CMYK-to-RGB formula rather than the Adobe
CMYK table. Observed: identical output either way. The flag is plumbed
through `CPDF_RenderStatus`, `CPDF_ImageRenderer`, `CPDF_ImageLoader` and
`CPDF_Image` into `CPDF_DIB::std_cs_`, but no conversion that reaches a
pixel runs while the counter is raised.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

The only readers of the counter are the two `kDeviceCMYK` arms in
`core/fpdfapi/page/cpdf_devicecs.cpp:65` (`GetRGB`) and `:119`
(`TranslateImageLine`); `CPDF_BasedCS` merely forwards it. The counter is
raised in `CPDF_DIB::ContinueToLoadMask` (`core/fpdfapi/page/cpdf_dib.cpp:153`)
and lowered at `:244`, `:322` and `:857`. But:

- `StartLoadDIBBase` (`:199`) runs `LoadInternal`, including `LoadPalette`
  (`:184`), and `CreateDecoder` **before** `ContinueToLoadMask`, so an
  Indexed palette over CMYK is built with the counter at zero.
- `TranslateImageLine` is reached only from `TranslateScanline24bpp`
  (`:1007`), called only from `CPDF_DIB::GetScanline` (`:1129`), a `const`
  accessor over `mutable` buffers that the rasterizer pulls during
  compositing, long after the load has finished and the counter is back to
  zero. The `:119` check is therefore always false.
- The one conversion left inside the window is the `/Matte` colour at
  `:839`, so the only observable case would be a `DeviceCMYK` image whose
  `/SMask` carries `/Matte`, drawn offscreen. No file under
  `testing/corpus` or `testing/resources` pairs `/Matte` with `DeviceCMYK`.

This looks like a mechanism whose consumer was hollowed out when scanline
conversion became lazy. Either the `SetStdCS` plumbing and the two
`IsStdConversionEnabled()` branches can be deleted, or — if the
offscreen-conversion behaviour is still intended — the counter needs to be
consulted where `GetScanline` translates. A pure-Rust reimplementation that
carried the flag to the same five sites and then threaded it to the
translate path measured no change across 1757 renders, which is what
prompted this report.
