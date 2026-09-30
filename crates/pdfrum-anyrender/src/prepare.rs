//! Everything the pages draw with, embedded once before any page is drawn: a pdfrum canvas
//! names fonts and images the session already holds, so the scenes are walked first for the
//! faces and images they use.

use crate::faces::{FaceKey, Metrics};
use crate::sources::{EncodedImage, ImageCodec, ImageSources};
use crate::write::Page;
use anyrender::Paint;
use anyrender::recording::RenderCommand;
use pdfrum_edit::{EditDoc, EmbeddedImage, Error, FontInstance, GlyphFont, PixelFormat};
use pdfrum_object::ByteSpan;
use peniko::{ImageAlphaType, ImageData, ImageFormat};
use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// The faces and images of a set of pages, embedded in their session.
pub(crate) struct Prepared {
    pub(crate) faces: HashMap<FaceKey, (GlyphFont, Metrics)>,
    pub(crate) images: HashMap<u64, EmbeddedImage>,
}

impl Prepared {
    /// Embed in `edit` every face and image `pages` use.
    ///
    /// # Errors
    ///
    /// Whatever pdfrum refuses: a face it cannot read or instance, an image it cannot embed.
    pub(crate) fn new(
        edit: &mut EditDoc<'_>,
        pages: &[Page],
        images: &ImageSources,
    ) -> Result<Self, Error> {
        let mut prepared = Prepared {
            faces: HashMap::new(),
            images: HashMap::new(),
        };
        for command in pages.iter().flat_map(|page| &page.scene.commands) {
            match command {
                RenderCommand::GlyphRun(run) => {
                    let key = FaceKey::of(&run.font_data, &run.normalized_coords);
                    if prepared.faces.contains_key(&key) {
                        continue;
                    }
                    let Some(metrics) = Metrics::load(&run.font_data, &run.normalized_coords)
                    else {
                        continue;
                    };
                    let font = edit.embed_glyph_font(
                        ByteSpan::from_owner(run.font_data.data.clone()),
                        run.font_data.index,
                        FontInstance::Normalized(run.normalized_coords.clone()),
                    )?;
                    prepared.faces.insert(key, (font, metrics));
                }
                RenderCommand::Fill(fill) => {
                    if let Paint::Image(brush) = &fill.brush
                        && let Entry::Vacant(slot) = prepared.images.entry(brush.image.data.id())
                    {
                        let source = images.get(*slot.key());
                        slot.insert(embed(edit, source, &brush.image)?);
                    }
                }
                _ => {}
            }
        }
        Ok(prepared)
    }
}

/// `decoded` as an image: from its source when there is one that PDF can take as stored and
/// that has the decoded size, else from its pixels.
fn embed(
    edit: &mut EditDoc<'_>,
    source: Option<&EncodedImage>,
    decoded: &ImageData,
) -> Result<EmbeddedImage, Error> {
    let stored = source.and_then(|source| {
        let image = match source.codec() {
            ImageCodec::Jpeg => edit.embed_jpeg(source.bytes()),
            ImageCodec::Png => edit.embed_png(source.bytes()),
        }
        .ok()?;
        // A source a renderer rotated or scaled while decoding is not the image painted.
        (image.width() == decoded.width && image.height() == decoded.height).then_some(image)
    });
    match stored {
        Some(image) => Ok(image),
        None => edit.embed_image(
            &straight_rgba(decoded),
            decoded.width,
            decoded.height,
            PixelFormat::Rgba8,
        ),
    }
}

/// The decoded pixels as straight-alpha RGBA, which is what an `/SMask` split wants.
fn straight_rgba(decoded: &ImageData) -> Vec<u8> {
    let (quads, _) = decoded.data.data().as_chunks::<4>();
    quads
        .iter()
        .flat_map(|pixel| straight(*pixel, decoded.format, decoded.alpha_type))
        .collect()
}

/// One pixel as straight RGBA.
fn straight(pixel: [u8; 4], format: ImageFormat, alpha: ImageAlphaType) -> [u8; 4] {
    let [r, g, b, a] = match format {
        ImageFormat::Bgra8 => [pixel[2], pixel[1], pixel[0], pixel[3]],
        _ => pixel,
    };
    match (alpha, a) {
        (ImageAlphaType::AlphaPremultiplied, 1..=254) => {
            let unmultiply = |channel: u8| (u16::from(channel) * 255 / u16::from(a)).min(255) as u8;
            [unmultiply(r), unmultiply(g), unmultiply(b), a]
        }
        _ => [r, g, b, a],
    }
}

#[cfg(test)]
mod tests {
    use super::straight;
    use peniko::{ImageAlphaType, ImageFormat};

    type Case = (([u8; 4], ImageFormat, ImageAlphaType), [u8; 4]);

    const CASES: &[Case] = &[
        (
            ([1, 2, 3, 4], ImageFormat::Rgba8, ImageAlphaType::Alpha),
            [1, 2, 3, 4],
        ),
        (
            ([1, 2, 3, 4], ImageFormat::Bgra8, ImageAlphaType::Alpha),
            [3, 2, 1, 4],
        ),
        (
            (
                [50, 0, 100, 127],
                ImageFormat::Rgba8,
                ImageAlphaType::AlphaPremultiplied,
            ),
            [100, 0, 200, 127],
        ),
        (
            (
                [9, 9, 9, 0],
                ImageFormat::Rgba8,
                ImageAlphaType::AlphaPremultiplied,
            ),
            [9, 9, 9, 0],
        ),
    ];

    #[test]
    fn pixels_become_straight_rgba() {
        for &((pixel, format, alpha), want) in CASES {
            assert_eq!(straight(pixel, format, alpha), want, "{pixel:?} {format:?}");
        }
    }
}
