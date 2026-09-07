//! The tables, generated from the JSON and nothing else, so the README's
//! numbers can always be regenerated with `compare report <json>`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::child::Status;
use crate::model::{Op, median, percentile};
use crate::run::{Row, RunJson};

/// SSIM buckets, in table order. The first is the conformance floor.
const SSIM_BUCKETS: [(&str, f64, f64); 4] = [
    (">= 0.99", 0.99, f64::INFINITY),
    ("0.95-0.99", 0.95, 0.99),
    ("0.80-0.95", 0.80, 0.95),
    ("< 0.80", f64::NEG_INFINITY, 0.80),
];

/// A file where a peer came closer to the oracle than pdfrum did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Loss {
    pub file: String,
    pub op: Op,
    pub engine: String,
    pub theirs: String,
    pub ours: String,
}

fn rows_for<'a>(json: &'a RunJson, engine: &str, op: Op) -> impl Iterator<Item = &'a Row> {
    json.rows
        .iter()
        .filter(move |row| row.engine == engine && row.op == op)
}

/// A Markdown table row.
fn row(out: &mut String, cells: &[String]) {
    let _ = writeln!(out, "| {} |", cells.join(" | "));
}

/// A Markdown header separator for `n` columns.
fn separator(out: &mut String, n: usize) {
    let _ = writeln!(out, "|{}", "---|".repeat(n));
}

/// A row whose first cell after the name explains why the rest are blank.
fn blank_row(out: &mut String, n: usize, name: &str, reason: &str) {
    let mut cells = vec![name.to_owned(), reason.to_owned()];
    cells.resize(n, String::new());
    row(out, &cells);
}

fn fmt_ms(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |ms| format!("{ms:.2}"))
}

fn fmt_pct(num: usize, den: usize) -> String {
    if den == 0 {
        "-".to_owned()
    } else {
        format!("{num}/{den} ({:.0}%)", 100.0 * num as f64 / den as f64)
    }
}

/// Renders every table as Markdown.
pub fn render(json: &RunJson) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Run `{}` — profile `{}` — commit {} — {} files — {} DPI — timeout {} s — {} warm runs — generated {}",
        json.label,
        json.profile.name(),
        json.commit,
        json.corpus.files,
        json.dpi,
        json.timeout_secs,
        json.warm_runs,
        json.generated_at
    );
    let _ = writeln!(
        out,
        "Machine: {} ({} CPUs, {}). Load before: `{}`; after: `{}`.",
        json.machine.hostname,
        json.machine.cpus,
        json.machine.rustc,
        json.machine.uptime_before,
        json.machine.uptime_after
    );
    let _ = writeln!(
        out,
        "Corpus: `{}` — {}.",
        json.corpus.root, json.corpus.rule
    );
    let _ = writeln!(
        out,
        "Oracle: `{}` at {} with fonts `{}`.\n",
        json.oracle.binary,
        json.oracle.checkout_commit.as_deref().unwrap_or("?"),
        json.oracle.font_dir
    );

    let _ = writeln!(out, "### Engines\n");
    let _ = writeln!(out, "| engine | version | C in build | ops | ran |");
    separator(&mut out, 5);
    for engine in &json.engines {
        let ops: Vec<&str> = engine.ops.iter().map(|op| op.name()).collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            engine.name,
            engine.version,
            if engine.c_in_build { "yes" } else { "no" },
            ops.join(", "),
            if engine.ran {
                "yes".to_owned()
            } else {
                engine.reason.clone().unwrap_or_default()
            }
        );
    }

    work_matrix(&mut out, json);

    let _ = writeln!(
        out,
        "\n### Correctness — render, page 1 at {} DPI against `pdfium_test --png`\n",
        json.dpi
    );
    let render_columns = 10 + SSIM_BUCKETS.len();
    let _ = writeln!(
        out,
        "| engine | rendered | exact | {} | size mismatch | error | panic | crash | timeout | median SSIM | mean differing px |",
        SSIM_BUCKETS
            .iter()
            .map(|b| b.0)
            .collect::<Vec<_>>()
            .join(" | ")
    );
    separator(&mut out, render_columns);
    for engine in &json.engines {
        if !engine.ops.contains(&Op::Render) {
            blank_row(&mut out, render_columns, &engine.name, "not supported");
            continue;
        }
        if !engine.ran {
            blank_row(
                &mut out,
                render_columns,
                &engine.name,
                engine.reason.as_deref().unwrap_or("not run"),
            );
            continue;
        }
        let rows: Vec<&Row> = rows_for(json, &engine.name, Op::Render).collect();
        let total = rows.len();
        let compared: Vec<&Row> = rows
            .iter()
            .copied()
            .filter(|row| row.render.is_some())
            .collect();
        let exact = compared
            .iter()
            .filter(|row| row.render.is_some_and(|d| d.exact))
            .count();
        let ssims: Vec<f64> = compared
            .iter()
            .filter_map(|row| row.render.map(|d| d.ssim))
            .collect();
        let diff_pct: Vec<f64> = compared
            .iter()
            .filter_map(|row| row.render.map(|d| d.differing_pct))
            .collect();
        let buckets: Vec<String> = SSIM_BUCKETS
            .iter()
            .map(|(_, lo, hi)| {
                ssims
                    .iter()
                    .filter(|s| **s >= *lo && **s < *hi)
                    .count()
                    .to_string()
            })
            .collect();
        let count = |status: Status| rows.iter().filter(|row| row.status == status).count();
        let mismatch = rows.iter().filter(|row| row.render_error.is_some()).count();
        let mean_diff = if diff_pct.is_empty() {
            0.0
        } else {
            diff_pct.iter().sum::<f64>() / diff_pct.len() as f64
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.2}% |",
            engine.name,
            fmt_pct(compared.len(), total),
            exact,
            buckets.join(" | "),
            mismatch,
            count(Status::Error),
            count(Status::Panic),
            count(Status::Crash),
            count(Status::Timeout),
            median(&ssims).map_or_else(|| "-".to_owned(), |s| format!("{s:.4}")),
            mean_diff
        );
    }

    let _ = writeln!(
        out,
        "\n### Correctness — text, page 1 against `pdfium_test --txt`\n"
    );
    let _ = writeln!(
        out,
        "| engine | extracted | exact | whitespace-normalized | token F1 >= 0.9 | median token F1 | error | panic | crash | timeout |"
    );
    separator(&mut out, 10);
    for engine in &json.engines {
        if !engine.ops.contains(&Op::Text) {
            blank_row(&mut out, 10, &engine.name, "not supported");
            continue;
        }
        if !engine.ran {
            blank_row(
                &mut out,
                10,
                &engine.name,
                engine.reason.as_deref().unwrap_or("not run"),
            );
            continue;
        }
        let rows: Vec<&Row> = rows_for(json, &engine.name, Op::Text).collect();
        let total = rows.len();
        let compared: Vec<&Row> = rows
            .iter()
            .copied()
            .filter(|row| row.text.is_some())
            .collect();
        let exact = compared
            .iter()
            .filter(|row| row.text.is_some_and(|d| d.exact))
            .count();
        let normalized = compared
            .iter()
            .filter(|row| row.text.is_some_and(|d| d.normalized))
            .count();
        let f1s: Vec<f64> = compared
            .iter()
            .filter_map(|row| row.text.map(|d| d.token_f1))
            .collect();
        let good = f1s.iter().filter(|f| **f >= 0.9).count();
        let count = |status: Status| rows.iter().filter(|row| row.status == status).count();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            engine.name,
            fmt_pct(compared.len(), total),
            fmt_pct(exact, compared.len()),
            fmt_pct(normalized, compared.len()),
            fmt_pct(good, compared.len()),
            median(&f1s).map_or_else(|| "-".to_owned(), |f| format!("{f:.3}")),
            count(Status::Error),
            count(Status::Panic),
            count(Status::Crash),
            count(Status::Timeout)
        );
    }

    let _ = writeln!(out, "\n### Robustness — open\n");
    let _ = writeln!(out, "| engine | opened | error | panic | crash | timeout |");
    separator(&mut out, 6);
    for engine in &json.engines {
        if !engine.ops.contains(&Op::Open) {
            blank_row(&mut out, 6, &engine.name, "not supported");
            continue;
        }
        if !engine.ran {
            blank_row(
                &mut out,
                6,
                &engine.name,
                engine.reason.as_deref().unwrap_or("not run"),
            );
            continue;
        }
        let rows: Vec<&Row> = rows_for(json, &engine.name, Op::Open).collect();
        let count = |status: Status| rows.iter().filter(|row| row.status == status).count();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            engine.name,
            fmt_pct(count(Status::Ok), rows.len()),
            count(Status::Error),
            count(Status::Panic),
            count(Status::Crash),
            count(Status::Timeout)
        );
    }

    let _ = writeln!(
        out,
        "\n### Speed — milliseconds, warm median per file, then median and p95 across files (successful rows only)\n"
    );
    let _ = writeln!(
        out,
        "| engine | op | files | cold median | warm median | warm p95 | warm max |"
    );
    separator(&mut out, 7);
    for engine in &json.engines {
        for op in Op::ALL {
            if !engine.ops.contains(&op) {
                row(
                    &mut out,
                    &[
                        engine.name.clone(),
                        op.name().to_owned(),
                        "not supported".to_owned(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    ],
                );
                continue;
            }
            if !engine.ran {
                row(
                    &mut out,
                    &[
                        engine.name.clone(),
                        op.name().to_owned(),
                        engine
                            .reason
                            .clone()
                            .unwrap_or_else(|| "not run".to_owned()),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    ],
                );
                continue;
            }
            let ok: Vec<&Row> = rows_for(json, &engine.name, op)
                .filter(|row| row.status == Status::Ok)
                .collect();
            let cold: Vec<f64> = ok.iter().filter_map(|row| row.cold_ms).collect();
            let warm: Vec<f64> = ok.iter().filter_map(|row| row.warm_ms).collect();
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} |",
                engine.name,
                op.name(),
                ok.len(),
                fmt_ms(median(&cold)),
                fmt_ms(median(&warm)),
                fmt_ms(percentile(&warm, 95.0)),
                fmt_ms(warm.iter().copied().reduce(f64::max))
            );
        }
    }

    let _ = writeln!(
        out,
        "\n### Memory — peak RSS of the child process (VmHWM), MiB, over successful rows; the process floor before the engine ran is subtracted\n"
    );
    let _ = writeln!(out, "| engine | op | files | median | p95 | max | max on |");
    separator(&mut out, 7);
    for engine in &json.engines {
        if !engine.ran {
            continue;
        }
        for op in Op::ALL {
            if !engine.ops.contains(&op) {
                continue;
            }
            let ok: Vec<&Row> = rows_for(json, &engine.name, op)
                .filter(|row| row.status == Status::Ok)
                .collect();
            let mib = |row: &Row| {
                row.vm_hwm_kb
                    .map(|kb| kb.saturating_sub(row.baseline_hwm_kb.unwrap_or(0)) as f64 / 1024.0)
            };
            let values: Vec<f64> = ok.iter().filter_map(|row| mib(row)).collect();
            let max_on = ok
                .iter()
                .filter_map(|row| mib(row).map(|m| (m, row.file.as_str())))
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map_or("-", |(_, file)| file);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} |",
                engine.name,
                op.name(),
                ok.len(),
                fmt_ms(median(&values)),
                fmt_ms(percentile(&values, 95.0)),
                fmt_ms(values.iter().copied().reduce(f64::max)),
                max_on
            );
        }
    }

    let losses = losses(json);
    let _ = writeln!(
        out,
        "\n### Losses — files where a peer is closer to the oracle than pdfrum ({})\n",
        losses.len()
    );
    if losses.is_empty() {
        let _ = writeln!(out, "None.");
    } else {
        let mut by_engine: BTreeMap<(&str, Op), usize> = BTreeMap::new();
        for loss in &losses {
            *by_engine
                .entry((loss.engine.as_str(), loss.op))
                .or_default() += 1;
        }
        let _ = writeln!(out, "| peer | op | files where the peer is closer |");
        separator(&mut out, 3);
        for ((engine, op), n) in &by_engine {
            let _ = writeln!(out, "| {engine} | {} | {n} |", op.name());
        }
        let _ = writeln!(
            out,
            "\n| file | op | peer | peer's score | pdfrum's score |"
        );
        separator(&mut out, 5);
        for loss in &losses {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                loss.file,
                loss.op.name(),
                loss.engine,
                loss.theirs,
                loss.ours
            );
        }
    }

    if !json.features.is_empty() {
        let _ = writeln!(
            out,
            "\n### Coverage — one corpus file per feature; a cell is what the engine did with that file\n"
        );
        let engines: Vec<&str> = json.engines.iter().map(|e| e.name.as_str()).collect();
        let _ = writeln!(out, "| feature | file | {} |", engines.join(" | "));
        separator(&mut out, 2 + engines.len());
        for feature in &json.features {
            let cells: Vec<String> = engines
                .iter()
                .map(|engine| coverage_cell(json, engine, &feature.file))
                .collect();
            let _ = writeln!(
                out,
                "| {} | {} | {} |",
                feature.feature,
                feature.file,
                cells.join(" | ")
            );
        }
        let _ = writeln!(
            out,
            "\nCell grammar: `open`/`render`/`text` each as `ok` (with SSIM or token F1 against the oracle), `err`, `panic`, `crash`, `timeout`, `-` (not supported)."
        );
    }
    out
}

/// Two runs of the same corpus side by side: what the defaults cost and what
/// that buys, per engine and per file.
///
/// The pair is the finding. A parity number alone says only "we are
/// faster with things turned off", which nobody doubted; the gap and the
/// SSIM it costs are the finding. Every engine's rows appear in both
/// columns, so an engine the profile cannot move shows the same number
/// twice — which is itself the evidence that the profile did not silently
/// change it.
pub fn profiles(base: &RunJson, parity: &RunJson) -> anyhow::Result<String> {
    use crate::model::RenderProfile;
    if base.profile != RenderProfile::Default {
        anyhow::bail!(
            "the first JSON is a `{}` run, not a `default` one",
            base.profile.name()
        );
    }
    if parity.profile != RenderProfile::Parity {
        anyhow::bail!(
            "the second JSON is a `{}` run, not a `parity` one",
            parity.profile.name()
        );
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Profiles — `{}` (default, commit {}) against `{}` (parity, commit {}) — {} DPI\n",
        base.label, base.commit, parity.label, parity.commit, base.dpi
    );
    let _ = writeln!(
        out,
        "Load: default run `{}` → `{}`; parity run `{}` → `{}`.\n",
        base.machine.loadavg_before,
        base.machine.loadavg_after,
        parity.machine.loadavg_before,
        parity.machine.loadavg_after
    );

    work_matrix(&mut out, base);
    work_matrix(&mut out, parity);

    let _ = writeln!(
        out,
        "\n### Speed — median across files, milliseconds, default vs parity\n"
    );
    let _ = writeln!(
        out,
        "| engine | op | cold default | cold parity | cold delta | warm default | warm parity | warm delta |"
    );
    separator(&mut out, 8);
    for engine in &base.engines {
        if !engine.ran {
            continue;
        }
        for op in Op::ALL {
            if !engine.ops.contains(&op) {
                continue;
            }
            let times = |json: &RunJson, pick: fn(&Row) -> Option<f64>| -> Option<f64> {
                let values: Vec<f64> = rows_for(json, &engine.name, op)
                    .filter(|row| row.status == Status::Ok)
                    .filter_map(pick)
                    .collect();
                median(&values)
            };
            let cold_a = times(base, |row| row.cold_ms);
            let cold_b = times(parity, |row| row.cold_ms);
            let warm_a = times(base, |row| row.warm_ms);
            let warm_b = times(parity, |row| row.warm_ms);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                engine.name,
                op.name(),
                fmt_ms(cold_a),
                fmt_ms(cold_b),
                fmt_delta(cold_a, cold_b),
                fmt_ms(warm_a),
                fmt_ms(warm_b),
                fmt_delta(warm_a, warm_b)
            );
        }
    }

    let _ = writeln!(
        out,
        "\n### Fidelity — SSIM against the oracle, both profiles, files where parity changed the pixels\n"
    );
    let _ = writeln!(
        out,
        "| engine | file | SSIM default | SSIM parity | delta | differing px default | differing px parity |"
    );
    separator(&mut out, 7);
    let mut moved = 0usize;
    let mut same = 0usize;
    for engine in &base.engines {
        if !engine.ops.contains(&Op::Render) || !engine.ran {
            continue;
        }
        for row in rows_for(base, &engine.name, Op::Render) {
            let Some(ours) = row.render else { continue };
            let Some(theirs) = rows_for(parity, &engine.name, Op::Render)
                .find(|other| other.file == row.file)
                .and_then(|other| other.render)
            else {
                continue;
            };
            if (theirs.ssim - ours.ssim).abs() < 1e-6
                && (theirs.differing_pct - ours.differing_pct).abs() < 1e-6
            {
                same += 1;
                continue;
            }
            moved += 1;
            let _ = writeln!(
                out,
                "| {} | {} | {:.4} | {:.4} | {:+.4} | {:.2}% | {:.2}% |",
                engine.name,
                row.file,
                ours.ssim,
                theirs.ssim,
                theirs.ssim - ours.ssim,
                ours.differing_pct,
                theirs.differing_pct
            );
        }
    }
    if moved == 0 {
        let _ = writeln!(out, "| — | none | | | | | |");
    }
    let _ = writeln!(
        out,
        "\n{moved} compared files changed pixels under parity; {same} were byte-identical in both profiles."
    );
    Ok(out)
}

/// `parity - default` as a signed percentage of the default, or `-`.
fn fmt_delta(default: Option<f64>, parity: Option<f64>) -> String {
    match (default, parity) {
        (Some(a), Some(b)) if a > 0.0 => format!("{:+.1}%", 100.0 * (b - a) / a),
        _ => "-".to_owned(),
    }
}

/// What each render configuration computes — the table that makes an
/// asymmetry between two engines' settings visible instead of leaving it in
/// the `note` prose. One column per work item, one footnote per distinct
/// source, and `not determined` wherever the peer's source did not settle
/// the question.
fn work_matrix(out: &mut String, json: &RunJson) {
    let with_work: Vec<&crate::run::EngineRecord> = json
        .engines
        .iter()
        .filter(|engine| !engine.work.is_empty())
        .collect();
    if with_work.is_empty() {
        return;
    }
    let items: Vec<&str> = with_work[0].work.iter().map(|c| c.item.as_str()).collect();
    let _ = writeln!(
        out,
        "\n### What this configuration computes — profile `{}`\n",
        json.profile.name()
    );
    let _ = writeln!(out, "| engine | {} |", items.join(" | "));
    separator(out, 1 + items.len());
    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    for engine in &with_work {
        let cells: Vec<String> = engine
            .work
            .iter()
            .map(|cell| {
                let Some(source) = cell.source.as_ref() else {
                    return cell.answer.clone();
                };
                let next = sources.len() + 1;
                let mark = *sources.entry(source.clone()).or_insert(next);
                format!("{} [{mark}]", cell.answer)
            })
            .collect();
        let _ = writeln!(out, "| {} | {} |", engine.name, cells.join(" | "));
    }
    let mut ordered: Vec<(&String, &usize)> = sources.iter().collect();
    ordered.sort_by_key(|(_, mark)| **mark);
    let _ = writeln!(out);
    for (source, mark) in ordered {
        let _ = writeln!(out, "[{mark}] `{source}`");
    }
    let _ = writeln!(
        out,
        "\n`no knob` means the crate exposes no setting for that item, so the profile cannot move it; `not determined` means the crate's own source did not settle the question and nothing is guessed here."
    );
}

/// One coverage cell: the three ops, with the oracle score where there is one.
fn coverage_cell(json: &RunJson, engine: &str, file: &str) -> String {
    let mut parts = Vec::new();
    for op in Op::ALL {
        let Some(row) = json
            .rows
            .iter()
            .find(|row| row.engine == engine && row.file == file && row.op == op)
        else {
            continue;
        };
        let cell = match row.status {
            Status::Unsupported => "-".to_owned(),
            Status::Ok => match op {
                Op::Open => "ok".to_owned(),
                Op::Render => row.render.map_or_else(
                    || {
                        row.render_error
                            .as_ref()
                            .map_or_else(|| "ok(no oracle)".to_owned(), |_| "ok(size≠)".to_owned())
                    },
                    |d| format!("ok({:.3})", d.ssim),
                ),
                Op::Text => row.text.map_or_else(
                    || "ok(no oracle)".to_owned(),
                    |d| format!("ok({:.2})", d.token_f1),
                ),
            },
            Status::Error => "err".to_owned(),
            Status::Panic => "panic".to_owned(),
            Status::Crash => "crash".to_owned(),
            Status::Timeout => "timeout".to_owned(),
            Status::NotRun => "not-run".to_owned(),
        };
        parts.push(format!("{}={cell}", op.name()));
    }
    parts.join(" ")
}

/// Every (file, op) where some peer's score against the oracle beats pdfrum's.
pub fn losses(json: &RunJson) -> Vec<Loss> {
    let mut out = Vec::new();
    for row in json.rows.iter().filter(|row| row.engine == "pdfrum") {
        for peer in json
            .rows
            .iter()
            .filter(|r| r.engine != "pdfrum" && r.file == row.file && r.op == row.op)
        {
            match row.op {
                Op::Render => {
                    let ours = row.render.map(|d| d.ssim);
                    let theirs = peer.render.map(|d| d.ssim);
                    let (Some(theirs), ours) = (theirs, ours) else {
                        continue;
                    };
                    let beaten = ours.is_none_or(|ours| theirs > ours + 0.005);
                    if beaten {
                        out.push(Loss {
                            file: row.file.clone(),
                            op: row.op,
                            engine: peer.engine.clone(),
                            theirs: format!("SSIM {theirs:.4}"),
                            ours: ours.map_or_else(
                                || {
                                    format!(
                                        "{}{}",
                                        row.status.name(),
                                        row.render_error
                                            .as_deref()
                                            .map(|e| format!(" ({e})"))
                                            .unwrap_or_default()
                                    )
                                },
                                |s| format!("SSIM {s:.4}"),
                            ),
                        });
                    }
                }
                Op::Text => {
                    let ours = row.text;
                    let Some(theirs) = peer.text else {
                        continue;
                    };
                    let beaten = match ours {
                        None => true,
                        Some(ours) => {
                            (theirs.normalized && !ours.normalized)
                                || theirs.token_f1 > ours.token_f1 + 0.01
                        }
                    };
                    if beaten {
                        out.push(Loss {
                            file: row.file.clone(),
                            op: row.op,
                            engine: peer.engine.clone(),
                            theirs: format!(
                                "F1 {:.3}{}",
                                theirs.token_f1,
                                if theirs.normalized {
                                    " (normalized match)"
                                } else {
                                    ""
                                }
                            ),
                            ours: ours.map_or_else(
                                || row.status.name().to_owned(),
                                |o| {
                                    format!(
                                        "F1 {:.3}{}",
                                        o.token_f1,
                                        if o.normalized {
                                            " (normalized match)"
                                        } else {
                                            ""
                                        }
                                    )
                                },
                            ),
                        });
                    }
                }
                Op::Open => {
                    if peer.status == Status::Ok && row.status != Status::Ok {
                        out.push(Loss {
                            file: row.file.clone(),
                            op: row.op,
                            engine: peer.engine.clone(),
                            theirs: "opened".to_owned(),
                            ours: format!(
                                "{}{}",
                                row.status.name(),
                                row.detail
                                    .as_deref()
                                    .map(|d| format!(" ({d})"))
                                    .unwrap_or_default()
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}
