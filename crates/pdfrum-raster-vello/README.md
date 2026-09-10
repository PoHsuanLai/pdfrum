# pdfrum-raster-vello

GPU [`vello`](https://crates.io/crates/vello) behind
[`pdfrum_render::RasterBackend`], on a **caller-supplied**
[`wgpu`](https://crates.io/crates/wgpu) device and queue. This is the only
crate in the workspace that is not pure Rust down to the syscall layer, and
nothing in the core ring depends on it.

`vello` 0.10 pins **wgpu 29**. A `wgpu` from your own `Cargo.toml` will be a
different crate to the compiler even at the same version, so inject a device
made from this crate's re-export:

```rust,ignore
use pdfrum_raster_vello::{VelloBackend, wgpu};

let backend = VelloBackend::new(&device, &queue)?;
assert!(backend.accepts(2048, 2048));
```

The device is borrowed, not owned: [`VelloBackend`] holds `&wgpu::Device` and
`&wgpu::Queue` so it can live inside an application that already has a
renderer. [`VelloBackend::max_dimension`] and
[`VelloBackend::check_target`] are the pre-flight — a target larger than the
adapter's texture limit is an `Err` here rather than a driver fault later —
and [`VelloBackend::device_fault`] reports a lost device after the fact.
[`request_adapter`] and [`OwnedDevice`] exist for callers that have no
renderer of their own; [`try_real_gpu`] says whether the adapter is hardware
or a software fallback.

## When to use this backend

The GPU path is for a **viewer or editor that already holds a `wgpu::Device`**
and wants the page as a texture to composite, pan, and zoom. That caller
records with [`pdfrum_render::render_page_to_device`] and presents with
[`VelloBackend::render_to_view`] — no `map_async`, no host pixmap. The view
must be `Rgba8Unorm` with `STORAGE_BINDING`; a swapchain image is almost
never that, so [`VelloBackend::render_to_texture`] is the intermediate.

It is the wrong default for a CLI, a thumbnailer, wasm, or anything that
needs a CPU `Pixmap`. Those stay on `vello_cpu`. GPU rasterization also
loses on small text-heavy pages: every glyph is still a scene image, and
the CPU SIMD rasterizer is faster at that. It wins on heavy vector,
shading, and large images, and it wins on the present path whenever
staying on the GPU avoids a readback the embedder would only re-upload.

[`pdfrum_render::RasterBackend::finish`] still exists and still readbacks —
that is the CPU-shaped path, and [`VelloBackend::roundtrip_stats`] counts
how many of those a page took. The G3 bench reports both columns.

Tier C: not bit-reproducible across GPUs, not the facade default, not on the
conformance board. Facade feature `vello-gpu`, never a default dependency.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
