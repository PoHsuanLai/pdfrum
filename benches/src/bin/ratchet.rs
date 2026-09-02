//! The benchmark ratchet: compare a criterion run against the committed
//! `baseline.json`, fail on a regression, and record an improvement.
//!
//! This is the performance half of what `conformance/scoreboard.json` is for
//! correctness, and it is deliberately built the same way: a committed file of
//! numbers, a checker that refuses a change making them worse, and an explicit
//! update step that makes an improvement permanent. A benchmark suite with no
//! ratchet reports drift; one with a ratchet prevents it.
//!
//! ```text
//! cargo bench --workspace                # produces target/criterion/**
//! cargo run --release -p pdfrum-bench --bin ratchet -- check
//! cargo run --release -p pdfrum-bench --bin ratchet -- update
//! ```
//!
//! # Where the benchmarks live, and why this still works
//!
//! Since M12's per-crate split there is no single bench crate: `pdfrum-parser`
//! owns `open`, `pdfrum-page` owns `build`, `pdfrum-render` owns the six render
//! groups, `pdfrum-text` owns `text` and `pdfrum-edit` owns `save`. That change
//! is invisible here, and deliberately so — this binary reads
//! `target/criterion/**`, which criterion keys by *group name*, not by which
//! crate's binary produced it. `cargo bench --workspace` fills the same
//! directory the single crate used to. What it does mean is that
//! `cargo bench -p pdfrum-render` leaves `open`, `build`, `text` and `save`
//! stale on disk, so `check` would compare four groups against a run that did
//! not happen; the "not run" report cannot see that, because the files are
//! there. Run the whole workspace before a `check` that decides anything.
//!
//! # The id mapping across the split
//!
//! The render group names changed and the ids therefore did too:
//!
//! ```text
//! render-agg/<class>/<stem>   ->  render-cold-agg/<class>/<stem>
//! render-tinyskia/...           ->  render-cold-tinyskia/...
//! render-vello/...              ->  render-cold-vello/...
//! (new)                         ->  render-warm-{exact,tinyskia,vello}/...
//! (new)                         ->  build/<class>/<stem>
//! ```
//!
//! `open`, `text` and `save` keep their ids exactly. The three renamed groups
//! measure the same thing they did — a fresh session per iteration — so their
//! old numbers were *transferable* in principle, and the baseline was
//! nonetheless re-initialized rather than renamed. The reason is that the same
//! commit changed what the render path costs (the two outlier fixes), so a
//! carried-over number would have shown a large improvement in a file whose
//! purpose is to make improvements visible one at a time. Re-initializing
//! records the new floor honestly; `docs/status/M12.md` §10 carries the
//! before/after comparison the ratchet would otherwise have printed.
//!
//! # The rule
//!
//! For each benchmark, `check` compares the new median against the committed
//! one:
//!
//! - **slower by more than the noise band** → a regression. Reported, and the
//!   process exits non-zero.
//! - **faster by more than the noise band** → an improvement. Reported;
//!   `update` writes it into the baseline so it cannot silently be given back.
//! - **inside the band** → unchanged, and nothing happens. The baseline keeps
//!   the *old* number rather than jittering toward the new one, because a
//!   baseline that absorbs every in-band sample ratchets itself downward one
//!   noise-width at a time and ends up failing on a machine that is behaving
//!   perfectly.
//!
//! A benchmark absent from the baseline is new, not a failure. A benchmark
//! absent from the run is *not* checked and is reported as such — running a
//! filtered subset must not look like a pass over the whole corpus.
//!
//! # The band
//!
//! Per group, from `baseline.json`'s `bands` map, because the groups do not
//! have the same repeatability: `open` is microseconds and jitters several
//! percent between runs on an idle machine, where a render is milliseconds and
//! sits inside two. `docs/status/M12.md` §"The noise band" has the measured
//! distribution each number comes from — they are empirical, not chosen to be
//! round.
//!
//! # Why the median and not the mean or criterion's own slope
//!
//! The median, with criterion's own 95% confidence interval available as a
//! sanity check. A wall-clock sample is bounded below by the real cost and
//! unbounded above by whatever else the machine was doing, so the mean of a
//! contended run is permanently inflated while the median is not. Criterion's
//! `slope` estimate is better still for benchmarks whose iteration count
//! varies, but it is absent for the ones criterion measures in "flat" mode, and
//! a ratchet that changes statistic depending on the benchmark cannot be
//! compared across a corpus.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One benchmark's committed number.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// The median, in nanoseconds.
    median_ns: f64,
}

/// The committed baseline: numbers, plus the band each group is judged with.
#[derive(Debug, Default)]
struct Baseline {
    /// `group/class/stem` → the committed median.
    entries: BTreeMap<String, Entry>,
    /// `group` → the fractional noise band, e.g. `0.05` for 5%.
    bands: BTreeMap<String, f64>,
}

/// The default band for a group `baseline.json` does not name.
///
/// Deliberately tight rather than forgiving: a group with no measured band has
/// not been characterised, and the right failure mode for an uncharacterised
/// benchmark is a false alarm somebody investigates, not a real regression
/// nobody sees.
const DEFAULT_BAND: f64 = 0.05;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let root = workspace_root();
    let baseline_path = root.join("benches/baseline.json");
    let criterion_dir = target_dir().join("criterion");

    let baseline = match read_baseline(&baseline_path) {
        Ok(baseline) => baseline,
        Err(err) => {
            eprintln!("cannot read {}: {err}", baseline_path.display());
            std::process::exit(2);
        }
    };
    let measured = match read_criterion(&criterion_dir) {
        Ok(measured) => measured,
        Err(err) => {
            eprintln!("cannot read {}: {err}", criterion_dir.display());
            eprintln!("run `cargo bench --workspace` first.");
            std::process::exit(2);
        }
    };
    if measured.is_empty() {
        eprintln!("no benchmark results under {}", criterion_dir.display());
        eprintln!("run `cargo bench --workspace` first.");
        std::process::exit(2);
    }

    match mode.as_str() {
        "check" => {
            check(&baseline, &measured, false);
        }
        "update" => {
            if let Some(updated) = check(&baseline, &measured, true) {
                write_baseline(&baseline_path, &updated, &baseline.bands);
            }
        }
        "init" => {
            let entries = measured
                .iter()
                .map(|(k, v)| (k.clone(), Entry { median_ns: *v }))
                .collect();
            write_baseline(&baseline_path, &entries, &baseline.bands);
        }
        _ => {
            eprintln!("usage: ratchet <check|update|init>");
            eprintln!();
            eprintln!("  check   compare target/criterion against benches/baseline.json;");
            eprintln!("          exit non-zero on a regression outside the noise band");
            eprintln!("  update  the same comparison, then write improvements into the");
            eprintln!("          baseline (regressions still fail and write nothing)");
            eprintln!("  init    overwrite the baseline with the current run wholesale;");
            eprintln!("          for establishing one, not for maintaining it");
            std::process::exit(2);
        }
    }
}

/// Compare, report, and exit non-zero on a regression.
///
/// Returns the entries an `update` should commit, or `None` when there is
/// nothing to write.
fn check(
    baseline: &Baseline,
    measured: &BTreeMap<String, f64>,
    updating: bool,
) -> Option<BTreeMap<String, Entry>> {
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    let mut fresh = Vec::new();
    let mut unchanged = 0usize;
    let mut updated = baseline.entries.clone();

    for (id, &now) in measured {
        let band = baseline.band_for(id);
        let Some(old) = baseline.entries.get(id) else {
            fresh.push((id.clone(), now));
            updated.insert(id.clone(), Entry { median_ns: now });
            continue;
        };
        if old.median_ns <= 0.0 {
            continue;
        }
        let delta = (now - old.median_ns) / old.median_ns;
        if delta > band {
            regressions.push((id.clone(), old.median_ns, now, delta, band));
        } else if delta < -band {
            improvements.push((id.clone(), old.median_ns, now, delta));
            updated.insert(id.clone(), Entry { median_ns: now });
        } else {
            unchanged += 1;
        }
    }

    let not_run: Vec<&String> = baseline
        .entries
        .keys()
        .filter(|id| !measured.contains_key(*id))
        .collect();

    println!(
        "ratchet: {} benchmarks measured, {} in the baseline",
        measured.len(),
        baseline.entries.len()
    );
    println!(
        "  {unchanged} unchanged, {} improved, {} regressed, {} new, {} not run",
        improvements.len(),
        regressions.len(),
        fresh.len(),
        not_run.len()
    );

    // Biggest win first, which is `delta` ascending because an improvement is
    // negative.
    improvements.sort_by(|a, b| a.3.total_cmp(&b.3));
    report(&improvements, &fresh, &not_run, baseline.entries.len());

    if !regressions.is_empty() {
        println!();
        println!("REGRESSIONS (slower by more than the group's noise band):");
        regressions.sort_by(|a, b| b.3.total_cmp(&a.3));
        for (id, old, now, delta, band) in &regressions {
            println!(
                "  {:>+7.1}%  {id}  {} -> {}   (band +{:.1}%)",
                delta * 100.0,
                human(*old),
                human(*now),
                band * 100.0
            );
        }
        println!();
        println!(
            "The ratchet only tightens. If a regression is a deliberate trade — a\n\
             correctness fix that costs time — say so in docs/status/M12.md and\n\
             raise the number in benches/baseline.json in the same commit, so the\n\
             next reader sees a decision rather than a drift."
        );
        std::process::exit(1);
    }

    println!();
    if updating {
        Some(updated)
    } else {
        println!("ratchet: no regressions.");
        None
    }
}

/// Print the improvement, new-benchmark and not-run sections.
///
/// Split out of [`check`] so that function stays about the *decision* — what
/// counts as a regression — rather than about formatting three lists.
fn report(
    improvements: &[(String, f64, f64, f64)],
    fresh: &[(String, f64)],
    not_run: &[&String],
    baseline_len: usize,
) {
    if !improvements.is_empty() {
        println!();
        println!("improvements:");

        for (id, old, now, delta) in improvements {
            println!(
                "  {:>7.1}%  {id}  {} -> {}",
                delta * 100.0,
                human(*old),
                human(*now)
            );
        }
    }

    if !fresh.is_empty() {
        println!();
        println!("new (no committed number; `update` will record them):");
        for (id, now) in fresh {
            println!("  {:>8}  {id}", human(*now));
        }
    }

    if !not_run.is_empty() {
        println!();
        println!(
            "not run ({} of {}): a filtered bench run does not prove the rest still pass",
            not_run.len(),
            baseline_len
        );
        for id in not_run.iter().take(10) {
            println!("  {id}");
        }
        if not_run.len() > 10 {
            println!("  ... and {} more", not_run.len() - 10);
        }
    }
}

/// Format nanoseconds the way criterion does.
fn human(ns: f64) -> String {
    if ns >= 1_000_000.0 {
        format!("{:.3} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.3} us", ns / 1_000.0)
    } else {
        format!("{ns:.1} ns")
    }
}

impl Baseline {
    /// The noise band for a benchmark, from its group.
    fn band_for(&self, id: &str) -> f64 {
        let group = id.split('/').next().unwrap_or(id);
        self.bands.get(group).copied().unwrap_or(DEFAULT_BAND)
    }
}

/// Where the workspace root is, relative to this binary's working directory.
///
/// `cargo run -p pdfrum-bench` runs from the workspace root and `cargo bench`
/// from `benches/`, so both spellings have to work.
fn workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join("benches/baseline.json").exists() || cwd.join("Cargo.lock").exists() {
        return cwd;
    }
    cwd.parent().map_or(cwd.clone(), Path::to_path_buf)
}

/// Where cargo put `criterion/`.
///
/// `CARGO_TARGET_DIR` is honoured because this workspace uses one, and a
/// ratchet that looked only in `./target` would silently find nothing and
/// report a vacuous pass.
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace_root().join("target"), PathBuf::from)
}

/// Read every `new/estimates.json` under `criterion/`, keyed by benchmark id.
///
/// Criterion stores a benchmark at `criterion/<group>/<id>/new/estimates.json`
/// with `/` in the id replaced by `_`. The id is reconstructed as
/// `<group>/<id-with-underscores>`, which is stable across runs and is what the
/// baseline keys on — the round trip back to slashes is ambiguous (a stem may
/// contain an underscore) and unnecessary.
fn read_criterion(dir: &Path) -> std::io::Result<BTreeMap<String, f64>> {
    let mut out = BTreeMap::new();
    for group in std::fs::read_dir(dir)? {
        let group = group?.path();
        if !group.is_dir() {
            continue;
        }
        let Some(group_name) = group.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // criterion's own bookkeeping directory, not a benchmark group.
        if group_name == "report" {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&group) else {
            continue;
        };
        for bench in entries {
            let bench = bench?.path();
            let Some(bench_name) = bench.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if bench_name == "report" {
                continue;
            }
            let estimates = bench.join("new/estimates.json");
            if !estimates.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&estimates)?;
            if let Some(median) = median_ns(&text) {
                out.insert(format!("{group_name}/{bench_name}"), median);
            }
        }
    }
    Ok(out)
}

/// Pull `median.point_estimate` out of a criterion `estimates.json`.
///
/// Hand-parsed rather than pulled through a JSON crate, for the reason
/// `conformance/src/json.rs` gives about the scoreboard and DEPS.md gives about
/// SSIM: this file is the project's performance fitness function, and its
/// numbers must not shift under a dependency update. The schema is criterion's
/// and it is two levels deep, so a scan for the key and then for the next
/// number after it is the whole parser.
fn median_ns(text: &str) -> Option<f64> {
    let at = text.find("\"median\"")?;
    let rest = text.get(at..)?;
    let at = rest.find("\"point_estimate\"")?;
    let rest = rest.get(at + "\"point_estimate\"".len()..)?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == '-' || c == '+'))
        .unwrap_or(rest.len());
    rest.get(..end)?.parse().ok()
}

/// Read `baseline.json`.
///
/// Absent is not an error: it is the state before the baseline is established,
/// and `init` is how it stops being that.
fn read_baseline(path: &Path) -> std::io::Result<Baseline> {
    if !path.exists() {
        return Ok(Baseline::default());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(Baseline {
        bands: parse_section(&text, "\"bands\""),
        entries: parse_section(&text, "\"benchmarks\"")
            .into_iter()
            .map(|(k, v)| (k, Entry { median_ns: v }))
            .collect(),
    })
}

/// Pull one flat `{"key": number, ...}` object out of the baseline.
///
/// Same hand-parsing rationale as [`median_ns`], over a schema this file owns
/// and this binary is the only writer of.
fn parse_section(text: &str, key: &str) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let Some(at) = text.find(key) else {
        return out;
    };
    let Some(rest) = text.get(at + key.len()..) else {
        return out;
    };
    let Some(open) = rest.find('{') else {
        return out;
    };
    let Some(body) = rest.get(open + 1..) else {
        return out;
    };
    let Some(close) = body.find('}') else {
        return out;
    };
    let Some(body) = body.get(..close) else {
        return out;
    };
    for pair in body.split(',') {
        let Some((name, value)) = pair.split_once(':') else {
            continue;
        };
        let name = name.trim().trim_matches('"');
        if name.is_empty() {
            continue;
        }
        if let Ok(value) = value.trim().parse::<f64>() {
            out.insert(name.to_owned(), value);
        }
    }
    out
}

/// Write `baseline.json`.
fn write_baseline(path: &Path, entries: &BTreeMap<String, Entry>, bands: &BTreeMap<String, f64>) {
    let mut out = String::new();
    // A JSON array of lines rather than one string with escaped newlines: a
    // raw newline inside a JSON string is invalid, and an escaped one makes an
    // unreadable single line in the committed file. An array reads correctly
    // in a diff and parses.
    out.push_str(
        "{\n  \"_\": [\n\
         \x20   \"The M12 performance ratchet. Medians in nanoseconds, taken\",\n\
         \x20   \"from criterion's own estimates.json. Regenerate with:\",\n\
         \x20   \"\",\n\
         \x20   \"  cargo bench --workspace\",\n\
         \x20   \"  cargo run --release -p pdfrum-bench --bin ratchet -- update\",\n\
         \x20   \"\",\n\
         \x20   \"Edit a number by hand only to record a deliberate trade, and\",\n\
         \x20   \"say so in docs/status/M12.md in the same commit. The bands\",\n\
         \x20   \"below are measured, not chosen; see that document's section\",\n\
         \x20   \"on the noise band before widening one.\"\n\
         \x20 ],\n",
    );

    let bands = if bands.is_empty() {
        default_bands()
    } else {
        bands.clone()
    };
    out.push_str("  \"bands\": {\n");
    let mut first = true;
    for (group, band) in &bands {
        if !first {
            out.push_str(",\n");
        }
        first = false;
        let _ = write!(out, "    \"{group}\": {band}");
    }
    out.push_str("\n  },\n");

    out.push_str("  \"benchmarks\": {\n");
    let mut first = true;
    for (id, entry) in entries {
        if !first {
            out.push_str(",\n");
        }
        first = false;
        let _ = write!(out, "    \"{id}\": {:.1}", entry.median_ns);
    }
    out.push_str("\n  }\n}\n");

    match std::fs::write(path, &out) {
        Ok(()) => println!(
            "ratchet: wrote {} entries to {}",
            entries.len(),
            path.display()
        ),
        Err(err) => {
            eprintln!("cannot write {}: {err}", path.display());
            std::process::exit(2);
        }
    }
}

/// The bands used when the baseline names none.
///
/// These are the measured ones — see `docs/status/M12.md` §"The noise band" for
/// the runs behind each number. They are written into the file on `init` so
/// that the committed baseline is self-describing rather than depending on this
/// binary's defaults.
fn default_bands() -> BTreeMap<String, f64> {
    [
        ("open", 0.08),
        ("build", 0.04),
        ("render-cold-agg", 0.03),
        ("render-cold-tinyskia", 0.03),
        ("render-cold-vello", 0.04),
        // The warm groups run at 20 samples where the cold ones run at 50, and
        // they measure a quantity that is often an order of magnitude smaller —
        // so the *absolute* interval is tighter and the *fractional* one is
        // wider. Both effects are real and they do not cancel; the bands are
        // re-measured at the warm counts rather than inherited from the cold
        // ones, and docs/status/M12.md §2 carries the distribution.
        ("render-warm-agg", 0.04),
        ("render-warm-tinyskia", 0.04),
        ("render-warm-vello", 0.05),
        ("text", 0.04),
        ("save", 0.05),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}
