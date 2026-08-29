//! Print a PDF's text, and optionally search it.
//!
//! ```text
//! cargo run --example extract-text -- tests/fixtures/hello_world.pdf
//! cargo run --example extract-text -- tests/fixtures/hello_world.pdf world
//! ```
//!
//! With a search term, prints each match's page, character offset and the
//! rectangles it covers — which is what a viewer needs to draw a highlight.

use std::path::Path;
use std::process::ExitCode;

use pdfrum::{Document, FindOptions};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: extract-text <input.pdf> [search-term]");
        return ExitCode::FAILURE;
    };

    match run(Path::new(input), args.get(1).map(String::as_str)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, needle: Option<&str>) -> Result<(), pdfrum::Error> {
    let doc = Document::open(input)?;

    // One build context for the whole document, so a font used on every page
    // is parsed once rather than once per page.
    let mut ctx = pdfrum::BuildContext::new();

    for page in doc.pages() {
        let text = page.text_with(&mut ctx);

        match needle {
            None => {
                // A page label is what a reader shows in its page box: "iv"
                // rather than "4". Most documents have none.
                let label = doc
                    .page_label(page.index())
                    .unwrap_or_else(|| (page.index() + 1).to_string());
                println!("=== page {label} ({} chars) ===", text.char_count());
                println!("{}", text.all_text());
            }
            Some(needle) => {
                for range in text.find(needle, FindOptions::default()) {
                    // `find` reports offsets into the *search* text, which is
                    // not the same sequence as the character list — control
                    // characters and placeholders are stripped from one and
                    // not the other. `rects` takes character indices, so the
                    // two are bridged through the page's own index.
                    let rects = text.rects(range.start, Some(range.len()));
                    println!(
                        "page {} chars {}..{} in {} box(es)",
                        page.index() + 1,
                        range.start,
                        range.end,
                        rects.len()
                    );
                    for rect in rects {
                        println!(
                            "    x {:.1}..{:.1}  y {:.1}..{:.1}",
                            rect.x0, rect.x1, rect.y0, rect.y1
                        );
                    }
                }
            }
        }

        // Web and mail addresses the page's text contains, whether or not the
        // file marked them up as link annotations.
        for link in text.web_links() {
            println!("link: {}", link.url);
        }
    }
    Ok(())
}
