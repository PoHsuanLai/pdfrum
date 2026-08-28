//! `vello_cpu` implementation of `pdfrum-render`'s `RenderDevice` and
//! `RasterBackend` traits — the primary rasterizer (PLAN.md §3). Fully wraps
//! the backend so its still-moving API never leaks into the engine.

#![forbid(unsafe_code)]
