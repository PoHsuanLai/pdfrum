//! The one thing this crate can fail at.
//!
//! Nothing here describes damaged input — damage is recovered and recorded in
//! a [`Diagnostics`](pdfrum_common::Diagnostics) sink. These variants all mean
//! "no bytes can be produced": a size that cannot be represented, a size the
//! caller forbade, or one of the two shapes PDFium itself refuses.

/// Why a filter could not produce any bytes at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Decoding would produce more than `Limits::max_decoded_stream_len`
    /// bytes. PDFium has no such cap; see the crate docs.
    #[error("decoded output would exceed the {limit}-byte limit")]
    OutputTooLarge {
        /// The cap that was exceeded, in bytes.
        limit: usize,
    },
    /// A run-length stream declares an output at or past its own 20 MiB cap,
    /// which PDFium rejects before allocating anything.
    #[error("run-length stream decodes to {size} bytes, at or over the 20 MiB limit")]
    RunLengthTooLarge {
        /// The output size the stream's run headers add up to.
        size: u64,
    },
    /// An LZW stream is malformed in one of the two ways PDFium rejects: it
    /// opens with a dictionary code that no literal has defined, or it decodes
    /// to nothing at all.
    #[error("LZW stream is malformed: {0}")]
    LzwMalformed(&'static str),
    /// `/DecodeParms` cannot describe a row of samples.
    #[error("predictor parameters are invalid: {0}")]
    BadPredictorParams(&'static str),
    /// A size computation over untrusted numbers overflowed. PDFium aborts the
    /// process at several of these points; we return instead.
    #[error("size computation overflowed")]
    SizeOverflow,
    /// CCITT parameters name an image no decoder can produce.
    #[error("CCITT parameters are out of range: {0}")]
    BadCcittParams(&'static str),
}
