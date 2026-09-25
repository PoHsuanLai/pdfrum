//! PNG passthrough: a PNG's compressed image data is already what PDF's
//! `/FlateDecode` with the PNG predictors reads (ISO 32000-1 §7.4.4.4), so
//! for the shapes PDF can describe the `IDAT` stream becomes the image
//! stream unchanged — nothing decoded, nothing re-compressed.
//!
//! Those shapes are the ones with no alpha to separate and no interlacing
//! to undo: greyscale, RGB and palette images, not interlaced, at a bit depth
//! PDF allows. Anything else is refused with
//! [`Error::PngNeedsDecoding`], and the caller embeds decoded samples with
//! [`EditDoc::embed_image`] instead, which splits the alpha into a soft mask.

use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, PdfString, Stream};

use super::EmbeddedImage;
use super::jpeg::image_dict;
use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// What the `IHDR` says, and the chunks passthrough needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Png {
    width: u32,
    height: u32,
    depth: u8,
    colour: Colour,
    palette: Vec<u8>,
    transparency: Transparency,
    interlaced: Interlace,
    data: Vec<u8>,
}

/// A PNG colour type (PNG §11.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    Grey,
    Rgb,
    Palette,
    GreyAlpha,
    RgbAlpha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transparency {
    None,
    /// A `tRNS` chunk: colour-keyed or per-palette-entry alpha.
    Keyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interlace {
    None,
    Adam7,
}

pub(super) fn embed(doc: &mut EditDoc<'_>, bytes: &[u8]) -> Result<EmbeddedImage, Error> {
    let png = parse(bytes).ok_or(Error::UnrecognisedImageData)?;
    let components: u8 = match (png.colour, png.transparency, png.interlaced, png.depth) {
        (_, _, Interlace::Adam7, _)
        | (Colour::GreyAlpha | Colour::RgbAlpha, ..)
        | (_, Transparency::Keyed, ..)
        | (_, _, _, 16) => return Err(Error::PngNeedsDecoding),
        (Colour::Grey | Colour::Palette, ..) => 1,
        (Colour::Rgb, ..) => 3,
    };
    let mut dict = image_dict(png.width, png.height);
    let space = match png.colour {
        Colour::Palette => {
            let entries = png.palette.len() / 3;
            if entries == 0 {
                return Err(Error::UnrecognisedImageData);
            }
            Object::Array(Array::of([
                Object::Name(Name::from("Indexed")),
                Object::Name(names::DEVICE_RGB.clone()),
                Object::Int(i64::try_from(entries - 1).unwrap_or(0)),
                Object::Str(PdfString::hex(
                    png.palette.get(..entries * 3).unwrap_or_default(),
                )),
            ]))
        }
        Colour::Grey => Object::Name(names::DEVICE_GRAY.clone()),
        Colour::Rgb | Colour::GreyAlpha | Colour::RgbAlpha => {
            Object::Name(names::DEVICE_RGB.clone())
        }
    };
    dict.push(names::COLOR_SPACE.clone(), space);
    dict.push(
        names::BITS_PER_COMPONENT.clone(),
        Object::Int(i64::from(png.depth)),
    );
    dict.push(
        names::FILTER.clone(),
        Object::Name(names::FLATE_DECODE.clone()),
    );
    dict.push(
        names::DECODE_PARMS.clone(),
        Object::Dict(Dict::from_pairs([
            (Name::from("Predictor"), Object::Int(15)),
            (Name::from("Colors"), Object::Int(i64::from(components))),
            (
                names::BITS_PER_COMPONENT.clone(),
                Object::Int(i64::from(png.depth)),
            ),
            (names::COLUMNS.clone(), Object::Int(i64::from(png.width))),
        ])),
    );
    let (width, height) = (png.width, png.height);
    let image = doc.add(Object::Stream(Box::new(Stream::new(
        dict,
        ByteSpan::from(png.data),
    ))));
    Ok(EmbeddedImage {
        image,
        width,
        height,
    })
}

/// The chunks of a PNG; `None` for anything that is not one.
fn parse(bytes: &[u8]) -> Option<Png> {
    let mut rest = bytes.strip_prefix(&SIGNATURE)?;
    let mut png: Option<Png> = None;
    while rest.len() >= 12 {
        let length = usize::try_from(u32::from_be_bytes(rest.get(..4)?.try_into().ok()?)).ok()?;
        let kind = rest.get(4..8)?;
        let body = rest.get(8..8usize.checked_add(length)?)?;
        rest = rest.get(12usize.checked_add(length)?..)?;
        match kind {
            b"IHDR" => png = Some(header(body)?),
            b"PLTE" => png.as_mut()?.palette = body.to_vec(),
            b"tRNS" => png.as_mut()?.transparency = Transparency::Keyed,
            b"IDAT" => png.as_mut()?.data.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
    }
    png.filter(|png| !png.data.is_empty())
}

fn header(body: &[u8]) -> Option<Png> {
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(body.get(at..at + 4)?.try_into().ok()?))
    };
    let (width, height) = (word(0)?, word(4)?);
    let depth = *body.get(8)?;
    let colour = match body.get(9)? {
        0 => Colour::Grey,
        2 => Colour::Rgb,
        3 => Colour::Palette,
        4 => Colour::GreyAlpha,
        6 => Colour::RgbAlpha,
        _ => return None,
    };
    let interlaced = match body.get(12)? {
        0 => Interlace::None,
        1 => Interlace::Adam7,
        _ => return None,
    };
    (width > 0 && height > 0 && [1, 2, 4, 8, 16].contains(&depth)).then_some(Png {
        width,
        height,
        depth,
        colour,
        palette: Vec::new(),
        transparency: Transparency::None,
        interlaced,
        data: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Colour, parse};

    /// A 1 x 1 RGB PNG: IHDR, one IDAT (a zlib stream of one filter byte and
    /// three samples), IEND. CRCs are not checked by the parser.
    fn one_pixel(colour_type: u8) -> Vec<u8> {
        let chunk = |kind: &[u8], body: &[u8]| {
            let mut out = u32::try_from(body.len())
                .unwrap_or(0)
                .to_be_bytes()
                .to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out.extend_from_slice(&[0, 0, 0, 0]);
            out
        };
        let mut bytes = super::SIGNATURE.to_vec();
        bytes.extend(chunk(
            b"IHDR",
            &[0, 0, 0, 1, 0, 0, 0, 1, 8, colour_type, 0, 0, 0],
        ));
        bytes.extend(chunk(
            b"IDAT",
            &[0x78, 0x01, 0x01, 0x04, 0x00, 0xFB, 0xFF, 0, 255, 0, 0],
        ));
        bytes.extend(chunk(b"IEND", &[]));
        bytes
    }

    #[test]
    fn the_header_and_data_are_read() {
        let png = parse(&one_pixel(2)).expect("a PNG");
        assert_eq!(
            (png.width, png.height, png.depth, png.colour),
            (1, 1, 8, Colour::Rgb)
        );
        assert_eq!(png.data.len(), 11);
    }

    #[test]
    fn junk_is_not_a_png() {
        assert!(parse(b"GIF89a").is_none());
        assert!(parse(&super::SIGNATURE).is_none());
    }
}
