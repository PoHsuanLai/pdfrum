//! Draw a header rule with a page number on every page.
//!
//! ```text
//! cargo run --example header-rule -- tests/fixtures/hello_world_2_pages.pdf out.pdf "Quarterly report"
//! ```
//!
//! The other half of the canvas's worked pair: a stroked line, a line of
//! text on the left and a right-aligned page number, all placed against
//! `c.size()` so the header sits the same distance below the top of every
//! page whatever size or rotation that page has.

use std::path::Path;
use std::process::ExitCode;

use pdfrum::{Color, Document, Point, SaveOptions, StandardFont, Stroke};

/// Points from the page's edges to the rule.
const MARGIN: f64 = 48.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: header-rule <input.pdf> <output.pdf> [title]");
        return ExitCode::FAILURE;
    };
    let title = args.get(2).map_or("", String::as_str);

    match run(Path::new(input), Path::new(output), title) {
        Ok(pages) => {
            println!("headed {pages} page(s) into {output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, output: &Path, title: &str) -> Result<u32, pdfrum::Error> {
    let doc = Document::open(input)?;
    let total = doc.page_count();
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica)?;

    edit.draw_pages(|c| {
        let (right, y) = (c.size().width - MARGIN, c.size().height - MARGIN);
        let grey = Color::from_rgb8(90, 90, 90);
        c.line(
            Point::new(MARGIN, y),
            Point::new(right, y),
            Stroke::new(grey, 0.75),
        );
        c.text(title, &font, 9.0, Point::new(MARGIN, y + 4.0), grey);
        // Every canvas `draw_pages` hands out is a page's, so this is
        // always `Some`; the `None` case is a Form XObject.
        let index = c.page().map_or(0, u32::from);
        let number = format!("{} / {total}", index + 1);
        let width = c.text_width(&number, &font, 9.0);
        c.text(
            &number,
            &font,
            9.0,
            Point::new(right - width, y + 4.0),
            grey,
        );
    })?;

    edit.save(output, &SaveOptions::default())?;
    Ok(total)
}
