//! Render every page of a PDF to a PNG file.
//!
//! ```text
//! cargo run --example render-to-png -- tests/fixtures/hello_world.pdf out/ 2.0
//! ```
//!
//! The third argument is the scale in pixels per PDF point: `1.0` renders a
//! US Letter page at 612x792, and `300.0 / 72.0` renders it at 300 DPI.
//!
//! Note where the work is split. `pdfrum` hands out pixels — a [`Pixmap`] of
//! premultiplied RGBA — and does not encode image files, which is why the
//! `png` crate appears here rather than in the engine. An encoder is a
//! choice the caller should keep making.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pdfrum::{Document, Pixmap, RenderOptions};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(out_dir)) = (args.first(), args.get(1)) else {
        eprintln!("usage: render-to-png <input.pdf> <output-dir> [scale]");
        return ExitCode::FAILURE;
    };
    let scale: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);

    match run(Path::new(input), Path::new(out_dir), scale) {
        Ok(count) => {
            println!("wrote {count} page(s) to {out_dir}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, out_dir: &Path, scale: f64) -> Result<usize, Box<dyn std::error::Error>> {
    let doc = Document::open(input)?;
    std::fs::create_dir_all(out_dir)?;

    // A document that had to be repaired to open still opens; say so rather
    // than pretending the file was clean.
    if doc.xref_was_rebuilt() {
        eprintln!("note: this file's cross-reference table was rebuilt by scanning");
    }

    let options = RenderOptions::scaled(scale);
    let mut written = 0;
    for page in doc.pages() {
        let pixmap = page.render(&options)?;
        let path = out_dir.join(format!("page-{:04}.png", page.index().get() + 1));
        write_png(&path, &pixmap)?;
        println!(
            "page {:>4}  {}x{} -> {}",
            page.index().get() + 1,
            pixmap.width(),
            pixmap.height(),
            path.display()
        );
        written += 1;
    }
    Ok(written)
}

/// Encode a pixmap as an RGBA PNG.
///
/// `to_straight_rgba` un-premultiplies, which is what every image format
/// outside a compositor expects.
fn write_png(path: &PathBuf, pixmap: &Pixmap) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, pixmap.width(), pixmap.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(&pixmap.to_straight_rgba())?;
    Ok(())
}
