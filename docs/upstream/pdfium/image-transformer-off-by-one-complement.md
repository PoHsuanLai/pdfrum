# `CFX_ImageTransformer`'s bilinear complement is `255 - res` against a mod-256 residue, darkening every transformed image by two levels

**Status:** drafted 2026-09-08, not yet filed.

**Confidence:** code-level, plus observed output.

**Repro files:** `image_transformer_other.{in,pdf}` and `rotated_image.{in,pdf}`
in `../repro/`, both copied from the checkout's own `testing/resources/`.

**Source read at** PDFium commit `a043bed4a` (2026-09-03).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: the arithmetic is self-checking — the defect is visible at the
exact sample points where the filter is required to be an identity, so no
second implementation is needed to adjudicate it.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## `CFX_ImageTransformer`'s bilinear complement is `255 - res` against a mod-256 residue, darkening every transformed image by two levels

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render `testing/resources/pixel/image_transformer_other.pdf`. Its four image
   XObjects are drawn under matrices with all four of `a`, `b`, `c`, `d`
   non-zero — e.g. `16 64 64 16 20 120 cm` — so each takes
   `CFX_ImageTransformer`'s `StretchType::kOther` path.
3. Read back the `/DeviceGray` block (`{{object 7 0}}`), whose four source rows
   are the constant bytes `00`, `33`, `99`, `cc` — decimal 0, 51, 153, 204.
4. Compare any rendered pixel in the interior of a constant row against its
   source byte.

`testing/resources/rotated_image.pdf` shows the same thing under
`30 -30 40 40 100 100 cm`, which is a skew rather than a quarter turn and so
also lands in `kOther` rather than `kRotate`.

**What is the expected output? What do you see instead?**

Each of these images is a block of constant-valued rows, and the interior of a
constant region has no gradient for a reconstruction filter to interpolate: a
bilinear tap over four identical samples must return that sample exactly,
whatever the fractional position. Instead every channel comes back **two levels
low**, uniformly, including at sample points where the filter should reduce to
an identity:

| source | expected | observed |
|---|---|---|
| 51 (`0x33`) | 51 | 49 |
| 153 (`0x99`) | 153 | 151 |
| 204 (`0xcc`) | 204 | 202 |

The `-2` is exact and not a rounding fringe. On `rotated_image.pdf`, 6990
differing channels are exactly `-2` while only about 700 differ by anything
else, and those few are genuine edge coverage where neighbouring samples really
do differ.

**What version of the product are you using? On what operating system?**

PDFium at `a043bed4a` (2026-09-03), built from source. Ubuntu 22.04.5 LTS,
x86-64. Not V8-dependent; reproduces in a plain build.

**Please provide any additional information below.**

`core/fxge/dib/cfx_imagetransformer.cpp:36-48`, in `BilinearInterpolate`:

```cpp
const int i_resx = 255 - data.res_x;
...
uint8_t r_pos_0 = (*src_pos0 * i_resx + *src_pos1 * data.res_x) >> 8;
uint8_t r_pos_1 = (*src_pos2 * i_resx + *src_pos3 * data.res_x) >> 8;
return (r_pos_0 * (255 - data.res_y) + r_pos_1 * data.res_y) >> 8;
```

The two weights are meant to partition unity in the fixed-point scale the shift
divides by. The shift is `>> 8`, so that scale is 256 — but the complement is
taken against 255, so the weights sum to `255`, not `256`. Every tap is
therefore multiplied by `255/256` instead of `1`, and the truncating shift turns
that deficit into a whole lost level.

`res_x` and `res_y` genuinely are mod-256 residues, not 0..255 fractions scaled
to 255. `kBase` is `256` (`:28`), and `CFX_BilinearMatrix::Transform` produces
them as `static_cast<int>(val.x) % kBase` (`:66-67`), with the negative case
folded by `*res_x = kBase + *res_x` (`:68-71`). So `256 - res` is the correct
complement by construction, and `255 - res` is off by one for every residue.

The error compounds because the filter is separable and applied twice: the
horizontal pass at `:46-47` loses one level, and the vertical pass at `:48`
loses a second from the already-low intermediates — which is why the observed
deficit is exactly `2` rather than `1`.

Worked for a constant region, where all four taps equal `v` and the answer must
be `v` for any residue:

```
r_pos_0 = (v*(255-rx) + v*rx) >> 8 = (255*v) >> 8 = v - 1   (for v >= 1)
return    (r_pos_0*(255-ry) + r_pos_0*ry) >> 8 = (255*(v-1)) >> 8 = v - 2
```

Evaluated over every `(rx, ry)` in `[0,256)^2`, the current form returns a
single value for each constant input, and it is uniformly `v - 2`: `51 -> 49`,
`153 -> 151`, `204 -> 202`, `255 -> 253`, `128 -> 126`. Substituting `256 - res`
in all three places returns `v` exactly, for every input and every residue.

**Suggested fix.** Take both complements against `kBase` rather than `255`:

```cpp
const int i_resx = kBase - data.res_x;
...
return (r_pos_0 * (kBase - data.res_y) + r_pos_1 * data.res_y) >> 8;
```

With weights summing to 256 the shift is an exact divide, the identity case is
preserved, and no result can exceed 255 — the maximum is
`(255*256) >> 8 == 255` — so the existing `uint8_t` narrowing stays safe. A
rounding shift (`+ 128` before `>> 8`) would also remove the bias, but it does
not restore the identity on its own and it changes more pixels than necessary;
correcting the complement is the narrower fix.

**Scope.** Every `kOther` transform passes through this function: `CalcAlpha`,
`CalcMono` and both `CalcColor` variants each call `BilinearInterpolate`
(`:296`, `:314`, `:330-352`, `:363-369`), and nothing gates it on a smoothing
option. So any image drawn under a matrix with a skew or a non-axis-aligned
rotation is darkened, including its alpha channel where one is present.

**Context.** Found while building a Rust PDF engine checked against
`pdfium_test` as an oracle over a 1675-file conformance corpus.
`image_transformer_other.pdf` and `rotated_image.pdf` are the two files in that
corpus whose residual is this bias, at SSIM 0.875 and 0.981 respectively; in
both, the independent implementation reproduces the source bytes exactly and
PDFium is the side that is two levels low.
