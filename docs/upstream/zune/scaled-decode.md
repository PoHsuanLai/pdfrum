# Upstream feature request draft — `zune-jpeg`

Ready to file against the `zune-jpeg` repository. Everything below the rule is
the request text; nothing above it is meant to be posted.

**Status:** drafted, not yet filed.

**Re-checked 2026-09-05.** Someone got there first: zune-image
[#434](https://github.com/etemesi254/zune-image/issues/434), "zune-jpeg:
scaled decode (reduced-size IDCT, libjpeg scale_num-style) for known-small
render targets", opened 2026-08-18 by ObjSal, open, asks for exactly this
(a 4000×4000 JPEG at 76 MB peak against 35.6 MB scaled). **File this as a
comment on #434 carrying our measurements, not as a new issue.** Two
related open pull requests, #412 ("Implement resizing/downscaling support")
and #414, do post-decode resizing, which is not the same thing. Latest
release is 0.5.16-rc1; nothing in its notes mentions scaled decoding.

**A second user since drafting.** PDFium itself replaced its Skia JPEG path
with zune-jpeg on 2026-09-03 (`a043bed4a`, "Replace Skia JPEG decoder with
direct Rust FFI (zune-jpeg)"). Its `decode_jpeg_to_buf` takes the same
`scale_denom` libjpeg took and, for `scale_denom > 1`, decodes the full
image and box-averages `scale × scale` blocks afterwards — the reduced-size
IDCT it had from libjpeg is gone from that path. The paragraph below the
rule that says so is new.

**Why it is not fixed locally.** Reduced-resolution decoding happens *inside*
the inverse DCT — you dequantize only the low-frequency coefficients of each
8x8 block and run a smaller IDCT — so it cannot be reached from outside the
crate. There is no wrapper, no post-pass, and no combination of
`DecoderOptions` that gets it: `set_max_width`/`set_max_height` are rejection
guards, and `idct_1x1_func`/`idct_4x4_func` look like libjpeg's scaled IDCTs
but are not (see the "what we checked first" section below). So the fix belongs
upstream, or nowhere.

**Why it is not a blocker for us.** We measured before asking. On our corpus the
scaled decode is worth about 59 ms on one document, and the surrounding work we
*could* reach was worth 675 ms on the same document. Both numbers are in the
request, because a maintainer deciding whether this is worth implementing
deserves the honest ratio rather than only the flattering half.

---

## Feature request: `scale_denom` — decode at 1/2, 1/4 or 1/8 inside the IDCT

### Summary

`zune-jpeg` always decodes at full resolution. libjpeg (and libjpeg-turbo) let a
caller set `cinfo.scale_denom` to 1, 2, 4 or 8 before `jpeg_start_decompress`,
which makes the decoder run a reduced inverse DCT and emit an image
`ceil(dim / scale_denom)` on each axis. The saving is not a post-decode resample
— the coefficients above the retained band are never dequantized and the IDCT
itself is smaller — so a 1/8 decode does roughly 1/64 of the sample work.

This matters for PDF rendering specifically, and it is why we are asking. A PDF
routinely embeds an image far larger than the region it is drawn into, and a
renderer knows the destination size before it decodes. PDFium does exactly this:
it computes a level count from the destination and passes the resulting
`scale_denom` to libjpeg, calling the decoder's own dimensions "authoritative"
thereafter.

### The API we would use

Something as small as:

```rust
let options = DecoderOptions::default().jpeg_set_scale_denom(8); // 1, 2, 4 or 8
let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
let pixels = decoder.decode()?;
let info = decoder.info().unwrap(); // reports the REDUCED dimensions
```

The two properties we depend on:

1. **`info()` reports what was actually produced**, not what the file declares.
   A caller must never have to compute the output size itself; a decoder that is
   free to ignore or clamp the request is fine, as long as it says so.
2. **A value other than 1, 2, 4, 8 is rejected or rounded**, rather than
   silently producing a size the caller cannot predict.

We do not need arbitrary `m/n` scaling. Powers of two to 1/8 are what PDF
renderers use and what libjpeg guarantees `ceil(dim / n)` for.

### One correctness rule worth inheriting

libjpeg-turbo's reduced decoding is **not exact for images whose dimensions are
not a whole number of MCUs**. The encoder had to pad the partial edge blocks,
that padding is not required to replicate the edge (some encoders fill it with
black), and reduced decoding cannot crop it back out — so it contaminates the
visible edge pixels, typically as a too-dark right or bottom fringe.

Chromium hit this as <https://crbug.com/890745>, and libjpeg-turbo as
<https://github.com/libjpeg-turbo/libjpeg-turbo/issues/297>. PDFium's fix is to
refuse the reduction entirely in that case: it derives the MCU size from the
per-component sampling factors (`max_samp_factor * DCTSIZE`, valid immediately
after `read_header`) and resets `scale_denom` to 1 unless both dimensions are
whole multiples of it.

We would suggest `zune-jpeg` do the same, and report the resulting dimensions
through `info()` per property 1 above — that way a caller cannot be surprised.
It also means a caller never has to know the sampling factors to predict the
output.

### What we checked first, so you do not have to

We looked for an existing path before writing this, and two things in the crate
look like one and are not:

- **`idct_1x1_func` and `idct_4x4_func` are not scaled IDCTs.** `src/mcu.rs`
  (around lines 585-591 in 0.5.15) selects them by **coefficient sparsity** —
  `len <= 1` and `len <= 10` — and both still write a full 8x8 block. They are a
  sparse-block shortcut, and cannot be repurposed for scaled output.
- **`DecoderOptions::set_max_width` / `set_max_height` are rejection guards.**
  They cause a decode to fail on an oversized image; they do not scale one.

### Our numbers

Context: a pure-Rust PDF engine using `zune-jpeg` for `/DCTDecode`. Machine is
an AMD Ryzen 9 7950X, release build, `opt-level = 3`.

The document is `bug_718762.pdf` from PDFium's own test corpus: **1.6 KB of PDF
declaring a 5000x5000 CMYK JPEG that is drawn onto a 64x64 page.** PDFium's rule
gives `log2(min(5000/64, 5000/64)) = 6` levels, capped by libjpeg to 3, so it
decodes at **625x625 — 1/64th of the samples.**

Full-resolution decode of that codestream (171,641 bytes → 5000x5000x4):

| | before our own work | after it |
|---|---:|---:|
| whole first render of the page | 1180.8 ms | **523.1 ms** |
| of which: `zune-jpeg` full decode | 58.6 ms | 58.6 ms |
| of which: everything downstream that scales with the decoded buffer | 1122.2 ms | 464.5 ms |

So on the file that motivates this request, a 1/8 decode would remove ~59 ms of
decode **plus most of the remaining 464 ms of downstream per-pixel work**, which
exists only because the buffer is 100 MB rather than the 1.5 MB a 625x625 decode
would produce. That second part is by far the larger share, and it is the honest
reason to want this: the win is not mainly in the decoder, it is in everything
the decoder's output size forces on its caller.

For completeness — and because it is the part that argues *against* urgency —
the downstream work is where our own optimization went first, and it took the
page from 1181 ms to 523 ms without touching the decoder at all. A scaled decode
is the remaining structural fix, not the only one, and we would understand a
maintainer weighing it accordingly.

### Prior art

- libjpeg-turbo: `jpeg_decompress_struct::scale_num` / `scale_denom`, honoured
  by `jpeg_calc_output_dimensions` and the `jpeg_idct_*` family.
- PDFium: `core/fxcodec/jpeg/libjpeg_scanline_decoder.cpp` — the `scale_denom`
  plumbing, the `ScaledJpegSize` = `ceil(dim / denom)` contract, and the MCU
  guard described above; `core/fpdfapi/page/cpdf_dib.cpp:220-224` computes the
  level count from the destination size.
- `image-rs`: has wanted this for a while; the `jpeg-decoder` crate it is
  migrating away from never had it either.

We are happy to attempt the implementation if the direction is welcome — the
question we cannot answer from outside is how you would want the MCU-alignment
refusal surfaced.

**Another consumer, since this was drafted.** PDFium's own JPEG path moved to
zune-jpeg on 2026-09-03 (pdfium `a043bed4a`). Its bridge keeps libjpeg's
`scale_denom` parameter and, when it is above 1, decodes at full size and
box-averages afterwards — so the reduced-size IDCT that libjpeg gave PDFium
for years is now missing from the same renderer through the same crate. A
scaled decode inside zune-jpeg would be picked up there directly.

