# Upstream performance-issue draft — `vello_cpu`

Ready to file against <https://github.com/linebender/vello>. Everything below
the rule is the issue text; nothing above it is meant to be posted.

**Status:** drafted, not yet filed.

**Checked against upstream 2026-09-06, and it is not a duplicate.**

- `gh issue list -R linebender/vello --search "pack SIMD"` returns nothing
  relevant (two hits, neither about the fine kernel); searches for `SIMDify`,
  `F32Kernel`, and `highp SIMD` over issues and over open **and** closed pull
  requests all return empty.
- **The decisive check:** both `// TODO` comments are still present on
  upstream `main`, at the same line numbers as in the released 0.2.0
  (`sparse_strips/vello_cpu/src/fine/highp/mod.rs:275` and `:290`, read
  through `gh api .../contents/...?ref=main`). Nothing has landed.
- `vello_cpu` 0.2.0 (`Sparse Strips v0.2.0`, 2026-08-07) is the latest release
  and the version we pin. Its `CHANGELOG.md` `[Unreleased]` section is empty
  of anything touching the fine kernel; the 0.2.0 and 0.1.0 `Optimized`
  entries are lazy task dispatch and the coarse-rasterizer rewrite, not
  `pack`/`unpack`.

So this is **not** "upgrade to vello_cpu X" in `DEPS.md`'s terms — there is no
X. It is a genuine upstream report, and the version bump only becomes the
answer once a fix lands.

**Why it is not fixed locally.** `F32Kernel` is `vello_cpu`'s own fine-kernel
implementation, selected inside the dispatcher
(`src/dispatch/single_threaded.rs:415-470`). There is no trait we can
implement, no setting that reaches it, and no wrapper position from which the
per-pixel loop can be replaced. Vendoring a patched `vello_cpu` would work and
is what we would fall back to, but the change belongs upstream: the code is
already marked `TODO` by its authors, and the sibling `U8Kernel` shows the
shape they intend.

**What we measured on our side, and did not put in the report.** Switching our
backend to `RenderMode::OptimizeSpeed` (the `U8Kernel`, which sidesteps this
cost rather than fixing it) is deterministic and buys 1.9x-3.4x marginal `Ir`
per render, but it moves 14 conformance rows below our SSIM floor, so we have
not taken it. Full numbers in `docs/design/mupdf-comparison.md` section 8.
That measurement is *why* we care about the f32 path specifically: it is the
path we are staying on.

**Proposed change is described in prose, not as a patch.** Vello's maintainers
own this design — in particular whether `pack` should keep bit-exact
`(x * 255.0 + 0.5) as u8` rounding, and how the tail below one SIMD width is
handled. We offered to write it, but did not presume the shape.

---

## `F32Kernel::pack` / `unpack` are scalar, and cost ~48 instructions per pixel per render

### Summary

`vello_cpu`'s f32 fine kernel converts between `f32` scratch and `u8`
destination with two scalar per-channel loops, both already marked `TODO:
SIMDify` by their authors. Because they run over every pixel of the target on
every flush, their cost is a **fixed per-pixel tax that does not vary with
scene content** — measured at **38.0 `Ir`/px for `pack` and 10.1 `Ir`/px for
`unpack`** on a trivial scene, and **44.0 + 11.1 `Ir`/px** in a real
PDF-rendering workload.

At 1240x1753 (A4 at 150 DPI) that is roughly **96 M instructions per render
for `pack` alone**. In our renderer it is 28%-43% of the total inclusive cost
of a warm page render, and it is by a wide margin the single largest cost
centre in profiles that are otherwise dominated by real drawing work.

### The two code sites

`sparse_strips/vello_cpu/src/fine/highp/mod.rs` (line numbers from released
0.2.0 and unchanged on `main` as of 2026-09-06):

```rust
// :272-285
fn pack(_simd: S, scratch: &[Self::Numeric], width: usize, region: &mut Region<'_>) {
    for y in 0..region.height {
        let row = &mut region.row_mut(y)[..width * COLOR_COMPONENTS];
        // TODO: SIMDify
        for (dx, pixel) in row.chunks_exact_mut(COLOR_COMPONENTS).enumerate() {
            let idx = COLOR_COMPONENTS * (Tile::HEIGHT as usize * dx + usize::from(y));
            let src = &scratch[idx..idx + COLOR_COMPONENTS];
            pixel[0] = (src[0] * 255.0 + 0.5) as u8;
            pixel[1] = (src[1] * 255.0 + 0.5) as u8;
            pixel[2] = (src[2] * 255.0 + 0.5) as u8;
            pixel[3] = (src[3] * 255.0 + 0.5) as u8;
        }
    }
}

// :287-297
fn unpack(_simd: S, region: &mut Region<'_>, width: usize, scratch: &mut [Self::Numeric]) {
    for y in 0..region.height {
        let row = &region.row_mut(y)[..width * COLOR_COMPONENTS];
        // TODO: SIMDify + multiply by 1.0/255.0 instead.
        for (dx, pixel) in row.chunks_exact(COLOR_COMPONENTS).enumerate() {
            let idx = COLOR_COMPONENTS * (Tile::HEIGHT as usize * dx + usize::from(y));
            scratch[idx] = pixel[0] as f32 / 255.0;
            scratch[idx + 1] = pixel[1] as f32 / 255.0;
            scratch[idx + 2] = pixel[2] as f32 / 255.0;
            scratch[idx + 3] = pixel[3] as f32 / 255.0;
        }
    }
}
```

Note that `unpack` performs four **divides** per pixel where its own comment
asks for a multiply by `1.0/255.0`. That half is a one-line, bit-exact change
independent of any SIMD work: `x / 255.0` and `x * (1.0/255.0)` differ for
some inputs in IEEE-754 exact terms, but `1.0/255.0` is exactly representable
after rounding and the operands here are the 256 values `0..=255`, for which
both forms round to the same `f32` — so it can be made without touching output.

Both take `_simd: S` and ignore it. The sibling `U8Kernel::pack` in
`src/fine/lowp/mod.rs:308-360` already does the vectorized thing
(`simd.vectorize` over a `pack_block` / `pack_tail` split), which is presumably
the intended shape for `highp` too.

### Reproduction, with no renderer involved

This is `vello_cpu` 0.2.0 and nothing else. It builds against the released
crate and shows the per-pixel constant directly.

`Cargo.toml`:

```toml
[package]
name = "vrepro"
version = "0.1.0"
edition = "2021"

[dependencies]
vello_cpu = { version = "=0.2.0", default-features = false, features = ["std", "f32_pipeline"] }

[profile.release]
opt-level = 3
debug = true
```

`src/main.rs`:

```rust
// vello_cpu 0.2.0 only. Measures `F32Kernel::pack`/`unpack` as a fixed
// per-pixel cost per render, independent of scene content.
//
//   cargo build --release
//   valgrind --tool=callgrind --callgrind-out-file=cg ./target/release/vrepro 1240 1753 5
//   callgrind_annotate cg | grep 'fine/highp'
//
// Divide the `pack` count by (width * height * renders): the quotient is the
// same for any width, height, or scene.

use vello_cpu::color::palette::css::REBECCA_PURPLE;
use vello_cpu::kurbo::Rect;
use vello_cpu::{
    CompositeMode, Level, PixelFormat, Pixmap, RasterizerSettings, RenderContext, RenderMode,
    RenderSettings, Resources,
};

fn arg(n: usize, default: u32) -> u32 {
    std::env::args().nth(n).and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn main() {
    let w = arg(1, 1240) as u16;
    let h = arg(2, 1753) as u16;
    let renders = arg(3, 5) as usize;

    let settings = RenderSettings { level: Level::baseline(), num_threads: 0 };
    let raster = RasterizerSettings {
        render_mode: RenderMode::OptimizeQuality, // the f32 pipeline
        composite_mode: CompositeMode::SrcOver,
        pixel_format: PixelFormat::Rgba8,
        offset: (0, 0),
    };

    let mut ctx = RenderContext::new_with(w, h, settings);
    let mut target = Pixmap::new(w, h);
    let mut resources = Resources::new();

    for _ in 0..renders {
        ctx.reset();
        ctx.set_paint(REBECCA_PURPLE);
        // One small rectangle: the scene is deliberately trivial, so whatever
        // `pack` costs is not the drawing.
        ctx.fill_rect(&Rect::new(10.0, 10.0, 40.0, 40.0));
        ctx.flush();
        ctx.render_with(&mut target, &mut resources, raster);
    }

    println!(
        "{renders} renders of {w}x{h} = {} px-renders; first pixel {:?}",
        usize::from(w) * usize::from(h) * renders,
        &target.data_as_u8_slice()[..4]
    );
}
```

Results (AMD Ryzen 9 7950X, `callgrind`, three target sizes, 5 renders each,
one 30x30 rectangle drawn per render):

| target | px-renders | `pack` `Ir` | `Ir`/px | `unpack` `Ir` | `Ir`/px |
|---|---:|---:|---:|---:|---:|
| 1240x1753 | 10,868,600 | 413,024,360 | **38.002** | 109,457,380 | **10.071** |
| 1275x1650 | 10,518,750 | 399,729,020 | **38.002** | 105,756,790 | **10.054** |
| 640x480 | 1,536,000 | 58,372,800 | **38.003** | 15,571,200 | **10.137** |

**38.00 `Ir` per pixel per render for `pack`, constant to five significant
figures across three unrelated target sizes**, for a scene that draws one
900-pixel rectangle. Together with `unpack`, that is **~48 `Ir`/px/render**
spent purely converting between `f32` and `u8`, and in this program it is
**86% of the entire process**.

### The same measurement in a real workload

Context: a pure-Rust PDF engine that uses `vello_cpu` as its primary
rasterizer, on PDF pages rendered at A4 / 150 DPI = 1240x1753 = 2,173,720 px.
Method is a `--warm 4` profile minus a `--warm 0` profile, divided by 4, so
one-time setup cancels and what remains is the marginal cost of one more
render of the same page.

The striking observation, and what sent us looking: **`pack` reports the
identical figure, 478,367,460 `Ir`, on four unrelated pages** — a text-heavy
page, an image page, a vector page and a mixed/forms page — all of which
happen to be A4. A cost that does not move with page content is not a
rendering cost.

| page kind | `pack` `Ir` (5 renders) | `unpack` `Ir` | `pack` share of the render |
|---|---:|---:|---:|
| text-heavy | 478,367,460 | 120,926,385 | 27.9% |
| image | 478,367,460 | 120,926,385 | 32.1% |
| vector | 478,367,460 | 120,926,385 | 42.6% |
| mixed / forms | 478,367,460 | 120,926,385 | 41.2% |
| shading (1240x1753, more tiles touched) | 432,770,880 | 109,078,404 | 24.5% |

Divided out: 478,367,460 / (2,173,720 px x 5 renders) = **44.0 `Ir`/px** for
`pack` and **11.1** for `unpack`, ~55 together — slightly above the standalone
program's 48 because a real page touches more tiles per flush, but the same
constant.

For scale, on the vector page `pack` + `unpack` is **71% of the entire
marginal cost of rendering the page**, drawing included.

### What a SIMD `pack` would plausibly save

We want to be careful here, because we have measured the cost that exists and
not the cost of a replacement — this part is an estimate and we flag it as one.

The operation is embarrassingly vectorizable: four `f32` lanes to four `u8`
lanes with a multiply-add and a saturating narrowing convert, over contiguous
scratch. `U8Kernel::pack` next door already does the analogous thing through
`fearless_simd`, and `NumericVec<S> for u8x16<S>` (`src/fine/mod.rs:107-120`)
already contains exactly the conversion `pack` needs, written in SIMD:

```rust
fn from_f32(simd: S, val: f32x16<S>) -> Self {
    let v1 = f32x16::splat(simd, 255.0);
    let v2 = f32x16::splat(simd, 0.5);
    let mulled = val.mul_add(v1, v2);
    f32_to_u8(mulled)
}
```

so the arithmetic is not in question, only the loop around it. On a 4-wide
baseline SIMD level, a 3x-4x reduction is the conventional expectation for
this shape; wider levels should do better. At 3x, `pack` falls from 44.0 to
~15 `Ir`/px, which on an A4 page is **~65 M instructions saved per render**.
For comparison, an entire warm page render in MuPDF's software rasterizer
costs 48-119 M instructions on the same pages — so this one function is worth
roughly a whole competing renderer's page budget.

The `unpack` divide-to-multiply change is separately worth having and is much
smaller in scope: four `f32` divides per pixel become four multiplies, with no
change to the output for the 256 possible inputs.

### The proposed change, in prose

1. **`unpack`: replace the divide with a multiply by `1.0/255.0`**, as the
   existing comment asks. Independent of everything else, one line per channel,
   no output change for the inputs this function can receive. This alone is
   worth taking even if the rest is deferred.

2. **`pack`: vectorize the inner loop** using the already-available
   `NumericVec::from_f32` conversion, in the shape `U8Kernel::pack` uses — a
   `simd.vectorize` closure with a full-width block loop and a scalar tail for
   the remainder below one vector width. The tail keeps the current scalar code
   verbatim, so the rounding on the edge cannot drift from the interior.

3. **`unpack`: vectorize similarly**, using `NumericVec::from_u8` /
   `u8_to_f32`, which are likewise already present.

The design question we deliberately have not answered, because it is yours:
whether the vectorized `pack` must be **bit-exact** with today's
`(x * 255.0 + 0.5) as u8`. We believe it can be — `mul_add` plus a saturating
convert reproduces it for the `0.0..=1.0` range the scratch holds — but
`mul_add`'s fused rounding is not identical to a separate multiply and add for
out-of-range inputs, and whether the scratch can hold such values (unclamped
blend results, or a wide-gamut colour) is something the fine kernel's authors
know and we do not. If bit-exactness matters for your snapshot tests, a
non-fused `mul` + `add` avoids the question entirely at a small cost.

We would be glad to write the patch if the direction is welcome. What we
cannot decide from outside is that rounding question and the tail-handling
convention you would want it to follow.

### Environment

- `vello_cpu` 0.2.0, `default-features = false`, features `["std",
  "f32_pipeline"]`, `Level::baseline()` pinned (we pin the SIMD level for
  cross-machine reproducibility; the scalar `pack` is scalar at every level,
  so this does not affect the measurement).
- AMD Ryzen 9 7950X, Linux, Rust release profile `opt-level = 3`.
- Instruction counts are `valgrind --tool=callgrind` inclusive `Ir`, which is
  deterministic and machine-independent, hence the exactly repeating figures.
