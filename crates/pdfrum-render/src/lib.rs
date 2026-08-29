//! The rendering engine (ISO 32000 §8.4–8.7 realization): the `RenderDevice`
//! and `RasterBackend` traits — the only seam between engine and rasterizers,
//! spoken in kurbo/peniko vocabulary — plus the page-graph walker, the layer
//! compositor for isolated groups and soft masks, and image resampling
//! (SPEC.md §8).

#![forbid(unsafe_code)]
// Every number reaching this crate came from an untrusted file by way of
// `pdfrum-page`: index with `get()` and do arithmetic with `checked_*`.
#![warn(clippy::indexing_slicing)]

pub mod blend;
pub mod clip;
pub mod color;
pub mod ctx;
pub mod device;
mod error;
pub mod group;
pub mod image;
pub mod options;
pub mod paint;
pub mod path;
pub mod pixmap;
pub mod shading;
pub mod softmask;
pub mod stroke;
pub mod text;
pub mod transfer;
pub mod walk;
pub mod zero_area;

pub use color::{Argb, ObjectKind};
pub use ctx::{RenderCaches, RenderCtx};
pub use device::{
    AntiAlias, Brush, FillRule, ImageQuality, MAX_TARGET_DIMENSION, RasterBackend, RasterImage,
    RenderDevice,
};
pub use error::Error;
pub use options::{ColorMode, ColorScheme, RenderOptions, TextAa};
pub use pixmap::{AlphaMask, Pixmap};
pub use walk::render_page;
