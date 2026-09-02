//! One thread count, every multi-page corpus document, one line each.
//!
//! `scripts/bench-scaling.nu` runs this once per thread count and reads the
//! curve off the output. It is a separate process per configuration on purpose:
//! rayon's global pool is built once and cannot be resized, so a single process
//! sweeping thread counts would measure the first one several times.
//!
//! The parallel shape is `map_init(RenderSession::new, …)`, which is what
//! `crates/pdfrum/examples/parallel-render.rs` documents as the right one — a
//! session per *worker*, reused across every page that worker handles, rather
//! than one per page. A benchmark that built a session per page would measure
//! cache construction and call it thread scaling.

use std::hint::black_box;
use std::time::{Duration, Instant};

use pdfrum::{Document, Pixmap, RenderOptions, RenderSession};
use pdfrum_bench::corpus::{bytes, multipage};
use rayon::prelude::*;

fn main() {
    let mut threads = 1usize;
    let mut rounds = 3u32;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while let Some(flag) = argv.get(i) {
        match flag.as_str() {
            "--threads" => {
                i += 1;
                threads = argv.get(i).and_then(|v| v.parse().ok()).unwrap_or(1);
            }
            "--rounds" => {
                i += 1;
                rounds = argv.get(i).and_then(|v| v.parse().ok()).unwrap_or(3);
            }
            other => {
                eprintln!("unexpected argument {other}");
                eprintln!("usage: scaling [--threads N] [--rounds N]");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .is_err()
    {
        eprintln!("warning: rayon's global pool was already built; the thread");
        eprintln!("         count below may not be the one requested.");
    }

    let actual = rayon::current_num_threads();
    println!("threads={actual} (requested {threads})");
    println!(
        "{:<26} {:>6} {:>12} {:>12}",
        "document", "pages", "ms", "ms/page"
    );
    println!("{:-<26} {:->6} {:->12} {:->12}", "", "", "", "");

    let mut total = Duration::ZERO;
    for doc in multipage() {
        let Ok(opened) = Document::from_bytes(bytes(doc.stem)) else {
            continue;
        };
        let options = RenderOptions::default();
        let pages: Vec<_> = opened.pages().collect();

        // One untimed pass, so the fonts and images are decoded into the
        // document's own caches before the clock starts. Otherwise the first
        // round measures decoding and every later one does not, and the
        // "best of three" would systematically pick a warm run at high thread
        // counts and a cold one at low ones.
        let warm: Vec<Pixmap> = pages
            .par_iter()
            .map_init(RenderSession::new, |session, page| {
                page.render_session(&options, session).ok()
            })
            .flatten()
            .collect();
        black_box(warm.len());

        let mut best = Duration::MAX;
        for _ in 0..rounds {
            let started = Instant::now();
            let out: Vec<Pixmap> = pages
                .par_iter()
                .map_init(RenderSession::new, |session, page| {
                    page.render_session(&options, session).ok()
                })
                .flatten()
                .collect();
            let elapsed = started.elapsed();
            black_box(out.len());
            best = best.min(elapsed);
        }
        total += best;

        let ms = best.as_secs_f64() * 1000.0;
        println!(
            "{:<26} {:>6} {:>12.3} {:>12.3}",
            doc.stem,
            doc.pages,
            ms,
            ms / f64::from(doc.pages.max(1))
        );
    }

    println!("{:-<26} {:->6} {:->12} {:->12}", "", "", "", "");
    println!(
        "{:<26} {:>6} {:>12.3}",
        "TOTAL",
        "",
        total.as_secs_f64() * 1000.0
    );
    println!();
}
