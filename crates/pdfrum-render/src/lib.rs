//! The rendering engine (ISO 32000 §8.4–8.7 realization): the `RenderDevice`
//! and `RasterBackend` traits — the only seam between engine and rasterizers,
//! spoken in kurbo/peniko vocabulary — plus the page-graph walker, the layer
//! compositor for isolated/knockout groups and soft masks, and image
//! resampling (SPEC.md §8).

#![forbid(unsafe_code)]
