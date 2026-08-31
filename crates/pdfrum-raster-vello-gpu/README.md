# pdfrum-raster-vello-gpu

**GPU `vello` implementation of pdfrum-render's RenderDevice, on a
caller-supplied `wgpu` device.**

A rasterizer backend built on [`vello`](https://crates.io/crates/vello): it
implements `pdfrum-render`'s `RenderDevice` and `RasterBackend` traits — path
fills and strokes, images, clips, layers — and hands back premultiplied RGBA8
pixmaps, rendered on the GPU and read back to the host.

This is the fourth backend and the only one that is not pure Rust all the way
down, because `wgpu` reaches the platform's graphics drivers. That exemption is
granted for one reason and bounded by two rules:

- **The reason.** pdfrum's most likely consumer is a Rust GUI frontend — egui,
  iced, dioxus are all wgpu-backed — and in that process `wgpu` is already in
  the tree with a device already open. Refusing it preserves purity for nobody
  and merely forces a CPU rasterize-then-upload path inside an application
  holding a GPU device the whole time.
- **Isolation.** Nothing in the core ring depends on this crate. The `pdfrum`
  facade and `pdfrum-tool` do not, so a headless build still resolves a tree
  with zero `wgpu` — asserted by `scripts/check-no-wgpu.sh` in CI, not by this
  paragraph.
- **Device injection.** `VelloGpuBackend::new` takes the caller's `Device` and
  `Queue` **by reference**. Opening a second device inside a PDF library is
  waste an embedder cannot opt out of. `request_adapter()` exists for a
  headless process that has no device to lend — it is named after
  `wgpu::Instance::request_adapter`, which is what it does.

## Which `wgpu`

`vello` 0.10 resolves **`wgpu` 29**, not 30. Device injection only typechecks
against the exact `wgpu` a `vello::Renderer` was built with, so this crate
re-exports it:

```rust,ignore
use pdfrum_raster_vello_gpu::{VelloGpuBackend, wgpu};
// `device` and `queue` must come from `pdfrum_raster_vello_gpu::wgpu`.
let backend = VelloGpuBackend::new(device, queue)?;
```

## What it is not

GPU rasterization is not bit-reproducible across vendors and drivers, so this
backend is a **Tier C participant only**: the CPU backends stay the
oracle-compared ones and this is compared against *them*, inside a stated
divergence budget. It is not the facade's default and it never joins the
conformance scoreboard.

It also has no per-primitive antialiasing control — vello picks one mode for a
whole render — so `AntiAlias::Off` and `AntiAlias::FullCover` come out
antialiased here where the CPU backends threshold them. That is a documented
divergence, measured in `docs/status/M12c.md` §7, not an oversight.

## The cost model

`vello::Scene` is retained: drawing records an encoding and pixels appear only
when the scene is rendered. Every offscreen target the engine asks for — a
transparency group, a soft mask, each tiling-pattern cell — becomes a texture
allocation, a GPU dispatch and a host stall waiting for the readback. A page
that is one big scene wins; a page built from many small offscreen targets
loses, for a structural reason rather than a tuning one.
`docs/status/M12c.md` §8 has the measured crossover and says which corpus
documents fall on which side.
