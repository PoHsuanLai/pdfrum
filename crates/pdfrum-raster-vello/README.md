# pdfrum-raster-vello

GPU [`vello`](https://crates.io/crates/vello) on a caller-supplied `wgpu`
device. The only crate that is not pure Rust to the syscall layer. Nothing
in the core ring depends on it.

`vello` 0.10 pins **wgpu 29**. Inject a device from this crate's re-export:

```rust,ignore
use pdfrum_raster_vello::{VelloBackend, wgpu};
let backend = VelloBackend::new(device, queue)?;
```

Tier C only — not bit-reproducible across GPUs, not the facade default, not
on the conformance board. Facade feature `vello-gpu`.

MIT OR Apache-2.0
