//! `pdfrum schema`: the JSON shape a command's `--json` prints, so a
//! script or an agent can read the keys without running the command on a
//! document first.
//!
//! The shapes are the `serde::Serialize` structs of the commands, kept here
//! by hand as one example document each — every key with a representative
//! value, an optional key shown rather than left out. A unit test walks the
//! command tree and fails when a command has `--json` and no entry here,
//! and the CLI tests compare the examples' keys with what the commands
//! print on the fixtures, so an example cannot drift from its struct
//! unnoticed.

use std::process::ExitCode;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::out::{self, Align, Table};
use crate::term::{Style, Term};

/// One command that has `--json`, and what it prints.
pub struct Entry {
    /// The command as words: `extract words`.
    pub command: &'static str,
    /// One line, for the listing.
    pub description: &'static str,
    /// The example: an object for a command whose answer is one document,
    /// an array of one object for a list (which `--jsonl` prints one per
    /// line).
    pub example: fn() -> Value,
}

/// Every command with `--json`, in the order `pdfrum --help` lists them.
pub fn entries() -> Vec<Entry> {
    let mut all = document();
    all.extend(extract());
    all.extend(inspect());
    all.extend(whole_file());
    all
}

/// The commands that answer about a document.
fn document() -> Vec<Entry> {
    vec![
        Entry {
            command: "info",
            description: "the document summary; an array of them for several files",
            example: || {
                json!({
                    "file": "report.pdf",
                    "version": "1.7",
                    "pages": 12,
                    "encrypted": false,
                    "xref_rebuilt": false,
                    "permissions": {"print": true, "modify": true, "copy": true, "annotate": true, "fill_form": true},
                    "metadata": {
                        "title": "Quarterly report", "author": "A. Person", "subject": null, "keywords": null,
                        "creator": "Writer", "producer": "pdfrum", "creation_date": "D:20260905120000Z", "modification_date": null
                    },
                    "id": {"permanent": "1b4e28ba2fa1d5e6c7f8a9b0c1d2e3f4", "revision": "1b4e28ba2fa1d5e6c7f8a9b0c1d2e3f4", "pristine": true},
                    "outline_entries": 6,
                    "attachments": 0,
                    "signatures": [{"sub_filter": "ETSI.CAdES.detached", "reason": "approved", "time": "D:20200624093114+02'00'", "byte_range": [0, 10, 30, 10], "doc_mdp_permission": 0}],
                    "page_boxes": [{
                        "page": 1, "width": 612.0, "height": 792.0, "rotation": 0,
                        "media_box": [0.0, 0.0, 612.0, 792.0], "crop_box": [0.0, 0.0, 612.0, 792.0],
                        "bleed_box": [0.0, 0.0, 612.0, 792.0], "trim_box": [0.0, 0.0, 612.0, 792.0], "art_box": [0.0, 0.0, 612.0, 792.0]
                    }]
                })
            },
        },
        Entry {
            command: "doctor",
            description: "what the parser recovered or dropped; an array for several files",
            example: || {
                json!({
                    "file": "damaged.pdf", "xref_rebuilt": true, "recovered": 4, "suspicious": 0, "dropped": 0,
                    "notices": [{"severity": "recovered", "offset": 471, "what": "BadStartXref"}]
                })
            },
        },
        Entry {
            command: "search",
            description: "the hits; for several files an array of {file, hits}",
            example: || json!([{"page": 1, "start": 7, "end": 12, "line": "Hello, world!", "rects": [[52.92, 49.84, 80.55, 58.2]]}]),
        },
        Entry {
            command: "forms dump",
            description: "the form fields with their kinds and values",
            example: || json!([{"name": "Name", "kind": "text", "value": "A. Person", "checked": false, "options": ["yes", "no"], "read_only": false, "required": true, "tooltip": "Your name", "widgets": 1}]),
        },
    ]
}

/// `extract …`.
fn extract() -> Vec<Entry> {
    vec![
        Entry {
            command: "extract text",
            description: "the text of each page",
            example: || json!([{"page": 1, "text": "Hello, world!\n"}]),
        },
        Entry {
            command: "extract words",
            description: "every word with its box, font, size and character range",
            example: || json!([{"page": 1, "text": "Hello,", "x0": 20.23, "y0": 48.32, "x1": 49.0, "y1": 58.2, "font": "Times-Roman", "size": 12.0, "start": 0, "end": 6}]),
        },
        Entry {
            command: "extract markdown",
            description: "each page as Markdown",
            example: || json!([{"page": 1, "markdown": "# Title\n\nHello, world!\n"}]),
        },
        Entry {
            command: "extract links",
            description: "link annotations and URLs in the text",
            example: || json!([{"page": 1, "rect": [69.0, 338.0, 180.0, 358.0], "kind": "uri", "target_page": 2, "uri": "https://example.com/"}]),
        },
        Entry {
            command: "extract toc",
            description: "the outline, one row per bookmark",
            example: || json!([{"depth": 0, "title": "Introduction", "page": 2}]),
        },
        Entry {
            command: "extract attachments",
            description: "the embedded files",
            example: || json!([{"name": "data.csv", "size": 1024, "description": "the raw numbers", "subtype": "text/csv", "written": "attachments/data.csv"}]),
        },
        Entry {
            command: "extract annotations",
            description: "every annotation with its kind, box and text",
            example: || json!([{"page": 1, "subtype": "Highlight", "rect": [72.0, 700.0, 300.0, 712.0], "hidden": false, "name": "note-1", "title": "A. Reviewer", "contents": "Check this", "modified": "D:20260905120000Z"}]),
        },
        Entry {
            command: "extract signatures",
            description: "the signature fields",
            example: || json!([{"sub_filter": "ETSI.CAdES.detached", "reason": "approved", "time": "D:20200624093114+02'00'", "byte_range": [0, 10, 30, 10], "doc_mdp_permission": 0, "contents_len": 20}]),
        },
        Entry {
            command: "extract images",
            description: "one row per picture, with its size and format",
            example: || json!([{"page": 1, "index": 1, "object": 5, "uses": 1, "width": 50, "height": 50, "is_mask": false, "format": "png", "written": "images/report-1.png"}]),
        },
        Entry {
            command: "extract fonts",
            description: "the embedded font programs",
            example: || json!([{"name": "Helvetica", "kind": "truetype", "object": 8, "size": 512, "written": "fonts/Helvetica-8.ttf"}]),
        },
    ]
}

/// `inspect …`.
fn inspect() -> Vec<Entry> {
    vec![
        Entry {
            command: "inspect object",
            description: "one object in JSON; `inspect object --help` gives the encoding",
            example: || {
                json!({
                    "object": 1, "generation": 0,
                    "value": {"Type": {"name": "Catalog"}, "Pages": {"ref": [2, 0]}},
                    "hints": {"Pages": "Pages"}
                })
            },
        },
        Entry {
            command: "inspect xref",
            description: "the cross-reference table with its trailer; --jsonl the rows",
            example: || {
                json!({
                    "file": "report.pdf", "rebuilt": false, "entries": 2, "trailer": "<<\n  /Root 1 0 R\n  /Size 7\n>>",
                    "rows": [
                        {"object": 1, "generation": 0, "kind": "offset", "what": "Catalog", "offset": 15},
                        {"object": 2, "generation": 0, "kind": "in_stream", "what": "Pages", "stream": 5, "index": 0}
                    ]
                })
            },
        },
        Entry {
            command: "inspect revisions",
            description: "the incremental-update history",
            example: || json!([{"revision": 1, "xref_offset": 466, "xref_stream": false, "end": 633}]),
        },
        Entry {
            command: "inspect structure",
            description: "the structure tree, one row per element",
            example: || json!([{"page": 1, "depth": 1, "kind": "P", "content_ids": [0, 1], "alt": "a picture", "actual_text": "the words"}]),
        },
    ]
}

/// The commands about the file as a whole, and the scripts when built in.
fn whole_file() -> Vec<Entry> {
    #[cfg(feature = "javascript")]
    let scripts = Some(Entry {
        command: "scripts run",
        description: "what the document's scripts said, one line each",
        example: || json!([{"line": "Alert: hello"}]),
    });
    #[cfg(not(feature = "javascript"))]
    let scripts = None;
    vec![
        Entry {
            command: "diff",
            description: "what changed per page, and the pixels with --visual",
            example: || {
                json!({
                    "left": "v1.pdf", "right": "v2.pdf", "left_pages": 2, "right_pages": 2,
                    "pages": [{
                        "page": 1, "removed": ["old line"], "added": ["new line"],
                        "pixels": {"differing": 120, "total": 480_000, "bbox": [10, 20, 30, 40], "written": "diffs/diff-1.png"}
                    }]
                })
            },
        },
        Entry {
            command: "hash",
            description: "the three fingerprints; an array for several files",
            example: || {
                json!({
                    "file": "report.pdf", "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "id": ["1b4e28ba2fa1d5e6c7f8a9b0c1d2e3f4", "1b4e28ba2fa1d5e6c7f8a9b0c1d2e3f4"],
                    "semantic": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "objects": 7
                })
            },
        },
    ]
    .into_iter()
    .chain(scripts)
    .collect()
}

/// `pdfrum schema [COMMAND…]`: the example for one command, or the list of
/// commands that have one.
pub fn run(command: &[String], term: Term) -> Result<ExitCode> {
    let entries = entries();
    if command.is_empty() {
        let mut table = Table::new(&[("COMMAND", Align::Left), ("DESCRIPTION", Align::Left)]);
        for e in &entries {
            table.row(vec![
                term.paint(Style::Ident, e.command),
                e.description.to_owned(),
            ]);
        }
        table.print(term, 0);
        return Ok(ExitCode::SUCCESS);
    }
    let name = command.join(" ");
    let Some(entry) = entries.iter().find(|e| e.command == name) else {
        bail!("`{name}` has no --json output; `pdfrum schema` lists the commands that do");
    };
    out::json(&(entry.example)())?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::entries;

    /// Every command under `cmd` that has a `--json` flag, as words.
    fn json_commands(cmd: &clap::Command, prefix: &str, found: &mut Vec<String>) {
        for sub in cmd.get_subcommands() {
            let path = if prefix.is_empty() {
                sub.get_name().to_owned()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if sub.get_arguments().any(|a| a.get_long() == Some("json")) {
                found.push(path.clone());
            }
            json_commands(sub, &path, found);
        }
    }

    #[test]
    fn every_command_with_json_output_has_a_schema_and_nothing_else_does() {
        let mut with_json = Vec::new();
        json_commands(&crate::Cli::command(), "", &mut with_json);
        with_json.sort();
        let mut listed: Vec<String> = entries().iter().map(|e| e.command.to_owned()).collect();
        listed.sort();
        assert_eq!(with_json, listed);
        for e in entries() {
            let example = (e.example)();
            let object = example.as_array().map_or(&example, |a| &a[0]);
            assert!(
                object.is_object(),
                "{}: the example is not an object",
                e.command
            );
        }
    }
}
