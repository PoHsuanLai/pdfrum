#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
// Every number reaching this crate came from an untrusted file by way of
// `pdfrum-page`: index with `get()` and do arithmetic with `checked_*`.
#![warn(clippy::indexing_slicing)]

// The engine's own machinery. Private: the surface is the
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
// default-off `profiling` feature and only then*: the module always
// exists, because the walk calls its entry points unconditionally and they
// compile to empty inline functions with the feature off, but its forty-five
// reporting items are part of the instrument rather than of the crate a
// `cargo add pdfrum-render` reaches, which is why the committed API
// snapshots deliberately do not cover the `profiling` feature.
// Its own docs make the
// argument for the thread-local, and it holds.
#[cfg(feature = "profiling")]
pub mod walkprofile;
#[cfg(not(feature = "profiling"))]
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

/// A decoded image as a pixmap, composed the way a page draw would compose
/// it: its mask applied, its matte removed, a stencil painted black.
///
/// For an image that is not on a page — a `/Thumb`, an embedded file — where
/// there is no graphics state to take a fill colour or a transfer function
/// from.
#[must_use]
pub fn image_to_pixmap(image: &pdfrum_page::ImageData) -> Pixmap {
    image::to_pixmap(image, Argb::BLACK, None)
}
