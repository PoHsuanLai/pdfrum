//! Parallel-render throughput: every page of every file, one opened document
//! shared by N threads, pages per second. In-process, because the point is
//! the document being shared; a per-file JSON write keeps the run resumable.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::engines;
use crate::model::Ctx;

/// The engines that render, in table order.
pub const ENGINES: &[&str] = &["pdfrum", "hayro", "pdf_oxide", "pdfium-render", "mupdf"];

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
    /// `ok`, `error`, or `single-thread-only` (the engine's rules, or its
    /// document type is not `Sync`; measured on one thread and reported as
    /// such for every thread count).
    pub status: String,
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
            let single_only = engines::single_threaded(engine);
            for &threads in thread_counts {
                let effective = if single_only.is_some() { 1 } else { threads };
                let (status, detail, pages, secs) = if info.compiled {
                    match render_all(engine, file, ctx, effective) {
                        Ok((pages, elapsed)) => (
                            if single_only.is_some() {
                                "single-thread-only".to_owned()
                            } else {
                                "ok".to_owned()
                            },
                            single_only.map(str::to_owned),
                            pages,
                            elapsed.as_secs_f64(),
                        ),
                        Err(err) => ("error".to_owned(), Some(format!("{err:#}")), 0, 0.0),
                    }
                } else {
                    (
                        "not-run".to_owned(),
                        Some("not compiled in".to_owned()),
                        0,
                        0.0,
                    )
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
/// over the files every thread count of that engine rendered.
pub fn render(rows: &[ThroughputRow], thread_counts: &[usize]) -> String {
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
            .filter(|r| r.status != "error" && r.status != "not-run")
            .map(|r| r.file.as_str())
            .filter(|file| {
                thread_counts.iter().all(|t| {
                    mine.iter().any(|r| {
                        r.file == *file
                            && r.threads == *t
                            && r.status != "error"
                            && r.status != "not-run"
                    })
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
                .find(|r| r.threads == *t && r.status == "single-thread-only")
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
    let errors: Vec<&ThroughputRow> = rows.iter().filter(|r| r.status == "error").collect();
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
