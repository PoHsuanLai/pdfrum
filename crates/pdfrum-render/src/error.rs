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
    /// [`RenderSession::deadline`](crate::RenderSession::deadline) passed:
    /// before the target was allocated, or during the walk, which stops at
    /// the next object and reports here rather than handing back a page with
    /// the rest missing.
    #[error(transparent)]
    Limit(pdfrum_common::LimitExceeded),
    /// A PNG could not be encoded — only with the `png` feature. The
    /// encoder's own message, since its error type is neither comparable nor
    /// cloneable and this one is both.
    #[cfg(feature = "png")]
    #[error("PNG encoding failed: {0}")]
    Png(String),
    /// A file could not be written — only with the `png` feature; the I/O
    /// error's message.
    #[cfg(feature = "png")]
    #[error("writing the file failed: {0}")]
    Io(String),
}

#[cfg(feature = "png")]
impl From<png::EncodingError> for Error {
    fn from(error: png::EncodingError) -> Self {
        Error::Png(error.to_string())
    }
}

#[cfg(feature = "png")]
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error.to_string())
    }
}
