//! `pdfrum doctor`: what the parser recovered or dropped, without saving.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use pdfrum::{Diagnostic, Diagnostics, Severity};
use serde::Serialize;

use crate::out::{self, outln};

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
        print(&report);
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

fn print(r: &Report) {
    if r.notices.is_empty() && r.dropped == 0 {
        outln!("{}: clean — nothing to recover", r.file);
        return;
    }
    outln!(
        "{}: {} recovered, {} suspicious{}{}",
        r.file,
        r.recovered,
        r.suspicious,
        if r.dropped > 0 {
            format!(", {} more not kept", r.dropped)
        } else {
            String::new()
        },
        if r.xref_rebuilt {
            "; cross-reference table rebuilt"
        } else {
            ""
        },
    );
    for n in &r.notices {
        let offset = n
            .at_text()
            .map_or_else(|| "        -".to_owned(), |o| format!("{o:>9}"));
        outln!("  {:<10} {offset}  {}", n.severity, n.what);
    }
}

impl Notice {
    fn at_text(&self) -> Option<String> {
        self.offset.map(|o| format!("@{o}"))
    }
}
