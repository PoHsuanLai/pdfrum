//! The `pdfrum` command line.
//!
//! A client of the `pdfrum` facade and nothing below it: every fact a
//! command prints comes through the same API a `cargo add pdfrum` user has,
//! so a gap here is a gap there and is filled in the library first.
//!
//! Two conventions run through every command. Human output goes to stdout as
//! plain text; `--json` replaces it with one JSON document for scripts, with
//! `snake_case` keys that do not change between releases. Notices — a rebuilt
//! cross-reference table, a dropped object — go to stderr, so a pipe never
//! sees them. Exit codes: 0, 1 for an error, 2 for a usage mistake (clap's),
//! and 3 from `doctor --strict` when the parser had something to report.

mod cmd;
mod out;
mod pages;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

/// Inspect, render and extract from PDF files.
#[derive(Parser)]
#[command(name = "pdfrum", version, about, long_about = None)]
struct Cli {
    /// Password for an encrypted document. Either the user or the owner
    /// password opens it; the permissions reported are the ones it grants.
    #[arg(short, long, global = true, value_name = "PASSWORD")]
    password: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarize a document: pages, metadata, security, signatures, identity.
    Info {
        #[command(flatten)]
        input: Input,
        /// One JSON document instead of the text summary.
        #[arg(long)]
        json: bool,
    },
    /// Report what the parser had to recover or drop, without touching the file.
    Doctor {
        #[command(flatten)]
        input: Input,
        /// One JSON document instead of the text report.
        #[arg(long)]
        json: bool,
        /// Exit 3 when anything at all was recorded.
        #[arg(long)]
        strict: bool,
        /// Also build every page and extract its text, so what only a page
        /// load would find is found now.
        #[arg(long)]
        scan_all: bool,
    },
    /// Render pages to PNG files.
    Render {
        #[command(flatten)]
        input: Input,
        /// Output path; `{n}` is replaced by the 1-based page number, `{stem}`
        /// by the input's file name without its extension. `-` writes a single
        /// page's PNG to stdout.
        #[arg(short, long, value_name = "PATH", default_value = "{stem}-{n}.png")]
        output: String,
        /// Pages to render, 1-based: `3`, `1-5`, `2,7,10-end`. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// Resolution in dots per inch.
        #[arg(long, default_value_t = 150.0, conflicts_with = "scale")]
        dpi: f64,
        /// Scale factor instead of a resolution: 1.0 is one pixel per point.
        #[arg(long, value_name = "FACTOR")]
        scale: Option<f64>,
        /// Leave annotations and form fields out of the picture.
        #[arg(long)]
        no_annotations: bool,
    },
    /// Extract text, links, bookmarks, attachments, annotations or signatures.
    Extract {
        #[command(subcommand)]
        what: Extract,
    },
}

#[derive(Subcommand)]
enum Extract {
    /// The text of each page, in reading order.
    Text {
        #[command(flatten)]
        input: Input,
        /// Pages to extract, 1-based: `3`, `1-5`, `2,7,10-end`. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// One JSON document: an array of `{page, text}`.
        #[arg(long)]
        json: bool,
    },
    /// Link annotations with where each one leads, plus URLs found in the text.
    Links {
        #[command(flatten)]
        input: Input,
        /// Pages to scan, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// The outline (bookmarks) as an indented tree.
    Toc {
        #[command(flatten)]
        input: Input,
        /// One JSON document: an array of `{depth, title, page}`.
        #[arg(long)]
        json: bool,
    },
    /// Embedded files: list them, or write them into a directory.
    Attachments {
        #[command(flatten)]
        input: Input,
        /// Write every attachment into this directory.
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Annotations: highlights, notes, stamps, links, widgets.
    Annotations {
        #[command(flatten)]
        input: Input,
        /// Pages to scan, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Digital signature fields: filter, reason, time, byte range.
    Signatures {
        #[command(flatten)]
        input: Input,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
}

/// The document every command reads.
#[derive(Args)]
struct Input {
    /// The PDF file.
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let password = cli.password.as_deref();
    let outcome = match cli.command {
        Command::Info { input, json } => cmd::info::run(&input.file, password, json),
        Command::Doctor {
            input,
            json,
            strict,
            scan_all,
        } => cmd::doctor::run(&input.file, password, json, strict, scan_all),
        Command::Render {
            input,
            output,
            pages,
            dpi,
            scale,
            no_annotations,
        } => cmd::render::run(&cmd::render::Request {
            file: &input.file,
            password,
            output: &output,
            pages: pages.as_deref(),
            scale: scale.unwrap_or(dpi / 72.0),
            annotations: !no_annotations,
        }),
        Command::Extract { what } => match what {
            Extract::Text { input, pages, json } => {
                cmd::extract::text(&input.file, password, pages.as_deref(), json)
            }
            Extract::Links { input, pages, json } => {
                cmd::extract::links(&input.file, password, pages.as_deref(), json)
            }
            Extract::Toc { input, json } => cmd::extract::toc(&input.file, password, json),
            Extract::Attachments {
                input,
                output,
                json,
            } => cmd::extract::attachments(&input.file, password, output.as_deref(), json),
            Extract::Annotations { input, pages, json } => {
                cmd::extract::annotations(&input.file, password, pages.as_deref(), json)
            }
            Extract::Signatures { input, json } => {
                cmd::extract::signatures(&input.file, password, json)
            }
        },
    };
    match outcome {
        Ok(code) => code,
        Err(err) => {
            eprintln!("pdfrum: {err:#}");
            ExitCode::from(1)
        }
    }
}
