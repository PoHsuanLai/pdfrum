//! The vocabulary every module shares: the three operations, what an engine
//! hands back in-process, and the one JSON line a child reports to its
//! parent.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// One thing an engine is asked to do to a file.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    /// Parse the file and reach its page tree.
    Open,
    /// Page 1 to RGBA at the run's DPI.
    Render,
    /// Page 1's text.
    Text,
}

impl Op {
    /// Every operation, in table order.
    pub const ALL: [Op; 3] = [Op::Open, Op::Render, Op::Text];

    /// The name a table and a command line use.
    pub fn name(self) -> &'static str {
        match self {
            Op::Open => "open",
            Op::Render => "render",
            Op::Text => "text",
        }
    }
}

/// A rendered page as an engine produced it. `premultiplied` says whether
/// the alpha has already been multiplied into the colour channels, which the
/// comparison undoes by compositing over white either way.
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA order.
    pub rgba: Vec<u8>,
    pub premultiplied: bool,
}

/// What an engine produced for one operation.
#[derive(Debug)]
pub enum Output {
    /// The file opened; how many pages it has and, where the engine walks its
    /// objects eagerly, how many objects it holds.
    Opened {
        pages: usize,
        objects: Option<usize>,
    },
    /// Page 1's pixels.
    Rendered(Raster),
    /// Page 1's text.
    Text(String),
}

/// An operation's timings plus its last result.
#[derive(Debug)]
pub struct Timed {
    /// Wall time per run, milliseconds. The first is the cold run.
    pub times_ms: Vec<f64>,
    pub output: Output,
}

/// What every engine needs to know about the run it is in.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    /// Render resolution in dots per inch.
    pub dpi: f64,
    /// The hermetic font directory the oracle renders with, for engines that
    /// can be pointed at one.
    pub font_dir: Option<&'a Path>,
    /// Where `libpdfium.so` is, for `pdfium-render`.
    #[cfg_attr(
        not(feature = "pdfium-render"),
        expect(dead_code, reason = "read by the pdfium-render engine only")
    )]
    pub pdfium_lib: Option<&'a Path>,
    /// The user password the file needs, if any.
    pub password: Option<&'a str>,
    /// How many warm runs to attempt after the cold one.
    pub warm_runs: usize,
    /// Wall-clock budget for the warm runs; a slow file keeps fewer.
    pub budget: Duration,
}

impl Ctx<'_> {
    /// Pixels per PDF point at this run's DPI.
    pub fn scale(&self) -> f64 {
        self.dpi / 72.0
    }

    /// The pixel size the oracle picks for a page of `w` x `h` points:
    /// `pdfium_test` truncates `FPDF_GetPageWidthF(page) * scale` to `int`.
    #[cfg_attr(
        not(feature = "pdfium-render"),
        expect(dead_code, reason = "used by the pdfium-render engine only")
    )]
    pub fn oracle_size(&self, w: f64, h: f64) -> (u32, u32) {
        let width = (w * self.scale()).max(0.0) as u32;
        let height = (h * self.scale()).max(0.0) as u32;
        (width, height)
    }

    /// One cold run of `f`, then warm runs until either `warm_runs` are in or
    /// the next one would not fit the budget — judged by the last run's
    /// duration, so a slow file is measured once rather than killed at the
    /// parent's deadline for repeating itself. The output kept is the last run's.
    pub fn measure<T>(
        &self,
        mut f: impl FnMut() -> anyhow::Result<T>,
    ) -> anyhow::Result<(Vec<f64>, T)> {
        let started = Instant::now();
        let mut times = Vec::with_capacity(self.warm_runs + 1);
        let clock = Instant::now();
        let mut last = f()?;
        let mut last_duration = clock.elapsed();
        times.push(last_duration.as_secs_f64() * 1000.0);
        for _ in 0..self.warm_runs {
            if started.elapsed() + last_duration > self.budget {
                break;
            }
            let clock = Instant::now();
            last = f()?;
            last_duration = clock.elapsed();
            times.push(last_duration.as_secs_f64() * 1000.0);
        }
        Ok((times, last))
    }
}

/// The one line a child prints on stdout when it is done.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildReport {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Wall time of the first (cold) run, milliseconds.
    pub cold_ms: f64,
    /// Wall time of each warm run, milliseconds.
    pub warm_ms: Vec<f64>,
    /// Peak resident set of the child, kilobytes, from `/proc/self/status`.
    pub vm_hwm_kb: u64,
    /// The same reading taken before the engine ran, so the process's own
    /// floor can be subtracted.
    pub baseline_hwm_kb: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_len: Option<usize>,
}

/// `VmHWM` from `/proc/self/status`, in kilobytes; zero where unreadable.
pub fn vm_hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| {
                    rest.split_whitespace()
                        .next()
                        .and_then(|kb| kb.parse().ok())
                })
        })
        .unwrap_or(0)
}

/// The median of a sample, or `None` of an empty one.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    })
}

/// The `p`-th percentile (0..=100) by nearest rank, or `None` of an empty sample.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.saturating_sub(1)).copied()
}
