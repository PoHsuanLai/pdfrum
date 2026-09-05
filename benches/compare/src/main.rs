//! `compare` — pdfrum beside its peers on one corpus, against the oracle.
//!
//! ```text
//! compare run       --corpus DIR [--every N] --engines a,b --out JSON ...
//! compare report    JSON            # regenerate the tables from a run
//! compare adoption  --out JSON ...  # crates, C, build time, size, unsafe, licence
//! compare child     ...             # what `run` spawns; not for hands
//! ```
//!
//! docs/benchmarks/README.md is the method; PLAN.md §M21 is the contract.

#![forbid(unsafe_code)]
#![allow(
    clippy::too_many_lines,
    reason = "table writers are long and flat by nature"
)]

mod adoption;
mod child;
mod engines;
mod model;
mod oracle;
mod pixels;
mod report;
mod run;
mod text;
mod throughput;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::model::{Ctx, Op};

#[derive(Parser)]
#[command(
    name = "compare",
    about = "pdfrum beside its peers: correctness, speed, memory, adoption"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run every engine on every file and write the JSON the tables come from.
    Run(RunArgs),
    /// Print the tables for a run's JSON.
    Report {
        /// A file written by `run`.
        json: PathBuf,
        /// Only the losses, as a list.
        #[arg(long)]
        losses: bool,
    },
    /// Measure the cost of adopting each engine.
    Adoption(AdoptionArgs),
    /// Pages per second rendering every page of every file on N threads.
    Throughput(ThroughputArgs),
    /// One engine, one file, one operation, in this process (spawned by `run`).
    #[command(hide = true)]
    Child(ChildCli),
}

#[derive(clap::Args)]
struct RunArgs {
    /// A name for the run, recorded in the JSON.
    #[arg(long, default_value = "run")]
    label: String,
    /// Directory of PDFs, walked recursively.
    #[arg(long)]
    corpus: PathBuf,
    /// Keep every N-th file of the sorted listing.
    #[arg(long, default_value_t = 1)]
    every: usize,
    /// Start the sampling at this index of the sorted listing.
    #[arg(long, default_value_t = 0)]
    offset: usize,
    /// A JSON list of `{feature, file, note}` naming corpus-relative files;
    /// replaces the walk and adds the coverage matrix to the report.
    #[arg(long)]
    spec: Option<PathBuf>,
    /// Comma-separated engine names; default is every engine this build knows.
    #[arg(long, value_delimiter = ',')]
    engines: Vec<String>,
    /// Where to write the JSON.
    #[arg(long)]
    out: PathBuf,
    /// Working directory for oracle outputs and engine artefacts.
    #[arg(long)]
    scratch: PathBuf,
    /// The PDFium checkout (for its commit and the default font dir).
    #[arg(long, env = "PDFRUM_ORACLE_CHECKOUT")]
    checkout: Option<PathBuf>,
    /// `pdfium_test`; defaults to `<checkout>/out/Release/pdfium_test`.
    #[arg(long, env = "PDFRUM_ORACLE_BIN")]
    oracle_bin: Option<PathBuf>,
    /// Hermetic fonts; defaults to `<checkout>/third_party/test_fonts`.
    #[arg(long)]
    font_dir: Option<PathBuf>,
    /// `libpdfium.so` for pdfium-render; without it that engine is "not run".
    #[arg(long, env = "PDFIUM_DYNAMIC_LIB_PATH")]
    pdfium_lib: Option<PathBuf>,
    /// Per (engine, file, op) wall-clock limit.
    #[arg(long, default_value_t = 10.0)]
    timeout_secs: f64,
    /// Warm runs after the cold one.
    #[arg(long, default_value_t = 3)]
    warm: usize,
    /// Render resolution.
    #[arg(long, default_value_t = 150.0)]
    dpi: f64,
    /// Keep the rows `--out` already holds and run only the files it lacks.
    #[arg(long)]
    resume: bool,
}

#[derive(clap::Args)]
struct AdoptionArgs {
    #[arg(long, value_delimiter = ',')]
    engines: Vec<String>,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    scratch: PathBuf,
    #[arg(long, env = "PDFIUM_DYNAMIC_LIB_PATH")]
    pdfium_lib: Option<PathBuf>,
    /// Keep the engines `--out` already holds and measure only the rest.
    #[arg(long)]
    resume: bool,
}

#[derive(clap::Args)]
struct ThroughputArgs {
    /// Directory of PDFs, walked recursively.
    #[arg(long)]
    corpus: PathBuf,
    #[arg(long, value_delimiter = ',')]
    engines: Vec<String>,
    /// Thread counts to measure.
    #[arg(long, value_delimiter = ',', default_values_t = [1, 4, 8])]
    threads: Vec<usize>,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, env = "PDFRUM_ORACLE_CHECKOUT")]
    checkout: Option<PathBuf>,
    #[arg(long)]
    font_dir: Option<PathBuf>,
    #[arg(long, env = "PDFIUM_DYNAMIC_LIB_PATH")]
    pdfium_lib: Option<PathBuf>,
    #[arg(long, default_value_t = 150.0)]
    dpi: f64,
    /// Table only the files with at least this many pages (the README's
    /// second throughput table is `--min-pages 4`).
    #[arg(long, default_value_t = 0)]
    min_pages: usize,
    /// Keep the rows `--out` already holds and run only the files it lacks.
    #[arg(long)]
    resume: bool,
}

#[derive(clap::Args)]
struct ChildCli {
    #[arg(long)]
    engine: String,
    #[arg(long)]
    op: Op,
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = 150.0)]
    dpi: f64,
    #[arg(long)]
    font_dir: Option<PathBuf>,
    #[arg(long)]
    pdfium_lib: Option<PathBuf>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long, default_value_t = 3)]
    warm: usize,
    #[arg(long, default_value_t = 6000)]
    budget_ms: u64,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn default_engines() -> Vec<String> {
    engines::all()
        .into_iter()
        .map(|e| e.name.to_owned())
        .collect()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Child(args) => {
            let ctx = Ctx {
                dpi: args.dpi,
                font_dir: args.font_dir.as_deref(),
                pdfium_lib: args.pdfium_lib.as_deref(),
                password: args.password.as_deref(),
                warm_runs: args.warm,
                budget: Duration::from_millis(args.budget_ms),
            };
            child::run_child(&args.engine, args.op, &args.file, &args.out, &ctx)
        }
        Cmd::Report { json, losses } => {
            let text = std::fs::read_to_string(&json)
                .with_context(|| format!("reading {}", json.display()))?;
            let run: run::RunJson = serde_json::from_str(&text)?;
            if losses {
                println!("{}", serde_json::to_string_pretty(&report::losses(&run))?);
            } else {
                print!("{}", report::render(&run));
            }
            Ok(())
        }
        Cmd::Adoption(args) => {
            let engines = if args.engines.is_empty() {
                adoption::ENGINES.iter().map(|e| (*e).to_owned()).collect()
            } else {
                args.engines
            };
            let rows = adoption::run(
                &engines,
                &args.scratch,
                &repo_root(),
                args.pdfium_lib.as_deref(),
                &args.out,
                args.resume,
            )?;
            print!("{}", adoption::render(&rows));
            Ok(())
        }
        Cmd::Throughput(args) => {
            let checkout = args
                .checkout
                .clone()
                .or_else(|| Some(repo_root().join("../pdfium-c++")).filter(|p| p.is_dir()));
            let font_dir = args
                .font_dir
                .or_else(|| checkout.map(|c| c.join("third_party/test_fonts")))
                .filter(|d| d.is_dir());
            let corpus_root = args
                .corpus
                .canonicalize()
                .with_context(|| format!("corpus {}", args.corpus.display()))?;
            let files = run::list_corpus(&corpus_root, 1, 0)?;
            let engines = if args.engines.is_empty() {
                throughput::ENGINES
                    .iter()
                    .map(|e| (*e).to_owned())
                    .collect()
            } else {
                args.engines
            };
            let ctx = Ctx {
                dpi: args.dpi,
                font_dir: font_dir.as_deref(),
                pdfium_lib: args.pdfium_lib.as_deref(),
                password: None,
                warm_runs: 0,
                budget: Duration::ZERO,
            };
            let rows = throughput::run(
                &corpus_root,
                &files,
                &engines,
                &args.threads,
                &ctx,
                &args.out,
                args.resume,
            )?;
            print!(
                "{}",
                throughput::render(&rows, &args.threads, args.min_pages)
            );
            Ok(())
        }
        Cmd::Run(args) => {
            let checkout = args
                .checkout
                .clone()
                .or_else(|| Some(repo_root().join("../pdfium-c++")).filter(|p| p.is_dir()));
            let binary = match (args.oracle_bin, &checkout) {
                (Some(bin), _) => bin,
                (None, Some(checkout)) => checkout.join("out/Release/pdfium_test"),
                (None, None) => bail!("no --oracle-bin and no --checkout"),
            };
            let font_dir = match (args.font_dir, &checkout) {
                (Some(dir), _) => dir,
                (None, Some(checkout)) => checkout.join("third_party/test_fonts"),
                (None, None) => bail!("no --font-dir and no --checkout"),
            };
            if !binary.is_file() {
                bail!("oracle binary not found at {}", binary.display());
            }
            if !font_dir.is_dir() {
                bail!("font directory not found at {}", font_dir.display());
            }
            let corpus_root = args
                .corpus
                .canonicalize()
                .with_context(|| format!("corpus {}", args.corpus.display()))?;
            let (files, rule, features) = if let Some(spec) = &args.spec {
                let text = std::fs::read_to_string(spec)
                    .with_context(|| format!("reading {}", spec.display()))?;
                let features: Vec<run::FeatureSpec> = serde_json::from_str(&text)?;
                let mut files: Vec<PathBuf> =
                    features.iter().map(|f| corpus_root.join(&f.file)).collect();
                files.sort();
                files.dedup();
                for file in &files {
                    if !file.is_file() {
                        bail!("spec names a file that does not exist: {}", file.display());
                    }
                }
                (
                    files,
                    format!("the files named in {}", spec.display()),
                    features,
                )
            } else {
                let files = run::list_corpus(&corpus_root, args.every, args.offset)?;
                let rule = if args.every > 1 {
                    format!(
                        "every {}th .pdf of the sorted recursive listing, from index {}",
                        args.every, args.offset
                    )
                } else {
                    "every .pdf of the recursive listing".to_owned()
                };
                (files, rule, Vec::new())
            };
            let engines = if args.engines.is_empty() {
                default_engines()
            } else {
                args.engines
            };
            let config = run::RunConfig {
                label: args.label,
                corpus_root,
                files,
                rule,
                engines,
                oracle: oracle::Oracle {
                    binary,
                    font_dir,
                    cache_root: args.scratch.join("oracle"),
                    dpi: args.dpi,
                },
                checkout,
                scratch: args.scratch,
                out: args.out,
                pdfium_lib: args.pdfium_lib,
                timeout: Duration::from_secs_f64(args.timeout_secs),
                warm_runs: args.warm,
                features,
                repo_root: repo_root(),
                resume: args.resume,
            };
            let json = run::run(&config)?;
            print!("{}", report::render(&json));
            eprintln!("wrote {}", config.out.display());
            Ok(())
        }
    }
}
