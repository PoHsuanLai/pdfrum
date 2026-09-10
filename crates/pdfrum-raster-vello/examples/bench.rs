//! G3: what the GPU backend costs, two ways.
//!
//! **`gpu` is the pixmap path** — a whole `render_page`, ending in
//! `RasterBackend::finish` (texture, dispatch, `copy_texture_to_buffer`,
//! `map_async`). That is what a thumbnailer or a test pays.
//!
//! **`present` is the GUI path** — `render_page_to_device` plus
//! `render_to_texture`, no host readback. That is what a viewer that already
//! holds a `wgpu::Device` pays: the page stays a texture. Comparing only the
//! pixmap column against `vello_cpu` answers the wrong question for that
//! caller.
//!
//! It is also explicit about what a good outcome looks like: "a backend that
//! loses on the corpus but wins on heavy vector pages is a **success with a
//! documented envelope**, not a failure — say which documents fall on which
//! side." So this reports the **crossover** rather than a single ratio, and
//! sorts by it.
//!
//! The CPU column is `pdfrum-raster-vello-cpu` (`vello_cpu`), the facade's default
//! and the backend an API user actually gets. `pdfrum-raster-agg` is the
//! conformance default but has no SIMD and does not try to be fast, so timing
//! against it would flatter the GPU for a reason that has nothing to do with
//! the GPU. §7's *correctness* column uses `exact` for the opposite reason.
//!
//! Run: `cargo run --release -p pdfrum-raster-vello --example bench`

#[path = "shared/harness.rs"]
mod harness;

use std::time::{Duration, Instant};

use pdfrum_raster_vello::{RoundtripStats, VelloBackend, try_real_gpu};
use pdfrum_raster_vello_cpu::VelloCpuBackend;

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
        "# GPU vello vs vello_cpu\n\
         # gpu = pixmap finish (readback included)\n\
         # present = record + render_to_texture (the GUI path, no map_async)\n\
         # adapter: {}\n\
         # median of {ITERATIONS} renders, one pixel per PDF point\n",
        report.map_or_else(|| "<unknown>".to_owned(), |r| r.to_string())
    );
    let cpu = VelloCpuBackend::new();

    println!(
        "{:<28} {:>9} {:>10} {:>10} {:>10} {:>8} {:>8} {:>7}",
        "document", "pixels", "cpu ms", "gpu ms", "pres ms", "gpu/cpu", "pres/cpu", "class"
    );

    let mut rows = Vec::new();
    for doc in pdfrum_corpus::CORPUS {
        let path = pdfrum_corpus::path(doc.stem);
        let Some(subject) = harness::subject(&path, doc.stem) else {
            println!("{:<28} {:>9}", doc.stem, "skipped");
            continue;
        };
        let (Some(cpu_time), Some(gpu_time), Some(present_time)) = (
            harness::time(&subject, &cpu, ITERATIONS),
            harness::time(&subject, &gpu, ITERATIONS),
            time_present(&subject, &gpu, ITERATIONS),
        ) else {
            println!("{:<28} {:>9}", doc.stem, "no-render");
            continue;
        };
        let ratio = gpu_time.as_secs_f64() / cpu_time.as_secs_f64();
        let present_ratio = present_time.as_secs_f64() / cpu_time.as_secs_f64();
        println!(
            "{:<28} {:>9} {:>10.2} {:>10.2} {:>10.2} {:>8.2}x {:>8.2}x {:>7}",
            doc.stem,
            subject.pixels(),
            ms(cpu_time),
            ms(gpu_time),
            ms(present_time),
            ratio,
            present_ratio,
            doc.class.name()
        );
        // One extra render, after the timed window, so the counters describe
        // a single page rather than ten stacked on a warm-up. Reset first:
        // the timed window already ran and would otherwise dominate.
        gpu.reset_roundtrip_stats();
        let stats = if harness::render_once(&subject, &gpu).is_some() {
            gpu.roundtrip_stats()
        } else {
            RoundtripStats::default()
        };
        rows.push(Row {
            stem: doc.stem,
            class: doc.class.name(),
            cpu: cpu_time,
            gpu: gpu_time,
            present: present_time,
            ratio,
            present_ratio,
            stats,
        });
    }

    summarize(&rows);
}

/// Time the GUI path: record the page, dispatch to a storage texture, no
/// `map_async`. A viewer that already has a `wgpu` device pays this, not
/// `RasterBackend::finish`.
fn time_present(
    subject: &harness::Subject,
    backend: &VelloBackend<'_>,
    iterations: usize,
) -> Option<Duration> {
    let once = || {
        let mut diags = pdfrum_common::Diagnostics::default();
        let device = pdfrum_render::render_page_to_device(
            &subject.page,
            &subject.options,
            backend,
            &mut diags,
        )
        .ok()?;
        backend.render_to_texture(&device).ok()
    };
    once()?;
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let ok = once().is_some();
        let elapsed = start.elapsed();
        if !ok {
            return None;
        }
        samples.push(elapsed);
    }
    samples.sort_unstable();
    samples.get(samples.len() / 2).copied()
}

struct Row {
    stem: &'static str,
    class: &'static str,
    cpu: Duration,
    gpu: Duration,
    present: Duration,
    ratio: f64,
    present_ratio: f64,
    stats: RoundtripStats,
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
    let present_geo = geomean(rows.iter().map(|r| r.present_ratio));
    let wins: Vec<&Row> = rows.iter().filter(|r| r.ratio < 1.0).collect();
    let present_wins: Vec<&Row> = rows.iter().filter(|r| r.present_ratio < 1.0).collect();

    println!("\n# summary over {} documents", rows.len());
    println!("geometric mean gpu/cpu:      {geo:.2}x   (pixmap finish, below 1.0 = GPU faster)");
    println!("geometric mean present/cpu:  {present_geo:.2}x   (GUI path, no readback)");
    println!(
        "documents where GPU pixmap wins:  {} of {}",
        wins.len(),
        rows.len()
    );
    println!(
        "documents where GPU present wins: {} of {}",
        present_wins.len(),
        rows.len()
    );

    print_roundtrips(rows);

    println!("\n# per class (geometric mean)");
    let mut classes: Vec<&str> = rows.iter().map(|r| r.class).collect();
    classes.sort_unstable();
    classes.dedup();
    for class in classes {
        let of: Vec<&Row> = rows.iter().filter(|r| r.class == class).collect();
        let g = geomean(of.iter().map(|r| r.ratio));
        let p = geomean(of.iter().map(|r| r.present_ratio));
        let w = of.iter().filter(|r| r.ratio < 1.0).count();
        let pw = of.iter().filter(|r| r.present_ratio < 1.0).count();
        println!(
            "{class:<10} pixmap {g:>7.2}x ({w}/{})   present {p:>7.2}x ({pw}/{})",
            of.len(),
            of.len()
        );
    }

    // The crossover, read off the data rather than asserted.
    // Sorting by CPU time answers "how much work does a page need before the
    // GPU is worth it", which is the question an embedder deciding per page
    // actually has.
    let mut by_cost: Vec<&Row> = rows.iter().collect();
    by_cost.sort_by_key(|r| r.cpu);
    println!("\n# crossover: documents by CPU render time");
    println!(
        "{:<28} {:>10} {:>10} {:>10} {:>8} {:>8}",
        "document", "cpu ms", "gpu ms", "pres ms", "gpu/cpu", "pres/cpu"
    );
    for r in &by_cost {
        println!(
            "{:<28} {:>10.2} {:>10.2} {:>10.2} {:>8.2}x {:>8.2}x",
            r.stem,
            ms(r.cpu),
            ms(r.gpu),
            ms(r.present),
            r.ratio,
            r.present_ratio,
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

fn print_roundtrips(rows: &[Row]) {
    let sum = |f: fn(&RoundtripStats) -> u64| rows.iter().map(|r| f(&r.stats)).sum::<u64>();
    println!(
        "roundtrips (one render each): finish {}  snapshot {}  new_target {}  \
         pixels_read {}  bytes_uploaded {}  tex created/reused {}/{}",
        sum(|s| s.finishes),
        sum(|s| s.snapshots),
        sum(|s| s.new_targets),
        sum(|s| s.pixels_read),
        sum(|s| s.bytes_uploaded),
        sum(|s| s.textures_created),
        sum(|s| s.textures_reused),
    );

    println!("\n# roundtrips per document (one render)");
    println!(
        "{:<28} {:>6} {:>6} {:>6} {:>10} {:>10} {:>7}",
        "document", "fin", "snap", "tgt", "px read", "up bytes", "tex r/c"
    );
    for r in rows {
        let s = r.stats;
        println!(
            "{:<28} {:>6} {:>6} {:>6} {:>10} {:>10} {:>3}/{:<3}",
            r.stem,
            s.finishes,
            s.snapshots,
            s.new_targets,
            s.pixels_read,
            s.bytes_uploaded,
            s.textures_reused,
            s.textures_created,
        );
    }
}
