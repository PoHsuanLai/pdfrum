//! Convert one PDF page to SVG on stdout, with its rasterized-region report
//! on stderr.
//!
//! ```text
//! cargo run -p pdfrum-svg --example convert -- file.pdf [page] > page.svg
//! ```
//!
//! The smallest thing that exercises the whole crate end to end, and the way
//! to look at what a document actually converts to rather than at a score.

// Through the facade alone, which is the shortest path a caller has: the
// `svg-export` feature puts `Page::to_svg` on the facade's own page type, so nothing
// here names an engine crate.
use pdfrum::{Document, TinySkiaBackend};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or("usage: convert <file.pdf> [page]")?;
    let index: u32 = args
        .next()
        .and_then(|a| a.into_string().ok())
        .map_or(Ok(0), |a| a.parse())?;

    let doc = Document::open(path)?;
    let page = doc.page(index)?;
    let converted = page.to_svg(TinySkiaBackend)?;

    for (cause, n) in converted.report.counts() {
        eprintln!("{:<20} {n}", cause.name());
    }
    if converted.report.is_empty() {
        eprintln!("all vectors");
    }
    print!("{}", converted.svg);
    Ok(())
}
