//! What can go wrong badly enough to abandon a render.
//!
//! Almost nothing does. Rendering a damaged page is the normal case, and
//! every recovery — an undrawable matrix, a shading whose function will not
//! evaluate, a pattern whose steps are degenerate — is a skipped object and a
//! [`Diagnostic`](pdfrum_common::Diagnostic), not an error. `Err` is reserved
//! for "there is no output at all".

/// A render that could not produce a pixmap.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum Error {
    /// The requested target exceeds what a backend can allocate.
    ///
    /// `vello_cpu` sizes every scene, pixmap and mask with `u16`, so 65535 is
    /// the hard ceiling in either axis; the bound is enforced in the engine so
    /// both backends agree rather than one of them failing later.
    #[error("render target {width}x{height} exceeds the {limit}px backend limit")]
    TargetTooLarge {
        /// The requested width in pixels.
        width: u32,
        /// The requested height in pixels.
        height: u32,
        /// The largest dimension a backend accepts.
        limit: u32,
    },

    /// The requested target has a zero axis, so there is nothing to render
    /// into.
    #[error("render target {width}x{height} is empty")]
    TargetEmpty {
        /// The requested width in pixels.
        width: u32,
        /// The requested height in pixels.
        height: u32,
    },
}
