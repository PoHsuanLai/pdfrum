//! The rendering engine (ISO 32000 §8.4–8.7 realization): the `RenderDevice`
//! and `RasterBackend` traits — the only seam between engine and rasterizers,
//! spoken in kurbo/peniko vocabulary — plus the page-graph walker, the layer
//! compositor for isolated groups and soft masks, and image resampling
//! (SPEC.md §8).
//!
//! # Rendering a page
//!
//! [`render_page`] takes a [`Page`](pdfrum_page::Page) from `pdfrum-page`
//! and any [`RasterBackend`], and returns a premultiplied RGBA8 [`Pixmap`].
//! The defaults reproduce the oracle: an opaque page renders onto white, a
//! transparent one onto nothing.
//!
//! ```
//! use pdfrum_common::Diagnostics;
//! use pdfrum_page::Page;
//! use pdfrum_render::{RenderOptions, render_page};
//!
//! # fn main() -> Result<(), pdfrum_render::Error> {
//! // Any backend implementing `RasterBackend` will do; the engine never
//! // learns which one it has.
//! # struct Backend;
//! # use pdfrum_render::{AlphaMask, AntiAlias, Brush, FillRule, ImageQuality,
//! #     Pixmap, RasterBackend, RasterImage, RenderDevice};
//! # use kurbo::{Affine, BezPath, Rect, Stroke};
//! # struct Device { w: u32, h: u32 }
//! # impl RenderDevice for Device {
//! #     fn fill_path(&mut self, _: &BezPath, _: Affine, _: &Brush<'_>, _: FillRule, _: AntiAlias) {}
//! #     fn stroke_path(&mut self, _: &BezPath, _: Affine, _: &Brush<'_>, _: &Stroke, _: AntiAlias) {}
//! #     fn draw_image(&mut self, _: &RasterImage, _: Affine, _: ImageQuality, _: f32) {}
//! #     fn push_clip(&mut self, _: &BezPath, _: FillRule) {}
//! #     fn push_clip_rect(&mut self, _: Rect) {}
//! #     fn push_layer(&mut self, _: pdfrum_page::BlendMode, _: f32, _: Option<&AlphaMask>) {}
//! #     fn pop(&mut self) {}
//! # }
//! # impl RasterBackend for Backend {
//! #     type Device = Device;
//! #     fn new_target(&self, w: u32, h: u32, _: peniko::Color) -> Device { Device { w, h } }
//! #     fn new_target_with_backdrop(&self, b: &Pixmap) -> Device {
//! #         Device { w: b.width(), h: b.height() }
//! #     }
//! #     fn snapshot(&self, d: &Device) -> Pixmap { Pixmap::new(d.w, d.h) }
//! #     fn finish(&self, d: Device) -> Pixmap { Pixmap::new(d.w, d.h) }
//! # }
//! let page = Page::empty();
//! let mut diags = Diagnostics::default();
//! let pixmap = render_page(&page, &RenderOptions::default(), &Backend, &mut diags)?;
//!
//! // An empty US Letter page at the default scale.
//! assert_eq!((pixmap.width(), pixmap.height()), (612, 792));
//! # Ok(())
//! # }
//! ```
//!
//! Damage is never an error: an undrawable matrix, a shading whose function
//! will not evaluate, a pattern with degenerate steps — each is a skipped
//! object and a [`Diagnostic`](pdfrum_common::Diagnostic). [`Error`] is
//! reserved for a target that cannot exist at all.

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
pub use walk::{needs_alpha_background, render_page};
