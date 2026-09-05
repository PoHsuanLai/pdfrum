# Image rows: the type-driven image pipeline

Scoped 2026-09-05 from the side-by-side with PDFium (`docs/status/queue.md`
§"Image pipeline pass"), approved by the user the same day. The pass that
implements it lands stage by stage, each commit measured by `Ir` on the
guide (`benches/corpus/text_quick_start.pdf`, all pages, 150 DPI) and gated
by a byte-identical board.

## The problem

PDFium's image path is one fused scanline loop: `CStretchEngine::Continue`
pulls `CPDF_DIB::GetScanline` per source row (`cstretchengine.cpp:346`), so
decode, unpack, colour conversion and the horizontal stretch happen row by
row and the only full-height buffer is destination-width. Ours
materializes the whole source as RGBA (`pdfrum-render/src/image.rs`
`to_pixmap`), allocates two more full-height buffers in `reduce_to`, and
then the backend resamples the result a second time. Like for like on the
guide: ~3.65 G `Ir` against PDFium's ~0.99 G for the same pixels.

## The shape

Rows flow through stages; a stage is a type; the whole is a pull pipeline.
The representation of a row is its type, so a stage can only accept what
the previous one produces, and a full-size RGBA copy cannot exist because
no type asks for one.

```rust
/// A run of pixels in one representation.
pub struct Row<'a, P>(&'a [P]);          // P: Gray8, Rgb8, Cmyk8, Index8, Rgba8

/// Whatever yields the image's samples one row at a time, at source width.
pub trait Rows {
    type Pixel;
    fn next(&mut self) -> Option<Row<'_, Self::Pixel>>;
}

/// 1. Packed samples to 8-bit components, per row, into a reusable buffer.
pub struct Unpacked<S: Rows> { src: S, depth: Depth, buf: Vec<u8> }

/// 2. Components to RGBA through the colour space's one row method.
pub struct Converted<S: Rows> { src: S, space: ColorSpace, decode: Decode, buf: Vec<Rgba8> }

/// 3. One horizontal box pass to destination width, fixed point.
pub struct Narrowed<S: Rows<Pixel = Rgba8>> { src: S, taps: Taps, buf: Vec<Rgba16> }

/// 4. Vertical accumulation; a finished destination row when a bin closes.
pub struct Shortened<S: Rows<Pixel = Rgba16>> { src: S, taps: Taps, acc: Vec<Rgba32> }
```

- `Depth::{One, Two, Four, Eight, Sixteen}`; `Rgba8`, `Rgba16`, `Rgba32`
  are `#[repr(C)]` newtypes over arrays, so loops index pixels, not bytes.
- `ColorSpace::convert_row(&self, src: &[u8], decode: &Decode, dst: &mut [Rgba8])`
  is the one conversion function; `Indexed` carries a `Palette<Rgb8>`, a
  lookup rather than a re-parse per pixel. The existing fallbacks stay:
  an absent index is 0, an absent palette entry is black.
- Fixed point is a type: `Weight(u32)` with the shift as an associated
  constant, `Taps { first: u32, weights: Box<[Weight]> }` built by integer
  arithmetic only. `f64` cannot enter the weight path.

  *Corrected while landing step 1:* this said `Weight(u16)`, which does not
  work. `Weight::ONE` is `1 << 16`, one past the top of a `u16`, and it is a
  real single-tap value: a destination pixel whose footprint falls outside the
  source is clamped to the nearest source pixel *entire*. A `u16` would have
  forced a `whole: bool` beside the slice — a flag encoding a state that the
  weight itself should carry, which is the shape this design exists to avoid.
  The width was never the point; the integer arithmetic is.
- Masks: the stencil and `/SMask` alpha join at stage 2, per row, as they
  do today inside `to_pixmap`; nothing is expanded ahead of time.
- The JPEG source wraps zune-jpeg's whole-image buffer until the decoder
  can yield rows or scaled output (zune-image #434); the pipeline shape
  does not change when it can.

## "Already at device size" is a type

`reduction_for` returns

```rust
pub enum Placement {
    /// The reduced pixmap is the device pixels: draw it as is.
    Exact(DeviceSize),
    /// The reduced pixmap still needs the backend's sampler.
    Filtered { size: (u32, u32), remaining: Affine },
}
```

and the backends' `draw_image` takes it. `Exact` is drawn with the nearest
sampler; it cannot be filtered twice because the filtered path needs the
`remaining` transform the exact variant does not carry. This is the one
step that moves pixels; it lands last and is gated file by file on the
board.

## What stays and what goes

The render cache keeps its key (`ObjRef`, size, fill, transfer) and stores
`Shortened`'s output, which is what it stores today. `Option` and bounds
checks stay (the user's call: safety over the ~120 M they cost). Gone: the
three zeroed full-size pixmaps (291 M of memset and calloc), the second
resample (311 M), the `f64` weights (54 M), and `unpack` as a pass over
the whole image.

## Order of landing

1. `Taps`/`Weight` in fixed point (pixel-identical only if the tap positions
   come out the same — the test is the board).
2. `Unpacked` and `Converted` as row stages, `to_pixmap` rewritten on them
   (still producing the full pixmap): the unpack pass disappears.
3. `Narrowed` and `Shortened`, `reduce_to` rewritten to pull rows from
   `Converted`: the full-size RGBA copy and the two intermediates go.
4. `Placement::Exact` and the nearest draw, gated on the board.

Each step: `Ir` before/after on the guide and `vector_en_tem`, the ratchet,
the board.
