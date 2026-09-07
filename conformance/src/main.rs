//! Conformance harness: runs `pdfrum-tool` over the corpus and
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
//! - `tier-c` renders each file with **both** rasterizers and diffs them
//!   against each other rather than against the oracle, which separates an
//!   engine bug from a backend one.

#![forbid(unsafe_code)]

mod corpus;
mod divergences;
mod generate;
mod goldens;
mod json;
mod mutation;
mod oracle;
mod pixels;
mod pool;
mod run;
mod saveroundtrip;
mod scoreboard;
mod ssim;
mod suppressions;
mod thresholds;
mod tierc;
mod transcode;
mod triage;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use corpus::Roots;
use goldens::Store;
use oracle::{DirtyPolicy, OraclePaths};
use run::{ToolPaths, ToolState};
use scoreboard::{FileResult, Scoreboard};

/// Default oracle binary, relative to the oracle checkout.
///
/// Resolved against `CorpusArgs::checkout()`, so `--checkout` and
/// `$PDFRUM_ORACLE_CHECKOUT` move the binary with the tree they name;
/// `--oracle` / `$PDFRUM_ORACLE_BIN` override it outright.
const DEFAULT_ORACLE: &str = "out/Release/pdfium_test";

/// The environment spelling of `--allow-dirty-oracle`.
///
/// Any non-empty value other than `0` enables the override, so both `=1` and
/// `=true` do what the person typing them meant.
const ALLOW_DIRTY_ENV: &str = "PDFRUM_ALLOW_DIRTY_ORACLE";

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
    /// Diff the two rasterizers against each other over our own engine.
    TierC(TierCArgs),
    /// Save every corpus file and check that the oracle reopens it .
    SaveRoundTrip(SaveArgs),
    /// Mutate a page of every corpus file, save it, and check that the
    /// oracle's render of the result matches ours .
    MutateRoundTrip(MutateArgs),
}

/// Options shared by the corpus-walking subcommands.
#[derive(Debug, Args)]
struct CorpusArgs {
    /// The read-only pdfium C++ checkout holding testing/corpus and resources.
    #[arg(long, env = "PDFRUM_ORACLE_CHECKOUT")]
    checkout: Option<PathBuf>,
    /// Golden store root (default: conformance/goldens).
    #[arg(long, env = "PDFRUM_GOLDENS")]
    goldens: Option<PathBuf>,
    /// Parallel workers.
    #[arg(long)]
    jobs: Option<usize>,
    /// Process at most this many files (smoke tests).
    #[arg(long)]
    limit: Option<usize>,
    /// Run even though the oracle checkout has tracked modifications.
    ///
    /// The checkout is the answer key, so the default is to refuse; this is
    /// for someone who knows why their tree differs.
    ///
    /// The environment spelling is deliberately *not* clap's `env =`: that
    /// parses the value as a bool, so the `=1` an operator reaches for is a
    /// hard parse error rather than the override they asked for. It is read
    /// in `dirty_policy` instead, where any non-empty value except `0`
    /// enables it.
    #[arg(long)]
    allow_dirty_oracle: bool,
}

#[derive(Debug, Args)]
struct GenerateArgs {
    #[command(flatten)]
    corpus: CorpusArgs,
    /// Path to the oracle's `pdfium_test` binary.
    #[arg(long, env = "PDFRUM_ORACLE_BIN")]
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
    /// Fail if any file passing in this scoreboard now fails.
    #[arg(long, value_name = "OLD_SCOREBOARD")]
    check_regressions: Option<PathBuf>,
}

/// Options for the cross-backend comparison.
#[derive(Debug, Args)]
struct TierCArgs {
    #[command(flatten)]
    corpus: CorpusArgs,
    /// Path to the pdfrum-tool binary under test.
    #[arg(long, env = "PDFRUM_TOOL")]
    tool: Option<PathBuf>,
    /// Hermetic font directory (default: `<checkout>/third_party/test_fonts`).
    #[arg(long)]
    font_dir: Option<PathBuf>,
}

/// Options for the save round-trip sweep.
///
/// The only mode that needs **both** binaries: pdfrum writes the file and the
/// oracle reopens it, because `pdfium_test` cannot save at all.
#[derive(Debug, Args)]
struct SaveArgs {
    #[command(flatten)]
    corpus: CorpusArgs,
    /// Path to the pdfrum-tool binary under test.
    #[arg(long, env = "PDFRUM_TOOL")]
    tool: Option<PathBuf>,
    /// Path to the oracle's `pdfium_test` binary, which reopens what we save.
    #[arg(long, env = "PDFRUM_ORACLE_BIN")]
    oracle: Option<PathBuf>,
    /// Hermetic font directory (default: `<checkout>/third_party/test_fonts`).
    #[arg(long)]
    font_dir: Option<PathBuf>,
    /// Re-render at most this many saved files and diff them against the
    /// original's golden (Tier B). Rendering is the slow half, so the sweep
    /// checks reopening over everything and pixels over a sample.
    #[arg(long, default_value_t = 250)]
    render_sample: usize,
}

/// Arguments for the mutation sweep (exit check).
///
/// Needs both binaries for the same reason the save sweep does, and for a
/// sharper one: the comparison is between the two implementations' renders of
/// one file that *neither* has a golden for, because pdfrum wrote it.
#[derive(Debug, Args)]
struct MutateArgs {
    #[command(flatten)]
    corpus: CorpusArgs,
    /// Path to the pdfrum-tool binary under test.
    #[arg(long, env = "PDFRUM_TOOL")]
    tool: Option<PathBuf>,
    /// Path to the oracle's `pdfium_test`, which reopens and renders what we
    /// mutate.
    #[arg(long, env = "PDFRUM_ORACLE_BIN")]
    oracle: Option<PathBuf>,
    /// Hermetic font directory (default: `<checkout>/third_party/test_fonts`).
    #[arg(long)]
    font_dir: Option<PathBuf>,
    /// Mutate at most this many files, sampled evenly across the corpus.
    ///
    /// Every pair costs three renders, so the sweep samples rather than
    /// exhausts; the sample is a stride rather than a prefix because the
    /// corpus is grouped by directory and a prefix would be one feature.
    #[arg(long, default_value_t = 200)]
    sample: usize,
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
        Command::TierC(args) => tier_c(&args),
        Command::SaveRoundTrip(args) => save_round_trip(&args),
        Command::MutateRoundTrip(args) => mutate_round_trip(&args),
    }
}

/// The Tier-B floors, or the built-in default when the file is absent.
///
/// A malformed file is an error rather than a fallback: the ratchet is the
/// project's fitness function, and silently reverting to the global floor
/// would let a typo loosen every per-file threshold at once.
fn load_thresholds() -> Result<thresholds::Thresholds> {
    let path = conformance_dir().join("thresholds.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => thresholds::parse(&text).with_context(|| format!("parsing {}", path.display())),
        Err(_) => Ok(thresholds::Thresholds::default()),
    }
}

/// The deliberate divergences, or nothing when the file is absent.
///
/// A malformed file is an error rather than a fallback, for the reason
/// [`load_thresholds`] gives: this is the other half of the ratchet, and a
/// typo must not quietly turn every excused row back into a failure — nor an
/// unparsed one into a pass.
fn load_divergences() -> Result<divergences::Divergences> {
    let path = conformance_dir().join("divergences.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            divergences::parse(&text).with_context(|| format!("parsing {}", path.display()))
        }
        Err(_) => Ok(divergences::Divergences::default()),
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
    /// The oracle checkout, **having checked that it is unmodified**.
    ///
    /// Every subcommand that reads the checkout resolves its path through
    /// here, so the hygiene check cannot be forgotten at a new entry point:
    /// there is no other way to learn where the tree is.
    fn checkout(&self) -> Result<PathBuf> {
        let path = self
            .checkout
            .clone()
            .unwrap_or_else(|| repo_root().join("../pdfium-c++"));
        oracle::require_clean_checkout(&path, self.dirty_policy())?;
        Ok(path)
    }

    fn dirty_policy(&self) -> DirtyPolicy {
        let from_env = std::env::var_os(ALLOW_DIRTY_ENV)
            .is_some_and(|value| !value.is_empty() && value != "0");
        if self.allow_dirty_oracle || from_env {
            DirtyPolicy::Allow
        } else {
            DirtyPolicy::Refuse
        }
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
    let checkout = args.corpus.checkout()?;
    let oracle = OraclePaths {
        binary: args
            .oracle
            .clone()
            .unwrap_or_else(|| checkout.join(DEFAULT_ORACLE)),
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
    let checkout = args.corpus.checkout()?;
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

    let thresholds = load_thresholds()?;

    let divergences = load_divergences()?;

    let store = args.corpus.store();
    let fixup = checkout.join("testing/tools/fixup_pdf_template.py");
    let per_file = score_entries(
        entries,
        &tool,
        &state,
        &store,
        &thresholds,
        &fixup,
        args.corpus.workers(),
    )?;
    let per_file = mark_divergences(per_file, &divergences);
    if !divergences.is_empty() {
        println!(
            "divergences.toml excuses {} file(s) from the denominator",
            divergences.len()
        );
    }
    for path in inert_divergences(&per_file, &divergences) {
        // The entry is doing nothing: either the oracle defect was fixed
        // upstream, or the row left the corpus. Both are good news, and both
        // mean the row can be deleted — so say so rather than leave it to rot.
        println!("divergence for {path} is inert; the file agrees with the oracle");
    }
    let board = Scoreboard::new(now_utc(), per_file);
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| conformance_dir().join("scoreboard.json"));
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&out, board.to_text()).with_context(|| format!("writing {}", out.display()))?;

    let totals = board.totals();
    // Three numbers, always. `files` is the denominator the rate is taken
    // over, and `diverged` sits outside it rather than inside `pass`, so a
    // reader can never mistake an excused row for a matched one.
    println!(
        "run: {} files, {} pass ({:.1}%), {} diverged, {} fail -> {}",
        totals.files,
        totals.pass,
        totals.pass_rate().unwrap_or(0.0) * 100.0,
        totals.diverged,
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

/// Re-labels the rows named in `divergences.toml` as deliberate divergences.
///
/// Only a row that *failed* is re-labelled. An entry over a file that already
/// passes is inert rather than an error: the corpus and the oracle both move,
/// and an upstream fix landing is exactly the case where a row should go
/// quiet before anyone gets round to deleting it — an inert entry is visible
/// in the file and costs nothing, where an error would break the board on the
/// day the bug was fixed.
fn mark_divergences(
    per_file: Vec<scoreboard::FileResult>,
    divergences: &divergences::Divergences,
) -> Vec<scoreboard::FileResult> {
    per_file
        .into_iter()
        .map(|result| match divergences.get(&result.path) {
            Some(entry) if result.status == scoreboard::Status::Fail => result.diverged(&entry.why),
            _ => result,
        })
        .collect()
}

/// The excused paths that name no diverged row.
fn inert_divergences(
    per_file: &[scoreboard::FileResult],
    divergences: &divergences::Divergences,
) -> Vec<String> {
    let diverged: BTreeSet<&str> = per_file
        .iter()
        .filter(|result| result.status == scoreboard::Status::Diverged)
        .map(|result| result.path.as_str())
        .collect();
    divergences
        .iter()
        .map(|(path, _)| path)
        .filter(|path| !diverged.contains(path.as_str()))
        .cloned()
        .collect()
}

/// Scores every listing entry, plus a `#form-events` row when a sibling
/// `.evt` is present and a `#js-transcript` row for each javascript fixture.
///
/// Each optional slot emits no row when it answers `None`, which is what
/// keeps a non-`.evt` file out of the events family and every non-javascript
/// file out of the transcript family.
fn score_entries(
    entries: Vec<corpus::Entry>,
    tool: &ToolPaths,
    state: &ToolState,
    store: &Store,
    thresholds: &thresholds::Thresholds,
    fixup: &Path,
    workers: usize,
) -> Result<Vec<FileResult>> {
    let base = scratch_root("run")?;
    let indexed: Vec<(usize, corpus::Entry)> = entries.into_iter().enumerate().collect();
    let results: Vec<(FileResult, Option<FileResult>, Option<FileResult>)> =
        pool::map(&indexed, workers, |(index, entry)| {
            let regular = run::score_one(
                entry,
                tool,
                state,
                store,
                thresholds,
                &run::scratch(&base, *index),
                fixup,
            );
            let events = run::score_form_events(
                entry,
                tool,
                state,
                store,
                thresholds,
                &run::scratch_events(&base, *index),
                fixup,
            );
            let js = run::score_js_transcript(
                entry,
                tool,
                state,
                &run::scratch_js(&base, *index),
                fixup,
            );
            (regular, events, js)
        });
    std::fs::remove_dir_all(&base).ok();
    let mut per_file = Vec::with_capacity(results.len() * 2);
    for (regular, events, js) in results {
        per_file.push(regular);
        if let Some(events) = events {
            per_file.push(events);
        }
        if let Some(js) = js {
            per_file.push(js);
        }
    }
    Ok(per_file)
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

/// Render each file with both rasterizers and diff them against each other.
///
/// This is a *cross-backend* check, not an oracle one: it asks whether the
/// engine decided a page's pixels, or whether a rasterizer did. Both runs go
/// through the same `pdfrum-tool`, selected by `PDFRUM_BACKEND`, so the two
/// differ only in which `RasterBackend` the engine was handed.
fn tier_c(args: &TierCArgs) -> Result<ExitCode> {
    let checkout = args.corpus.checkout()?;
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
    let suppressed = load_suppressions(&checkout)?;
    let listing = corpus::list(&Roots::under(&checkout), &suppressed)
        .with_context(|| format!("walking the corpus under {}", checkout.display()))?;
    let mut entries = listing.entries;
    if let Some(limit) = args.corpus.limit {
        entries.truncate(limit);
    }

    let fixup = checkout.join("testing/tools/fixup_pdf_template.py");
    let base = scratch_root("tierc")?;
    let indexed: Vec<(usize, corpus::Entry)> = entries.into_iter().enumerate().collect();
    let outcomes: Vec<run::TierCOutcome> =
        pool::map(&indexed, args.corpus.workers(), |(index, entry)| {
            run::compare_backends(entry, &tool, &base.join(format!("f{index}")), &fixup)
        });
    std::fs::remove_dir_all(&base).ok();

    let compared = outcomes.iter().filter(|o| o.pages > 0).count();
    let hard: Vec<&run::TierCOutcome> = outcomes.iter().filter(|o| o.hard_fail).collect();
    let soft = outcomes
        .iter()
        .filter(|o| !o.hard_fail && !o.within_budget)
        .count();
    let worst = outcomes.iter().map(|o| o.edge_rate).fold(0.0f64, f64::max);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a corpus file count is far inside f64's exact integer range"
    )]
    let divergent_rate = if compared == 0 {
        0.0
    } else {
        (hard.len() + soft) as f64 / compared as f64
    };

    // The analytic backend is a third column, not a third gate: it shares the
    // engine's own compositing arithmetic, so diffing it against either
    // wrapped backend tests less than diffing the two wrapped ones against
    // each other. `TierCOutcome::agg_edge_rate` records the reasoning.
    let agg_rates: Vec<f64> = outcomes.iter().filter_map(|o| o.agg_edge_rate).collect();
    let agg_worst = agg_rates.iter().copied().fold(0.0f64, f64::max);

    println!("tier-c: {compared} files compared under the gating pair (tiny-skia vs vello_cpu)");
    println!("  hard failures (engine bugs)   {}", hard.len());
    println!("  over the 1% edge budget       {soft}");
    println!("  worst edge divergence         {:.4}%", worst * 100.0);
    println!(
        "  divergent files               {:.2}%",
        divergent_rate * 100.0
    );
    println!(
        "  agg backend (reported)        {} files, worst edge divergence {:.4}%",
        agg_rates.len(),
        agg_worst * 100.0
    );
    for outcome in hard.iter().take(triage::EXAMPLES) {
        println!("    {} - {}", outcome.path, outcome.note);
    }
    Ok(ExitCode::SUCCESS)
}

/// Print the sweep's scoreboard.
///
/// Split out from [`save_round_trip`] because the numbers are a report rather
/// than a step of the check: what each means is documented on
/// [`saveroundtrip::SaveTotals`], and the only thing decided here is the order
/// they print in.
fn report_save_totals(totals: &saveroundtrip::SaveTotals, files: usize) {
    let percent =
        |rate: Option<f64>| rate.map_or_else(|| "n/a".to_owned(), |r| format!("{:.2}%", r * 100.0));
    println!("save round-trip over {files} corpus files");
    println!(
        "  saved                         {} ({} skipped before any check)",
        totals.saved, totals.skipped
    );
    println!(
        "  oracle reopened               {} / {}  {}",
        totals.oracle_reopened,
        totals.saved,
        percent(totals.reopen_rate())
    );
    println!("  saved renders as well as the original (fidelity)");
    println!(
        "                                {} / {}  {}",
        totals.matches_original,
        totals.compared,
        percent(totals.fidelity_rate())
    );
    println!(
        "  saved clears the Tier-B floor {} / {}  {}",
        totals.within_floor,
        totals.compared,
        percent(totals.pixel_rate())
    );
    println!(
        "  incremental append discipline {} / {}  {}",
        totals.incremental_ok,
        totals.incremental_checked,
        percent(totals.incremental_rate())
    );
    // an encrypted file must save encrypted. The oracle opening it with
    // the password says the cipher is right; the oracle refusing it without
    // one says a cipher is there at all.
    if totals.encrypted > 0 {
        println!(
            "  encrypted stayed encrypted    {} / {}  {}",
            totals.still_encrypted,
            totals.encrypted,
            percent(totals.still_encrypted_rate())
        );
    }
}

/// Save every corpus file, check the oracle reopens it, and diff a sample's
/// pixels against the original's golden render.
///
/// The numbers this prints are what and are graded on. Only the first
/// two are gates: a file the tool could not *open* is skipped rather than
/// failed, because Tier B already scores that and counting it twice would let
/// a parse regression read as a writer bug.
fn save_round_trip(args: &SaveArgs) -> Result<ExitCode> {
    let checkout = args.corpus.checkout()?;
    let font_dir = args
        .font_dir
        .clone()
        .unwrap_or_else(|| checkout.join("third_party/test_fonts"));
    let tool = ToolPaths {
        binary: args
            .tool
            .clone()
            .unwrap_or_else(|| repo_root().join("target/release/pdfrum-tool")),
        font_dir: font_dir.clone(),
    };
    let oracle = OraclePaths {
        binary: args
            .oracle
            .clone()
            .unwrap_or_else(|| checkout.join(DEFAULT_ORACLE)),
        font_dir,
    };
    generate::check_oracle(&oracle)?;

    let store = args.corpus.store();
    let thresholds = load_thresholds()?;
    let suppressed = load_suppressions(&checkout)?;
    let listing = corpus::list(&Roots::under(&checkout), &suppressed)
        .with_context(|| format!("walking the corpus under {}", checkout.display()))?;
    let mut entries = listing.entries;
    if let Some(limit) = args.corpus.limit {
        entries.truncate(limit);
    }

    // Rendering is the slow half, so the pixel diff runs over an evenly
    // spread sample rather than the first N files — the corpus is grouped by
    // directory, and taking a prefix would sample one feature cluster.
    let stride = entries.len().div_ceil(args.render_sample.max(1)).max(1);

    let fixup = checkout.join("testing/tools/fixup_pdf_template.py");
    let base = scratch_root("save")?;
    let indexed: Vec<(usize, corpus::Entry)> = entries.into_iter().enumerate().collect();
    let outcomes: Vec<saveroundtrip::SaveOutcome> =
        pool::map(&indexed, args.corpus.workers(), |(index, entry)| {
            saveroundtrip::check_one(
                entry,
                &tool,
                &oracle,
                &store,
                &thresholds,
                &base.join(format!("f{index}")),
                &fixup,
                index % stride == 0,
            )
        });
    std::fs::remove_dir_all(&base).ok();

    let mut totals = saveroundtrip::SaveTotals::default();
    for outcome in &outcomes {
        totals.add(outcome);
    }

    report_save_totals(&totals, outcomes.len());

    let mut failures: Vec<&saveroundtrip::SaveOutcome> = outcomes
        .iter()
        // Only a real loss is a failure: a file below the floor whose
        // original renders the same has cost the save nothing.
        .filter(|o| {
            o.saved
                && (!o.oracle_reopened
                    || !o.matches_original
                    || o.incremental_ok == Some(false)
                    || o.still_encrypted == Some(false))
        })
        .collect();
    failures.sort_by(|a, b| a.path.cmp(&b.path));
    for outcome in failures.iter().take(triage::EXAMPLES) {
        println!("    {} - {}", outcome.path, outcome.note);
    }
    if failures.len() > triage::EXAMPLES {
        println!("    ... and {} more", failures.len() - triage::EXAMPLES);
    }

    Ok(ExitCode::SUCCESS)
}

fn mutate_round_trip(args: &MutateArgs) -> Result<ExitCode> {
    let checkout = args.corpus.checkout()?;
    let font_dir = args
        .font_dir
        .clone()
        .unwrap_or_else(|| checkout.join("third_party/test_fonts"));
    let tool = ToolPaths {
        binary: args
            .tool
            .clone()
            .unwrap_or_else(|| repo_root().join("target/release/pdfrum-tool")),
        font_dir: font_dir.clone(),
    };
    let oracle = OraclePaths {
        binary: args
            .oracle
            .clone()
            .unwrap_or_else(|| checkout.join(DEFAULT_ORACLE)),
        font_dir,
    };
    generate::check_oracle(&oracle)?;

    let suppressed = load_suppressions(&checkout)?;
    let listing = corpus::list(&Roots::under(&checkout), &suppressed)
        .with_context(|| format!("walking the corpus under {}", checkout.display()))?;
    let mut entries = listing.entries;
    if let Some(limit) = args.corpus.limit {
        entries.truncate(limit);
    }
    let stride = entries.len().div_ceil(args.sample.max(1)).max(1);
    let sampled: Vec<(usize, corpus::Entry)> = entries
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % stride == 0)
        .collect();

    let fixup = checkout.join("testing/tools/fixup_pdf_template.py");
    let base = scratch_root("mutate")?;
    let batches: Vec<Vec<mutation::MutationOutcome>> =
        pool::map(&sampled, args.corpus.workers(), |(index, entry)| {
            mutation::check_one(
                entry,
                &tool,
                &oracle,
                &mutation::scratch_for(&base, *index),
                &fixup,
            )
        });
    std::fs::remove_dir_all(&base).ok();
    let outcomes: Vec<mutation::MutationOutcome> = batches.into_iter().flatten().collect();

    let mut tally = mutation::MutationTally::default();
    for outcome in &outcomes {
        tally.add(outcome);
    }
    report_mutation_tally(&tally, sampled.len());

    // A file that never got as far as a save is reported too: the sweep is
    // meant to exercise the mutation path, and a run where nothing reached it
    // is a broken harness rather than a clean result.
    let mut failures: Vec<&mutation::MutationOutcome> = outcomes
        .iter()
        .filter(|o| !o.passed() || !o.saved)
        .collect();
    failures.sort_by(|a, b| (&a.path, &a.mutation).cmp(&(&b.path, &b.mutation)));
    // The pixel shortfalls come first: a file the tool cannot open at all is
    // already Tier B's business, and burying the interesting failures under a
    // list of those would defeat the report.
    failures.sort_by_key(|o| !(o.saved && o.mutated));
    let show = if std::env::var_os("PDFRUM_SHOW_ALL").is_some() {
        failures.len()
    } else {
        triage::EXAMPLES
    };
    for outcome in failures.iter().take(show) {
        println!(
            "    {} [{}] - {}",
            outcome.path, outcome.mutation, outcome.note
        );
    }
    if failures.len() > show {
        println!("    ... and {} more", failures.len() - show);
    }

    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The mutation sweep's summary, in the shape the save sweep's uses.
fn report_mutation_tally(tally: &mutation::MutationTally, files: usize) {
    let pct = |rate: Option<f64>| match rate {
        Some(rate) => format!("{:.1}%", rate * 100.0),
        None => "n/a".to_owned(),
    };
    println!(
        "mutate round trip over {files} files x {} mutations",
        mutation::MUTATIONS.len()
    );
    println!("  mutations applied:  {}", tally.mutated);
    println!("  nothing to mutate:  {}", tally.nothing_to_do);
    println!("  never saved:        {}", tally.skipped);
    println!(
        "  oracle reopened:    {} ({})",
        tally.reopened,
        pct(tally.reopen_rate())
    );
    println!(
        "  agreed at >= {:.2}:  {} ({})",
        mutation::FLOOR,
        tally.within_floor,
        pct(tally.agreement_rate())
    );
    // A file the two renderers already disagreed on is counted apart, because
    // the mutation did not cause the disagreement.
    println!(
        "  + already disagreed: {} — the two renderers differ on a plain save of it too",
        tally.baseline_explained
    );
    println!(
        "  lost nothing:       {} ({})",
        tally.within_floor + tally.baseline_explained,
        pct(tally.no_loss_rate())
    );
    match tally.worst_ssim {
        Some(worst) => println!("  worst ssim:         {worst:.6}"),
        None => println!("  worst ssim:         n/a"),
    }
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
        assert_eq!(
            names,
            [
                "generate-goldens",
                "run",
                "triage",
                "tier-c",
                "save-round-trip",
                "mutate-round-trip"
            ]
        );
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
    fn tier_c_parses_its_flags() {
        // Note the absent `--backend`: the two rasterizers are selected by
        // `PDFRUM_BACKEND` inside the tool, deliberately out of band, so the
        // tool's flag surface stays exactly the oracle's.
        let cli = Cli::try_parse_from([
            "conformance",
            "tier-c",
            "--tool",
            "/t/pdfrum-tool",
            "--limit",
            "50",
        ])
        .unwrap();
        match cli.command {
            Command::TierC(args) => {
                assert_eq!(args.tool, Some(PathBuf::from("/t/pdfrum-tool")));
                assert_eq!(args.corpus.limit, Some(50));
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
        assert_eq!(DEFAULT_ORACLE, "out/Release/pdfium_test");
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
