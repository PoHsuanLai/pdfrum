//! CLI mirroring the C++ oracle's `pdfium_test` flags and output formats, so
//! the conformance harness diffs pdfrum against it like-for-like (SPEC.md §13).
//!
//! # What is implemented
//!
//! `--show-metadata` and `--show-pageinfo` produce byte-exact output over
//! `pdfrum-parser`, together with the `Unsupported feature:` notices that
//! share their stdout stream and the stderr chatter the harness reads a page
//! count out of. `--password=` and `--pages=` affect those dumps as they do in
//! the oracle.
//!
//! `--png`, `--txt`, `--annot`, `--show-structure` and the save-* family are
//! **accepted and do nothing**, which is deliberate. The harness runs one
//! fixed set of passes against every candidate; a tool that rejected the flags
//! for crates that do not exist yet would tag those tiers as a tool error
//! rather than as unimplemented, and would lose the page counts the same
//! invocation reports. Each unimplemented flag prints one line on stderr
//! naming the crate that will supply it.
//!
//! # Where the rest plugs in
//!
//! [`run::dump_page`] is the single dispatch point: a new output format is one
//! arm there plus a module beside [`pageinfo`], with everything else — the
//! page walk, the notices, the counts, the exit conventions — already agreeing
//! with the oracle. Formats that write files rather than stdout will need the
//! `<input>.<page>.<ext>` naming the harness harvests by.

#![forbid(unsafe_code)]

mod metadata;
mod options;
mod pageinfo;
mod run;
mod unsupported;

use std::io::Write;
use std::process::ExitCode;

use options::{OutputFormat, ParseError};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();

    let options = match options::parse(&args) {
        Ok(options) => options,
        Err(ParseError::Rejected(message)) => {
            let _ = writeln!(stderr, "{message}");
            let _ = write!(stderr, "{}", options::USAGE);
            return ExitCode::FAILURE;
        }
    };

    if options.files.is_empty() {
        let _ = writeln!(stderr, "No input files.");
        return ExitCode::FAILURE;
    }

    for flag in &options.unsupported {
        let _ = writeln!(stderr, "pdfrum-tool: {flag} accepted but not implemented");
    }
    if let Some(note) = unimplemented_format(options.format) {
        let _ = writeln!(stderr, "pdfrum-tool: {note}");
    }

    let mut streams = run::Streams {
        out: &mut stdout,
        err: &mut stderr,
    };
    for path in &options.files {
        let name = path.to_string_lossy().into_owned();
        // The oracle skips a file it cannot read without a word, because
        // `GetFileContents` prints its own message and returns nothing.
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if run::process_file(&name, bytes, &options, &mut streams).is_err() {
            // A closed pipe. Nothing useful is left to say on the stream that
            // just refused a write.
            return ExitCode::FAILURE;
        }
    }
    // `pdfium_test`'s main returns 0 whatever the files did: a load failure,
    // a bad page, and a clean render all exit the same way.
    ExitCode::SUCCESS
}

/// The note for an output format we accept but cannot produce yet.
fn unimplemented_format(format: OutputFormat) -> Option<String> {
    let (flag, crate_name) = match format {
        OutputFormat::None | OutputFormat::PageInfo => return None,
        OutputFormat::Structure => ("--show-structure", "pdfrum-doc"),
        OutputFormat::Render(_) => ("page rendering", "pdfrum-render"),
        OutputFormat::Text => ("--txt", "pdfrum-text"),
        OutputFormat::Annot => ("--annot", "pdfrum-doc"),
    };
    Some(format!("{flag} not implemented yet (awaits {crate_name})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_implemented_formats_carry_no_note() {
        assert_eq!(unimplemented_format(OutputFormat::None), None);
        assert_eq!(unimplemented_format(OutputFormat::PageInfo), None);
    }

    #[test]
    fn each_unimplemented_format_names_the_crate_that_will_supply_it() {
        for (format, crate_name) in [
            (OutputFormat::Text, "pdfrum-text"),
            (OutputFormat::Annot, "pdfrum-doc"),
            (OutputFormat::Structure, "pdfrum-doc"),
            (OutputFormat::Render("png"), "pdfrum-render"),
        ] {
            let note = unimplemented_format(format).unwrap();
            assert!(note.contains(crate_name), "{note}");
            assert!(note.contains("not implemented yet"), "{note}");
        }
    }

    #[test]
    fn the_notes_never_say_the_words_the_harness_probes_for() {
        // `probe_tool` reads "no functionality implemented" / "not
        // implemented" out of a bare run as the stub marker. Our notes say
        // "not implemented yet" about one flag, which would be read the same
        // way -- so they must never appear on the probe's own command line
        // (`--png --md5`, whose format note is the render one).
        //
        // This test pins the *shape*: the probe is a render run, so if the
        // render note ever stops naming a crate the harness would silently
        // start tagging every file `unsupported-tool` again.
        let note = unimplemented_format(OutputFormat::Render("png")).unwrap();
        assert!(note.contains("pdfrum-render"), "{note}");
    }
}
