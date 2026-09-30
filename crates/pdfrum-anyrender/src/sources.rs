//! What the caller knows that anyrender does not carry: the text behind each glyph run and the
//! encoded bytes behind each decoded image.

use crate::run_text::RunTexts;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Everything a scene may look up while it paints; built once per document, shared by pages.
#[derive(Debug, Default)]
pub struct Sources {
    /// The text behind each glyph run, where the caller could say.
    pub texts: RunTexts,
    /// The encoded source of each image the caller could trace, by the id of its decoded blob.
    pub images: ImageSources,
}

/// How an image's source is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageCodec {
    /// Written to the PDF as it is (DCTDecode): never re-encoded, never larger than the source.
    Jpeg,
    /// Decoded and written losslessly (Flate), with its alpha as a soft mask.
    Png,
}

impl ImageCodec {
    /// The codec `bytes` start with, by their signature; `None` for anything else, which is
    /// then drawn from its decoded pixels.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [0xFF, 0xD8, 0xFF, ..] => Some(ImageCodec::Jpeg),
            [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', ..] => Some(ImageCodec::Png),
            _ => None,
        }
    }
}

/// An image's bytes as they arrived, before anything decoded them.
#[derive(Clone)]
pub struct EncodedImage {
    codec: ImageCodec,
    bytes: Arc<Vec<u8>>,
}

impl EncodedImage {
    /// `bytes` if they are a JPEG or a PNG.
    pub fn sniffed(bytes: Vec<u8>) -> Option<Self> {
        let codec = ImageCodec::sniff(&bytes)?;
        Some(EncodedImage {
            codec,
            bytes: Arc::new(bytes),
        })
    }

    /// How the bytes are encoded.
    pub fn codec(&self) -> ImageCodec {
        self.codec
    }

    /// The bytes themselves.
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }
}

impl fmt::Debug for EncodedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedImage")
            .field("codec", &self.codec)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// Encoded sources keyed by the id of the decoded blob a brush paints
/// (`peniko::Blob::id`): a renderer decodes once and paints that blob wherever the image
/// appears, so the id names the image across every draw.
#[derive(Debug, Default)]
pub struct ImageSources {
    by_blob: HashMap<u64, EncodedImage>,
}

impl ImageSources {
    /// Record that the blob `blob` was decoded from `source`.
    pub fn insert(&mut self, blob: u64, source: EncodedImage) {
        self.by_blob.insert(blob, source);
    }

    /// The source of the blob `blob`, if it was recorded.
    pub fn get(&self, blob: u64) -> Option<&EncodedImage> {
        self.by_blob.get(&blob)
    }

    /// How many images have a recorded source.
    pub fn len(&self) -> usize {
        self.by_blob.len()
    }

    /// Whether no image has a recorded source.
    pub fn is_empty(&self) -> bool {
        self.by_blob.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ImageCodec;

    const CASES: &[(&[u8], Option<ImageCodec>)] = &[
        (&[0xFF, 0xD8, 0xFF, 0xE0], Some(ImageCodec::Jpeg)),
        (b"\x89PNG\r\n\x1a\nrest", Some(ImageCodec::Png)),
        (b"GIF89a", None),
        (b"", None),
    ];

    #[test]
    fn codecs_are_sniffed_by_signature() {
        for &(bytes, want) in CASES {
            assert_eq!(ImageCodec::sniff(bytes), want, "{bytes:?}");
        }
    }
}
