//! `pdfrum doctor`: what the parser recovered or dropped, without saving.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use pdfrum::{Diagnostic, Document, Severity};
use serde::Serialize;

use crate::out;
use crate::out::{Align, Table, outln};
use crate::term::{Style, Term};

#[derive(Serialize)]
pub struct Report {
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
    /// The variant identifier, which `schema doctor` publishes and the
    /// table prints.
    what: String,
}

pub fn run(
    files: &[PathBuf],
    password: Option<&str>,
    json: bool,
    strict: bool,
    scan_all: bool,
    term: Term,
) -> Result<ExitCode> {
    let (reports, failed) = out::per_file(files, term, |file| {
        let doc = out::open_quietly(file, password)?;
        Ok(report(&doc, file, scan_all))
    });
    let found = reports.iter().any(|r| !r.notices.is_empty());
    if json {
        out::documents(files, &reports)?;
    } else {
        for (i, report) in reports.iter().enumerate() {
            if i > 0 {
                outln!();
            }
            print(report, term);
        }
    }
    Ok(out::exit(
        failed,
        if strict && found {
            ExitCode::from(3)
        } else {
            ExitCode::SUCCESS
        },
    ))
}

/// What the parser recorded for `doc`; `scan_all` builds every page first,
/// so what only a page load would find is found now.
pub fn report(doc: &Document, file: &Path, scan_all: bool) -> Report {
    if scan_all {
        // A page's own notices surface when it is built; text extraction
        // builds every page and touches every font without rasterizing.
        for page in doc.pages() {
            drop(page.text());
        }
    }
    let diags = doc.all_diagnostics();
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
            "table",
            Some(term.paint(Style::Warn, "rebuilt by scanning the file")),
        ));
    }
    out::record(term, &rows);
    if r.notices.is_empty() {
        return;
    }
    let mut table = Table::new(&[
        ("SEVERITY", Align::Left),
        ("AT", Align::Right),
        ("WHAT", Align::Left),
    ]);
    for n in &r.notices {
        let severity = match n.severity {
            "suspicious" => term.paint(Style::Warn, n.severity),
            other => other.to_owned(),
        };
        table.row(vec![
            severity,
            n.offset.map_or(String::new(), |o| format!("byte {o}")),
            n.what.clone(),
        ]);
    }
    table.print(term, 0);
}
