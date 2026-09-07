//! What a render costs, per document, per backend, on both conventions.
//!
//! Six criterion groups over the 44 documents in `benches/corpus` (see its
//! `PROVENANCE.md`): three backends x **two measured conventions**, every id
//! spelt `<class>/<stem>` so a reader — and `benches/src/bin/ratchet.rs` —
//! can aggregate by class without a second table mapping one to the other.
//!
//! ```text
//! render-cold-agg       render-warm-agg
//! render-cold-tinyskia    render-warm-tinyskia
//! render-cold-vello-cpu       render-warm-vello-cpu
//! ```
//!
//! # The two conventions, and why there have to be two
//!
//! This is the correction the internal working notes asked for in writing and
//! §1.8 now records. Until it landed, the suite had one render group and it
//! measured the wrong thing against the oracle:
//!
//! - **`render-cold`** builds a fresh [`RenderSession`] *inside* the timed
//!   closure, so every iteration pays a cold [`RenderCaches`]: the glyph
//!   outlines are re-flattened, and the rendered-image cache M12 §9.1 built is
//!   empty. This is **first-render latency** — what a caller who opens a
//!   document, draws one page and exits actually waits for. It is a real and
//!   worth-tracking number, and it is the behaviour the committed baseline had.
//!
//! - **`render-warm`** hoists the session *out* of the closure, so the caches
//!   persist across iterations. This is **steady-state throughput** — what a
//!   viewer scrolling a document, a thumbnailer, or a server rendering a page
//!   twice pays from the second render on.
//!
//! The oracle's number is a warm number. `pdfium_test --render-repeats` renders
//! the same document repeatedly *in one process*, and `CPDF_PageImageCache` is
//! on by default (§9.3), so every timed pass after the first hits a warm image
//! cache by construction. Comparing our cold group against that column — which
//! is what the `6279821` baseline table did — is measuring our worst case
//! against their best one and calling the quotient a speed ratio. §9.1 had
//! already measured the gap at fourteen times on `image_bug_718762` and said so.
//!
//! So: **the M12 exit target is judged on `render-warm`**, because that is the
//! convention the oracle column was taken in. `render-cold` keeps its own
//! ratchet entries and no oracle target — a latency metric, tracked so it
//! cannot regress, not a comparison.
//!
//! # Per document, not per page
//!
//! Every group times a whole document, because the oracle's `pdfium_test
//! --render-repeats` also times a whole document and comparing a per-page
//! average against a whole-document time would be arithmetic dressed up as
//! measurement. `scripts/bench-oracle.nu` times the same files the same way.
//!
//! # Three backends, not one parameterised group
//!
//! They are separate groups rather than one group with a backend parameter,
//! because they are not three implementations of one number: `vello_cpu` is a
//! retained-scene rasterizer that does all its work in `finish`, and the two
//! immediate-mode backends do theirs per primitive. A ratchet that averaged them
//! would hide a regression in one behind an improvement in another, so the
//! class has to be measurable on its own.
//!
//! # Through the facade, not through this crate's own entry point
//!
//! `pdfrum::Page::render_on` and not `pdfrum_render::render_page_with`,
//! even though this bench lives in `pdfrum-render`. The facade's `paint` builds
//! the page graph and overlays annotation appearances before it calls this
//! crate; the oracle's `pdfium_test` does both too. A bench that entered at this
//! crate's seam would measure a rasterizer against a whole engine. The
//! dev-dependency on the facade is a cycle cargo permits and a
//! *dev*-dependency only — it is in no published tree.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_main};
use pdfrum::{Document, RasterBackend, RenderOptions, RenderSession, VelloCpuBackend};
use pdfrum_corpus::{CORPUS, bytes};
use pdfrum_raster_agg::AggBackend;
use pdfrum_raster_tinyskia::TinySkiaBackend;

/// How many samples one document's render is worth, on the cold group.
///
/// Named documents rather than a measured threshold, deliberately: criterion
/// needs the count *before* it has timed anything, so a "measure then decide"
/// rule would need a warm-up pass of its own. These are the corpus's
/// second-scale renders — the three pathological JPEGs plus the two heaviest
/// real pages — identified in `benches/corpus/PROVENANCE.md` and stable as long
/// as the corpus is. A document not named here keeps the suite's 50.
///
/// If a new document turns out to be slow, the symptom is a suite that takes an
/// extra ten minutes per backend, and this list is where it is fixed.
fn cold_samples(stem: &str) -> usize {
    match stem {
        // ~0.6-1.2 s per render: a 5000x5000 JPEG and its two siblings.
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443" => 10,
        // ~0.4-0.7 s per render.
        "image_en_fqa" | "vector_en_system" => 20,
        _ => 50,
    }
}

/// How many samples the warm group takes.
///
/// A flat 20 against the cold group's 50, and the reason is arithmetic rather
/// than impatience. Doubling the suite's render groups from three to six would
/// double a forty-minute run; the warm group is the half that can afford to be
/// cheaper, because a warm render is *faster* — often by an order of magnitude
/// on the image class — so its absolute confidence interval at 20 samples is
/// tighter than the cold group's at 50 on the same document. The standard error
/// of a median falls as 1/sqrt(n): 20 samples buy 78% of the interval 50 do, on
/// a quantity a tenth the size. §2's re-measured bands were taken at these
/// counts rather than assumed to carry over from the old ones.
///
/// The three pathological JPEGs stay lower still. Warm, they are 84 ms rather
/// than 1.2 s, so they no longer dominate — but they are also the documents
/// whose *first* iteration is 1.2 s, and criterion's warm-up has to absorb that
/// before the measured window starts.
fn warm_samples(stem: &str) -> usize {
    match stem {
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443" => 10,
        _ => 20,
    }
}

/// Whether this document leaves a resident set the next benchmark would pay
/// for.
///
/// The same three stems [`cold_samples`] and [`warm_samples`] already single
/// out, for a second consequence of the same property. They are measured
/// **last** within their group, because a criterion group is one process and
/// what they leave on the heap is charged to whatever runs next.
///
/// Measured with `/usr/bin/time -v` on the bench binary: `shading_gouraud`
/// alone peaks at 27,184 kB and reads 1.2546 ms; `image_bug_583804` then
/// `shading_gouraud` peaks at 520,980 kB and reads 1.3004 ms. Repeating the
/// pairing gave +5.4% once and +0.4% another time — the magnitude is unstable,
/// which is the signature of allocator and page-cache state rather than a code
/// path. The retention itself is deliberate: the decoded-image cache holds up
/// to its 100 MiB budget rather than empty itself
/// (`crates/pdfrum-page/src/image/cache.rs`), which is what makes
/// `image_bug_583804`'s own warm render 30-34% faster.
///
/// Ordering them last does not make the suite immune — it removes the one
/// perturbation large enough to clear a 3-5% band. On 2026-09-06 it was
/// clearing them wholesale: two runs of the same binary at the same commit
/// reported 34 and 27 regressions sharing only 13 rows, and the warm cluster
/// moved from `render-warm-vello-cpu` (14 rows) to `render-warm-tinyskia`
/// (13 rows), which no code change can do.
fn leaves_a_large_heap(stem: &str) -> bool {
    matches!(
        stem,
        "image_bug_718762" | "image_bug_583804" | "image_bug_898443"
    )
}

/// The corpus with the heavy documents moved to the end, order otherwise kept.
///
/// Returns them in two passes rather than sorting, so the light documents keep
/// exactly the order they had and a reader can see that the only change is
/// where the heavy three sit.
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
        // Criterion's floor is `sample_size` iterations *regardless* of the
        // measurement window, so a benchmark costs `max(window, n x iteration)`.
        // At a flat 50 the corpus's three pathological image documents — 0.6 to
        // 1.2 seconds each, per render — become minute-long benchmarks, three
        // times over for the three backends, and they alone outweigh the other
        // 41 documents combined. Measured: the suite does not finish in an hour.
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
/// M12 exit target is judged on.
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
        // than defaulted — the internal working notes has the measurement behind them.
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
