//! Render every page in parallel with rayon, and show that it pays.
//!
//! ```text
//! cargo run --release --example parallel-render -- tests/fixtures/bookmarks.pdf 4.0
//! ```
//!
//! # Why this Just Works
//!
//! [`Document`] is `Send + Sync` and its object store is internally
//! synchronized, so every worker shares one `&Document` and pulls the pages
//! it needs. The rendering engine is single-threaded *per page* on purpose:
//! a page is the natural unit of data parallelism, and the rasterizer below
//! is already vectorized within one.
//!
//! The one thing worth knowing is the cache. A [`RenderSession`] holds the
//! decoded fonts, colour spaces and images of a document together with the
//! flattened glyph outlines drawn from them, and it is reached through `&mut`
//! — so it cannot be *shared*, only owned. `map_init` gives each rayon worker
//! its own, which is the right granularity: a document's fonts are decoded
//! once per thread rather than once per page. The backend beside it is `Sync`
//! and stateless, so one serves every worker.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use pdfrum::{Document, Error, Pixmap, RenderOptions, RenderSession, VelloCpuBackend};
use rayon::prelude::*;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: parallel-render <input.pdf> [scale]");
        return ExitCode::FAILURE;
    };
    let scale: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);

    match run(Path::new(input), scale) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, scale: f64) -> Result<(), Error> {
    let doc = Document::open(input)?;
    let options = RenderOptions::scaled(scale);

    // Collect the page handles first. Each is a cheap record of where a page
    // is, so this costs a few dictionary lookups, not an interpretation.
    let pages: Vec<_> = doc.pages().collect();
    println!(
        "{} page(s) at {scale}x on {} threads",
        pages.len(),
        rayon::current_num_threads()
    );

    // One backend for every worker: it is `Sync` and holds no per-page state,
    // so unlike a session it is shared rather than cloned.
    let backend = VelloCpuBackend;

    let started = Instant::now();
    let serial: Vec<Pixmap> = {
        let mut session = RenderSession::new();
        pages
            .iter()
            .map(|page| page.render_on(backend, &options, &mut session))
            .collect::<Result<_, _>>()?
    };
    let serial_time = started.elapsed();

    let started = Instant::now();
    // `map_init` builds one session per worker, reused across every page that
    // worker handles.
    let parallel: Vec<Pixmap> = pages
        .par_iter()
        .map_init(RenderSession::new, |session, page| {
            page.render_on(backend, &options, session)
        })
        .collect::<Result<_, _>>()?;
    let parallel_time = started.elapsed();

    println!("serial:   {serial_time:>10.2?}");
    println!("parallel: {parallel_time:>10.2?}");

    // Rendering is deterministic, so the two runs must agree pixel for pixel.
    // If they ever did not, the parallelism would be hiding a data race.
    assert_eq!(serial, parallel, "parallel output must match serial output");
    println!("both runs produced identical pixels");

    let total: usize = parallel
        .iter()
        .map(|p| p.width() as usize * p.height() as usize)
        .sum();
    println!("{total} pixels rendered");
    Ok(())
}
