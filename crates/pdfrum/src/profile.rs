//! The three stages `Page::paint` times, into `pdfrum-page`'s accumulator
//! under this crate's `profiling`.
//!
//! A module of its own so `paint` and `build` read as the render rather than
//! as the instrument, and so the feature-off build names no `renderprofile`
//! item at all: that module is `pub` in `pdfrum-page` only with the feature,
//! and a crate cannot `#[cfg]` on another crate's flag.
//!
//! The stage set here is *this crate's three* rather than all eight. The other
//! five are the annotation and appearance pass's, timed in `pdfrum-doc` where
//! that pass lives, and a name nothing in this crate can reach is a name this
//! crate should not carry.

#[cfg(feature = "profiling")]
pub(crate) use pdfrum_page::renderprofile::{Stage, stage};

/// The three stages this crate names, without the feature.
#[cfg(not(feature = "profiling"))]
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stage {
    /// The `/Contents` decode and the operator scan.
    ContentParse,
    /// Folding the operators into a page-object graph.
    Interpretation,
    /// The rasterizing walk.
    Raster,
}

/// The body, unclocked, without the feature.
#[cfg(not(feature = "profiling"))]
#[inline]
pub(crate) fn stage<T>(_stage: Stage, body: impl FnOnce() -> T) -> T {
    body()
}
