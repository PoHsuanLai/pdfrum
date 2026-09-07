//! One operation, one file, in a loop. `scripts/profile.nu` drives this.
//!
//! Criterion interleaves its own statistics between iterations, so a
//! `perf` of `cargo bench` is criterion plus the engine. This process does
//! the operation and returns.
//!
//! ```text
//! profile --op render --file benches/corpus/text_foxittext.pdf --iterations 50
//! profile --op render --backend vello-cpu --file … --iterations 20
//! profile --op text --file … --sample
//! ```

use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pdfrum::{Document, RenderOptions, RenderSession, SaveOptions, VelloCpuBackend};
use pdfrum_raster_agg::AggBackend;
use pdfrum_raster_tinyskia::TinySkiaBackend;

/// Which rasterizer `--backend` names.
///
/// CLI choice. The seam below is the `RasterBackend` trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// The analytic AGG-parity rasterizer.
    Agg,
    /// `tiny-skia`.
    TinySkia,
    /// `vello_cpu`.
    VelloCpu,
}

/// What to measure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `Document::from_bytes` — parse, cross-reference, catalog.
    Open,
    /// Every page rendered through one shared session.
    Render,
    /// Every page's text extracted.
    Text,
    /// A full rewrite to an in-memory sink.
    Save,
    /// The annotation/widget appearance overlay, isolated by an A/B against
    /// the same render with `RenderOptions::annotations` off.
    Forms,
}

impl Op {
    /// Parse the `--op` value.
    fn parse(s: &str) -> Option<Op> {
        match s {
            "open" => Some(Op::Open),
            "render" => Some(Op::Render),
            "text" => Some(Op::Text),
            "save" => Some(Op::Save),
            "forms" => Some(Op::Forms),
            _ => None,
        }
    }
}

/// Parse the `--backend` value.
fn backend(s: &str) -> Option<Backend> {
    match s {
        "agg" | "exact" => Some(Backend::Agg),
        "tinyskia" | "tiny-skia" => Some(Backend::TinySkia),
        "vello-cpu" | "vello_cpu" | "vello" => Some(Backend::VelloCpu),
        _ => None,
    }
}

/// The command line, already validated.
struct Args {
    /// Which operation to run.
    op: Op,
    /// The file to run it on.
    file: PathBuf,
    /// How many times.
    iterations: u32,
    /// Which rasterizer, for `Op::Render`.
    backend: Backend,
    /// Print the in-process sampler's flat profile at the end.
    sample: bool,
    /// Hold one `RenderSession` across every iteration instead of building a
    /// fresh one per iteration.
    warm: bool,
}

/// Read the command line, or explain what was wrong with it.
fn parse_args() -> Result<Args, String> {
    let mut op = Op::Render;
    let mut file = None;
    let mut iterations = 50u32;
    let mut back = Backend::Agg;
    let mut sample = false;
    let mut warm = false;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while let Some(flag) = argv.get(i) {
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--op" => op = Op::parse(&value(&mut i)?).ok_or("unknown --op")?,
            "--file" => file = Some(PathBuf::from(value(&mut i)?)),
            "--iterations" => {
                iterations = value(&mut i)?
                    .parse()
                    .map_err(|_| "--iterations wants a number".to_owned())?;
            }
            "--backend" => back = backend(&value(&mut i)?).ok_or("unknown --backend")?,
            "--sample" => sample = true,
            "--warm" => warm = true,
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unexpected argument {other}\n\n{}", usage())),
        }
        i += 1;
    }

    Ok(Args {
        op,
        file: file.ok_or_else(|| format!("--file is required\n\n{}", usage()))?,
        iterations,
        backend: back,
        sample,
        warm,
    })
}

/// The `--help` text.
fn usage() -> String {
    "profile --op <open|render|text|save|forms> --file <pdf> \
     [--iterations N] [--backend agg|tinyskia|vello-cpu] [--sample] [--warm]"
        .to_owned()
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let bytes: Arc<[u8]> = match std::fs::read(&args.file) {
        Ok(bytes) => Arc::from(bytes),
        Err(err) => {
            eprintln!("cannot read {}: {err}", args.file.display());
            std::process::exit(1);
        }
    };

    // `--sample` on a render takes the instrumented path instead: it renders
    // through a decorated backend and splits the time at the engine/rasterizer
    // seam. On any other operation there is no seam to split at, so it just
    // reports the total.
    if args.op == Op::Forms {
        return forms_split(&args, &bytes);
    }

    if args.sample && args.op == Op::Render {
        return timed_render(&args, &bytes);
    }

    let (done, elapsed) = run(&args, &bytes);

    eprintln!(
        "{} iterations in {:.3} s = {:.3} ms/iteration",
        done,
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / f64::from(done.max(1)),
    );

    if args.op == Op::Render {
        render_report(&args, done, elapsed);
    }

    if args.sample {
        eprintln!();
        eprintln!(
            "note: --sample splits engine from raster on `--op render` only;\n\
             for {}, the total above is all this fallback can attribute.\n\
             A per-symbol profile needs `perf` — see scripts/profile.nu.",
            match args.op {
                Op::Open => "open",
                Op::Text => "text",
                Op::Save => "save",
                Op::Render => "render",
                Op::Forms => "forms",
            }
        );
    }
}

/// How many alternating rounds `--op forms` takes per arm.
///
/// Enough that one descheduled round cannot be the minimum, few enough that a
/// 68 ms document still finishes.
const ROUNDS: u32 = 5;

/// Appearance overlay cost: warm render with annotations on vs off.
///
/// Not `pdfrum-form`: the forms class vs vector is the overlay
/// (`RenderOptions::annotations`), not the interaction path. Interleaved
/// A/B in one process so a scheduling event cannot land in one arm.
fn forms_split(args: &Args, bytes: &Arc<[u8]>) {
    let Ok(doc) = Document::from_bytes(Arc::clone(bytes)) else {
        eprintln!("cannot open the document");
        std::process::exit(1);
    };

    let with = RenderOptions::default();
    let without = RenderOptions {
        annotations: false,
        ..RenderOptions::default()
    };

    let mut best_with = f64::INFINITY;
    let mut best_without = f64::INFINITY;
    for _ in 0..ROUNDS {
        best_with = best_with.min(warm_pass(args, &doc, &with));
        best_without = best_without.min(warm_pass(args, &doc, &without));
    }

    let overlay = best_with - best_without;
    let share = if best_with > 0.0 {
        overlay * 100.0 / best_with
    } else {
        0.0
    };

    eprintln!();
    eprintln!(
        "{} pages x {} iterations x {ROUNDS} rounds, backend={:?}, warm session per arm",
        doc.page_count(),
        args.iterations,
        args.backend,
    );
    eprintln!();
    eprintln!("{:<28} {:>12} {:>10}", "arm", "ms/iter", "share");
    eprintln!("{:-<28} {:->12} {:->10}", "", "", "");
    eprintln!(
        "{:<28} {best_without:>12.3} {:>9.1}%",
        "page content alone",
        100.0 - share
    );
    eprintln!("{:<28} {overlay:>12.3} {share:>9.1}%", "annotation overlay");
    eprintln!("{:-<28} {:->12} {:->10}", "", "", "");
    eprintln!(
        "{:<28} {best_with:>12.3} {:>9.1}%",
        "TOTAL (render, warm)", 100.0
    );
    eprintln!();
    eprintln!(
        "The overlay row is a difference of two minima, so it inherits both arms'\n\
         noise rather than one arm's. On a document where it is a small share of\n\
         a large render, read it as an upper bound and take the symbol profile\n\
         (`scripts/profile.nu forms <file>`) as the authority on where the time is."
    );
}

/// One warm arm: prepare every page once, then time `iterations`
/// whole-document draws through one session. Milliseconds per iteration.
///
/// Prepared once because that is what the reference tool's loop does: it
/// parses a page once and repeats only the draw, so a per-iteration
/// re-interpretation here would be counted on one side and not the other.
fn warm_pass(args: &Args, doc: &Document, options: &RenderOptions) -> f64 {
    let mut session = RenderSession::new();
    let pages: Vec<_> = doc.pages().collect();
    let prepared: Vec<_> = pages
        .iter()
        .map(|page| page.prepare(options, &mut session))
        .collect();
    for page in &prepared {
        drop(draw_one(args, page, &mut session));
    }

    let started = Instant::now();
    for _ in 0..args.iterations {
        for page in &prepared {
            black_box(draw_one(args, page, &mut session).ok());
        }
    }
    let elapsed = started.elapsed();
    elapsed.as_secs_f64() * 1000.0 / f64::from(args.iterations.max(1))
}

/// One page rendered on the backend `--backend` named.
///
/// The three-arm match is the same one `run` makes; it is here rather than
/// inlined at both call sites because `forms_split` needs it twice per round
/// and `run` needs it once, and a fourth copy of a three-arm match is worse
/// than a function.
fn render_one(
    args: &Args,
    page: &pdfrum::Page<'_>,
    options: &RenderOptions,
    session: &mut RenderSession,
) -> pdfrum::Result<pdfrum::Pixmap> {
    match args.backend {
        Backend::Agg => page.render_on(&AggBackend::new(), options, session),
        Backend::TinySkia => page.render_on(&TinySkiaBackend::new(), options, session),
        Backend::VelloCpu => page.render_on(&VelloCpuBackend::new(), options, session),
    }
}

/// One prepared page drawn on the backend `--backend` named; the warm arm's
/// half of [`render_one`].
fn draw_one(
    args: &Args,
    page: &pdfrum::PreparedPage<'_>,
    session: &mut RenderSession,
) -> pdfrum::Result<pdfrum::Pixmap> {
    match args.backend {
        Backend::Agg => page.render_on(&AggBackend::new(), session),
        Backend::TinySkia => page.render_on(&TinySkiaBackend::new(), session),
        Backend::VelloCpu => page.render_on(&VelloCpuBackend::new(), session),
    }
}

/// Render every page `iterations` times through a clocked backend, and print
/// where the time went.
///
/// This bypasses `Page::render_on` and calls `render_page_with`
/// directly, because the decorator has to be substituted for the backend and
/// the facade chooses one from `RenderOptions::backend`. The work is otherwise
/// the same: same page graph, same caches, same options — the `page_graph`
/// figure below is the part the facade does before it hands the graph over,
/// and it is measured rather than assumed so that "engine" here means the same
/// thing it means in .md.
fn timed_render(args: &Args, bytes: &Arc<[u8]>) {
    let Ok(doc) = Document::from_bytes(Arc::clone(bytes)) else {
        eprintln!("cannot open the document");
        std::process::exit(1);
    };
    // The facade's `RenderOptions::to_inner` is `pub(crate)` — deliberately,
    // the engine's option record is not public API — so the defaults are spelt
    // out here instead of widening a facade signature for a benchmark's
    // convenience. They are the defaults `RenderOptions::default()` produces;
    // the criterion groups render with those too, so the two agree.
    let inner = pdfrum_render::RenderOptions::default();

    // The page graphs are built once, outside the timed loop, and the cost of
    // building them reported separately: a profile that charges content
    // interpretation to every iteration would be measuring the parser on a
    // question about the rasterizer.
    let build_started = Instant::now();
    let mut ctx = pdfrum::BuildContext::new();
    let graphs: Vec<_> = doc.pages().map(|page| page.objects()).collect();
    let build = build_started.elapsed();
    black_box(&mut ctx);

    timed::reset();
    // The page-graph build above walks nothing, but `take` is what clears the
    // walk's accumulator and the loop below must start from zero either way.
    #[cfg(feature = "profiling")]
    let _ = pdfrum_render::walkprofile::take();
    let mut caches = pdfrum_render::RenderCaches::default();
    let mut diags = pdfrum_common::Diagnostics::default();

    let started = Instant::now();
    for _ in 0..args.iterations {
        for graph in &graphs {
            let session = pdfrum_render::RenderSession {
                caches: Some(&mut caches),
                ..Default::default()
            };
            let pixmap = match args.backend {
                Backend::Agg => pdfrum_render::render_page_with(
                    graph,
                    &inner,
                    &timed::TimedBackend(pdfrum_raster_agg::AggBackend::new()),
                    session,
                    &mut diags,
                ),
                Backend::TinySkia => pdfrum_render::render_page_with(
                    graph,
                    &inner,
                    &timed::TimedBackend(pdfrum_raster_tinyskia::TinySkiaBackend::new()),
                    session,
                    &mut diags,
                ),
                Backend::VelloCpu => pdfrum_render::render_page_with(
                    graph,
                    &inner,
                    &timed::TimedBackend(pdfrum_raster_vello_cpu::VelloCpuBackend::new()),
                    session,
                    &mut diags,
                ),
            };
            black_box(pixmap.ok());
        }
    }
    let total = started.elapsed();
    let t = timed::totals();

    report(&graphs, args, build, total, t);
}

/// A call count as calls per iteration.
///
/// Through `u32` because a bucket's count is bounded by the number of device
/// calls a page makes times the iteration count, and a page that made four
/// billion device calls would not have finished; the saturating conversion is
/// there so the report degrades to a large number rather than panicking or
/// silently wrapping if one ever did.
fn calls_per_iter(calls: u64, iters: f64) -> f64 {
    f64::from(u32::try_from(calls).unwrap_or(u32::MAX)) / iters
}

/// Print the phase table.
fn report(
    graphs: &[pdfrum_page::Page],
    args: &Args,
    build: std::time::Duration,
    total: std::time::Duration,
    t: timed::Totals,
) {
    let iters = f64::from(args.iterations.max(1));
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / iters;
    let pct = |d: std::time::Duration| {
        if total.is_zero() {
            0.0
        } else {
            d.as_secs_f64() * 100.0 / total.as_secs_f64()
        }
    };

    let raster = t.raster();
    let engine = total.saturating_sub(raster);

    eprintln!();
    eprintln!(
        "{} pages x {} iterations, backend={:?}",
        graphs.len(),
        args.iterations,
        args.backend
    );
    eprintln!(
        "page-graph build (once, not in the loop): {:.3} ms",
        build.as_secs_f64() * 1000.0
    );
    eprintln!();
    eprintln!(
        "{:<16} {:>10} {:>8} {:>12}",
        "phase", "ms/iter", "share", "calls/iter"
    );
    eprintln!("{:-<16} {:->10} {:->8} {:->12}", "", "", "", "");
    let row = |name: &str, b: timed::Bucket| {
        eprintln!(
            "{name:<16} {:>10.3} {:>7.1}% {:>12.0}",
            ms(b.time),
            pct(b.time),
            calls_per_iter(b.calls, iters)
        );
    };
    row("fill_path", t.fill);
    row("stroke_path", t.stroke);
    row("draw_image", t.image);
    row("clip", t.clip);
    row("layer", t.layer);
    row("new_target", t.target);
    row("finish/snapshot", t.finish);
    eprintln!("{:-<16} {:->10} {:->8} {:->12}", "", "", "", "");
    eprintln!(
        "{:<16} {:>10.3} {:>7.1}%",
        "RASTER (sum)",
        ms(raster),
        pct(raster)
    );
    eprintln!(
        "{:<16} {:>10.3} {:>7.1}%   walk + glyphs + colour + interpretation",
        "ENGINE (rest)",
        ms(engine),
        pct(engine)
    );
    eprintln!("{:<16} {:>10.3} {:>7.1}%", "TOTAL", ms(total), 100.0);
    eprintln!();
    eprintln!(
        "The decorator's own `Instant::now()` pairs are counted in ENGINE, and are\n\
         a few percent of a call that costs a microsecond. See the module docs."
    );
    walk_report(iters, engine);
}

/// The note that stands in for the whole-render split when the instrument is
/// off, for the same reason `walk_report`'s does.
#[cfg(not(feature = "profiling"))]
fn render_report(_args: &Args, _done: u32, _elapsed: std::time::Duration) {
    eprintln!();
    eprintln!(
        "note: the whole-render stage split is off. Rebuild with it:\n\
         \x20 cargo build --release -p pdfrum-bench --bin profile --features profiling\n\
         or run `scripts/profile.nu render <file> <iters> <backend> --warm --walk`."
    );
}

/// Where a whole page render's time goes, from the page dictionary to the
/// pixels.
///
/// **The one instrument that measures what the caller's clock measures.**
/// `--sample` builds the page graphs outside its loop and calls
/// `render_page_with` directly, and the walk's own phase split sits inside
/// that; both are honest about their scope and neither can see the work in
/// front of the raster. The rows below are placed around it instead, so their
/// sum plus one remainder is the figure printed above them.
///
/// The remainder is the instrument's own honesty check and is printed whatever
/// its size: a few percent is the `Instant` pairs and the arithmetic between
/// the stages, and a large one means a bucket is missing.
#[cfg(feature = "profiling")]
fn render_report(args: &Args, done: u32, elapsed: std::time::Duration) {
    use pdfrum_page::renderprofile::Stage;

    let p = pdfrum_page::renderprofile::take();
    if p.stage_calls.iter().all(|c| *c == 0) {
        eprintln!();
        eprintln!("note: the render ran but recorded no stages, which should not happen.");
        return;
    }

    let iters = f64::from(done.max(1));
    let total_ms = elapsed.as_secs_f64() * 1000.0 / iters;
    let share = |ms: f64| {
        if total_ms > 0.0 {
            ms * 100.0 / total_ms
        } else {
            0.0
        }
    };

    let ms_of = |stage: Stage| {
        p.stage_time
            .get(stage.index())
            .map_or(0.0, |t| t.as_secs_f64() * 1000.0 / iters)
    };
    let calls_of = |stage: Stage| {
        p.stage_calls
            .get(stage.index())
            .map_or(0.0, |c| calls_per_iter(*c, iters))
    };

    eprintln!();
    // Pages per iteration from the raster's own call count rather than from
    // the document: this instrument reports what ran, and a `--pages` range or
    // a page that refused to rasterize would make the two disagree.
    eprintln!(
        "WHOLE RENDER, by stage ({:.0} pages x {done} iterations, backend={:?}, warm={})",
        calls_of(Stage::Raster),
        args.backend,
        args.warm
    );
    eprintln!();
    eprintln!(
        "{:<22} {:>10} {:>8} {:>12}",
        "stage", "ms/iter", "share", "calls/iter"
    );
    eprintln!("{:-<22} {:->10} {:->8} {:->12}", "", "", "", "");

    let mut named = 0.0;
    let mut pass = 0.0;
    for stage in Stage::ALL {
        let ms = ms_of(stage);
        named += ms;
        if stage.in_annotation_pass() {
            pass += ms;
        }
        // The five annotation-pass rows are indented under the subtotal
        // printed after them, which is why the pass has no row of its own:
        // it has no span, only members.
        let indent = if stage.in_annotation_pass() { "  " } else { "" };
        eprint!(
            "{indent}{:<width$} {ms:>10.3} {:>7.1}% {:>12.0}",
            stage.name(),
            share(ms),
            calls_of(stage),
            width = 22 - indent.len(),
        );
        if stage == Stage::FormFonts {
            eprint!(
                "  {} misses/iter",
                calls_per_iter(p.form_font_misses, iters)
            );
        }
        eprintln!();
    }
    eprintln!("{:-<22} {:->10} {:->8} {:->12}", "", "", "", "");
    eprintln!(
        "{:<22} {pass:>10.3} {:>7.1}%   the five rows above, summed",
        "annotation pass",
        share(pass)
    );
    eprintln!(
        "{:<22} {named:>10.3} {:>7.1}%",
        "sum of stages",
        share(named)
    );
    eprintln!(
        "{:<22} {:>10.3} {:>7.1}%",
        "unattributed",
        total_ms - named,
        share(total_ms - named)
    );
    eprintln!("{:<22} {total_ms:>10.3} {:>7.1}%", "TOTAL", 100.0);
    eprintln!();
    eprintln!(
        "The stages are disjoint at the outermost level, which is what lets them be\n\
         summed; an appearance form's own parse and interpretation are charged to\n\
         their own rows *and* to the annotation loop that ran them, so a forms\n\
         document's sum can exceed its total. `form fonts`' miss count beside its\n\
         call count is the memoization: once per document is a hit rate of\n\
         (calls - 1) / calls, and once per page per render is no hit rate at all."
    );
}

/// The note that stands in for the phase split when the instrument is off.
///
/// A table of zeroes reads as a finding, so the remedy is printed instead.
/// This is the whole of `walk_report` without the feature, and it names no
/// `walkprofile` item — the module's surface is part of the feature, not of
/// the crate a `cargo add` reaches.
#[cfg(not(feature = "profiling"))]
fn walk_report(_iters: f64, _engine: std::time::Duration) {
    eprintln!();
    eprintln!(
        "note: the walk's own phase split is off. Rebuild with it:\n\
         \x20 cargo build --release -p pdfrum-bench --bin profile --features profiling\n\
         or run `scripts/profile.nu <op> <file> <iters> <backend> --walk`."
    );
}

/// Whether a phase's time is already inside another phase's, so that summing
/// it into the named total would double-count.
#[cfg(feature = "profiling")]
fn nested(phase: pdfrum_render::walkprofile::Phase) -> bool {
    use pdfrum_render::walkprofile::Phase;
    matches!(
        phase,
        Phase::PathPrep | Phase::RectTest | Phase::ZeroScan | Phase::PathXform | Phase::Patches
    )
}

/// Split the ENGINE half further, from the walk's own instrumentation.
#[cfg(feature = "profiling")]
fn walk_report(iters: f64, engine: std::time::Duration) {
    use pdfrum_render::walkprofile::{Phase, Site};

    let p = pdfrum_render::walkprofile::take();
    if p.phase_calls.iter().all(|c| *c == 0) && p.site_count.iter().all(|c| *c == 0) {
        eprintln!();
        eprintln!("note: the walk ran but recorded nothing, which should not happen.");
        return;
    }

    let engine_ms = engine.as_secs_f64() * 1000.0 / iters;
    eprintln!();
    eprintln!("ENGINE, split by phase (the buckets overlap where one phase nests in another):");
    eprintln!();
    eprintln!(
        "{:<16} {:>10} {:>10} {:>12}",
        "phase", "ms/iter", "of ENGINE", "calls/iter"
    );
    eprintln!("{:-<16} {:->10} {:->10} {:->12}", "", "", "", "");
    let mut named = 0.0;
    for phase in Phase::ALL {
        let i = phase.index();
        let (Some(t), Some(c)) = (p.phase_time.get(i), p.phase_calls.get(i)) else {
            continue;
        };
        let ms = t.as_secs_f64() * 1000.0 / iters;
        // `PathPrep` wraps the device calls a path makes, which the seam
        // decorator has already charged to RASTER. Adding it to `named` would
        // subtract raster time from the interpretation residue and make the
        // residue negative on a path-heavy document, so it is reported and
        // excluded from the sum, with the note below saying so. The three
        // phases added inside it — the rect test, the zero-area scan
        // and the geometry build — nest in `PathPrep` for the same reason and
        // are excluded on the same grounds; `Cull` is disjoint from every
        // other phase and does count.
        if !nested(phase) {
            named += ms;
        }
        eprintln!(
            "{:<16} {ms:>10.3} {:>9.1}% {:>12.0}{}",
            phase.name(),
            if engine_ms > 0.0 {
                ms * 100.0 / engine_ms
            } else {
                0.0
            },
            calls_per_iter(*c, iters),
            match phase {
                Phase::PathPrep | Phase::Patches =>
                    "  (includes its own fill/stroke calls, counted in RASTER)",
                Phase::RectTest | Phase::ZeroScan | Phase::PathXform => "  (nested in path prep)",
                _ => "",
            },
        );
    }
    eprintln!("{:-<16} {:->10} {:->10} {:->12}", "", "", "", "");
    let rest = engine_ms - named;
    eprintln!(
        "{:<16} {rest:>10.3} {:>9.1}%   dispatch, cull, state, recursion, path prep",
        "INTERPRETATION",
        if engine_ms > 0.0 {
            rest * 100.0 / engine_ms
        } else {
            0.0
        },
    );
    eprintln!("{:<16} {engine_ms:>10.3} {:>9.1}%", "ENGINE", 100.0);

    eprintln!();
    eprintln!("Allocation churn in the walk, by site:");
    eprintln!();
    eprintln!("{:<22} {:>14} {:>16}", "site", "allocs/iter", "KiB/iter");
    eprintln!("{:-<22} {:->14} {:->16}", "", "", "");
    let mut total_allocs = 0.0;
    let mut total_kib = 0.0;
    for site in Site::ALL {
        let i = site.index();
        let (Some(n), Some(b)) = (p.site_count.get(i), p.site_bytes.get(i)) else {
            continue;
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "a byte total of this size is exact in f64 far beyond any \
                      render's traffic; the column is a KiB figure to one decimal"
        )]
        let kib = (*b as f64) / 1024.0 / iters;
        let allocs = calls_per_iter(*n, iters);
        total_allocs += allocs;
        total_kib += kib;
        eprintln!("{:<22} {allocs:>14.0} {kib:>16.1}", site.name());
    }
    eprintln!("{:-<22} {:->14} {:->16}", "", "", "");
    eprintln!("{:<22} {total_allocs:>14.0} {total_kib:>16.1}", "TOTAL");
    eprintln!();
    eprintln!(
        "`RenderOptions clone` and `RenderCtx clone` are stack moves, not heap\n\
         allocations — every field of both is Copy. Their byte column is the\n\
         value's size and is there for scale, not for allocator traffic; the\n\
         count is what those two rows mean; every other row is real allocator\n\
         traffic. `draw_path BezPath` and `rect-test Vec<Point>` are in\n\
         `paint.rs`/`path.rs` rather than in `walk.rs`."
    );
}

/// Run the operation `iterations` times, returning how many ran and how long
/// the measured loop took.
///
/// The clock starts **after** the document is parsed and, under `--warm`, after
/// the priming render — neither is what this figure is of. Timing them was a
/// real defect: on `image_bug_718762` the priming render is ~1.2 s against a
/// warm render of well under a millisecond, so at eight iterations the reported
/// per-iteration cost was ~99% priming, and every `--warm` figure the harness
/// produced scaled with the iteration count instead of converging. `Op::Open`
/// is the exception — re-parsing *is* its operation — and it starts its own
/// clock below.
fn run(args: &Args, bytes: &Arc<[u8]>) -> (u32, std::time::Duration) {
    let options = RenderOptions::default();

    // `open` re-parses every iteration by definition; the other three parse
    // once, because a profile of rendering must not be three-quarters parser.
    if args.op == Op::Open {
        let started = Instant::now();
        for _ in 0..args.iterations {
            let doc = Document::from_bytes(Arc::clone(bytes));
            black_box(doc.map(|doc| doc.page_count()).ok());
        }
        return (args.iterations, started.elapsed());
    }

    let Ok(doc) = Document::from_bytes(Arc::clone(bytes)) else {
        eprintln!("cannot open the document");
        std::process::exit(1);
    };

    // Held across every iteration under `--warm`, primed with one untimed
    // render so the first measured iteration is warm by construction rather
    // than by luck — `render.rs`'s `warm` group primes for the same reason.
    // `None` is the cold convention: `Op::Render` builds a fresh session per
    // iteration below, which is what `render-cold-*` does.
    let mut held = args.warm.then(RenderSession::new);
    // Under `--warm` every page is prepared once, here, and the loop below
    // only draws it: that is what `pdfium_test --render-repeats` repeats, so
    // it is the only shape the two columns can be read against each other
    // in. The priming draw makes the first measured iteration warm by
    // construction rather than by luck.
    let prepared: Vec<pdfrum::PreparedPage<'_>> = match held.as_mut() {
        Some(session) => doc
            .pages()
            .map(|page| {
                let prepared = page.prepare(&options, session);
                drop(draw_one(args, &prepared, session));
                prepared
            })
            .collect(),
        None => Vec::new(),
    };

    // The priming render above is a *cold* one and its stages are not the
    // loop's, so the accumulator starts from zero at the same moment the clock
    // does.
    #[cfg(feature = "profiling")]
    let _ = pdfrum_page::renderprofile::take();

    let started = Instant::now();
    for _ in 0..args.iterations {
        match args.op {
            // `Open` returned above; `Forms` has its own loop in
            // `forms_split` and never reaches `run`.
            Op::Open | Op::Forms => {}
            Op::Render => {
                // `--warm` holds one session across iterations; without it a
                // fresh one is built here, inside the loop. That is not a
                // convenience flag: it is the difference between the
                // `render-cold-*` and `render-warm-*` criterion groups
                // (`crates/pdfrum-render/benches/render.rs`), and only the
                // second is comparable with `pdfium_test --render-repeats`.
                // See the header of `scripts/profile.nu`. The warm arm draws
                // the pages prepared above; the cold one replaces the session
                // and re-interprets every page each iteration, which is
                // exactly `render-cold-*`'s fresh-session-inside-the-closure.
                if args.warm {
                    let session = held.get_or_insert_with(RenderSession::new);
                    for page in &prepared {
                        black_box(draw_one(args, page, session).ok());
                    }
                } else {
                    let session = held.insert(RenderSession::new());
                    for page in doc.pages() {
                        black_box(render_one(args, &page, &options, session).ok());
                    }
                }
            }
            Op::Text => {
                let mut session = RenderSession::new();
                for page in doc.pages() {
                    black_box(page.text_on(&mut session).to_string().len());
                }
            }
            Op::Save => {
                let mut out = Vec::new();
                black_box(doc.write_to(&mut out, &SaveOptions::default()).ok());
                black_box(out.len());
            }
        }
    }
    (args.iterations, started.elapsed())
}

/// Where a render's time goes, attributed at the engine/rasterizer seam.
///
/// **This is instrumentation, not sampling, and the difference is the point.**
/// `scripts/profile.nu` prefers `perf record`, which samples the hardware and
/// unwinds DWARF; it is strictly better and this fallback does not pretend
/// otherwise. But on a machine where `perf` is unavailable — no binary, or
/// `kernel.perf_event_paranoid > 1`, which is the common case in a container
/// and is the case on the machine 's numbers were taken on
/// the alternatives are all bad: a backtrace sampler needs `unsafe`
/// (`unsafe_code = "forbid"` workspace-wide) or an unwinder crate, and a sampler that only reports elapsed time tells you nothing you
/// did not already know.
///
/// So this measures something real instead. It wraps the chosen
/// [`RasterBackend`](pdfrum_render::RasterBackend) in a decorator that clocks every `RenderDevice` call and
/// counts it, which splits a page render into the two halves the items
/// are actually about:
///
/// - **raster** — the sum of the device calls, i.e. everything below the seam:
///   scanline integration, span blitting, compositing, image sampling.
/// - **engine** — total minus raster: content interpretation, the page-graph
///   walk, glyph outline lookup and flattening, colour conversion, clip
///   bookkeeping.
///
/// and inside the raster half attributes to `fill_path`, `stroke_path`,
/// `draw_image`, the clip calls and the layer calls separately. That is
/// precisely the granularity at which "is the cost in the composite loop or in
/// the walk" is answerable, and it is answerable *exactly* rather than
/// statistically.
///
/// Its honest limitation: an `Instant::now()` pair around a call that costs a
/// microsecond is a few percent of that call, and the decorator itself is
/// counted in the engine half. Both are stated in .md beside the numbers,
/// and neither moves a conclusion that a 20% attribution difference rests on.
mod timed {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    use kurbo::{Affine, BezPath, Rect, Stroke};
    use pdfrum_page::BlendMode;
    use pdfrum_render::{
        AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, Pixmap, RasterBackend, RasterImage,
        RenderDevice,
    };

    /// One primitive's running total.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Bucket {
        /// How many calls landed here.
        pub calls: u64,
        /// How long they took in total.
        pub time: Duration,
    }

    impl Bucket {
        /// Fold one call in.
        fn add(&mut self, elapsed: Duration) {
            self.calls += 1;
            self.time += elapsed;
        }
    }

    /// The seven buckets a device call can land in.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Totals {
        /// `fill_path`.
        pub fill: Bucket,
        /// `stroke_path`.
        pub stroke: Bucket,
        /// `draw_image` — the glyph-bitmap blit path as well as real images.
        pub image: Bucket,
        /// `push_clip` and `push_clip_rect`.
        pub clip: Bucket,
        /// `push_layer` and `pop`.
        pub layer: Bucket,
        /// `new_target` and `new_target_with_backdrop` — allocation.
        pub target: Bucket,
        /// `finish` and `snapshot` — a retained-scene backend's whole raster.
        pub finish: Bucket,
    }

    impl Totals {
        /// Everything below the seam.
        pub fn raster(&self) -> Duration {
            self.fill.time
                + self.stroke.time
                + self.image.time
                + self.clip.time
                + self.layer.time
                + self.target.time
                + self.finish.time
        }
    }

    thread_local! {
        /// Where the decorator accumulates. A thread local rather than a field
        /// because `RasterBackend::new_target` returns a device by value and
        /// the engine owns it from then on, so there is nowhere to hang a
        /// borrow that outlives the call.
        static TOTALS: RefCell<Totals> = const { RefCell::new(Totals {
            fill: Bucket { calls: 0, time: Duration::ZERO },
            stroke: Bucket { calls: 0, time: Duration::ZERO },
            image: Bucket { calls: 0, time: Duration::ZERO },
            clip: Bucket { calls: 0, time: Duration::ZERO },
            layer: Bucket { calls: 0, time: Duration::ZERO },
            target: Bucket { calls: 0, time: Duration::ZERO },
            finish: Bucket { calls: 0, time: Duration::ZERO },
        }) };
    }

    /// Clear the running totals.
    pub fn reset() {
        TOTALS.with_borrow_mut(|t| *t = Totals::default());
    }

    /// Read the running totals.
    pub fn totals() -> Totals {
        TOTALS.with_borrow(|t| *t)
    }

    /// Time one call and fold it into `pick`'s bucket.
    fn timed<T>(pick: fn(&mut Totals) -> &mut Bucket, body: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let out = body();
        let elapsed = started.elapsed();
        TOTALS.with_borrow_mut(|t| pick(t).add(elapsed));
        out
    }

    /// A backend that clocks the one it wraps.
    pub struct TimedBackend<B>(pub B);

    /// A device that clocks the one it wraps.
    pub struct TimedDevice<D>(D);

    impl<B: RasterBackend> RasterBackend for TimedBackend<B> {
        type Device = TimedDevice<B::Device>;

        fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
            TimedDevice(timed(|t| &mut t.target, || self.0.new_target(w, h, clear)))
        }

        fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
            TimedDevice(timed(
                |t| &mut t.target,
                || self.0.new_target_with_backdrop(base),
            ))
        }

        fn snapshot(&self, d: &Self::Device) -> Pixmap {
            timed(|t| &mut t.finish, || self.0.snapshot(&d.0))
        }

        fn finish(&self, d: Self::Device) -> Pixmap {
            timed(|t| &mut t.finish, || self.0.finish(d.0))
        }
    }

    impl<D: RenderDevice> RenderDevice for TimedDevice<D> {
        fn fill_path(
            &mut self,
            path: &BezPath,
            transform: Affine,
            brush: &Brush<'_>,
            rule: FillRule,
            aa: AntiAlias,
        ) {
            timed(
                |t| &mut t.fill,
                || self.0.fill_path(path, transform, brush, rule, aa),
            );
        }

        fn stroke_path(
            &mut self,
            path: &BezPath,
            transform: Affine,
            brush: &Brush<'_>,
            stroke: &Stroke,
            aa: AntiAlias,
        ) {
            timed(
                |t| &mut t.stroke,
                || self.0.stroke_path(path, transform, brush, stroke, aa),
            );
        }

        fn draw_image(
            &mut self,
            img: &RasterImage,
            transform: Affine,
            quality: ImageQuality,
            alpha: f32,
        ) {
            timed(
                |t| &mut t.image,
                || self.0.draw_image(img, transform, quality, alpha),
            );
        }

        fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
            timed(|t| &mut t.clip, || self.0.push_clip(path, rule));
        }

        fn push_clip_rect(&mut self, rect: Rect) {
            timed(|t| &mut t.clip, || self.0.push_clip_rect(rect));
        }

        fn push_layer(&mut self, mode: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
            timed(|t| &mut t.layer, || self.0.push_layer(mode, alpha, mask));
        }

        fn pop(&mut self) {
            timed(|t| &mut t.layer, || self.0.pop());
        }
    }
}
