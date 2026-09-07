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

Tier C: not bit-reproducible across GPUs, not the facade default, not on the
conformance board. Facade feature `vello-gpu`, never a default dependency.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
