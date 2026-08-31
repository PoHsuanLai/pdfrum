//! G3: what the GPU backend costs, upload and readback included.
//!
//! PLAN.md §M12c is explicit about the accounting: **include upload and
//! readback**, because an embedder rendering a page to a texture pays them and
//! a GPU number that excludes them is marketing rather than measurement. So
//! the timed region is a whole `render_page_with_caches` call, which on the
//! GPU column ends in `RasterBackend::finish` — texture allocation, vello
//! dispatch, `copy_texture_to_buffer`, and the host stall waiting for
//! `map_async`. Nothing is subtracted.
//!
//! It is also explicit about what a good outcome looks like: "a backend that
//! loses on the corpus but wins on heavy vector pages is a **success with a
//! documented envelope**, not a failure — say which documents fall on which
//! side." So this reports the **crossover** rather than a single ratio, and
//! sorts by it.
//!
//! The CPU column is `pdfrum-raster-vello` (`vello_cpu`), the facade's default
//! and the backend an API user actually gets. `pdfrum-raster-exact` is the
//! conformance default but has no SIMD and does not try to be fast, so timing
//! against it would flatter the GPU for a reason that has nothing to do with
//! the GPU. §7's *correctness* column uses `exact` for the opposite reason.
//!
//! Run: `cargo run --release -p pdfrum-raster-vello-gpu --example bench`

#[path = "shared/harness.rs"]
mod harness;

use std::time::Duration;

use pdfrum_raster_vello::VelloBackend;
use pdfrum_raster_vello_gpu::try_real_gpu;

/// How many timed renders each document gets.
///
/// Ten, with a warm-up before the window and the **median** reported. This is
/// not criterion's hundreds, and the reason is the instrument rather than
/// impatience: every GPU iteration contains a host stall on a device shared
/// with a desktop session, so the distribution has a tail that more samples
/// characterise rather than average away. The median of ten is stable to about
/// a percent here, which is well inside the effect sizes below.
const ITERATIONS: usize = 10;

fn main() {
    let Some(gpu) = try_real_gpu() else {
        println!("no hardware wgpu adapter on this machine — nothing measured");
        return;
    };
    let report = gpu.adapter_report();
    println!(
        "# GPU vello vs vello_cpu, upload and readback included\n\
         # adapter: {}\n\
         # median of {ITERATIONS} renders, one pixel per PDF point\n",
        report.map_or_else(|| "<unknown>".to_owned(), |r| r.to_string())
    );
    let cpu = VelloBackend::new();

    println!(
        "{:<28} {:>9} {:>10} {:>10} {:>8} {:>7}",
        "document", "pixels", "cpu ms", "gpu ms", "gpu/cpu", "class"
    );

    let mut rows = Vec::new();
    for doc in pdfrum_corpus::CORPUS {
        let path = pdfrum_corpus::path(doc.stem);
        let Some(subject) = harness::subject(&path, doc.stem) else {
            println!("{:<28} {:>9}", doc.stem, "skipped");
            continue;
        };
        let (Some(cpu_time), Some(gpu_time)) = (
            harness::time(&subject, &cpu, ITERATIONS),
            harness::time(&subject, &gpu, ITERATIONS),
        ) else {
            println!("{:<28} {:>9}", doc.stem, "no-render");
            continue;
        };
        let ratio = gpu_time.as_secs_f64() / cpu_time.as_secs_f64();
        println!(
            "{:<28} {:>9} {:>10.2} {:>10.2} {:>8.2}x {:>7}",
            doc.stem,
            subject.pixels(),
            ms(cpu_time),
            ms(gpu_time),
            ratio,
            doc.class.name()
        );
        rows.push(Row {
            stem: doc.stem,
            class: doc.class.name(),
            cpu: cpu_time,
            gpu: gpu_time,
            ratio,
        });
    }

    summarize(&rows);
}

struct Row {
    stem: &'static str,
    class: &'static str,
    cpu: Duration,
    gpu: Duration,
    ratio: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The geometric mean of a set of ratios.
///
/// Geometric and not arithmetic because these are *ratios*: an arithmetic mean
/// is dominated by whichever direction happens to produce large numbers, so
/// 0.5x and 2.0x would average to 1.25x instead of cancelling to 1.0x.
fn geomean(ratios: impl Iterator<Item = f64>) -> f64 {
    let mut n = 0u32;
    let mut sum = 0.0;
    for r in ratios {
        if r > 0.0 {
            sum += r.ln();
            n += 1;
        }
    }
    if n == 0 {
        return f64::NAN;
    }
    (sum / f64::from(n)).exp()
}

fn summarize(rows: &[Row]) {
    if rows.is_empty() {
        println!("\nnothing measured");
        return;
    }
    let geo = geomean(rows.iter().map(|r| r.ratio));
    let wins: Vec<&Row> = rows.iter().filter(|r| r.ratio < 1.0).collect();

    println!("\n# summary over {} documents", rows.len());
    println!("geometric mean gpu/cpu:  {geo:.2}x   (below 1.0 = GPU faster)");
    println!("documents where GPU wins: {} of {}", wins.len(), rows.len());

    println!("\n# per class (geometric mean)");
    let mut classes: Vec<&str> = rows.iter().map(|r| r.class).collect();
    classes.sort_unstable();
    classes.dedup();
    for class in classes {
        let of: Vec<&Row> = rows.iter().filter(|r| r.class == class).collect();
        let g = geomean(of.iter().map(|r| r.ratio));
        let w = of.iter().filter(|r| r.ratio < 1.0).count();
        println!("{class:<10} {g:>7.2}x   GPU wins {w} of {}", of.len());
    }

    // The crossover PLAN.md asks for, read off the data rather than asserted.
    // Sorting by CPU time answers "how much work does a page need before the
    // GPU is worth it", which is the question an embedder deciding per page
    // actually has.
    let mut by_cost: Vec<&Row> = rows.iter().collect();
    by_cost.sort_by_key(|r| r.cpu);
    println!("\n# crossover: documents by CPU render time");
    println!(
        "{:<28} {:>10} {:>10} {:>8} {:>6}",
        "document", "cpu ms", "gpu ms", "gpu/cpu", "wins"
    );
    for r in &by_cost {
        println!(
            "{:<28} {:>10.2} {:>10.2} {:>8.2}x {:>6}",
            r.stem,
            ms(r.cpu),
            ms(r.gpu),
            r.ratio,
            if r.ratio < 1.0 { "gpu" } else { "cpu" }
        );
    }

    if let Some(slowest_cpu_win) = wins
        .iter()
        .map(|r| ms(r.cpu))
        .fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a| a.min(v)))
        })
    {
        println!("\ncheapest page the GPU still wins on: {slowest_cpu_win:.2} ms of CPU work");
    }
    let dearest_cpu_win = rows
        .iter()
        .filter(|r| r.ratio >= 1.0)
        .map(|r| ms(r.cpu))
        .fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a| a.max(v)))
        });
    if let Some(v) = dearest_cpu_win {
        println!("dearest page the CPU still wins on:  {v:.2} ms of CPU work");
    }
}
