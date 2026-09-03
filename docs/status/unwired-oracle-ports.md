# Unwired oracle ports

**Opened:** 2026-09-03 · **State:** three items open, all one blocked feature

The no-dead-code pass (`chore/no-dead-code`) went through every
`#[allow(dead_code)]` the idiomatic-API curation left behind. Most were test
helpers that had escaped into library scope, and they were gated under
`#[cfg(test)]` or deleted. Six were not: they are ports of behaviour the
oracle runs on a **production** path, and the port has no production caller
because *the feature around it was never wired*. Deleting them would throw
away a checked, tested spec for a gap that would then be invisible. The
stroked-text CTM split has since been wired; five remained.

**Ruled 2026-09-03, leaving three.** Items 1 and 2 are decided and their
helpers deleted — one was a real bug in the *shipping* path rather than a
missing wire, the other an optimisation proved to be pixel-identical and to
have nothing in the corpus to optimise. The remaining three are one feature
with one upstream blocker, re-verified against the current `zune-jpeg`. The
decided entries are kept in full below: an entry that only says "resolved"
teaches nobody why.

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

## 2. ~~`image::scanline::palette_index` — multi-component packed palettes~~ — **checked and decided, 2026-09-03**

**Ruled: an optimisation, not a behaviour, and one that would pay nothing.
Deleted.**

The oracle palettizes any image with `bpc * components <= 8`
(`cpdf_dib.cpp:164-171`), building one palette entry per possible packed
index (`LoadPalette`, `:905-980`) and looking each pixel up (`GetScanline`'s
packed loop, `:1200-1210`). pdfrum palettizes only `ColorSpace::Indexed` and
widens every other space's components to a byte each. The question was
whether that is a *correctness* gap or merely a different route to the same
pixels.

**It is the same pixels, proved exhaustively rather than argued.** Both
routes evaluate the same two functions in the same order: `decode_min +
decode_step * raw` per component — `DecodeMap::apply` on our side,
`comp_data_[j].decode_min_ + comp_data_[j].decode_step_ *
encoded_component` on theirs — followed by the colour space's own
conversion. The palette merely **precomputes that composition** for all
`1 << (bpc * components)` inputs, and a precomputed pure function cannot
give a different answer than the same function called per pixel.
`a_packed_palette_lookup_and_the_general_path_agree_on_every_index`
(`crates/pdfrum-page/src/image/scanline.rs`) enumerates every packed index
for the two shapes that qualify — 2-bpc `DeviceRGB` (64 indices) and 1-bpc
`DeviceCMYK` (16) — and asserts both routes produce the identical `Rgb`.

**And the optimisation would pay nothing on this corpus.** A scan of all
1319 corpus PDFs and 551 `.in` templates (structural walk with `pikepdf` over
page resources, nested form XObjects, patterns, annotation appearance streams
and inline images, cross-checked by a raw and inflated byte scan of all 1787
files) found **1315 images carrying `/BitsPerComponent`, of which 839
satisfy `bpc * components <= 8` — and every one is single-component**: 542
`/DeviceGray`, 295 `/Indexed`, one `/Separation`, one `/ICCBased` with
`N = 1`. All are already palettized or trivially wide. The nearest miss,
`resources/pixel/bug_554151.pdf`, is 4-bpc `/DeviceRGB` — `4 * 3 = 12 > 8`,
so the oracle takes the `kBgr` path there too.

So wiring it would add a second decode path, guarded by a predicate no
corpus file satisfies, for byte-identical output. The helper is deleted; the
packing rule survives as prose in the module doc and as the test's own local
fixture, which is where a rule with no production caller belongs.

**Board:** unchanged, and unchangeable by this item.

## 3-5. `image::dct::{scale_denominator, scaled_size, allows_reduced_resolution}`

**Re-checked 2026-09-03: still blocked upstream, and the blocker is
confirmed rather than assumed.**

| | |
|---|---|
| Items | `crates/pdfrum-page/src/image/dct.rs` |
| Oracle | `1 << min(levels, 3)` at `core/fpdfapi/page/cpdf_dib.cpp:531-532`; `ScaledJpegSize`, `core/fxcodec/jpeg/libjpeg_scanline_decoder.cpp:44-46`; the MCU-alignment guard at `libjpeg_scanline_decoder.cpp:150-156` |
| Oracle production? | **Yes, all three** — `cpdf_dib.cpp:535, :566, :623` (every `CreateDCTDecoder` path) and `libjpeg_scanline_decoder.cpp:157, :226` |
| pdfrum's live path | none — and it cannot be built from outside `zune-jpeg` |
| Blocked on | `zune-jpeg`, `docs/upstream/zune/scaled-decode.md` (drafted, not filed) |

These three are one contract: how far a JPEG may be decoded at reduced
resolution, what size that yields, and when the MCU grid forbids it.

**Upstream state, checked 2026-09-03.** `Cargo.lock` pins `zune-jpeg`
0.5.15; the newest published release is 0.5.16-rc1 (2026-08-07). Neither
carries the knob. `zune_core::options::DecoderOptions` exposes exactly two
JPEG setters — `jpeg_set_max_scans` and `jpeg_set_out_colorspace` — and
`JpegDecoder`'s inherent methods contain nothing resembling `scale_denom`,
a reduced decode, or a scaled IDCT. So the entry's premise is unchanged and
a version bump would not close it.

**Why it cannot be worked around locally.** Reduced decoding happens
*inside* the inverse DCT — only the low-frequency coefficients of each 8x8
block are dequantized and a smaller IDCT runs — so it is unreachable from
outside the crate. `set_max_width`/`set_max_height` are rejection guards, not
scaling requests, and `idct_1x1_func`/`idct_4x4_func` are not libjpeg's
scaled IDCTs. Decoding fully and downsampling ourselves is not the same
feature: it is the cost the feature exists to avoid.

**The concept is proved in our own tree, on the codec that offers the
knob.** `decode_jpx` takes a `RequestedSize` (`image/jpx.rs:287`) and is
wired at `image/mod.rs:313`; it hands `hayro-jpeg2000` a
`target_resolution` hint and then reads `image.width()` back rather than
shifting a number of its own, because the decoder's answer is authoritative
(`image/jpx.rs:260-275`). That decoder applies the same rule PDFium applies
to `cp_reduce` — the floored base-two logarithm of the smaller axis ratio,
`cpdf_dib.cpp:220-224` — clamped to the levels the codestream carries. So
the surrounding plumbing (a destination-size request threaded to the codec,
a decoder-authoritative result size) exists and works; **what is missing is
only the DCT codec's ability to honour the request**, which is precisely the
upstream gap. `decode_dct` still takes `(data, declared)` and is called at
`image/mod.rs:404` with no size at all.

`image/cache.rs:437`'s own test comment mentions "a JPEG whose MCUs are not
aligned", which is the cache having been designed expecting this.

**Blast radius:** performance, not pixels — a large JPEG drawn small is
decoded at full resolution and downsampled, which costs time and memory
rather than correctness. Measured on `bug_718762` (a 5000x5000 CMYK JPEG on
a 64x64 page, where PDFium decodes at 625x625): ~59 ms of decode plus most
of a further ~464 ms of downstream per-pixel work that exists only because
the buffer is 100 MB instead of 1.5 MB.

**So the three helpers stay**, with this citation, until `zune-jpeg` gains
the knob or we file and land the request. Wiring them would be a `[spec]`
DEPS.md bump and a re-measure of the `image_*` fixtures; neither is possible
today.

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
