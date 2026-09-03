# Unwired oracle ports

**Opened:** 2026-09-03 · **State:** open, four items (one decided)

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

## 1. ~~`image::scanline::rgb_line_to_bgr` — the 16-bpc high-byte arm~~ — **checked and decided, 2026-09-03**

**Ruled: scale and round. The arm is deleted and the shipping path was
wrong too.**

The entry claimed our general path "lands one count low for 767 of the
65 536 possible samples". That number was wrong by a factor of forty, and
finding out why is the whole ruling. Measured exhaustively in the decode's own
`f32` arithmetic, our shipping path differed from PDFium on **32 648** of
65 536 samples — because it *truncated* the float product
(`(value * 255.0) as u8`) rather than rounding it. It was a count low against
every candidate map, the oracle's included.

What the readers actually do:

- **pdf.js scales and rounds.** `DeviceRgbCS.getRgbBuffer`
  (`src/core/colorspace.js`) computes `scale = 255 / ((1 << bits) - 1)` and
  stores `scale * sample` into a `Uint8ClampedArray`, whose store rounds. That
  is ISO 32000-1 §8.9.5's linear map computed exactly.
- **PDFium truncates the high byte.** `cpdf_dib.cpp:1093-1101` writes
  `src_pos[4]`, `src_pos[2]`, `src_pos[0]` — `sample >> 8`. It is within one
  count of the rounded answer on every sample (16 256 of 65 536 differ).

So the two disagree only on rounding care, not on the map, and *the correct
answer is the rounded one*. Our own tree already said so twice: `decode_table`
(`image/mod.rs`) rounds, with a comment explaining that truncating gets the
`[1 0]` inversion wrong, and `Pixels::bytes_at`'s doc calls
`(v.clamp(0.0,1.0) * 255.0).round()` the encode. The general path was the odd
one out.

**Landed:** the general path now rounds (`image/mod.rs`); the unwired
`rgb_line_to_bgr`, `scale_to_byte` and `write` are deleted with their tests;
`decode_array.rs` pins the ruling over all 65 536 samples — the byte equals
`round(sample * 255 / 65535)` in `f64`, and is within one count of the
oracle's `>> 8` — plus a second test proving 1, 2, 4 and 8 bits are unmoved
(rounding, truncating and the oracle's integer `v * 255 / max` agree on every
raw value there, so the change is confined to 16 bpc).

**Not `[oracle-bug]`:** we do not deliberately diverge from a correct PDFium.
PDFium is approximating the same map, and stays within one count of us.

**Board:** unchanged — no 16-bit RGB image is in the corpus.

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
