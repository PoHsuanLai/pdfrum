//! Embed caller-supplied pixels, or a compressed codestream, as an image
//! `XObject`.
//!
//! A caller hands over bytes — a JPEG codestream or raw interleaved samples —
//! and this allocates the `/XObject` a content stream's `Do` can name.

mod jpeg;
mod png;
mod raw;

use pdfrum_object::ObjRef;

use crate::doc::EditDoc;
use crate::error::Error;

pub use raw::PixelFormat;

/// An image `XObject` this session added, ready to place on a page.
///
/// [`EmbeddedImage::object`] is the `/XObject` a page resource names, and what
/// `ImageBuilder::at` in the facade takes. The dimensions come back because
/// they are the caller's only statement of the aspect ratio the placement
/// rectangle should keep — a JPEG's are read out of its header, not supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedImage {
    image: ObjRef,
    width: u32,
    height: u32,
}

impl EmbeddedImage {
    /// The `/XObject` to name from a page resource.
    #[must_use]
    pub fn object(&self) -> ObjRef {
        self.image
    }

    /// Width in samples.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in samples.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }
}

impl EditDoc<'_> {
    /// Embed a JPEG or JPEG 2000 codestream as a new image `XObject`.
    ///
    /// The bytes become the stream verbatim under `/DCTDecode` or
    /// `/JPXDecode`; nothing is decoded or re-encoded. `/Width`, `/Height`,
    /// `/ColorSpace` and `/BitsPerComponent` are read from the codestream's
    /// own header, which is the file's statement of them and overrides any a
    /// caller could pass.
    ///
    /// # Errors
    ///
    /// [`Error::UnrecognisedImageData`] when the bytes are neither a JPEG nor
    /// a JPEG 2000 codestream, or their header cannot be read.
    pub fn embed_jpeg(&mut self, bytes: &[u8]) -> Result<EmbeddedImage, Error> {
        jpeg::embed(self, bytes)
    }

    /// Embed a PNG as a new image `XObject`, its compressed data passed
    /// through unchanged.
    ///
    /// PNG's `IDAT` stream is what `/FlateDecode` with the PNG predictors
    /// reads, so a greyscale, RGB or palette PNG that is not interlaced is
    /// stored as it came: no decode, no re-compression, no larger than the
    /// file. `/Width`, `/Height`, `/ColorSpace` and `/BitsPerComponent` come
    /// from its `IHDR` (and `PLTE`).
    ///
    /// # Errors
    ///
    /// [`Error::PngNeedsDecoding`] for a PNG PDF cannot take as stored — one
    /// with an alpha channel or `tRNS` transparency (PDF keeps alpha in a
    /// separate soft mask), an interlaced one, or 16 bits deep: decode it and
    /// use [`EditDoc::embed_image`]. [`Error::UnrecognisedImageData`] when
    /// the bytes are not a PNG.
    ///
    /// ```
    /// use pdfrum_edit::{EditDoc, Error, Size, blank_document};
    ///
    /// let base = blank_document(&[Size::new(100.0, 100.0)])?;
    /// let mut edit = EditDoc::new(&base);
    /// assert!(matches!(edit.embed_png(b"not a png"), Err(Error::UnrecognisedImageData)));
    /// # Ok::<(), pdfrum_edit::Error>(())
    /// ```
    pub fn embed_png(&mut self, bytes: &[u8]) -> Result<EmbeddedImage, Error> {
        png::embed(self, bytes)
    }

    /// Embed raw interleaved samples as a new image `XObject`.
    ///
    /// The samples are stored uncompressed and the stream writer flate-encodes
    /// them (there is no `/Filter` on the dictionary this writes). An
    /// [`PixelFormat::Rgba8`] alpha channel is split off into a separate
    /// `/DeviceGray` `/SMask` image; the colour channels keep their own
    /// stream.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyImage`] when either dimension is zero, and
    /// [`Error::ImageDataLength`] when `pixels` is not exactly the length the
    /// dimensions and format require.
    pub fn embed_image(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<EmbeddedImage, Error> {
        raw::embed(self, pixels, width, height, format)
    }
}
