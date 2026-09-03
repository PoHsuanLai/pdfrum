//! The rendering engine (ISO 32000 §8.4–8.7 realization): the `RenderDevice`
//! and `RasterBackend` traits — the only seam between engine and rasterizers,
//! spoken in kurbo/peniko vocabulary — plus the page-graph walker, the layer
//! compositor for isolated groups and soft masks, and image resampling.
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

// The engine's own machinery. Private per STYLE.md §4 — the surface is the
// `pub use` block below plus the four modules a *backend* implementing
// `RasterBackend` has to reach into, which are declared separately under
// "The backend seam".
mod clip;
mod color;
mod ctx;
mod device;
mod error;
mod group;
mod image;
mod imagecache;
mod options;
mod paint;
mod path;
mod pattern;
mod shading;
mod softmask;
mod stretch;
mod stroke;
mod text;
mod transfer;
mod walk;
mod zero_area;

// # The backend seam
//
// Four modules stay public because a crate implementing [`RasterBackend`]
// needs more than the trait's own signatures name: it rasterizes coverage,
// composites it, blits an LCD glyph, and rounds alpha. Doing any of those in
// the oracle's arithmetic means calling the engine's. `pdfrum-raster-agg` is
// the in-tree proof — a backend written against exactly these four and
// nothing else. Each module's own header says what a backend takes from it.
//
// A `///` on any of these lines would move the whole module's rustdoc into
// this scope and break every intra-module link in it, so the prose lives
// where the items do.
pub mod blend;
pub mod glyph;
pub mod pixmap;
pub mod scanline;

// The walk's own phase timers and allocation counters. Public *with the
// default-off `walk-profile` feature and only then*: the module always
// exists, because the walk calls its entry points unconditionally and they
// compile to empty inline functions with the feature off, but its forty-five
// reporting items are part of the instrument rather than of the crate a
// `cargo add pdfrum-render` reaches —
// `docs/status/api-baseline/README.md:103` says exactly that where it
// declines to snapshot the feature. Its own docs make the STYLE.md §1
// argument for the thread-local, and it holds.
#[cfg(feature = "walk-profile")]
pub mod walkprofile;
#[cfg(not(feature = "walk-profile"))]
mod walkprofile;

pub use color::{Argb, ObjectKind};
pub use ctx::RenderCaches;
pub use device::{
    AntiAlias, Brush, FillRule, ImageQuality, MAX_TARGET_DIMENSION, RasterBackend, RasterImage,
    RenderDevice,
};
pub use error::Error;
pub use options::{ColorMode, ColorScheme, RenderOptions, TextAa};
pub use pixmap::{AlphaMask, Pixmap};
pub use walk::{RenderSession, needs_alpha_background, render_page, render_page_with};
