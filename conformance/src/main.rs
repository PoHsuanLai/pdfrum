//! Conformance harness (PLAN.md §5): runs `pdfrum-tool` over the corpus and
//! resource PDFs, compares against the golden store in `goldens/` (Tier A
//! byte-exact, Tier B perceptual), and emits `scoreboard.json` — the fitness
//! function every burn-down loop optimizes.
//!
//! Three subcommands:
//!
//! - `generate-goldens` drives the read-only C++ oracle over the corpus and
//!   fills the golden store. Run once; resumable, so an interrupted run
//!   continues where it stopped.
//! - `run` scores `pdfrum-tool` against that store and writes the scoreboard,
//!   optionally enforcing the monotone rule against a previous one.
//! - `triage` clusters the scoreboard's failures into units of work.

#![forbid(unsafe_code)]

mod corpus;
mod generate;
mod goldens;
mod json;
mod oracle;
mod pixels;
mod pool;
mod run;
mod scoreboard;
mod ssim;
mod suppressions;
mod thresholds;
mod transcode;
mod triage;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use corpus::Roots;
use goldens::Store;
use oracle::OraclePaths;
use run::{ToolPaths, ToolState};
use scoreboard::{FileResult, Scoreboard};

/// Default oracle binary, relative to the repository root.
const DEFAULT_ORACLE: &str = "../pdfium-c++/out/Release/pdfium_test";

#[derive(Debug, Parser)]
#[command(
    name = "conformance",
    about = "pdfrum conformance harness: golden store, scoreboard, triage",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Drive the C++ oracle over the corpus and fill the golden store.
    GenerateGoldens(GenerateArgs),
    /// Score pdfrum-tool against the golden store and write scoreboard.json.
    Run(RunArgs),
    /// Cluster scoreboard failures into units of work.
    Triage(TriageArgs),
}

/// Options shared by the corpus-walking subcommands.
#[derive(Debug, Args)]
struct CorpusArgs {
    /// The read-only pdfium C++ checkout holding testing/corpus and resources.
    #[arg(long, env = "PDFRUM_ORACLE_CHECKOUT")]
    checkout: Option<PathBuf>,
    /// Golden store root (default: conformance/goldens).
    #[arg(long)]
    goldens: Option<PathBuf>,
    /// Parallel workers.
    #[arg(long)]
    jobs: Option<usize>,
    /// Process at most this many files (smoke tests).
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
struct GenerateArgs {
    #[command(flatten)]
    corpus: CorpusArgs,
    /// Path to the oracle's `pdfium_test` binary.
    #[arg(long, env = "PDFRUM_ORACLE")]
    oracle: Option<PathBuf>,
    /// Hermetic font directory (default: `<checkout>/third_party/test_fonts`).
    #[arg(long)]
    font_dir: Option<PathBuf>,
    /// Regenerate goldens that already exist.
    #[arg(long)]
    force: bool,
    /// List what would be done without running the oracle.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[command(flatten)]
    corpus: CorpusArgs,
    /// Path to the pdfrum-tool binary under test.
    #[arg(long, env = "PDFRUM_TOOL")]
    tool: Option<PathBuf>,
    /// Hermetic font directory (default: `<checkout>/third_party/test_fonts`).
    #[arg(long)]
    font_dir: Option<PathBuf>,
    /// Where to write the scoreboard (default: conformance/scoreboard.json).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Fail if any file passing in this scoreboard now fails (PLAN.md §7).
    #[arg(long, value_name = "OLD_SCOREBOARD")]
    check_regressions: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct TriageArgs {
    /// Scoreboard to read (default: conformance/scoreboard.json).
    #[arg(long)]
    scoreboard: Option<PathBuf>,
    /// Emit JSON instead of the human report.
    #[arg(long)]
    json: bool,
    /// Show at most this many clusters.
    #[arg(long, default_value_t = 10)]
    top: usize,
}

fn main() -> ExitCode {
    match dispatch() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("conformance: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch() -> Result<ExitCode> {
    match Cli::parse().command {
        Command::GenerateGoldens(args) => generate_goldens(&args),
        Command::Run(args) => run_corpus(&args),
        Command::Triage(args) => triage_report(&args),
    }
}

/// The `conformance/` directory, resolved from this binary's manifest so the
/// harness works from any working directory.
fn conformance_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root (the workspace directory above `conformance/`).
fn repo_root() -> PathBuf {
    conformance_dir()
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

impl CorpusArgs {
    fn checkout(&self) -> PathBuf {
        self.checkout
            .clone()
            .unwrap_or_else(|| repo_root().join("../pdfium-c++"))
    }

    fn store(&self) -> Store {
        Store::at(
            self.goldens
                .clone()
                .unwrap_or_else(|| conformance_dir().join("goldens")),
        )
    }

    fn workers(&self) -> usize {
        self.jobs.unwrap_or_else(pool::default_workers)
    }
}

/// Loads the suppression set for our `linux/nov8/noxfa/agg` oracle build.
fn load_suppressions(checkout: &Path) -> Result<BTreeSet<String>> {
    let path = checkout.join("testing/SUPPRESSIONS");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "note: no SUPPRESSIONS at {} - not filtering",
            path.display()
        );
        return Ok(BTreeSet::new());
    };
    suppressions::parse(&text, &suppressions::Selector::default())
        .with_context(|| format!("parsing {}", path.display()))
}

/// A scratch directory that is removed when the run ends.
fn scratch_root(tag: &str) -> Result<PathBuf> {
    let root =
        std::env::temp_dir().join(format!("pdfrum-conformance-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root)
        .with_context(|| format!("creating scratch root {}", root.display()))?;
    Ok(root)
}

fn generate_goldens(args: &GenerateArgs) -> Result<ExitCode> {
    let checkout = args.corpus.checkout();
    let oracle = OraclePaths {
        binary: args
            .oracle
            .clone()
            .unwrap_or_else(|| repo_root().join(DEFAULT_ORACLE)),
        font_dir: args
            .font_dir
            .clone()
            .unwrap_or_else(|| checkout.join("third_party/test_fonts")),
    };
    generate::check_oracle(&oracle)?;

    let fixup = checkout.join("testing/tools/fixup_pdf_template.py");
    if !fixup.is_file() {
        bail!(
            "template expander not found at {}\n\
             --checkout must point at a pdfium C++ checkout (currently {}).",
            fixup.display(),
            checkout.display()
        );
    }

    let suppressed = load_suppressions(&checkout)?;
    let listing = corpus::list(&Roots::under(&checkout), &suppressed)
        .with_context(|| format!("walking the corpus under {}", checkout.display()))?;
    let mut entries = listing.entries;
    if let Some(limit) = args.corpus.limit {
        entries.truncate(limit);
    }

    let store = args.corpus.store();
    eprintln!(
        "conformance: {} entries ({} skipped: xfa/suppressed), oracle {}",
        entries.len(),
        listing.skipped.len(),
        oracle.binary.display()
    );

    if args.dry_run {
        for entry in entries.iter().take(20) {
            println!("{}  {:?}", entry.id, entry.kind);
        }
        if entries.len() > 20 {
            println!("... and {} more", entries.len() - 20);
        }
        return Ok(ExitCode::SUCCESS);
    }

    let base = scratch_root("generate")?;
    let indexed: Vec<(usize, corpus::Entry)> = entries.into_iter().enumerate().collect();
    let outcomes = pool::map(&indexed, args.corpus.workers(), |(index, entry)| {
        generate::generate_one(
            entry,
            &oracle,
            &store,
            &generate::scratch_for(&base, *index),
            &fixup,
            args.force,
        )
    });
    std::fs::remove_dir_all(&base).ok();

    let (mut generated, mut skipped, mut failed, mut artifacts) = (0u64, 0u64, 0u64, 0u64);
    for outcome in &outcomes {
        match &outcome.outcome {
            generate::Outcome::Generated { artifacts: n, .. } => {
                generated += 1;
                artifacts += *n as u64;
            }
            generate::Outcome::Skipped { .. } => skipped += 1,
            generate::Outcome::Failed { reason } => {
                failed += 1;
                eprintln!("  FAILED {}: {reason}", outcome.id);
            }
        }
    }
    let stored = goldens::keys(&store.root).map_or(0, |k| k.len());
    println!(
        "generate-goldens: {generated} generated ({artifacts} artifacts), \
         {skipped} already present, {failed} failed; \
         store holds {stored} distinct PDFs"
    );
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn run_corpus(args: &RunArgs) -> Result<ExitCode> {
    let checkout = args.corpus.checkout();
    let tool = ToolPaths {
        binary: args
            .tool
            .clone()
            .unwrap_or_else(|| repo_root().join("target/release/pdfrum-tool")),
        font_dir: args
            .font_dir
            .clone()
            .unwrap_or_else(|| checkout.join("third_party/test_fonts")),
    };
    let state = run::probe_tool(&tool);
    if let ToolState::Unsupported(reason) = &state {
        eprintln!("conformance: {reason}");
        eprintln!("conformance: every file will be tagged `unsupported-tool`.");
    }

    let suppressed = load_suppressions(&checkout)?;
    let listing = corpus::list(&Roots::under(&checkout), &suppressed)
        .with_context(|| format!("walking the corpus under {}", checkout.display()))?;
    let mut entries = listing.entries;
    if let Some(limit) = args.corpus.limit {
        entries.truncate(limit);
    }

    let thresholds_path = conformance_dir().join("thresholds.toml");
    let thresholds = match std::fs::read_to_string(&thresholds_path) {
        Ok(text) => thresholds::parse(&text)
            .with_context(|| format!("parsing {}", thresholds_path.display()))?,
        Err(_) => thresholds::Thresholds::default(),
    };

    let store = args.corpus.store();
    let fixup = checkout.join("testing/tools/fixup_pdf_template.py");
    let base = scratch_root("run")?;
    let indexed: Vec<(usize, corpus::Entry)> = entries.into_iter().enumerate().collect();
    let results: Vec<FileResult> = pool::map(&indexed, args.corpus.workers(), |(index, entry)| {
        run::score_one(
            entry,
            &tool,
            &state,
            &store,
            &thresholds,
            &run::scratch(&base, *index),
            &fixup,
        )
    });
    std::fs::remove_dir_all(&base).ok();

    let board = Scoreboard::new(now_utc(), results);
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| conformance_dir().join("scoreboard.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&out, board.to_text()).with_context(|| format!("writing {}", out.display()))?;

    let totals = board.totals();
    println!(
        "run: {} files, {} pass, {} fail -> {}",
        totals.files,
        totals.pass,
        totals.fail,
        out.display()
    );
    for (tag, count) in &totals.by_tag {
        println!("  {tag:<20} {count:>6}");
    }
    print!("{}", text_summary(&totals));

    if let Some(previous_path) = &args.check_regressions {
        let text = std::fs::read_to_string(previous_path)
            .with_context(|| format!("reading {}", previous_path.display()))?;
        let previous = Scoreboard::from_text(&text)
            .with_context(|| format!("parsing {}", previous_path.display()))?;
        let regressions = Scoreboard::regressions(&previous, &board);
        if !regressions.is_empty() {
            eprintln!(
                "\nREGRESSION: {} previously-passing file(s) now fail:",
                regressions.len()
            );
            for path in regressions.iter().take(50) {
                eprintln!("  {path}");
            }
            if regressions.len() > 50 {
                eprintln!("  ... and {} more", regressions.len() - 50);
            }
            return Ok(ExitCode::FAILURE);
        }
        println!("no regressions against {}", previous_path.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// The two text pass rates, or nothing when no text golden was compared.
///
/// Printed as two lines because they answer different questions and the
/// second is the honest one: nearly half the corpus's text goldens are empty
/// (the oracle wrote a byte-order mark and no characters), so a tool that
/// printed nothing at all would score around 46% on the first line while
/// extracting no text whatsoever.
fn text_summary(totals: &scoreboard::Totals) -> String {
    let (Some(rate), Some(nonempty)) = (totals.text_rate(), totals.text_nonempty_rate()) else {
        return String::new();
    };
    let text = &totals.text;
    format!(
        "  text                 {}/{} pages ({:.1}%)\n  text-nonempty        {}/{} pages ({:.1}%)\n",
        text.matched,
        text.pages,
        rate * 100.0,
        text.substantive_matched,
        text.substantive,
        nonempty * 100.0,
    )
}

fn triage_report(args: &TriageArgs) -> Result<ExitCode> {
    let path = args
        .scoreboard
        .clone()
        .unwrap_or_else(|| conformance_dir().join("scoreboard.json"));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {} - run `conformance run` first", path.display()))?;
    let board =
        Scoreboard::from_text(&text).with_context(|| format!("parsing {}", path.display()))?;
    let mut clusters = triage::cluster(&board);
    clusters.truncate(args.top);

    if args.json {
        print!("{}", triage::to_json(&board, &clusters).to_pretty());
    } else {
        print!("{}", triage::render(&board, &clusters));
    }
    Ok(ExitCode::SUCCESS)
}

/// A UTC timestamp for the scoreboard header.
///
/// Formatted here rather than pulled from a date crate: the field is
/// informational and never compared, so seconds-since-epoch rendered as a
/// civil date via the standard leap-year rules is enough.
fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since the Unix epoch to a civil (year, month, day), by Howard
/// Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    // `m` is 1..=12 and `d` is 1..=31 by construction.
    (
        if m <= 2 { y + 1 } else { y },
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_subcommand_is_reachable() {
        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        assert_eq!(names, ["generate-goldens", "run", "triage"]);
    }

    #[test]
    fn generate_goldens_parses_its_flags() {
        let cli = Cli::try_parse_from([
            "conformance",
            "generate-goldens",
            "--oracle",
            "/o/pdfium_test",
            "--jobs",
            "4",
            "--force",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::GenerateGoldens(args) => {
                assert_eq!(args.oracle, Some(PathBuf::from("/o/pdfium_test")));
                assert_eq!(args.corpus.jobs, Some(4));
                assert!(args.force);
                assert!(args.dry_run);
            }
            _ => panic!("wrong subcommand parsed"),
        }
    }

    #[test]
    fn run_parses_the_regression_gate() {
        let cli = Cli::try_parse_from([
            "conformance",
            "run",
            "--tool",
            "/t/pdfrum-tool",
            "--check-regressions",
            "old.json",
        ])
        .unwrap();
        match cli.command {
            Command::Run(args) => {
                assert_eq!(args.tool, Some(PathBuf::from("/t/pdfrum-tool")));
                assert_eq!(args.check_regressions, Some(PathBuf::from("old.json")));
            }
            _ => panic!("wrong subcommand parsed"),
        }
    }

    #[test]
    fn triage_defaults_to_the_human_report_and_ten_clusters() {
        let cli = Cli::try_parse_from(["conformance", "triage"]).unwrap();
        match cli.command {
            Command::Triage(args) => {
                assert!(!args.json);
                assert_eq!(args.top, 10);
            }
            _ => panic!("wrong subcommand parsed"),
        }
    }

    #[test]
    fn the_default_oracle_path_is_the_documented_one() {
        assert_eq!(DEFAULT_ORACLE, "../pdfium-c++/out/Release/pdfium_test");
    }

    #[test]
    fn timestamps_render_as_rfc3339_utc() {
        let stamp = now_utc();
        assert_eq!(stamp.len(), 20);
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert!(stamp.contains('T'), "{stamp}");
    }

    #[test]
    fn the_text_summary_reports_both_rates() {
        let totals = scoreboard::Totals {
            text: scoreboard::TextScore {
                pages: 1712,
                matched: 790,
                substantive: 922,
                substantive_matched: 0,
            },
            ..scoreboard::Totals::default()
        };
        let summary = text_summary(&totals);
        assert!(
            summary.contains("text                 790/1712 pages (46.1%)"),
            "{summary}"
        );
        assert!(
            summary.contains("text-nonempty        0/922 pages (0.0%)"),
            "{summary}"
        );
    }

    #[test]
    fn the_text_summary_is_silent_when_nothing_was_compared() {
        assert_eq!(text_summary(&scoreboard::Totals::default()), "");
    }

    #[test]
    fn civil_dates_match_known_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // a leap day
        assert_eq!(civil_from_days(20_694), (2026, 8, 29));
    }
}
