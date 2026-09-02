# Unwired oracle ports

**Opened:** 2026-09-03 · **State:** open, five items

The no-dead-code pass (`chore/no-dead-code`) went through every
`#[allow(dead_code)]` the idiomatic-API curation left behind. Most were test
helpers that had escaped into library scope, and they were gated under
`#[cfg(test)]` or deleted. Six were not: they are ports of behaviour the
oracle runs on a **production** path, and the port has no production caller
because *the feature around it was never wired*. Deleting them would throw
away a checked, tested spec for a gap that would then be invisible. The
stroked-text CTM split has since been wired; five remain.

They stay in the tree with `#[allow(dead_code, reason = "unwired — see
docs/status/unwired-oracle-ports.md")]` until each is either wired or
consciously declined. This file is the "see" they point at.

## The rule that put them here

The pass's test was: *does the oracle call the C++ function this ports on a
production path, and does pdfrum reach the same behaviour another way?* Both
questions had to be answered from the oracle checkout, not from the port's own
doc comment. Four items looked like this shape and turned out **not** to be —
they are recorded at the bottom so nobody re-opens them.

## 1. `image::scanline::rgb_line_to_bgr` — the 16-bpc high-byte arm

| | |
|---|---|
| Item | `crates/pdfrum-page/src/image/scanline.rs`, with `scale_to_byte` and `write` |
| Oracle | `CPDF_DIB::TranslateScanline24bppDefaultDecode`, `core/fpdfapi/page/cpdf_dib.cpp:1056-1127` |
| Oracle production? | **Yes** — `cpdf_dib.cpp:1014` (`TranslateScanline24bpp`) ← `:1273` (`GetScanline`) |
| pdfrum's live path | the general `DecodeMap` route, `crates/pdfrum-page/src/image/mod.rs:856-882` |

The oracle's default-decode RGB fast path splits three ways on `bpc`. The
`1|2|4` arm scales with integer `min(v,max) * 255 / max`; **that arm is not a
gap** — the general path's `(base + step*raw).clamp(0,1) * 255.0 as u8` was
checked against it exhaustively over every raw value at bpc 1, 2 and 4 and is
bit-identical, because `step` is exactly `1/max` and the products are small
enough to be exact in `f32`.

The **`bpc == 16` arm is the gap.** The oracle takes the *high byte only*
(`cpdf_dib.cpp:1094-1099`: `*dest_pos++ = src_pos[4]`), discarding the low byte
with no rounding. pdfrum's general path reads the whole 16-bit sample through
`get_bits` and float-scales it, which lands one count low for 767 of the 65 536
possible samples — every value whose low byte is nonzero and whose scaled
product falls just under the next integer. `rgb_line_to_bgr`'s `16` arm is the
oracle's rule, ported and tested (`sixteen_bit_rgb_keeps_only_the_high_byte`),
and nothing calls it.

**Blast radius:** ±1 count on 16-bit RGB images with a default `/Decode` —
rare in the corpus (the board does not move), invisible to Tier B's threshold,
but a per-file SSIM difference wherever such an image exists.

## 2. `image::scanline::palette_index` — multi-component packed palettes

| | |
|---|---|
| Item | `crates/pdfrum-page/src/image/scanline.rs` |
| Oracle | the packed-index loop in `CPDF_DIB::GetScanline`, `core/fpdfapi/page/cpdf_dib.cpp:1200-1209`, inverse of `LoadPalette`'s enumeration at `:957-967` |
| Oracle production? | **Yes** — inside `GetScanline`, on the render path |
| pdfrum's live path | none |

The oracle builds a palette whenever `bpc * components <= 8`, so a 2-bit
3-component or 4-bit 2-component `DeviceN` image is drawn through a lookup
table whose index packs the components with the **first in the low bits**.
pdfrum palettizes only `ColorSpace::Indexed` (`image/mod.rs:822`, one
component) and widens every other space's components to a byte each. The
packing rule is ported and tested here; the palette path it belongs to is not
built.

**Blast radius:** whole-image, for the narrow class of low-bpc multi-component
images. None are in the corpus.

## 3-5. `image::dct::{scale_denominator, scaled_size, allows_reduced_resolution}`

| | |
|---|---|
| Items | `crates/pdfrum-page/src/image/dct.rs` |
| Oracle | `1 << min(levels, 3)` at `core/fpdfapi/page/cpdf_dib.cpp:531-532`; `ScaledJpegSize`, `core/fxcodec/jpeg/libjpeg_scanline_decoder.cpp:44-46`; the MCU-alignment guard at `libjpeg_scanline_decoder.cpp:150-156` |
| Oracle production? | **Yes, all three** — `cpdf_dib.cpp:535, :566, :623` (every `CreateDCTDecoder` path) and `libjpeg_scanline_decoder.cpp:157, :226` |
| pdfrum's live path | none |

These three are one contract: how far a JPEG may be decoded at reduced
resolution, what size that yields, and when the MCU grid forbids it. pdfrum
implements reduced-resolution decoding for **JPX only** — `decode_jpx` takes a
`RequestedSize` (`image/jpx.rs:287`, wired at `image/mod.rs:313`) while
`decode_dct` takes only `(data, declared)` and is called at
`image/mod.rs:404` with no size at all.

So the whole reduced-DCT feature is missing and these are its spec, already
ported and pinned. `image/cache.rs:437`'s own test comment mentions "a JPEG
whose MCUs are not aligned", which is the cache having been designed expecting
this.

**Blast radius:** performance, not pixels — a large JPEG drawn small is decoded
at full resolution and downsampled, which costs time and memory rather than
correctness. Wiring it would change output only where libjpeg's reduced decode
differs from a full decode plus our own scaler, which is exactly the
divergence the oracle accepts.

## Checked and *not* a gap

Recorded so the question is not re-opened:

- **`inline_image::inline_filters`** — the oracle's generic `GetDecoderArray`
  (`fpdf_parser_decode.cpp:393`) *is* production (`cpdf_dib.cpp:330`), but
  pdfrum reaches it through `pdfrum_filters::decoder_list`
  (`crates/pdfrum-filters/src/chain.rs:135`), for inline images at
  `crates/pdfrum-page/src/build.rs:1839`. Deleted.
- **`state::extgstate::ext_gstate_dash`** — the oracle's `/D` arm
  (`cpdf_allstates.cpp:64-77` ← `cpdf_streamcontentparser.cpp:968`) is
  production, and pdfrum ports it **inline twenty lines above the dead copy**,
  in the `b"D"` arm of `apply_ext_gstate`. Deleted, its nested-array test moved
  onto the live arm.
- **`image::dct::ADOBE_CMYK_DECODE`** — the oracle materialises `[1 0 1 0 1 0
  1 0]` only when *authoring* an image XObject from a JPEG file
  (`cpdf_image.cpp:121-125` ← `fpdf_editimg.cpp:112`). Never on a read or
  render path, and pdfrum has no image-writing API. Deleted.
- **`image::jpx::is_stock_device`** — the oracle has no such predicate; it
  compares pointers against the stock singletons in three places
  (`jpx_decode_conversion.cpp:42, :51, :66`) and pdfrum inlines the same
  three-way discrimination in `conversion_action` (`image/jpx.rs:105`).
  Deleted.
- **`image::destination_flips`** — ports `GetDimensionsFromUnitRect`'s
  `a < 0` / `d > 0` rule (`cpdf_imagerenderer.cpp:667-698`), which *is*
  production. But it is bookkeeping for the oracle's integer
  `StretchDIBits(left, top, width, height)` API, where a flip has to be
  spelled as a negative extent. pdfrum draws an image through a full affine
  (`walk.rs:1951-1959`) and the backend applies whatever flip the matrix
  carries, so the same behaviour is reached without ever asking the question.
  Deleted.
- **`blend::blend_gray`** — ports `GetGrayWithBlend`
  (`cfx_scanlinecompositor.cpp:226-235`), which is production for an **8-bpp
  gray destination**. `pdfrum_render::Pixmap` is BGRA-only and the engine has
  no gray destination format at all, so there is nothing to wire it into.
  Deleted.
