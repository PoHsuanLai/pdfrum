//! Render cost: three backends × cold and warm, ids `<class>/<stem>`.
//!
//! - **cold** — fresh [`RenderSession`] inside the timed closure. First-render
//!   latency.
//! - **warm** — session hoisted out. Steady-state. The oracle
//!   (`pdfium_test --render-repeats`) is warm; compare against that column.
//!
//! Whole document, not per page — the oracle times a whole document.
//! Separate groups per backend: averaging would hide a regression in one.
//! Enters at `pdfrum::Page::render_on`, not this crate's seam: the facade
//! builds the page and overlays annotations, and so does the oracle.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_main};
use pdfrum::{Document, RasterBackend, RenderOptions, RenderSession, VelloCpuBackend};
use pdfrum_corpus::{CORPUS, bytes};
use pdfrum_raster_agg::AggBackend;
use pdfrum_raster_tinyskia::TinySkiaBackend;

/// Cold-group sample count. Named because criterion needs `n` before it has
/// timed anything. Unnamed documents keep 50.
fn cold_samples(stem: &str) -> usize {
    match stem {
        // ~0.6-1.2 s per render: a 5000x5000 JPEG and its two siblings.
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443" => 10,
        // ~0.4-0.7 s per render.
        "image_en_fqa" | "vector_en_system" => 20,
        _ => 50,
    }
}

/// Warm-group sample count. 20 is enough: a warm render is faster than cold.
fn warm_samples(stem: &str) -> usize {
    match stem {
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443" => 10,
        _ => 20,
    }
}

/// Pathological JPEGs leave a large heap; run them last in the group.
fn leaves_a_large_heap(stem: &str) -> bool {
    matches!(
        stem,
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443"
    )
}

/// Corpus with the heavy documents last.
fn light_then_heavy() -> impl Iterator<Item = &'static pdfrum_corpus::Doc> {
    CORPUS
        .iter()
        .filter(|doc| !leaves_a_large_heap(doc.stem))
        .chain(CORPUS.iter().filter(|doc| leaves_a_large_heap(doc.stem)))
}

/// First-render latency: a fresh session inside the timed closure.
///
/// The cache state a caller gets who opens a document, draws a page, and exits.
fn cold<B: RasterBackend>(c: &mut Criterion, name: &str, backend: &B) {
    let mut group = c.benchmark_group(name);
    let options = RenderOptions::default();
    for doc in light_then_heavy() {
        let Ok(opened) = Document::from_bytes(bytes(doc.stem)) else {
            continue;
        };
        group.sample_size(cold_samples(doc.stem));
        group.bench_function(format!("{}/{}", doc.class.name(), doc.stem), |b| {
            b.iter(|| {
                let mut session = RenderSession::new();
                for page in opened.pages() {
                    black_box(page.render_on(backend, &options, &mut session).ok());
                }
            });
        });
    }
    group.finish();
}

/// Steady-state throughput: one session held across every iteration.
///
/// The cache state the oracle's own timed passes run in, and the convention the
/// exit target is judged on.
///
/// The session is built and *primed* with one untimed render before the closure,
/// so the first measured iteration is a warm one rather than the outlier that
/// fills the cache. Criterion's warm-up would do this eventually, but only
/// eventually: on `image_bug_718762` the cold render is fourteen times the warm
/// one, and leaving it to chance means the number depends on how many warm-up
/// iterations criterion happened to run.
fn warm<B: RasterBackend>(c: &mut Criterion, name: &str, backend: &B) {
    let mut group = c.benchmark_group(name);
    let options = RenderOptions::default();
    for doc in light_then_heavy() {
        let Ok(opened) = Document::from_bytes(bytes(doc.stem)) else {
            continue;
        };
        group.sample_size(warm_samples(doc.stem));
        group.bench_function(format!("{}/{}", doc.class.name(), doc.stem), |b| {
            let mut session = RenderSession::new();
            for page in opened.pages() {
                drop(page.render_on(backend, &options, &mut session));
            }
            b.iter(|| {
                for page in opened.pages() {
                    black_box(page.render_on(backend, &options, &mut session).ok());
                }
            });
        });
    }
    group.finish();
}

/// The analytic parity backend, cold.
fn cold_agg(c: &mut Criterion) {
    cold(c, "render-cold-agg", &AggBackend::new());
}

/// The `tiny-skia` cross-check backend, cold.
fn cold_tinyskia(c: &mut Criterion) {
    cold(c, "render-cold-tinyskia", &TinySkiaBackend::new());
}

/// The `vello_cpu` default backend, cold.
fn cold_vello_cpu(c: &mut Criterion) {
    cold(c, "render-cold-vello-cpu", &VelloCpuBackend::new());
}

/// The analytic parity backend, warm.
fn warm_agg(c: &mut Criterion) {
    warm(c, "render-warm-agg", &AggBackend::new());
}

/// The `tiny-skia` cross-check backend, warm.
fn warm_tinyskia(c: &mut Criterion) {
    warm(c, "render-warm-tinyskia", &TinySkiaBackend::new());
}

/// The `vello_cpu` default backend, warm.
fn warm_vello_cpu(c: &mut Criterion) {
    warm(c, "render-warm-vello-cpu", &VelloCpuBackend::new());
}

// `criterion_group!` generates a `pub fn` the `missing_docs` lint cannot see a
// doc comment for — the attribute would land inside the macro's expansion.
#[allow(missing_docs, reason = "the item is generated by criterion_group!")]
mod group {
    use super::{
        Criterion, Duration, cold_agg, cold_tinyskia, cold_vello_cpu, warm_agg, warm_tinyskia,
        warm_vello_cpu,
    };
    use criterion::criterion_group;

    criterion_group! {
        name = benches;
        // The settings are the ratchet's noise band, and they are chosen rather
        //
        // 50 samples over a 5-second window with a 2-second warm-up: measured
        // run-to-run spread under 3% on every group, and a full run in about
        // forty minutes. `sample_size(50)` rather than the default 100 because
        // criterion's floor is `sample_size` iterations *regardless* of the
        // window. The per-document overrides above lower it further where one
        // iteration is a second; the warm groups lower it to 20 throughout.
        config = Criterion::default()
            .measurement_time(Duration::from_secs(5))
            .warm_up_time(Duration::from_secs(2))
            .sample_size(50);
        targets = cold_agg, cold_tinyskia, cold_vello_cpu, warm_agg, warm_tinyskia, warm_vello_cpu
    }
}

criterion_main!(group::benches);
