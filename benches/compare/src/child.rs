//! One (engine, file, operation) per process. The child runs the engine,
//! writes its artefact (a PNG or a text file) and prints one JSON line; the
//! parent enforces the timeout, reads the peak RSS the child measured on
//! itself, and turns a dead child into a "panic" or "crash" row rather than
//! a missing one.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::engines;
use crate::model::{ChildReport, Ctx, Op, Output, RenderProfile, vm_hwm_kb};
use crate::pixels;

/// How an (engine, file, op) run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// The engine returned a result.
    Ok,
    /// The engine returned an error.
    Error,
    /// The child process panicked.
    Panic,
    /// The child died of a signal or an abort without a panic message.
    Crash,
    /// The child was still running at the deadline and was killed.
    Timeout,
    /// The engine's API does not offer the operation.
    Unsupported,
    /// The engine is not compiled into this binary, or could not be bound.
    NotRun,
}

impl Status {
    /// The table label.
    pub fn name(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
            Status::Panic => "panic",
            Status::Crash => "crash",
            Status::Timeout => "timeout",
            Status::Unsupported => "unsupported",
            Status::NotRun => "not-run",
        }
    }
}

/// What the parent learned from one child.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: Status,
    pub report: Option<ChildReport>,
    pub detail: Option<String>,
}

/// Runs the engine in this process and prints the report. The child's half.
pub fn run_child(engine: &str, op: Op, file: &Path, out: &Path, ctx: &Ctx<'_>) -> Result<()> {
    let baseline_hwm_kb = vm_hwm_kb();
    let outcome = engines::run(engine, op, file, ctx);
    let mut report = ChildReport {
        ok: false,
        error: None,
        cold_ms: 0.0,
        warm_ms: Vec::new(),
        vm_hwm_kb: 0,
        baseline_hwm_kb,
        pages: None,
        objects: None,
        size: None,
        text_len: None,
    };
    match outcome {
        Ok(timed) => {
            report.ok = true;
            report.cold_ms = timed.times_ms.first().copied().unwrap_or(0.0);
            report.warm_ms = timed.times_ms.iter().skip(1).copied().collect();
            match timed.output {
                Output::Opened { pages, objects } => {
                    report.pages = Some(pages);
                    report.objects = objects;
                }
                Output::Rendered(raster) => {
                    report.size = Some((raster.width, raster.height));
                    let image = pixels::over_white(&raster)?;
                    std::fs::write(out, pixels::encode_png(&image)?)?;
                }
                Output::Text(text) => {
                    report.text_len = Some(text.chars().count());
                    std::fs::write(out, text)?;
                }
            }
        }
        Err(err) => {
            report.error = Some(format!("{err:#}"));
        }
    }
    report.vm_hwm_kb = vm_hwm_kb();
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

/// The command-line shape the child is spawned with.
pub struct ChildArgs<'a> {
    pub exe: &'a Path,
    pub engine: &'a str,
    pub op: Op,
    pub file: &'a Path,
    pub out: &'a Path,
    pub dpi: f64,
    pub font_dir: Option<&'a Path>,
    pub pdfium_lib: Option<&'a Path>,
    pub password: Option<&'a str>,
    pub warm_runs: usize,
    /// Which render settings the child is asked for.
    pub profile: RenderProfile,
    pub budget: Duration,
    pub timeout: Duration,
}

/// Spawns a child and waits for it, killing it at the deadline. The parent's half.
pub fn spawn(args: &ChildArgs<'_>) -> Result<Outcome> {
    let mut command = Command::new(args.exe);
    command
        .arg("child")
        .arg("--engine")
        .arg(args.engine)
        .arg("--op")
        .arg(args.op.name())
        .arg("--file")
        .arg(args.file)
        .arg("--out")
        .arg(args.out)
        .arg("--dpi")
        .arg(args.dpi.to_string())
        .arg("--warm")
        .arg(args.warm_runs.to_string())
        .arg("--budget-ms")
        .arg(args.budget.as_millis().to_string())
        .arg("--profile")
        .arg(args.profile.name())
        // Single-threaded, as asks: `lopdf` parses with rayon by
        // default, and this keeps every engine on one core.
        .env("RAYON_NUM_THREADS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = args.font_dir {
        command.arg("--font-dir").arg(dir);
    }
    if let Some(lib) = args.pdfium_lib {
        command.arg("--pdfium-lib").arg(lib);
    }
    if let Some(password) = args.password {
        command.arg("--password").arg(password);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("spawning {}", args.exe.display()))?;

    // Drain stderr on a thread so a chatty engine cannot fill the pipe and
    // deadlock against the wait below.
    let stderr = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut buf);
        }
        buf
    });
    let stdout = child.stdout.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() > args.timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let stderr = String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned();
    let stdout = String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned();
    let tail = |text: &str| -> Option<String> {
        let tail: Vec<&str> = text
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let joined = tail.join("\n");
        (!joined.trim().is_empty()).then_some(joined)
    };

    let Some(status) = status else {
        return Ok(Outcome {
            status: Status::Timeout,
            report: None,
            detail: Some(format!("killed after {:.1} s", args.timeout.as_secs_f64())),
        });
    };

    let report = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with('{'))
        .and_then(|line| serde_json::from_str::<ChildReport>(line).ok());

    match report {
        Some(report) if status.success() => {
            let status = if report.ok { Status::Ok } else { Status::Error };
            let detail = report.error.clone();
            Ok(Outcome {
                status,
                report: Some(report),
                detail,
            })
        }
        _ => {
            let panicked = stderr.contains("panicked at");
            Ok(Outcome {
                status: if panicked {
                    Status::Panic
                } else {
                    Status::Crash
                },
                report: None,
                detail: tail(&stderr).or_else(|| Some(format!("exit {status}"))),
            })
        }
    }
}

/// Where a child's artefact goes, so the parent can compare it and a reader
/// can look at it afterwards.
pub fn artefact_path(scratch: &Path, engine: &str, op: Op, file_key: &str) -> PathBuf {
    let ext = match op {
        Op::Render => "png",
        Op::Text => "txt",
        Op::Open => "json",
    };
    scratch
        .join("out")
        .join(engine)
        .join(format!("{file_key}.{ext}"))
}
