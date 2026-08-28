//! `tiny-skia` implementation of `pdfrum-render`'s `RenderDevice` and
//! `RasterBackend` traits — the cross-check rasterizer and determinism
//! baseline that Tier-C conformance diffs against `vello_cpu` (PLAN.md §3).

#![forbid(unsafe_code)]
