//! `pdfrum scripts run`: the document's own JavaScript, run the way a
//! viewer runs it on open, and what it said.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pdfrum::{Diagnostics, FormSession, ScriptConfig};
use serde::Serialize;

use crate::out;
use crate::out::{out, outln};
use crate::term::{Style, Term};

/// One line of the transcript, for `--json`.
#[derive(Serialize)]
struct Line {
    line: String,
}

/// Run the open-time scripts, then open and close every page as a viewer
/// does — which runs each page's own actions and the fields' formatters —
/// and print the transcript: every `app.alert`, `console.println` and the
/// like, one per line, as the document produced them. A stream: nothing
/// is added to it. Scripts that threw are reported on stderr and do not
/// change the exit code; a document is answered even when its script is
/// broken.
///
/// `time` freezes the scripts' clock at that many seconds since the epoch,
/// so `Date` and `util.printd` give the same answer every run.
pub fn run(
    file: &Path,
    password: Option<&str>,
    time: Option<u64>,
    json: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let config = match time {
        Some(seconds) => ScriptConfig::frozen_at(seconds),
        None => ScriptConfig::wall_clock(),
    };
    let mut session = FormSession::with_scripts(&doc, &config)
        .with_context(|| format!("cannot start the script engine for {}", file.display()))?;
    session.open_document();
    for page in 0..doc.page_count() {
        session.load_page(page);
        session.page_opened(page);
        session.honour_focus_requests();
        session.page_closed(page);
    }
    let transcript = session
        .scripts()
        .map(pdfrum::ScriptCascade::transcript_text)
        .unwrap_or_default();
    let failures = session.script_failures(&mut Diagnostics::default());
    for failure in &failures {
        eprintln!("pdfrum: {}", failure.line());
    }
    if json {
        let lines: Vec<Line> = transcript
            .lines()
            .map(|l| Line { line: l.to_owned() })
            .collect();
        out::json(&lines)?;
    } else if transcript.is_empty() {
        if failures.is_empty() {
            out::none("script output");
        }
    } else if term.color {
        // The same bytes a pipe gets, with the line's kind — `Alert`,
        // `Print`, the console's — painted the way `search` paints its
        // page prefix.
        for line in transcript.lines() {
            match line.split_once(": ") {
                Some((kind, rest)) => outln!("{}: {rest}", term.paint(Style::Key, kind)),
                None => outln!("{line}"),
            }
        }
    } else {
        out!("{transcript}");
    }
    Ok(ExitCode::SUCCESS)
}
