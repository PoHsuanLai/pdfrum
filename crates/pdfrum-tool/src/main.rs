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
//! `--txt` writes each page's characters as UTF-32LE to
//! `<input>.<page>.txt`, over `pdfrum-page` and `pdfrum-text`.
//!
//! `--png` renders each page to `<input>.<page>.png`, over `pdfrum-page`,
//! `pdfrum-render` and `pdfrum-raster-tinyskia`; with `--md5` it also prints
//! the `MD5:<path>:<hex>` line the oracle prints, hashed over the **raw
//! bitmap buffer** rather than over the PNG.
//!
//! `--show-structure` prints each page's tagged-PDF structure tree over
//! `pdfrum-doc`, which owns the emitter because its field order, indentation
//! and sorted attribute keys are behavior rather than presentation.
//!
//! `--annot` writes each page's annotation dump to
//! `<input>.<page>.annot.txt`, over `pdfrum-doc` for the format and the
//! appearance generation it describes, and `pdfrum-page` for the object
//! counts.
//!
//! The other raster targets and the save-*
//! family are **accepted and do nothing**, which is deliberate. The harness runs one
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

mod annot;
mod content;
mod metadata;
mod options;
mod pageinfo;
mod render;
mod run;
mod structure;
mod text;
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
    match format {
        OutputFormat::None
        | OutputFormat::PageInfo
        | OutputFormat::Structure
        | OutputFormat::Annot
        | OutputFormat::Text
        // `--png` is the one render format we produce; the rest are the
        // oracle's other raster and vector targets, which we do not.
        | OutputFormat::Render("png") => None,
        // These name the format rather than a crate: the renderer exists,
        // this particular output does not.
        OutputFormat::Render(extension) => {
            Some(format!("--{extension} rendering not implemented yet"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_implemented_formats_carry_no_note() {
        assert_eq!(unimplemented_format(OutputFormat::None), None);
        assert_eq!(unimplemented_format(OutputFormat::PageInfo), None);
        assert_eq!(unimplemented_format(OutputFormat::Text), None);
    }

    #[test]
    fn only_the_unproduced_raster_targets_carry_a_note() {
        // The two document dumps are implemented and must stay silent.
        assert_eq!(unimplemented_format(OutputFormat::Structure), None);
        assert_eq!(unimplemented_format(OutputFormat::Annot), None);
        // Every raster target but PNG, which we do produce.
        let note = unimplemented_format(OutputFormat::Render("ppm")).unwrap();
        assert!(
            note.contains("ppm") && note.contains("not implemented yet"),
            "{note}"
        );
    }

    #[test]
    fn the_probes_own_command_line_carries_no_note() {
        // `probe_tool` hands the tool `--show-pageinfo` and reads the answer;
        // a stub is diagnosed by the *absence* of that line rather than by
        // any wording. The formats the probe and the harness's own passes use
        // must therefore stay silent, or a working tool would start
        // announcing itself as unimplemented on every file.
        for format in [
            OutputFormat::PageInfo,
            OutputFormat::Text,
            OutputFormat::Render("png"),
        ] {
            assert_eq!(unimplemented_format(format), None, "{format:?}");
        }
    }
}
