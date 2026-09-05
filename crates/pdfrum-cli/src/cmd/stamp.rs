//! `pdfrum stamp text|image`: a mark on every page, drawn over the content.
//!
//! A stamp is page content, not an annotation: one more content stream
//! after the page's own, so it paints on top and under any annotation a
//! viewer draws. Placement is on the page as displayed, so a corner on a
//! turned page is that corner on the screen. The facade stamps every page
//! of the document; there is no page selection here because there is none
//! in [`DocEdit::stamp_text`](pdfrum::DocEdit::stamp_text).

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use pdfrum::{Color, Document, StampOptions, StampPosition, StandardFont};

use crate::cmd::pages::{self, Decoded, save_options};
use crate::out;
use crate::term::Term;

/// `--position`: where the stamp sits on the page as displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum Position {
    /// Centred on the page.
    #[default]
    Center,
    /// The top-left corner, half an inch in.
    TopLeft,
    /// The top-right corner, half an inch in.
    TopRight,
    /// The bottom-left corner, half an inch in.
    BottomLeft,
    /// The bottom-right corner, half an inch in.
    BottomRight,
}

impl From<Position> for StampPosition {
    fn from(position: Position) -> Self {
        match position {
            Position::Center => StampPosition::Center,
            Position::TopLeft => StampPosition::TopLeft,
            Position::TopRight => StampPosition::TopRight,
            Position::BottomLeft => StampPosition::BottomLeft,
            Position::BottomRight => StampPosition::BottomRight,
        }
    }
}

/// The standard 14 by name, as the specification spells them.
const FONTS: [(&str, StandardFont); 14] = [
    ("Courier", StandardFont::Courier),
    ("Courier-Bold", StandardFont::CourierBold),
    ("Courier-BoldOblique", StandardFont::CourierBoldOblique),
    ("Courier-Oblique", StandardFont::CourierOblique),
    ("Helvetica", StandardFont::Helvetica),
    ("Helvetica-Bold", StandardFont::HelveticaBold),
    ("Helvetica-BoldOblique", StandardFont::HelveticaBoldOblique),
    ("Helvetica-Oblique", StandardFont::HelveticaOblique),
    ("Times-Roman", StandardFont::Times),
    ("Times-Bold", StandardFont::TimesBold),
    ("Times-BoldItalic", StandardFont::TimesBoldOblique),
    ("Times-Italic", StandardFont::TimesOblique),
    ("Symbol", StandardFont::Symbol),
    ("ZapfDingbats", StandardFont::Dingbats),
];

/// `--font`: one of the standard 14, by its name.
pub fn parse_font(name: &str) -> Result<StandardFont> {
    FONTS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, font)| *font)
        .with_context(|| {
            format!(
                "{name:?} is not one of the standard 14 fonts: {}",
                FONTS.map(|(known, _)| known).join(", ")
            )
        })
}

/// `--rgb`: `RRGGBB`, with or without a leading `#`.
pub fn parse_color(text: &str) -> Result<Color> {
    let hex = text.trim().trim_start_matches('#');
    let channel = |at: usize| {
        hex.get(at..at + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
    };
    match (hex.len(), channel(0), channel(2), channel(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok(Color::from_rgb8(r, g, b)),
        _ => bail!("{text:?} is not a colour; give RRGGBB, e.g. ff0000"),
    }
}

/// Where and how the mark is drawn: the fields text and image share.
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub position: Position,
    /// `0.0` (invisible) to `1.0` (opaque).
    pub opacity: f32,
    /// Degrees counter-clockwise about the stamp's centre.
    pub angle: f64,
}

/// The text's own: size, colour and face, the last two as typed.
#[derive(Debug, Clone, Copy)]
pub struct Type<'a> {
    pub size: f32,
    pub color: &'a str,
    pub font: &'a str,
}

/// The facade's options from what was typed, every value checked.
fn options(mark: Mark, text: Option<Type<'_>>) -> Result<StampOptions> {
    if !(0.0..=1.0).contains(&mark.opacity) {
        bail!(
            "--opacity is 0 (invisible) to 1 (opaque), not {}",
            mark.opacity
        );
    }
    if !mark.angle.is_finite() {
        bail!("--angle must be a number of degrees");
    }
    let mut options = StampOptions {
        position: mark.position.into(),
        opacity: mark.opacity,
        angle: mark.angle,
        ..StampOptions::default()
    };
    if let Some(text) = text {
        if !(text.size.is_finite() && text.size > 0.0) {
            bail!("--size must be a positive number of points");
        }
        options.font_size = text.size;
        options.color = parse_color(text.color)?;
        options.font = parse_font(text.font)?;
    }
    Ok(options)
}

/// `doc` with `text` drawn over every page, as the bytes of a saved file,
/// and how many pages that is.
pub fn text_bytes(
    doc: &Document,
    text: &str,
    mark: Mark,
    type_: Type<'_>,
    deterministic: bool,
) -> Result<(Vec<u8>, u32)> {
    if text.trim().is_empty() {
        bail!("the stamp text is empty");
    }
    let options = options(mark, Some(type_))?;
    let mut edit = doc.edit();
    edit.stamp_text(text, &options)?;
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, doc.page_count()))
}

/// `doc` with `image` drawn over every page, `width` points wide (its
/// pixel width by default) with its aspect kept, as the bytes of a saved
/// file, and how many pages that is.
pub fn image_bytes(
    doc: &Document,
    image: &Decoded,
    width: Option<f64>,
    mark: Mark,
    deterministic: bool,
) -> Result<(Vec<u8>, u32)> {
    let options = options(mark, None)?;
    let width = width.unwrap_or(f64::from(image.width));
    if !(width.is_finite() && width > 0.0) {
        bail!("--width must be a positive number of points");
    }
    let mut edit = doc.edit();
    let embedded = pages::embed(&mut edit, image)?;
    edit.stamp_image(&embedded, width, &options)?;
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, doc.page_count()))
}

/// What `stamp text` was asked for.
pub struct TextRequest<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub text: &'a str,
    pub mark: Mark,
    pub type_: Type<'a>,
    pub output: &'a Path,
    pub deterministic: bool,
}

/// `stamp text`: the text over every page.
pub fn text(req: &TextRequest<'_>, term: Term) -> Result<ExitCode> {
    let sink = out::Sink::new(req.output, "PDF")?;
    let doc = out::open(req.file, req.password)?;
    let (bytes, pages) = text_bytes(&doc, req.text, req.mark, req.type_, req.deterministic)?;
    sink.finish(term, &bytes, &stamped(pages), None)?;
    Ok(ExitCode::SUCCESS)
}

/// What `stamp image` was asked for.
pub struct ImageRequest<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub image: &'a Path,
    pub width: Option<f64>,
    pub mark: Mark,
    pub output: &'a Path,
    pub deterministic: bool,
}

/// `stamp image`: a JPEG or PNG over every page.
pub fn image(req: &ImageRequest<'_>, term: Term) -> Result<ExitCode> {
    let sink = out::Sink::new(req.output, "PDF")?;
    let decoded = pages::decode(req.image)?;
    let doc = out::open(req.file, req.password)?;
    let (bytes, pages) = image_bytes(&doc, &decoded, req.width, req.mark, req.deterministic)?;
    sink.finish(term, &bytes, &stamped(pages), None)?;
    Ok(ExitCode::SUCCESS)
}

/// The summary: `stamped 11 pages`.
fn stamped(pages: u32) -> String {
    format!("stamped {pages} page{}", if pages == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use pdfrum::{Color, StampPosition, StandardFont};

    use super::{Mark, Position, Type, options, parse_color, parse_font};

    #[test]
    fn fonts_and_colours_parse_by_name_and_refuse_the_rest() {
        assert_eq!(parse_font("Times-Roman").unwrap(), StandardFont::Times);
        assert_eq!(
            parse_font("Helvetica-BoldOblique").unwrap(),
            StandardFont::HelveticaBoldOblique
        );
        let err = parse_font("Arial").unwrap_err().to_string();
        assert!(err.contains("standard 14") && err.contains("ZapfDingbats"));
        assert_eq!(
            parse_color("#FF0080").unwrap(),
            Color::from_rgb8(255, 0, 128)
        );
        assert_eq!(parse_color("000000").unwrap(), Color::BLACK);
        assert!(parse_color("red").is_err());
        assert!(parse_color("fff").is_err());
        assert!(parse_color("gg0000").is_err());
    }

    #[test]
    fn the_options_carry_every_field_and_refuse_a_value_out_of_range() {
        let mark = Mark {
            position: Position::TopRight,
            opacity: 0.5,
            angle: 30.0,
        };
        let type_ = Type {
            size: 24.0,
            color: "ff0000",
            font: "Courier",
        };
        let o = options(mark, Some(type_)).unwrap();
        assert_eq!(o.position, StampPosition::TopRight);
        assert_eq!((o.opacity, o.angle, o.font_size), (0.5, 30.0, 24.0));
        assert_eq!(o.font, StandardFont::Courier);
        assert_eq!(o.color, Color::from_rgb8(255, 0, 0));
        let picture = options(mark, None).unwrap();
        assert_eq!(picture.font, StandardFont::Helvetica, "the default");
        assert!(
            options(
                Mark {
                    opacity: 1.5,
                    ..mark
                },
                None
            )
            .is_err()
        );
        assert!(
            options(
                Mark {
                    angle: f64::NAN,
                    ..mark
                },
                None
            )
            .is_err()
        );
        assert!(options(mark, Some(Type { size: 0.0, ..type_ })).is_err());
    }
}
