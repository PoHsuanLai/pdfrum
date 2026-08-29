//! The crate's one error type.
//!
//! Almost nothing here fails: content parsing is infallible by contract
//! (a bad operator becomes a diagnostic and is skipped), colorspace and
//! function loading answer `Option` because "this file has no such
//! colorspace" is a normal state of affairs in PDFium's world, and page
//! building always produces a `Page`. `Error` is reserved for the two places
//! where a caller genuinely cannot continue: evaluating a function against
//! the wrong number of values, and decoding an image whose pixels cannot be
//! produced at all.

use pdfrum_object::ObjRef;

/// What went wrong in a page-level operation that can actually fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A function was evaluated with a number of inputs its `/Domain` does
    /// not describe, or an output slice shorter than its `/Range`
    /// (ISO 32000-1 §7.10.1).
    #[error("function takes {expected} inputs and {outputs} outputs, got {got} and {got_outputs}")]
    FunctionArity {
        /// Inputs the function declares.
        expected: usize,
        /// Inputs the caller supplied.
        got: usize,
        /// Outputs the function declares.
        outputs: usize,
        /// Length of the caller's output slice.
        got_outputs: usize,
    },
    /// A function's `/Domain` names an interval whose low bound exceeds its
    /// high bound, which PDFium refuses to evaluate against.
    #[error("function domain or range interval is inverted")]
    FunctionInterval,
    /// An image dictionary's `/Width`, `/Height` or `/BitsPerComponent` is
    /// outside the range PDFium accepts (`docs/design/pdfrum-page.md` §1.19.1).
    #[error("image dimensions or bit depth are not usable: {what}")]
    ImageBadDict {
        /// Which value was rejected.
        what: &'static str,
    },
    /// No colorspace could be resolved for a non-mask image, so its samples
    /// cannot be turned into pixels.
    #[error("image has no usable colorspace")]
    ImageNoColorSpace,
    /// The image's sample data could not be produced: a filter with no
    /// decoder, a codec that rejected the stream, or a chain that decoded to
    /// fewer bytes than one scanline needs.
    #[error("image data could not be decoded: {what}")]
    ImageUndecodable {
        /// Which stage gave up.
        what: &'static str,
    },
    /// The image is larger than `Limits::max_image_bytes` allows, or its
    /// pitch computation overflowed.
    #[error("image needs more than the configured byte budget")]
    ImageTooLarge,
    /// A JBIG2 or JPEG 2000 codec rejected its input (SPEC.md §12).
    #[error("{codec} could not decode the embedded image")]
    CodecRejected {
        /// `"JBIG2"` or `"JPX"`.
        codec: &'static str,
    },
    /// An indirect object a page-level structure needed could not be fetched.
    #[error("object {0:?} could not be resolved")]
    Unresolved(ObjRef),
}
