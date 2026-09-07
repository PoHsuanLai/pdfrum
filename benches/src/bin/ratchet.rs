//! Compare a criterion run against `baseline.json`.
//!
//! ```text
//! cargo bench --workspace
//! cargo run --release -p pdfrum-bench --bin ratchet -- check
//! cargo run --release -p pdfrum-bench --bin ratchet -- update
//! ```
//!
//! Reads `target/criterion/**` by group name, not by crate. `cargo bench -p`
//! one crate leaves other groups stale on disk; `check` cannot see that.
//! Run `--workspace` first.
//!
//! Per benchmark, new median vs committed, outside that group's noise band:
//! slower → regression (`update` writes nothing unless `--accept-regressions`);
//! faster → improvement (`update` writes it); inside the band → keep the old
//! number, so the floor does not walk down with noise.
//!
//! Median, not mean: a wall-clock sample is bounded below by the real cost.
//! An idle machine. A filtered run that skips a group is not a pass.

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
            check(&baseline, &measured, Record::Nothing);
        }
        "update" => {
            // The flag is opt-in and position-free: `update --accept-regressions`
            // is the only spelling that records a regressed row, and a plain
            // `update` behaves exactly as it always has.
            let accepting = std::env::args().any(|a| a == "--accept-regressions");
            let record = if accepting {
                Record::RegressionsToo
            } else {
                Record::Improvements
            };
            if let Some(updated) = check(&baseline, &measured, record) {
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
            eprintln!("  update --accept-regressions");
            eprintln!("          the same, but also raise the regressed rows to what");
            eprintln!("          they measured -- for when every one has been");
            eprintln!("          attributed and the attribution is in the same commit");
            eprintln!("  init    overwrite the baseline with the current run wholesale;");
            eprintln!("          for establishing one, not for maintaining it");
            std::process::exit(2);
        }
    }
}

/// What a run is allowed to write.
///
/// Three states rather than a pair of bools, because only three are legal and
/// the illegal fourth — "report only, but record regressions" — should not be
/// spellable. The write path takes this value and reads the decision off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record {
    /// `check`: write nothing, whatever the comparison says.
    Nothing,
    /// `update`: write the improvements. A regression is still a failure and
    /// still writes nothing — this is the ratchet's default and its point.
    Improvements,
    /// `update --accept-regressions`: write the improvements *and* raise the
    /// regressed rows to what they measured.
    ///
    /// The operator is asserting that each regressed row has been attributed —
    /// a deliberate trade, or a baseline entry that was wrong — and that the
    /// attribution is written down in the same commit. The ratchet cannot tell
    /// a trade from a defect; only a person who has measured can, which is why
    /// this is an explicit flag and not a heuristic.
    RegressionsToo,
}

impl Record {
    /// Whether a regression should stop the run rather than be recorded.
    fn refuses_regressions(self) -> bool {
        !matches!(self, Self::RegressionsToo)
    }

    /// Whether anything is written at the end.
    fn writes(self) -> bool {
        !matches!(self, Self::Nothing)
    }
}

/// Compare, report, and exit non-zero on a regression.
///
/// Returns the entries an `update` should commit, or `None` when there is
/// nothing to write.
fn check(
    baseline: &Baseline,
    measured: &BTreeMap<String, f64>,
    record: Record,
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
        if record.refuses_regressions() {
            println!();
            println!(
                "The ratchet only tightens. Confirm this is an idle machine;\n\
                 on a loaded box these bands are noise.\n\
                 \n\
                 To accept the new numbers:\n\
                 \n\
                     cargo run --release -p pdfrum-bench --bin ratchet -- \\\n\
                         update --accept-regressions\n\
                 \n\
                 Do not edit benches/baseline.json by hand."
            );
            std::process::exit(1);
        }

        raise(&regressions, &mut updated);
    }

    println!();
    if record.writes() {
        Some(updated)
    } else {
        println!("ratchet: no regressions.");
        None
    }
}

/// Raise every regressed row to what it measured, and say so.
///
/// Only reached under [`Record::RegressionsToo`]. Split out of [`check`] for
/// the same reason [`report`] is: that function is about the decision, and
/// this is about carrying it out and printing what was done. Every row is
/// listed, because the point of the flag is that the reader of the commit can
/// see which numbers moved and by how much.
fn raise(regressions: &[(String, f64, f64, f64, f64)], updated: &mut BTreeMap<String, Entry>) {
    println!();
    println!("RAISING these rows, because --accept-regressions was given:");
    for (id, old, now, delta, _) in regressions {
        println!(
            "  {:>+7.1}%  {id}  {} -> {}",
            delta * 100.0,
            human(*old),
            human(*now)
        );
        updated.insert(id.clone(), Entry { median_ns: *now });
    }
    println!();
    println!(
        "{} rows raised. The commit that carries them must carry the\n\
         attribution too — otherwise the next reader sees a drift rather\n\
         than a decision.",
        regressions.len()
    );
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
/// `conformance/src/json.rs` gives about the scoreboard and gives about
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
         \x20   \"Performance ratchet. Medians in nanoseconds, taken from\",\n\
         \x20   \"criterion's estimates.json. Regenerate on the idle bench\",\n\
         \x20   \"box these numbers describe -- a run on a shared machine is\",\n\
         \x20   \"noise against the bands below:\",\n\
         \x20   \"\",\n\
         \x20   \"  cargo bench --workspace\",\n\
         \x20   \"  cargo run --release -p pdfrum-bench --bin ratchet -- update\",\n\
         \x20   \"\",\n\
         \x20   \"Edit a number by hand only to record a deliberate trade.\"\n\
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
/// These are the measured ones — see §"The noise band" for
/// the runs behind each number. They are written into the file on `init` so
/// that the committed baseline is self-describing rather than depending on this
/// binary's defaults.
fn default_bands() -> BTreeMap<String, f64> {
    [
        ("open", 0.08),
        ("build", 0.04),
        ("render-cold-agg", 0.03),
        ("render-cold-tinyskia", 0.03),
        ("render-cold-vello-cpu", 0.04),
        // The warm groups run at 20 samples where the cold ones run at 50, and
        // they measure a quantity that is often an order of magnitude smaller —
        // so the *absolute* interval is tighter and the *fractional* one is
        // wider. Both effects are real and they do not cancel; the bands are
        // re-measured at the warm counts rather than inherited from the cold
        // ones.
        ("render-warm-agg", 0.04),
        ("render-warm-tinyskia", 0.04),
        ("render-warm-vello-cpu", 0.05),
        ("text", 0.04),
        ("save", 0.05),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}
