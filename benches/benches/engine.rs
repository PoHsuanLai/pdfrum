//! What the engine costs, per document, over the M12 corpus.
//!
//! Six groups over the 44 documents in `../corpus` (see its `PROVENANCE.md`
//! for what each one is and where it came from), every benchmark id spelt
//! `<class>/<stem>` so a reader — and `src/bin/ratchet.rs` — can aggregate by
//! class without a second table mapping one to the other:
//!
//! - **open** — `Document::from_bytes` on bytes already in memory, so the
//!   number is parse and cross-reference recovery rather than the filesystem.
//! - **render-exact** — every page through `pdfrum-raster-exact`, the analytic
//!   parity backend, on one shared [`RenderSession`].
//! - **render-tinyskia** — the same pages through `tiny-skia`.
//! - **render-vello** — the same pages through `vello_cpu`, the facade's
//!   default.
//! - **text** — every page's text extracted.
//! - **save** — a full rewrite to an in-memory sink, so the number is the
//!   writer rather than the filesystem.
//!
//! # Per document, not per page
//!
//! Every group times a whole document, because the oracle's
//! `pdfium_test --render-repeats` also times a whole document and comparing a
//! per-page average against a whole-document time would be arithmetic dressed
//! up as measurement. `scripts/bench-oracle.sh` times the same files the same
//! way and `docs/status/M12.md` carries the comparison.
//!
//! # Three render groups, not one parameterised group
//!
//! The three backends are separate criterion groups rather than one group with
//! a backend parameter, because they are not three implementations of one
//! number: `vello_cpu` is a retained-scene rasterizer that does all its work in
//! `finish`, and the two immediate-mode backends do theirs per primitive. A
//! ratchet that averaged them would hide a regression in one behind an
//! improvement in another, and DEPS.md's performance ring admits a dependency
//! on a *class* moving — so the class has to be measurable on its own.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_main};
use pdfrum::{Backend, Document, RenderOptions, RenderSession, SaveOptions};
use pdfrum_bench::corpus::{CORPUS, bytes};

/// Opening a document: header, cross-reference, trailer, catalog.
///
/// Pages are deliberately *not* touched — the engine parses them on demand,
/// and this measures what `open` itself costs.
fn open(c: &mut Criterion) {
    let mut group = c.benchmark_group("open");
    for doc in CORPUS {
        let raw = bytes(doc.stem);
        group.bench_function(format!("{}/{}", doc.class.name(), doc.stem), |b| {
            b.iter(|| {
                let opened = Document::from_bytes(std::sync::Arc::clone(&raw));
                black_box(opened.map(|d| d.page_count()).ok())
            });
        });
    }
    group.finish();
}

/// Render every page of every document on one backend.
///
/// One [`RenderSession`] per iteration, not per page: that is the M8 reuse
/// path, it is what a caller rendering a document actually does, and it is the
/// configuration every render number in `docs/status/M12.md` is taken in.
fn render_with(c: &mut Criterion, name: &str, backend: Backend) {
    let mut group = c.benchmark_group(name);
    let options = RenderOptions {
        backend,
        ..RenderOptions::default()
    };
    for doc in CORPUS {
        let Ok(opened) = Document::from_bytes(bytes(doc.stem)) else {
            continue;
        };
        // Criterion's floor is `sample_size` iterations *regardless* of the
        // measurement window, so a benchmark costs `max(window, n x iteration)`.
        // At a flat 50 the corpus's three pathological image documents — 0.6 to
        // 1.2 seconds each, per render — become minute-long benchmarks, three
        // times over for the three backends, and they alone outweigh the other
        // 41 documents combined. Measured: the suite does not finish in an hour.
        //
        // So the sample count is chosen per document from what one iteration
        // costs. This is not a shortcut, it is the correct allocation: the
        // standard error of a median falls as 1/sqrt(n), so the *last* thirty
        // samples of a one-second benchmark buy a few percent of an interval
        // that is already tighter than the 5% the machine itself contributes,
        // and cost half an hour. `docs/status/M12.md` §2 carries the measured
        // bands, which were re-taken at these counts rather than assumed to
        // carry over.
        group.sample_size(samples_for(doc.stem));
        group.bench_function(format!("{}/{}", doc.class.name(), doc.stem), |b| {
            b.iter(|| {
                let mut session = RenderSession::new();
                for page in opened.pages() {
                    black_box(page.render_session(&options, &mut session).ok());
                }
            });
        });
    }
    group.finish();
}

/// How many samples one document's render is worth.
///
/// Named documents rather than a measured threshold, deliberately: criterion
/// needs the count *before* it has timed anything, so a "measure then decide"
/// rule would need a warm-up pass of its own. These five are the corpus's
/// second-scale renders — the three pathological JPEGs plus the two heaviest
/// real pages — identified in `corpus/PROVENANCE.md` and stable as long as the
/// corpus is. A document not named here keeps criterion's floor of 10 raised to
/// the suite's 50.
///
/// If a new document turns out to be slow, the symptom is a suite that takes an
/// extra ten minutes per backend, and this list is where it is fixed.
fn samples_for(stem: &str) -> usize {
    match stem {
        // ~0.6-1.2 s per render: a 5000x5000 JPEG and its two siblings.
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443" => 10,
        // ~0.4-0.7 s per render.
        "image_en_fqa" | "vector_en_system" => 20,
        _ => 50,
    }
}

/// The analytic parity backend.
fn render_exact(c: &mut Criterion) {
    render_with(c, "render-exact", Backend::Exact);
}

/// The `tiny-skia` cross-check backend.
fn render_tinyskia(c: &mut Criterion) {
    render_with(c, "render-tinyskia", Backend::TinySkia);
}

/// The `vello_cpu` default backend.
fn render_vello(c: &mut Criterion) {
    render_with(c, "render-vello", Backend::Vello);
}

/// Extracting every page's text.
fn text(c: &mut Criterion) {
    let mut group = c.benchmark_group("text");
    for doc in CORPUS {
        let Ok(opened) = Document::from_bytes(bytes(doc.stem)) else {
            continue;
        };
        group.bench_function(format!("{}/{}", doc.class.name(), doc.stem), |b| {
            b.iter(|| {
                let mut session = RenderSession::new();
                for page in opened.pages() {
                    black_box(page.text_session(&mut session).all_text().len());
                }
            });
        });
    }
    group.finish();
}

/// A full rewrite, to memory.
///
/// To a `Vec` and not to a file, deliberately: a save benchmark that writes to
/// disk measures the page cache on the second iteration and the disk on the
/// first, and neither is the writer. What is measured here is object
/// serialization, the cross-reference table and the stream re-encoding.
fn save(c: &mut Criterion) {
    let mut group = c.benchmark_group("save");
    let options = SaveOptions::default();
    for doc in CORPUS {
        let Ok(opened) = Document::from_bytes(bytes(doc.stem)) else {
            continue;
        };
        group.bench_function(format!("{}/{}", doc.class.name(), doc.stem), |b| {
            b.iter(|| {
                let mut out = Vec::new();
                black_box(opened.write_to(&mut out, &options).ok());
                black_box(out.len())
            });
        });
    }
    group.finish();
}

// `criterion_group!` generates a `pub fn` the `missing_docs` lint cannot see a
// doc comment for — the attribute would land inside the macro's expansion.
#[allow(missing_docs, reason = "the item is generated by criterion_group!")]
mod group {
    use super::{
        Criterion, Duration, open, render_exact, render_tinyskia, render_vello, save, text,
    };
    use criterion::criterion_group;

    criterion_group! {
        name = benches;
        // The settings are the ratchet's noise band, and they are chosen
        // rather than defaulted — docs/status/M12.md §"The noise band" has the
        // measurement behind them.
        //
        // The tension is real and worth stating. Rendering a document in this
        // corpus costs milliseconds to a second, not nanoseconds, so criterion
        // defaults leave the confidence interval wider than the regressions
        // the ratchet must catch. But 44 documents x 6 groups is 264
        // benchmarks, and every second added to the window costs four and a
        // half minutes of wall clock — a suite nobody runs catches nothing.
        //
        // 50 samples over a 5-second window with a 2-second warm-up is where
        // that lands: measured run-to-run spread under 3% on every group
        // (§"The noise band" has the distribution), and a full run in about
        // forty minutes. `sample_size(50)` rather than the default 100 because
        // criterion's floor is `sample_size` iterations *regardless* of the
        // window, and at 100 the two second-long image documents alone would
        // add ten minutes with no reduction in spread.
        config = Criterion::default()
            .measurement_time(Duration::from_secs(5))
            .warm_up_time(Duration::from_secs(2))
            .sample_size(50);
        targets = open, render_exact, render_tinyskia, render_vello, text, save
    }
}

criterion_main!(group::benches);
