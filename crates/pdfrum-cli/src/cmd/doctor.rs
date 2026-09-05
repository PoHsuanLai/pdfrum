//! `pdfrum doctor`: what the parser recovered or dropped, without saving.

use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use pdfrum::{Diagnostic, Diagnostics, Severity};
use serde::Serialize;

use crate::out;
use crate::out::{Align, Table};
use crate::term::{Style, Term};

#[derive(Serialize)]
struct Report {
    file: String,
    xref_rebuilt: bool,
    recovered: usize,
    suspicious: usize,
    /// Notices the parser had no room to keep, when the file had more than
    /// its limit.
    dropped: usize,
    notices: Vec<Notice>,
}

#[derive(Serialize)]
struct Notice {
    severity: &'static str,
    offset: Option<u64>,
    what: String,
}

pub fn run(
    file: &Path,
    password: Option<&str>,
    json: bool,
    strict: bool,
    scan_all: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open_quietly(file, password)?;
    if scan_all {
        // A page's own notices surface when it is built; text extraction
        // builds every page and touches every font without rasterizing.
        for page in doc.pages() {
            drop(page.text());
        }
    }
    let diags = doc.all_diagnostics();
    let report = report(&diags, &doc, file);
    if json {
        out::json(&report)?;
    } else {
        print(&report, term);
    }
    Ok(if strict && !diags.is_empty() {
        ExitCode::from(3)
    } else {
        ExitCode::SUCCESS
    })
}

fn report(diags: &Diagnostics, doc: &pdfrum::Document, file: &Path) -> Report {
    let mut entries: Vec<&Diagnostic> = diags.entries().iter().collect();
    // Offset order tells the story of the file front to back; the notices
    // without an offset are about the document as a whole and come first.
    entries.sort_by_key(|d| d.at);
    let notices = entries
        .iter()
        .map(|d| Notice {
            severity: severity_name(d.severity),
            offset: d.at,
            what: format!("{:?}", d.what),
        })
        .collect();
    Report {
        file: file.display().to_string(),
        xref_rebuilt: doc.xref_was_rebuilt(),
        recovered: entries
            .iter()
            .filter(|d| d.severity == Severity::Recovered)
            .count(),
        suspicious: entries
            .iter()
            .filter(|d| d.severity == Severity::Suspicious)
            .count(),
        dropped: diags.dropped(),
        notices,
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Recovered => "recovered",
        Severity::Suspicious => "suspicious",
    }
}

fn print(r: &Report, term: Term) {
    let state = if r.notices.is_empty() && r.dropped == 0 {
        term.paint(Style::Ok, "clean")
    } else {
        let mut text = format!("{} recovered, {} suspicious", r.recovered, r.suspicious);
        if r.dropped > 0 {
            let _ = write!(text, ", {} more not kept", r.dropped);
        }
        term.paint(Style::Warn, &text)
    };
    let mut rows = vec![("file", Some(r.file.clone())), ("state", Some(state))];
    if r.xref_rebuilt {
        rows.push((
            "xref",
            Some(term.paint(Style::Warn, "rebuilt by scanning the file")),
        ));
    }
    out::record(term, &rows);
    if r.notices.is_empty() {
        return;
    }
    let mut table = Table::new(&[
        ("SEVERITY", Align::Left),
        ("OFFSET", Align::Right),
        ("WHAT", Align::Left),
    ]);
    for n in &r.notices {
        let severity = match n.severity {
            "suspicious" => term.paint(Style::Warn, n.severity),
            other => other.to_owned(),
        };
        table.row(vec![
            severity,
            n.offset.map_or(String::new(), |o| format!("@{o}")),
            n.what.clone(),
        ]);
    }
    table.print(term, 0);
}
