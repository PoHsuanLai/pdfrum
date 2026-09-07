//! G2: what the GPU backend's divergence from a CPU backend actually is.
//!
//! settles the terms before any measurement, and this binary
//! implements exactly those terms:
//!
//! - GPU rasterization is **not bit-reproducible across vendors and drivers**,
//!   so this backend cannot join the conformance scoreboard and must never
//!   weaken it. It is a **Tier C participant only**.
//! - The CPU backends stay the oracle-compared ones; this is compared against
//!   *them*, with a budget stated up front.
//! - **If agreement would require changing CPU output, that is forbidden** —
//!   the CPU output is what the oracle validates. So this program only ever
//!   *reports*; nothing it finds may become a reason to touch the engine.
//!
//! The reference column is `pdfrum-raster-agg`, deliberately. It is the
//! conformance default and the backend whose edge quantisation is closest to
//! the oracle's own (SPEC §8 item 7), so a divergence measured against it is
//! the most meaningful available. `vello_cpu` would have been the other
//! candidate; §7 of the status doc records why it is reported second rather
//! than gated on.
//!
//! # The metric is the harness's, not a new one
//!
//! The populations, tolerances and the dilate-from-the-intersection rule are
//! `conformance/src/tierc.rs`'s, reimplemented here **with its constants
//! unchanged** because that crate is binary-only and exposes no library. The
//! reasoning behind each choice lives there and is not repeated; what matters
//! is that this does not quietly invent a friendlier metric for the backend it
//! is grading.
//!
//! Run: `cargo run --release -p pdfrum-raster-vello --bin gpu-tier-c`

#[path = "shared/harness.rs"]
mod harness;

use pdfrum_raster_agg::AggBackend;
use pdfrum_raster_vello::try_real_gpu;
use pdfrum_render::Pixmap;

/// The per-channel difference an *edge* pixel may show without being counted.
///
/// `conformance/src/tierc.rs`'s `EDGE_TOLERANCE`, unchanged: an antialiased
/// edge is a coverage ramp, and two integrations of the same ramp land a good
/// way apart at the steepest point without either being wrong.
const EDGE_TOLERANCE: u8 = 8;

/// The per-channel difference an *interior* pixel may show.
///
/// `conformance/src/tierc.rs`'s `INTERIOR_TOLERANCE`, unchanged.
const INTERIOR_TOLERANCE: u8 = 1;

fn main() {
    let Some(gpu) = try_real_gpu() else {
        println!("no hardware wgpu adapter on this machine — nothing measured");
        return;
    };
    let report = gpu.adapter_report();
    println!(
        "# Tier C: pdfrum-raster-vello against pdfrum-raster-agg\n\
         # adapter: {}\n",
        report.map_or_else(|| "<unknown>".to_owned(), |r| r.to_string())
    );
    let exact = AggBackend::new();

    println!(
        "{:<28} {:>9} {:>10} {:>10} {:>9} {:>7}",
        "document", "pixels", "edge%", "interior", "maxdiff", "class"
    );

    let mut rows = Vec::new();
    for doc in pdfrum_corpus::CORPUS {
        let path = pdfrum_corpus::path(doc.stem);
        let Some(subject) = harness::subject(&path, doc.stem) else {
            println!("{:<28} {:>9}", doc.stem, "skipped");
            continue;
        };
        let (Some(a), Some(b)) = (
            harness::render_once(&subject, &exact),
            harness::render_once(&subject, &gpu),
        ) else {
            println!("{:<28} {:>9}", doc.stem, "no-render");
            continue;
        };
        let Some(d) = compare(&a, &b) else {
            println!("{:<28} {:>9}", doc.stem, "size-mismatch");
            continue;
        };
        println!(
            "{:<28} {:>9} {:>9.3}% {:>10} {:>9} {:>7}",
            doc.stem,
            d.pixels,
            d.edge_rate() * 100.0,
            d.interior_differing,
            d.max_channel_diff,
            doc.class.name()
        );
        rows.push((doc.stem, doc.class.name(), d));
    }

    summarize(&rows);
}

/// What comparing one page under two backends found.
///
/// `conformance/src/tierc.rs`'s `Divergence`, field for field.
#[derive(Debug, Clone, Copy)]
struct Divergence {
    pixels: u64,
    edge_pixels: u64,
    edge_differing: u64,
    interior_differing: u64,
    max_channel_diff: u8,
    one_painted_nothing: bool,
}

impl Divergence {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a page's pixel count is far inside f64's exact integer range \
                  — conformance/src/tierc.rs carries the same expect"
    )]
    fn edge_rate(&self) -> f64 {
        if self.edge_pixels == 0 {
            return 0.0;
        }
        self.edge_differing as f64 / self.edge_pixels as f64
    }
}

/// The arithmetic mean of a set of rates.
///
/// Arithmetic and not geometric, unlike the *ratios* in `bench.rs`: these are
/// rates in [0, 1] and several are exactly zero, which a geometric mean cannot
/// represent at all.
fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let mut n = 0u32;
    let mut sum = 0.0;
    for v in values {
        sum += v;
        n += 1;
    }
    if n == 0 { 0.0 } else { sum / f64::from(n) }
}

fn summarize(rows: &[(&str, &str, Divergence)]) {
    if rows.is_empty() {
        println!("\nnothing compared");
        return;
    }
    let mean_edge = mean(rows.iter().map(|(_, _, d)| d.edge_rate()));
    let worst = rows
        .iter()
        .max_by(|a, b| a.2.edge_rate().total_cmp(&b.2.edge_rate()));
    let interior_hits: Vec<_> = rows
        .iter()
        .filter(|(_, _, d)| d.interior_differing > 0)
        .collect();
    let blank: Vec<_> = rows
        .iter()
        .filter(|(_, _, d)| d.one_painted_nothing)
        .collect();

    println!("\n# summary over {} documents", rows.len());
    println!("mean edge divergence:     {:.3}%", mean_edge * 100.0);
    if let Some((stem, _, d)) = worst {
        println!(
            "worst edge divergence:    {:.3}%  ({stem})",
            d.edge_rate() * 100.0
        );
    }
    println!(
        "documents with interior differences: {} of {}",
        interior_hits.len(),
        rows.len()
    );
    for (stem, class, d) in &interior_hits {
        println!(
            "  {stem:<26} {:>10} interior px  maxdiff {:>3}  [{class}]",
            d.interior_differing, d.max_channel_diff
        );
    }
    println!(
        "documents where exactly one backend painted: {}",
        blank.len()
    );
    for (stem, _, _) in &blank {
        println!("  {stem}");
    }

    // Per class, because the divergence's *cause* is class-shaped: the
    // antialiasing gap (§5.1) lands on whatever draws hard-edged rects.
    println!("\n# mean edge divergence per class");
    let mut classes: Vec<&str> = rows.iter().map(|(_, c, _)| *c).collect();
    classes.sort_unstable();
    classes.dedup();
    for class in classes {
        let of_class: Vec<_> = rows.iter().filter(|(_, c, _)| *c == class).collect();
        let mean = mean(of_class.iter().map(|(_, _, d)| d.edge_rate()));
        println!(
            "{class:<10} {:>8.3}%  ({} documents)",
            mean * 100.0,
            of_class.len()
        );
    }
}

/// Compare one page rendered by each backend.
fn compare(a: &Pixmap, b: &Pixmap) -> Option<Divergence> {
    if a.width() != b.width() || a.height() != b.height() {
        return None;
    }
    let edges = edge_mask(a, b);
    let mut out = Divergence {
        pixels: u64::from(a.width()) * u64::from(a.height()),
        edge_pixels: 0,
        edge_differing: 0,
        interior_differing: 0,
        max_channel_diff: 0,
        one_painted_nothing: false,
    };
    let (mut a_painted, mut b_painted) = (false, false);
    for y in 0..a.height() {
        for x in 0..a.width() {
            let (Some(pa), Some(pb)) = (a.pixel(x, y), b.pixel(x, y)) else {
                continue;
            };
            a_painted |= pa[3] != 0;
            b_painted |= pb[3] != 0;
            let diff = pa
                .iter()
                .zip(pb.iter())
                .map(|(l, r)| l.abs_diff(*r))
                .max()
                .unwrap_or(0);
            out.max_channel_diff = out.max_channel_diff.max(diff);
            let index = (y as usize) * (a.width() as usize) + (x as usize);
            if edges.get(index).copied().unwrap_or(false) {
                out.edge_pixels += 1;
                if diff > EDGE_TOLERANCE {
                    out.edge_differing += 1;
                }
            } else if diff > INTERIOR_TOLERANCE {
                out.interior_differing += 1;
            }
        }
    }
    out.one_painted_nothing = a_painted != b_painted;
    Some(out)
}

/// Which pixels sit on an edge in either image, dilated from the intersection.
///
/// `conformance/src/tierc.rs`'s `edge_mask`, including the reason the growth
/// starts from the **intersection** rather than the union: a region where the
/// two images differ carries a boundary in one of them by construction, so
/// dilating the union would roll that rim inward over the difference and let
/// the metric absorb exactly what it exists to detect.
fn edge_mask(a: &Pixmap, b: &Pixmap) -> Vec<bool> {
    let (w, h) = (a.width() as usize, a.height() as usize);
    let size = w.saturating_mul(h);
    let ma = boundaries(a);
    let mb = boundaries(b);
    let mut union = vec![false; size];
    let mut both = vec![false; size];
    for i in 0..size {
        let (ia, ib) = (
            ma.get(i).copied().unwrap_or(false),
            mb.get(i).copied().unwrap_or(false),
        );
        if let Some(s) = union.get_mut(i) {
            *s = ia || ib;
        }
        if let Some(s) = both.get_mut(i) {
            *s = ia && ib;
        }
    }
    let grown = dilate(&both, a.width(), a.height());
    for i in 0..size {
        if grown.get(i).copied().unwrap_or(false)
            && let Some(s) = union.get_mut(i)
        {
            *s = true;
        }
    }
    union
}

/// Where one image has a boundary: a pixel with a neighbour unlike itself.
fn boundaries(image: &Pixmap) -> Vec<bool> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut mask = vec![false; w.saturating_mul(h)];
    for y in 0..image.height() {
        for x in 0..image.width() {
            let Some(centre) = image.pixel(x, y) else {
                continue;
            };
            let mut is_edge = false;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (Ok(nx), Ok(ny)) = (
                        u32::try_from(i64::from(x) + dx),
                        u32::try_from(i64::from(y) + dy),
                    ) else {
                        continue;
                    };
                    let Some(n) = image.pixel(nx, ny) else {
                        continue;
                    };
                    if centre
                        .iter()
                        .zip(n.iter())
                        .any(|(c, v)| c.abs_diff(*v) > INTERIOR_TOLERANCE)
                    {
                        is_edge = true;
                    }
                }
            }
            if is_edge && let Some(s) = mask.get_mut((y as usize) * w + (x as usize)) {
                *s = true;
            }
        }
    }
    mask
}

/// Grow a mask by one pixel in all eight directions.
///
/// One round, which is `conformance/src/tierc.rs`'s `EDGE_DILATION` — the
/// design brief's own radius, deliberately not tuned past it. Reading from a
/// snapshot rather than in place is what keeps "one pixel" true.
fn dilate(mask: &[bool], width: u32, height: u32) -> Vec<bool> {
    let w = width as usize;
    let previous = mask.to_vec();
    let mut current = previous.clone();
    for y in 0..height {
        for x in 0..width {
            let index = (y as usize) * w + (x as usize);
            if previous.get(index).copied().unwrap_or(false) {
                continue;
            }
            let mut touched = false;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (Ok(nx), Ok(ny)) = (
                        u32::try_from(i64::from(x) + dx),
                        u32::try_from(i64::from(y) + dy),
                    ) else {
                        continue;
                    };
                    if nx >= width || ny >= height {
                        continue;
                    }
                    if previous
                        .get((ny as usize) * w + (nx as usize))
                        .copied()
                        .unwrap_or(false)
                    {
                        touched = true;
                    }
                }
            }
            if touched && let Some(s) = current.get_mut(index) {
                *s = true;
            }
        }
    }
    current
}
