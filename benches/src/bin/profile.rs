//! One operation, one file, in a loop, with nothing else in the process.
//!
//! This is the binary `scripts/profile.sh` records. It exists because a
//! criterion run is the wrong thing to profile: criterion interleaves its own
//! statistics, resampling and outlier analysis between iterations, so a
//! symbol's share of a `perf report` taken over `cargo bench` is its share of
//! *criterion plus the engine* rather than its share of the operation. Here
//! the process does the operation and returns.
//!
//! The code paths are the same ones `benches/engine.rs` measures, deliberately:
//! a profile that attributes cost to a function the benchmark never calls is
//! worse than no profile. Where the two differ the benchmark is the authority
//! and this binary is wrong.
//!
//! ```text
//! profile --op render --file benches/fixtures/foxittext.pdf --iterations 50
//! profile --op render --backend vello --file … --iterations 20
//! profile --op text --file … --sample     # the built-in sampler
//! ```

use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pdfrum::{Backend, Document, RenderOptions, RenderSession, SaveOptions};

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
}

impl Op {
    /// Parse the `--op` value.
    fn parse(s: &str) -> Option<Op> {
        match s {
            "open" => Some(Op::Open),
            "render" => Some(Op::Render),
            "text" => Some(Op::Text),
            "save" => Some(Op::Save),
            _ => None,
        }
    }
}

/// Parse the `--backend` value.
fn backend(s: &str) -> Option<Backend> {
    match s {
        "exact" => Some(Backend::Exact),
        "tinyskia" | "tiny-skia" => Some(Backend::TinySkia),
        "vello" | "vello_cpu" => Some(Backend::Vello),
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
}

/// Read the command line, or explain what was wrong with it.
fn parse_args() -> Result<Args, String> {
    let mut op = Op::Render;
    let mut file = None;
    let mut iterations = 50u32;
    let mut back = Backend::Exact;
    let mut sample = false;

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
    })
}

/// The `--help` text.
fn usage() -> String {
    "profile --op <open|render|text|save> --file <pdf> \
     [--iterations N] [--backend exact|tinyskia|vello] [--sample]"
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
    if args.sample && args.op == Op::Render {
        return timed_render(&args, &bytes);
    }

    let started = Instant::now();
    let done = run(&args, &bytes);
    let elapsed = started.elapsed();

    eprintln!(
        "{} iterations in {:.3} s = {:.3} ms/iteration",
        done,
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / f64::from(done.max(1)),
    );

    if args.sample {
        eprintln!();
        eprintln!(
            "note: --sample splits engine from raster on `--op render` only;\n\
             for {}, the total above is all this fallback can attribute.\n\
             A per-symbol profile needs `perf` — see scripts/profile.sh.",
            match args.op {
                Op::Open => "open",
                Op::Text => "text",
                Op::Save => "save",
                Op::Render => "render",
            }
        );
    }
}

/// Render every page `iterations` times through a clocked backend, and print
/// where the time went.
///
/// This bypasses `Page::render_session` and calls `render_page_with_caches`
/// directly, because the decorator has to be substituted for the backend and
/// the facade chooses one from `RenderOptions::backend`. The work is otherwise
/// the same: same page graph, same caches, same options — the `page_graph`
/// figure below is the part the facade does before it hands the graph over,
/// and it is measured rather than assumed so that "engine" here means the same
/// thing it means in M12.md.
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
    let mut caches = pdfrum_render::RenderCaches::default();
    let mut diags = pdfrum_common::Diagnostics::default();

    let started = Instant::now();
    for _ in 0..args.iterations {
        for graph in &graphs {
            let pixmap = match args.backend {
                pdfrum::Backend::Exact => pdfrum_render::render_page_with_caches(
                    graph,
                    &inner,
                    &timed::TimedBackend(pdfrum_raster_exact::ExactBackend::new()),
                    &mut caches,
                    &mut diags,
                ),
                pdfrum::Backend::TinySkia => pdfrum_render::render_page_with_caches(
                    graph,
                    &inner,
                    &timed::TimedBackend(pdfrum_raster_tinyskia::TinySkiaBackend::new()),
                    &mut caches,
                    &mut diags,
                ),
                pdfrum::Backend::Vello => pdfrum_render::render_page_with_caches(
                    graph,
                    &inner,
                    &timed::TimedBackend(pdfrum_raster_vello::VelloBackend::new()),
                    &mut caches,
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
}

/// Run the operation `iterations` times, returning how many actually ran.
fn run(args: &Args, bytes: &Arc<[u8]>) -> u32 {
    let options = RenderOptions {
        backend: args.backend,
        ..RenderOptions::default()
    };

    // `open` re-parses every iteration by definition; the other three parse
    // once, because a profile of rendering must not be three-quarters parser.
    if args.op == Op::Open {
        for _ in 0..args.iterations {
            let doc = Document::from_bytes(Arc::clone(bytes));
            black_box(doc.map(|doc| doc.page_count()).ok());
        }
        return args.iterations;
    }

    let Ok(doc) = Document::from_bytes(Arc::clone(bytes)) else {
        eprintln!("cannot open the document");
        std::process::exit(1);
    };

    for _ in 0..args.iterations {
        match args.op {
            Op::Open => {}
            Op::Render => {
                let mut session = RenderSession::new();
                for page in doc.pages() {
                    black_box(page.render_session(&options, &mut session).ok());
                }
            }
            Op::Text => {
                let mut session = RenderSession::new();
                for page in doc.pages() {
                    black_box(page.text_session(&mut session).all_text().len());
                }
            }
            Op::Save => {
                let mut out = Vec::new();
                black_box(doc.write_to(&mut out, &SaveOptions::default()).ok());
                black_box(out.len());
            }
        }
    }
    args.iterations
}

/// Where a render's time goes, attributed at the engine/rasterizer seam.
///
/// **This is instrumentation, not sampling, and the difference is the point.**
/// `scripts/profile.sh` prefers `perf record`, which samples the hardware and
/// unwinds DWARF; it is strictly better and this fallback does not pretend
/// otherwise. But on a machine where `perf` is unavailable — no binary, or
/// `kernel.perf_event_paranoid > 1`, which is the common case in a container
/// and is the case on the machine docs/status/M12.md's numbers were taken on —
/// the alternatives are all bad: a backtrace sampler needs `unsafe`
/// (`unsafe_code = "forbid"` workspace-wide) or an unwinder crate (DEPS.md is
/// closed), and a sampler that only reports elapsed time tells you nothing you
/// did not already know.
///
/// So this measures something real instead. It wraps the chosen
/// [`RasterBackend`](pdfrum_render::RasterBackend) in a decorator that clocks every `RenderDevice` call and
/// counts it, which splits a page render into the two halves the M12 P1 items
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
/// counted in the engine half. Both are stated in M12.md beside the numbers,
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
