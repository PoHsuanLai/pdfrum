//! Gradient fills, blend modes and PNG passthrough, drawn on a blank page and
//! checked on the page as pdfrum's own rasteriser paints it.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use kurbo::{Affine, Point, Rect};
use pdfrum_common::Limits;
use pdfrum_edit::{
    BlendMode, Canvas, EditDoc, Error, Fill, Gradient, GradientKind, GradientStop, SaveOptions,
    Size, blank_document, save,
};
use peniko::Color;

/// A 100 x 100 pt page drawn by `draw`, saved, reopened and rasterised at
/// 1 px per point; with the file's bytes.
fn painted(
    draw: impl FnOnce(&mut EditDoc<'_>) -> Box<dyn FnOnce(&mut Canvas<'_, '_>)>,
) -> (pdfrum::Pixmap, Vec<u8>) {
    use pdfrum::{RenderOptions, VelloCpuBackend};
    let base = blank_document(&[Size::new(100.0, 100.0)]).expect("a blank page");
    let mut edit = EditDoc::new(&base);
    let body = draw(&mut edit);
    edit.draw_page(0, &Limits::default(), body)
        .expect("the page draws");
    let mut out = Vec::new();
    save(&edit, &SaveOptions::default(), &mut out).expect("the file saves");
    let doc = pdfrum::Document::from_bytes(out.clone()).expect("the file reopens");
    let options = RenderOptions::builder().background(Color::WHITE).build();
    let pixmap = doc
        .page(0u32)
        .expect("page 1")
        .render_with(VelloCpuBackend, &options)
        .expect("the page rasterises");
    (pixmap, out)
}

/// The pixel at (`x`, `y`) in points from the page's top left, as RGB.
fn rgb(pixmap: &pdfrum::Pixmap, x: u32, y: u32) -> [u8; 3] {
    let [red, green, blue, _] = pixmap.pixel(x, y).expect("inside the page");
    [red, green, blue]
}

fn near(got: [u8; 3], want: [u8; 3], tolerance: u8) -> bool {
    got.iter()
        .zip(want)
        .all(|(g, w)| g.abs_diff(w) <= tolerance)
}

fn ramp(kind: GradientKind, from: Color, to: Color) -> Gradient {
    Gradient {
        kind,
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: from,
            },
            GradientStop {
                offset: 1.0,
                color: to,
            },
        ],
    }
}

#[test]
fn a_linear_gradient_runs_from_its_start_to_its_end() {
    let gradient = ramp(
        GradientKind::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
        },
        Color::from_rgb8(255, 0, 0),
        Color::from_rgb8(0, 0, 255),
    );
    let (pixmap, _) = painted(|_| {
        Box::new(move |c| {
            c.fill_gradient(
                Rect::new(0.0, 0.0, 100.0, 100.0),
                Fill::NonZero,
                &gradient,
                Affine::IDENTITY,
            );
        })
    });
    assert!(
        near(rgb(&pixmap, 1, 50), [255, 0, 0], 12),
        "{:?}",
        rgb(&pixmap, 1, 50)
    );
    assert!(
        near(rgb(&pixmap, 98, 50), [0, 0, 255], 12),
        "{:?}",
        rgb(&pixmap, 98, 50)
    );
    assert!(
        near(rgb(&pixmap, 50, 50), [128, 0, 128], 16),
        "{:?}",
        rgb(&pixmap, 50, 50)
    );
}

#[test]
fn a_fading_gradient_fades_to_the_page_not_to_black() {
    let gradient = ramp(
        GradientKind::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
        },
        Color::from_rgba8(255, 0, 0, 255),
        Color::from_rgba8(255, 0, 0, 0),
    );
    let (pixmap, _) = painted(|_| {
        Box::new(move |c| {
            c.fill_gradient(
                Rect::new(0.0, 0.0, 100.0, 100.0),
                Fill::NonZero,
                &gradient,
                Affine::IDENTITY,
            );
        })
    });
    assert!(
        near(rgb(&pixmap, 1, 50), [255, 0, 0], 12),
        "{:?}",
        rgb(&pixmap, 1, 50)
    );
    assert!(
        near(rgb(&pixmap, 98, 50), [255, 255, 255], 12),
        "{:?}",
        rgb(&pixmap, 98, 50)
    );
    assert!(
        near(rgb(&pixmap, 50, 50), [255, 128, 128], 20),
        "{:?}",
        rgb(&pixmap, 50, 50)
    );
}

#[test]
fn a_radial_gradient_runs_out_from_its_centre() {
    let gradient = ramp(
        GradientKind::Radial {
            start_center: Point::new(50.0, 50.0),
            start_radius: 0.0,
            end_center: Point::new(50.0, 50.0),
            end_radius: 50.0,
        },
        Color::from_rgb8(0, 200, 0),
        Color::from_rgb8(0, 0, 0),
    );
    let (pixmap, _) = painted(|_| {
        Box::new(move |c| {
            c.fill_gradient(
                Rect::new(0.0, 0.0, 100.0, 100.0),
                Fill::NonZero,
                &gradient,
                Affine::IDENTITY,
            );
        })
    });
    assert!(
        near(rgb(&pixmap, 50, 50), [0, 200, 0], 12),
        "{:?}",
        rgb(&pixmap, 50, 50)
    );
    assert!(
        near(rgb(&pixmap, 99, 50), [0, 0, 0], 12),
        "{:?}",
        rgb(&pixmap, 99, 50)
    );
}

#[test]
fn the_gradient_is_clipped_to_its_shape() {
    let gradient = ramp(
        GradientKind::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
        },
        Color::BLACK,
        Color::BLACK,
    );
    let (pixmap, _) = painted(|_| {
        Box::new(move |c| {
            c.fill_gradient(
                Rect::new(20.0, 20.0, 40.0, 40.0),
                Fill::NonZero,
                &gradient,
                Affine::IDENTITY,
            );
        })
    });
    // Canvas y is up: the square spans rows 60-80 from the top.
    assert!(near(rgb(&pixmap, 30, 70), [0, 0, 0], 12));
    assert!(near(rgb(&pixmap, 60, 30), [255, 255, 255], 12));
}

#[test]
fn multiply_darkens_what_is_underneath() {
    let (pixmap, _) = painted(|_| {
        Box::new(|c| {
            c.fill_rect(
                Rect::new(0.0, 0.0, 100.0, 100.0),
                Color::from_rgb8(0, 255, 255),
            );
            c.saved(|c| {
                c.blend(BlendMode::Multiply);
                c.fill_rect(
                    Rect::new(0.0, 0.0, 50.0, 100.0),
                    Color::from_rgb8(255, 255, 0),
                );
            });
        })
    });
    assert!(
        near(rgb(&pixmap, 25, 50), [0, 255, 0], 12),
        "{:?}",
        rgb(&pixmap, 25, 50)
    );
    assert!(
        near(rgb(&pixmap, 75, 50), [0, 255, 255], 12),
        "{:?}",
        rgb(&pixmap, 75, 50)
    );
}

/// A `width` x `height` 8-bit PNG of `colour_type` (2 RGB, 6 RGBA), each
/// pixel `pixel`.
fn png(width: u32, height: u32, colour: png::ColorType, pixel: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(colour);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        let data: Vec<u8> = (0..width * height)
            .flat_map(|_| pixel.iter().copied())
            .collect();
        writer.write_image_data(&data).expect("the samples");
    }
    bytes
}

/// The concatenated `IDAT` bodies of a PNG.
fn idat(bytes: &[u8]) -> Vec<u8> {
    let mut rest = &bytes[8..];
    let mut out = Vec::new();
    while rest.len() >= 12 {
        let length = u32::from_be_bytes(rest[..4].try_into().expect("four bytes")) as usize;
        if &rest[4..8] == b"IDAT" {
            out.extend_from_slice(&rest[8..8 + length]);
        }
        rest = &rest[12 + length..];
    }
    out
}

#[test]
fn an_rgb_png_passes_through_as_stored() {
    let source = png(8, 4, png::ColorType::Rgb, &[30, 144, 255]);
    let (pixmap, file) = painted(|edit| {
        let image = edit.embed_png(&source).expect("an RGB PNG passes through");
        Box::new(move |c| c.image(&image, Rect::new(0.0, 0.0, 100.0, 100.0)))
    });
    assert!(
        near(rgb(&pixmap, 50, 50), [30, 144, 255], 4),
        "{:?}",
        rgb(&pixmap, 50, 50)
    );
    let data = idat(&source);
    let found = file
        .windows(data.len())
        .filter(|window| *window == data.as_slice())
        .count();
    assert_eq!(
        found, 1,
        "the compressed samples are in the file once, as they were"
    );
}

#[test]
fn a_png_with_alpha_asks_to_be_decoded() {
    let source = png(2, 2, png::ColorType::Rgba, &[1, 2, 3, 128]);
    let base = blank_document(&[Size::new(10.0, 10.0)]).expect("a blank page");
    let mut edit = EditDoc::new(&base);
    assert!(matches!(
        edit.embed_png(&source),
        Err(Error::PngNeedsDecoding)
    ));
}
