//! Parallel-render throughput: every page of every file on N threads, pages
//! per second. Each engine is driven in its own multi-threading model — see
//! [`crate::engines::Sharing`] — so the table's footnote, not the numbers
//! alone, says what was parallel. In-process, because the point is the
//! sharing; a per-file JSON write keeps the run resumable.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::engines::{self, Sharing};
use crate::model::Ctx;

/// The engines that render, in table order.
pub const ENGINES: &[&str] = &["pdfrum", "hayro", "pdf_oxide", "pdfium-render", "mupdf"];

/// What became of one measurement — and, when it succeeded, which
/// parallelism model produced it. The JSON spellings are the ones run 1
/// wrote, so the data files under `docs/benchmarks/data` still read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Ran on the requested threads, sharing one opened document.
    Ok,
    /// Ran on the requested threads: serial display-list build, parallel
    /// rasterization ([`Sharing::DisplayLists`]).
    DisplayLists,
    /// The engine's rules put it on one thread; this figure is the
    /// one-thread figure, repeated for every requested count.
    SingleThreadOnly,
    /// The engine failed on this file; `detail` says how.
    Error,
    /// Not compiled into this build.
    NotRun,
    /// A spelling this binary does not know, from an older data file.
    #[serde(other)]
    Unknown,
}

impl Status {
    /// Whether the row carries a usable measurement.
    fn measured(self) -> bool {
        matches!(self, Self::Ok | Self::DisplayLists | Self::SingleThreadOnly)
    }

    /// The status a successful run under `sharing` gets.
    fn of(sharing: Sharing) -> Self {
        match sharing {
            Sharing::Document => Self::Ok,
            Sharing::DisplayLists => Self::DisplayLists,
            Sharing::SingleThread { .. } => Self::SingleThreadOnly,
        }
    }
}

/// One (file, engine, threads) measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputRow {
    pub file: String,
    pub engine: String,
    pub threads: usize,
    pub pages: usize,
    /// Wall seconds for the measured pass (after one untimed warm-up pass).
    pub secs: f64,
    pub pages_per_sec: f64,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Renders every page of `path` on `threads` threads, once to warm and once
/// timed. Returns the page count and the timed pass's duration.
fn render_all(
    engine: &str,
    path: &Path,
    ctx: &Ctx<'_>,
    threads: usize,
) -> Result<(usize, Duration)> {
    engines::render_all(engine, path, ctx, threads)?;
    let clock = Instant::now();
    let pages = engines::render_all(engine, path, ctx, threads)?;
    Ok((pages, clock.elapsed()))
}

/// Runs the measurement, writing `out` after every file.
pub fn run(
    corpus_root: &Path,
    files: &[PathBuf],
    engine_names: &[String],
    thread_counts: &[usize],
    ctx: &Ctx<'_>,
    out: &Path,
    resume: bool,
) -> Result<Vec<ThroughputRow>> {
    let mut rows: Vec<ThroughputRow> = if resume {
        std::fs::read_to_string(out)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    for file in files {
        let relative = file
            .strip_prefix(corpus_root)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned();
        for engine in engine_names {
            if rows
                .iter()
                .any(|r| r.file == relative && &r.engine == engine)
            {
                continue;
            }
            let Some(info) = engines::info(engine) else {
                anyhow::bail!("unknown engine {engine}");
            };
            eprintln!("{relative} {engine}");
            let sharing = engines::sharing(engine);
            let why = match sharing {
                Sharing::SingleThread { why } => Some(why.to_owned()),
                Sharing::Document | Sharing::DisplayLists => None,
            };
            for &threads in thread_counts {
                let (status, detail, pages, secs) = if info.compiled {
                    match render_all(engine, file, ctx, sharing.threads(threads)) {
                        Ok((pages, elapsed)) => (
                            Status::of(sharing),
                            why.clone(),
                            pages,
                            elapsed.as_secs_f64(),
                        ),
                        Err(err) => (Status::Error, Some(format!("{err:#}")), 0, 0.0),
                    }
                } else {
                    (Status::NotRun, Some("not compiled in".to_owned()), 0, 0.0)
                };
                rows.push(ThroughputRow {
                    file: relative.clone(),
                    engine: engine.clone(),
                    threads,
                    pages,
                    secs,
                    pages_per_sec: if secs > 0.0 { pages as f64 / secs } else { 0.0 },
                    status,
                    detail,
                });
            }
            std::fs::write(out, serde_json::to_string_pretty(&rows)?)?;
        }
    }
    Ok(rows)
}

/// The throughput table: pages per second per engine at each thread count,
/// over the files every thread count of that engine rendered. `min_pages`
/// keeps only the files with at least that many pages — the second table of
/// docs/benchmarks/README.md is this one with `min_pages = 4`, since the
/// corpus is half one-page files and thread counts are clamped to the page
/// count.
///
/// The cells are plain numbers for every engine that ran on the threads
/// asked for, whichever model it used; the footnote under the table says
/// which model each engine used, because a `mupdf` cell and a `pdfrum` cell
/// at 8 threads are not measuring the same amount of parallel work.
pub fn render(rows: &[ThroughputRow], thread_counts: &[usize], min_pages: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "| engine | files | pages | {} |",
        thread_counts
            .iter()
            .map(|t| format!("{t} thread{} pages/s", if *t == 1 { "" } else { "s" }))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let _ = writeln!(out, "|{}", "---|".repeat(3 + thread_counts.len()));
    for engine in ENGINES {
        let mine: Vec<&ThroughputRow> = rows.iter().filter(|r| r.engine == *engine).collect();
        if mine.is_empty() {
            continue;
        }
        // Files where every thread count succeeded, so the columns describe
        // the same work.
        let files: std::collections::BTreeSet<&str> = mine
            .iter()
            .filter(|r| r.status.measured() && r.pages >= min_pages)
            .map(|r| r.file.as_str())
            .filter(|file| {
                thread_counts.iter().all(|t| {
                    mine.iter()
                        .any(|r| r.file == *file && r.threads == *t && r.status.measured())
                })
            })
            .collect();
        let mut cells = Vec::new();
        let mut pages_total = 0;
        for (i, t) in thread_counts.iter().enumerate() {
            let (pages, secs) = mine
                .iter()
                .filter(|r| r.threads == *t && files.contains(r.file.as_str()))
                .fold((0usize, 0.0f64), |(p, s), r| (p + r.pages, s + r.secs));
            if i == 0 {
                pages_total = pages;
            }
            let note = mine
                .iter()
                .find(|r| r.threads == *t && r.status == Status::SingleThreadOnly)
                .map_or("", |_| " (1 thread)");
            cells.push(if secs > 0.0 {
                format!("{:.1}{note}", pages as f64 / secs)
            } else {
                "-".to_owned()
            });
        }
        let _ = writeln!(
            out,
            "| {engine} | {} | {} | {} |",
            files.len(),
            pages_total,
            cells.join(" | ")
        );
    }
    // What was parallel, per engine that has rows: a mupdf cell and a
    // pdfrum cell at 8 threads do not describe the same amount of parallel
    // work, and the table cannot show that by itself.
    let mut notes: Vec<String> = Vec::new();
    for engine in ENGINES {
        if !rows
            .iter()
            .any(|r| r.engine == *engine && r.status.measured())
        {
            continue;
        }
        notes.push(match engines::sharing(engine) {
            Sharing::Document => format!(
                "- `{engine}`: one opened document shared by N threads — parse and rasterization both parallel."
            ),
            Sharing::DisplayLists => format!(
                "- `{engine}`: N threads, MuPDF's own model — the calling thread loads every page and records it into a display list (serial, and inside the timed pass), then N threads rasterize those lists, each on its own cloned context. Only the rasterization is parallel."
            ),
            Sharing::SingleThread { why } => format!(
                "- `{engine}`: one thread whatever the column says ({why}); the figure is the one-thread figure repeated, marked."
            ),
        });
    }
    if !notes.is_empty() {
        let _ = writeln!(out, "\nWhat ran in parallel:\n");
        for note in notes {
            let _ = writeln!(out, "{note}");
        }
    }

    let errors: Vec<&ThroughputRow> = rows.iter().filter(|r| r.status == Status::Error).collect();
    if !errors.is_empty() {
        let _ = writeln!(out, "\nFiles excluded for an error ({}):\n", errors.len());
        for row in errors {
            let _ = writeln!(
                out,
                "- {} — {} at {} threads: {}",
                row.file,
                row.engine,
                row.threads,
                row.detail
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
            );
        }
    }
    out
}
