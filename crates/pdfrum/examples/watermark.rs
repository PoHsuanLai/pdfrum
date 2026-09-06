//! Draw a diagonal translucent watermark across every page.
//!
//! ```text
//! cargo run --example watermark -- tests/fixtures/hello_world_2_pages.pdf out.pdf DRAFT
//! ```
//!
//! The whole watermark is the [`run`] body below — under twenty lines, and
//! none of them a content-stream operator. The canvas's coordinate space is
//! the page as displayed, so `c.bounds().center()` is the middle of the page
//! a reader sees, whatever the crop box and `/Rotate` say.

use std::path::Path;
use std::process::ExitCode;

use pdfrum::{Affine, Color, Document, Point, SaveOptions, StandardFont};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: watermark <input.pdf> <output.pdf> [text]");
        return ExitCode::FAILURE;
    };
    let text = args.get(2).map_or("DRAFT", String::as_str);

    match run(Path::new(input), Path::new(output), text) {
        Ok(pages) => {
            println!("watermarked {pages} page(s) into {output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, output: &Path, text: &str) -> Result<u32, pdfrum::Error> {
    let doc = Document::open(input)?;
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica)?;

    edit.draw_pages(|c| {
        let centre = c.bounds().center();
        let size = c.size().width / 5.0;
        let width = c.text_width(text, &font, size);
        c.saved(|c| {
            c.opacity(0.15);
            c.transform(Affine::rotate_about(0.6, centre));
            let at = Point::new(centre.x - width / 2.0, centre.y - size / 3.0);
            c.text(text, &font, size, at, Color::from_rgb8(180, 0, 0));
        });
    })?;

    edit.save(output, &SaveOptions::default())?;
    Ok(doc.page_count())
}
