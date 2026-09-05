//! The parent: the corpus, the oracle, every engine on every file, and one
//! JSON file with everything a table is generated from.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::child::{self, ChildArgs, Status};
use crate::engines;
use crate::model::{Op, median};
use crate::oracle::Oracle;
use crate::pixels::{self, RenderDiff};
use crate::text::{self, TextDiff};

/// One (file, engine, op) row of the JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub file: String,
    pub engine: String,
    pub op: Op,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_ms: Option<f64>,
    /// Median of the warm runs; the cold run when no warm run fit the budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_ms: Option<f64>,
    #[serde(default)]
    pub warm_samples: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm_hwm_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_hwm_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<RenderDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextDiff>,
    /// The oracle produced nothing to compare against for this op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle_error: Option<String>,
}

/// An engine as the run saw it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRecord {
    pub name: String,
    pub version: String,
    pub ops: Vec<Op>,
    pub c_in_build: bool,
    /// Whether the engine ran at all; `reason` says why not.
    pub ran: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub note: String,
}

/// A coverage-matrix entry: one feature verified by one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSpec {
    pub feature: String,
    pub file: String,
    #[serde(default)]
    pub note: String,
    /// The user password the file needs, passed to the oracle and to every
    /// engine that has a password API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// The whole run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunJson {
    pub schema: u32,
    pub generated_at: String,
    pub commit: String,
    pub label: String,
    pub machine: Machine,
    pub corpus: Corpus,
    pub oracle: OracleRecord,
    pub dpi: f64,
    pub timeout_secs: f64,
    pub warm_runs: usize,
    pub engines: Vec<EngineRecord>,
    #[serde(default)]
    pub features: Vec<FeatureSpec>,
    pub rows: Vec<Row>,
}

/// Where the numbers were taken.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub hostname: String,
    pub cpus: usize,
    pub rustc: String,
    pub uptime_before: String,
    pub uptime_after: String,
    pub loadavg_before: String,
    pub loadavg_after: String,
}

/// Which files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corpus {
    pub root: String,
    pub rule: String,
    pub files: usize,
}

/// Which oracle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleRecord {
    pub binary: String,
    pub font_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_commit: Option<String>,
}

/// What `run` was asked to do.
pub struct RunConfig {
    pub label: String,
    pub corpus_root: PathBuf,
    pub files: Vec<PathBuf>,
    pub rule: String,
    pub engines: Vec<String>,
    pub oracle: Oracle,
    pub checkout: Option<PathBuf>,
    pub scratch: PathBuf,
    pub out: PathBuf,
    pub pdfium_lib: Option<PathBuf>,
    pub timeout: Duration,
    pub warm_runs: usize,
    pub features: Vec<FeatureSpec>,
    pub repo_root: PathBuf,
}

impl RunConfig {
    /// The password the spec gives for `relative`, if any.
    fn password_for(&self, relative: &str) -> Option<&str> {
        self.features
            .iter()
            .find(|f| f.file == relative)
            .and_then(|f| f.password.as_deref())
    }
}

fn command_line(cmd: &str, args: &[&str], cwd: Option<&Path>) -> String {
    let mut command = Command::new(cmd);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// A stable, filesystem-safe key for a corpus-relative path.
pub fn file_key(relative: &str) -> String {
    relative.replace(['/', '\\'], "__")
}

/// Lists every `.pdf` under `root`, sorted, keeping every `every`-th from `offset`.
pub fn list_corpus(root: &Path, every: usize, offset: usize) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    let every = every.max(1);
    Ok(files.into_iter().skip(offset).step_by(every).collect())
}

/// Runs everything and writes the JSON.
pub fn run(config: &RunConfig) -> Result<RunJson> {
    let exe = std::env::current_exe().context("locating this binary")?;
    std::fs::create_dir_all(&config.scratch)?;

    let mut engine_records = Vec::new();
    for name in &config.engines {
        let info = engines::info(name).ok_or_else(|| anyhow!("unknown engine {name}"))?;
        let (ran, reason) = if !info.compiled {
            (
                false,
                Some(format!("not compiled in: build with --features {name}")),
            )
        } else if name == "pdfium-render" && config.pdfium_lib.is_none() {
            (
                false,
                Some("not run, needs libpdfium.so (--pdfium-lib)".to_owned()),
            )
        } else {
            (true, None)
        };
        engine_records.push(EngineRecord {
            name: info.name.to_owned(),
            version: info.version.to_owned(),
            ops: info.ops.to_vec(),
            c_in_build: info.c_in_build,
            ran,
            reason,
            note: info.note.to_owned(),
        });
    }
    if config.files.is_empty() {
        bail!("no files to run");
    }

    let uptime_before = command_line("uptime", &[], None);
    let loadavg_before = loadavg();
    let mut rows = Vec::new();
    let total = config.files.len();
    for (index, file) in config.files.iter().enumerate() {
        let relative = file
            .strip_prefix(&config.corpus_root)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned();
        let key = file_key(&relative);
        eprintln!("[{}/{total}] {relative}", index + 1);
        let password = config.password_for(&relative);
        let oracle = config.oracle.outputs(file, password)?;
        let oracle_image = match &oracle.png {
            Ok(path) => std::fs::read(path)
                .map_err(|err| err.to_string())
                .and_then(|bytes| pixels::decode_png(&bytes).map_err(|err| err.to_string())),
            Err(err) => Err(err.clone()),
        };
        let oracle_text = match &oracle.txt {
            Ok(path) => std::fs::read(path)
                .map(|bytes| text::decode_utf32le(&bytes))
                .map_err(|err| err.to_string()),
            Err(err) => Err(err.clone()),
        };

        for engine in &engine_records {
            for op in Op::ALL {
                let mut row = Row {
                    file: relative.clone(),
                    engine: engine.name.clone(),
                    op,
                    status: Status::NotRun,
                    detail: None,
                    cold_ms: None,
                    warm_ms: None,
                    warm_samples: 0,
                    vm_hwm_kb: None,
                    baseline_hwm_kb: None,
                    pages: None,
                    objects: None,
                    render: None,
                    render_error: None,
                    text: None,
                    oracle_error: None,
                };
                if !engine.ops.contains(&op) {
                    row.status = Status::Unsupported;
                    rows.push(row);
                    continue;
                }
                if !engine.ran {
                    row.detail.clone_from(&engine.reason);
                    rows.push(row);
                    continue;
                }
                let out = child::artefact_path(&config.scratch, &engine.name, op, &key);
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let _ = std::fs::remove_file(&out);
                let outcome = child::spawn(&ChildArgs {
                    exe: &exe,
                    engine: &engine.name,
                    op,
                    file,
                    out: &out,
                    dpi: config.oracle.dpi,
                    font_dir: Some(&config.oracle.font_dir),
                    pdfium_lib: config.pdfium_lib.as_deref(),
                    password,
                    warm_runs: config.warm_runs,
                    budget: config.timeout.mul_f64(0.6),
                    timeout: config.timeout,
                })?;
                row.status = outcome.status;
                row.detail = outcome.detail;
                if let Some(report) = outcome.report {
                    row.cold_ms = Some(report.cold_ms);
                    row.warm_samples = report.warm_ms.len();
                    row.warm_ms = median(&report.warm_ms).or(Some(report.cold_ms));
                    row.vm_hwm_kb = Some(report.vm_hwm_kb);
                    row.baseline_hwm_kb = Some(report.baseline_hwm_kb);
                    row.pages = report.pages;
                    row.objects = report.objects;
                }
                if row.status == Status::Ok {
                    match op {
                        Op::Render => match &oracle_image {
                            Ok(golden) => {
                                let candidate = std::fs::read(&out)
                                    .map_err(|err| err.to_string())
                                    .and_then(|bytes| {
                                        pixels::decode_png(&bytes).map_err(|err| err.to_string())
                                    });
                                match candidate.and_then(|image| {
                                    pixels::compare(golden, &image).map_err(|err| err.to_string())
                                }) {
                                    Ok(diff) => row.render = Some(diff),
                                    Err(err) => row.render_error = Some(err),
                                }
                            }
                            Err(err) => row.oracle_error = Some(err.clone()),
                        },
                        Op::Text => match &oracle_text {
                            Ok(golden) => {
                                let produced = std::fs::read_to_string(&out).unwrap_or_default();
                                row.text = Some(text::compare(golden, &produced));
                            }
                            Err(err) => row.oracle_error = Some(err.clone()),
                        },
                        Op::Open => {}
                    }
                }
                rows.push(row);
            }
        }
    }
    let uptime_after = command_line("uptime", &[], None);
    let loadavg_after = loadavg();

    let json = RunJson {
        schema: 1,
        generated_at: command_line("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"], None),
        commit: command_line(
            "git",
            &["rev-parse", "--short=12", "HEAD"],
            Some(&config.repo_root),
        ),
        label: config.label.clone(),
        machine: Machine {
            hostname: command_line("hostname", &[], None),
            cpus: std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
            rustc: command_line("rustc", &["--version"], None),
            uptime_before,
            uptime_after,
            loadavg_before,
            loadavg_after,
        },
        corpus: Corpus {
            root: config.corpus_root.display().to_string(),
            rule: config.rule.clone(),
            files: config.files.len(),
        },
        oracle: OracleRecord {
            binary: config.oracle.binary.display().to_string(),
            font_dir: config.oracle.font_dir.display().to_string(),
            checkout_commit: config.checkout.as_deref().and_then(Oracle::checkout_commit),
        },
        dpi: config.oracle.dpi,
        timeout_secs: config.timeout.as_secs_f64(),
        warm_runs: config.warm_runs,
        engines: engine_records,
        features: config.features.clone(),
        rows,
    };
    if let Some(parent) = config.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config.out, serde_json::to_string_pretty(&json)?)?;
    Ok(json)
}
