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
mod syntax;
mod term;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

/// Inspect, render and extract from PDF files.
#[derive(Parser)]
#[command(name = "pdfrum", version, about, long_about = None)]
struct Cli {
    /// Password for an encrypted document. Either the user or the owner
    /// password opens it; the permissions reported are the ones it grants.
    #[arg(short, long, global = true, value_name = "PASSWORD")]
    password: Option<String>,

    /// Colour in the output: on when stdout is a terminal, unless `NO_COLOR`.
    #[arg(long, global = true, value_enum, default_value_t, value_name = "WHEN")]
    color: term::When,

    /// Clickable links (OSC 8) in the output: on when stdout is a terminal.
    #[arg(long, global = true, value_enum, default_value_t, value_name = "WHEN")]
    hyperlinks: term::When,

    /// How `preview` and `view` draw pages: picked from the terminal's own
    /// announcements unless told.
    #[arg(long, global = true, value_enum, default_value_t, value_name = "MODE")]
    graphics: term::GraphicsMode,

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
    /// Show a page right here in the terminal.
    Preview {
        #[command(flatten)]
        input: Input,
        /// The page to show, 1-based.
        #[arg(long, default_value_t = 1)]
        page: u32,
        /// Width in terminal columns; the window's width by default.
        #[arg(long, value_name = "COLUMNS")]
        width: Option<u16>,
    },
    /// Read the document in the terminal: pages, search, zoom.
    View {
        #[command(flatten)]
        input: Input,
        /// The page to open on, 1-based.
        #[arg(long, default_value_t = 1)]
        page: u32,
    },
    /// Find text, `grep`-style: every hit with its line and page.
    Search {
        /// What to look for.
        #[arg(value_name = "TEXT")]
        needle: String,
        #[command(flatten)]
        input: Input,
        /// Match regardless of case.
        #[arg(short, long)]
        ignore_case: bool,
        /// Pages to search, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// One JSON document: an array of hits with page, offsets, line and boxes.
        #[arg(long)]
        json: bool,
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
    /// Merge, split, slice, reorder, impose, or make pages from images.
    Pages {
        #[command(subcommand)]
        what: Pages,
    },
    /// Interactive forms: list fields, fill them, or bake them into the page.
    Forms {
        #[command(subcommand)]
        what: Forms,
    },
    /// Open with recovery and write a clean, fully rewritten file.
    ///
    /// Opening is the repair: a wrong startxref, a bad stream length, a
    /// broken cross-reference table are reconstructed on the way in. The
    /// rewrite then drops every object nothing points at.
    Repair {
        #[command(flatten)]
        input: Input,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// Rewrite the file compactly: unreferenced objects dropped, streams
    /// re-encoded.
    Optimize {
        #[command(flatten)]
        input: Input,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// Encryption.
    Security {
        #[command(subcommand)]
        what: Security,
    },
    /// Look inside: objects, the cross-reference table, revisions, structure.
    Inspect {
        #[command(subcommand)]
        what: Inspect,
    },
    /// What changed between two documents: text per page, and with
    /// `--visual` the pixels. Exit 1 when they differ.
    Diff {
        /// The older document.
        #[arg(value_name = "LEFT")]
        left: PathBuf,
        /// The newer document.
        #[arg(value_name = "RIGHT")]
        right: PathBuf,
        /// Render every page of both and compare the pixels too.
        #[arg(long)]
        visual: bool,
        /// With `--visual`: write `diff-<n>.png` for each differing page into
        /// this directory, the changes in red over the faded page.
        #[arg(short, long, value_name = "DIR", requires = "visual")]
        output: Option<PathBuf>,
        /// Resolution for `--visual`, dots per inch.
        #[arg(long, default_value_t = 72.0)]
        dpi: f64,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Fingerprints: SHA-256 of the file, the trailer's /ID, and a semantic
    /// hash that ignores timestamps and layout on disk.
    Hash {
        #[command(flatten)]
        input: Input,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Shell completions, generated from this command tree.
    Completions {
        /// The shell to generate for.
        #[arg(value_enum, value_name = "SHELL")]
        shell: Shell,
    },
    /// Manual pages, one per command, into a directory.
    Manpage {
        /// The directory to write `pdfrum.1`, `pdfrum-extract.1`, … into.
        #[arg(short, long, value_name = "DIR")]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum Inspect {
    /// One indirect object in PDF syntax.
    Object {
        #[command(flatten)]
        input: Input,
        /// The object number.
        #[arg(value_name = "NUM")]
        num: u32,
        /// The generation number.
        #[arg(value_name = "GEN", default_value_t = 0)]
        generation: u16,
        /// For a stream: write its decoded data to stdout instead.
        #[arg(long)]
        decode: bool,
    },
    /// The cross-reference table as the parser holds it: every object's
    /// place, and the trailer.
    Xref {
        #[command(flatten)]
        input: Input,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// The incremental-update history: one row per saved revision.
    Revisions {
        #[command(flatten)]
        input: Input,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// The file as it stood at an earlier revision, written out whole.
    Revision {
        #[command(flatten)]
        input: Input,
        /// Which revision, 1-based, as `inspect revisions` numbers them.
        #[arg(long, value_name = "N")]
        rev: usize,
        /// The file to write.
        #[arg(short, long, value_name = "PATH")]
        output: PathBuf,
    },
    /// The structure tree (tagged PDF) of each page, indented.
    Structure {
        #[command(flatten)]
        input: Input,
        /// Pages to show, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum Pages {
    /// Join files into one, in the order given.
    Merge {
        /// The PDF files to join.
        #[arg(value_name = "FILE", required = true)]
        files: Vec<PathBuf>,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// One file per page, `<stem>-<n>.pdf`, into a directory.
    Split {
        #[command(flatten)]
        input: Input,
        /// Pages to split out, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// The directory to write into.
        #[arg(short, long, value_name = "DIR")]
        output: PathBuf,
        /// Reproducible output: the same input gives the same bytes.
        #[arg(long)]
        deterministic: bool,
    },
    /// Keep some pages, in document order, rotated or cropped.
    Slice {
        #[command(flatten)]
        input: Input,
        /// Pages to keep, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// Rotate the kept pages by this many degrees (a multiple of 90).
        #[arg(long, value_name = "DEGREES", allow_negative_numbers = true)]
        rotate: Option<i32>,
        /// Set the kept pages' crop box: `x0,y0,x1,y1` in points.
        #[arg(long, value_name = "BOX")]
        crop: Option<String>,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// Pages in the order named, duplicates allowed: `3,1,1,2`.
    Reorder {
        #[command(flatten)]
        input: Input,
        /// The order, 1-based.
        #[arg(long, value_name = "RANGE", required = true)]
        pages: String,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// A PDF from JPEG and PNG files, one page per image.
    Create {
        /// The images, in page order.
        #[arg(value_name = "IMAGE", required = true)]
        images: Vec<PathBuf>,
        /// Pixels per inch the images are placed at; sets the page size.
        #[arg(long, default_value_t = 72.0)]
        dpi: f64,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// N-up imposition: several pages per sheet.
    Nup {
        #[command(flatten)]
        input: Input,
        /// Pages to lay out, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// Columns by rows per sheet.
        #[arg(long, value_name = "CxR", default_value = "2x1")]
        grid: String,
        /// Sheet size: `WIDTHxHEIGHT` in points, `letter` or `a4`.
        #[arg(long, value_name = "SIZE", default_value = "letter")]
        sheet: String,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// Saddle-stitch booklet: two pages per side, ordered for folding.
    Booklet {
        #[command(flatten)]
        input: Input,
        #[command(flatten)]
        save: SaveArgs,
    },
}

#[derive(Subcommand)]
enum Forms {
    /// List the fields with their kinds and values.
    Dump {
        #[command(flatten)]
        input: Input,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Set field values from a JSON object and save.
    Fill {
        #[command(flatten)]
        input: Input,
        /// A JSON file: `{"name": "text", "box": true}`.
        #[arg(long, value_name = "JSON", required = true)]
        data: PathBuf,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// Bake annotations and fields into static page content.
    Flatten {
        #[command(flatten)]
        input: Input,
        /// Keep only what prints (the `/Print` flag), as a printer would.
        #[arg(long)]
        print: bool,
        #[command(flatten)]
        save: SaveArgs,
    },
}

#[derive(Subcommand)]
enum Security {
    /// Write the document without its encryption.
    Decrypt {
        #[command(flatten)]
        input: Input,
        #[command(flatten)]
        save: SaveArgs,
    },
    /// Encrypt the document: AES-256, a user password to open it and an
    /// owner password that unlocks everything.
    Encrypt {
        #[command(flatten)]
        input: Input,
        /// The password that opens the document with `--allow`'s rights.
        /// Empty means anyone can open it.
        #[arg(long, value_name = "PASSWORD", default_value = "")]
        user_password: String,
        /// The password that opens the document with every right. Empty
        /// means the user password serves as both.
        #[arg(long, value_name = "PASSWORD", default_value = "")]
        owner_password: String,
        /// What the user password allows, comma-separated: print, modify,
        /// copy, annotate, fill-forms, extract, assemble, print-hq, all, none.
        #[arg(long, value_name = "LIST")]
        allow: Option<String>,
        /// Leave the XMP metadata stream readable without the password.
        #[arg(long)]
        plain_metadata: bool,
        #[command(flatten)]
        save: SaveArgs,
    },
}

/// Where a writing command puts its result.
#[derive(Args)]
struct SaveArgs {
    /// The file to write.
    #[arg(short, long, value_name = "PATH")]
    output: PathBuf,
    /// Reproducible output: the same input gives the same bytes.
    #[arg(long)]
    deterministic: bool,
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
        /// Keep the layout: columns stay columns, gaps stay gaps.
        #[arg(long)]
        layout: bool,
        /// One JSON document: an array of `{page, text}`.
        #[arg(long)]
        json: bool,
    },
    /// Each page as Markdown: the structure tree where there is one,
    /// typography where there is not.
    Markdown {
        #[command(flatten)]
        input: Input,
        /// Pages to convert, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// One JSON document: an array of `{page, markdown}`.
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
    /// Images on the pages: list them, or write them into a directory.
    Images {
        #[command(flatten)]
        input: Input,
        /// Pages to scan, 1-based. All by default.
        #[arg(long, value_name = "RANGE")]
        pages: Option<String>,
        /// Write every image into this directory: JPEG and JPEG 2000 data as
        /// it is in the file, everything else decoded to PNG.
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// Every draw, including repeats of one image and images under 4
        /// pixels on a side (spacers, rules), which are otherwise folded
        /// into one row or left out.
        #[arg(long)]
        all: bool,
        /// One JSON document.
        #[arg(long)]
        json: bool,
    },
    /// Embedded font programs: list them, or write them into a directory.
    Fonts {
        #[command(flatten)]
        input: Input,
        /// Write every font program into this directory.
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
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
    let term = term::Term::detect(cli.color, cli.hyperlinks, cli.graphics);
    let outcome = match cli.command {
        Command::Info { input, json } => cmd::info::run(&input.file, password, json, term),
        Command::Doctor {
            input,
            json,
            strict,
            scan_all,
        } => cmd::doctor::run(&input.file, password, json, strict, scan_all, term),
        Command::Preview { input, page, width } => {
            cmd::terminal::preview(&input.file, password, page, width, term)
        }
        Command::View { input, page } => cmd::terminal::view(&input.file, password, page, term),
        Command::Search {
            needle,
            input,
            ignore_case,
            pages,
            json,
        } => cmd::terminal::search(
            &input.file,
            password,
            &needle,
            ignore_case,
            pages.as_deref(),
            json,
            term,
        ),
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
        Command::Extract { what } => run_extract(what, password, term),
        Command::Pages { what } => run_pages(what, password, term),
        Command::Forms { what } => run_forms(what, password, term),
        Command::Repair { input, save } => cmd::file::rewrite(
            &input.file,
            password,
            &save.output,
            save.deterministic,
            "repaired",
            term,
        ),
        Command::Optimize { input, save } => cmd::file::rewrite(
            &input.file,
            password,
            &save.output,
            save.deterministic,
            "rewritten",
            term,
        ),
        Command::Security { what } => run_security(what, password, term),
        Command::Inspect { what } => run_inspect(what, password, term),
        Command::Diff {
            left,
            right,
            visual,
            output,
            dpi,
            json,
        } => cmd::diff::run(&cmd::diff::Request {
            left: &left,
            right: &right,
            password,
            visual,
            out_dir: output.as_deref(),
            dpi,
            json,
            term,
        }),
        Command::Hash { input, json } => cmd::hash::run(&input.file, password, json, term),
        Command::Completions { shell } => Ok(cmd::shell::completions(shell)),
        Command::Manpage { output } => cmd::shell::manpage(&output, term),
    };
    match outcome {
        Ok(code) => code,
        Err(err) => {
            eprintln!(
                "{}",
                term.paint(term::Style::Error, &format!("pdfrum: {}", error_line(&err)))
            );
            ExitCode::from(1)
        }
    }
}

fn run_extract(
    what: Extract,
    password: Option<&str>,
    term: term::Term,
) -> anyhow::Result<ExitCode> {
    match what {
        Extract::Text {
            input,
            pages,
            layout,
            json,
        } => cmd::extract::text(&input.file, password, pages.as_deref(), layout, json),
        Extract::Markdown { input, pages, json } => {
            cmd::extract::markdown(&input.file, password, pages.as_deref(), json)
        }
        Extract::Links { input, pages, json } => {
            cmd::extract::links(&input.file, password, pages.as_deref(), json, term)
        }
        Extract::Toc { input, json } => cmd::extract::toc(&input.file, password, json, term),
        Extract::Attachments {
            input,
            output,
            json,
        } => cmd::extract::attachments(&input.file, password, output.as_deref(), json, term),
        Extract::Annotations { input, pages, json } => {
            cmd::extract::annotations(&input.file, password, pages.as_deref(), json, term)
        }
        Extract::Signatures { input, json } => {
            cmd::extract::signatures(&input.file, password, json, term)
        }
        Extract::Images {
            input,
            pages,
            output,
            all,
            json,
        } => cmd::extract::images(
            &input.file,
            password,
            pages.as_deref(),
            output.as_deref(),
            all,
            json,
            term,
        ),
        Extract::Fonts {
            input,
            output,
            json,
        } => cmd::extract::fonts(&input.file, password, output.as_deref(), json, term),
    }
}

fn run_security(
    what: Security,
    password: Option<&str>,
    term: term::Term,
) -> anyhow::Result<ExitCode> {
    match what {
        Security::Decrypt { input, save } => cmd::file::decrypt(
            &input.file,
            password,
            &save.output,
            save.deterministic,
            term,
        ),
        Security::Encrypt {
            input,
            user_password,
            owner_password,
            allow,
            plain_metadata,
            save,
        } => cmd::file::encrypt(
            &cmd::file::EncryptRequest {
                file: &input.file,
                password,
                user_password: &user_password,
                owner_password: &owner_password,
                allow: allow.as_deref(),
                encrypt_metadata: !plain_metadata,
                output: &save.output,
                deterministic: save.deterministic,
            },
            term,
        ),
    }
}

fn run_inspect(
    what: Inspect,
    password: Option<&str>,
    term: term::Term,
) -> anyhow::Result<ExitCode> {
    match what {
        Inspect::Object {
            input,
            num,
            generation,
            decode,
        } => cmd::inspect::object(&input.file, password, num, generation, decode, term),
        Inspect::Xref { input, json } => cmd::inspect::xref(&input.file, password, json, term),
        Inspect::Revisions { input, json } => {
            cmd::inspect::revisions(&input.file, password, json, term)
        }
        Inspect::Revision { input, rev, output } => {
            cmd::inspect::revision(&input.file, password, rev, &output, term)
        }
        Inspect::Structure { input, pages, json } => {
            cmd::inspect::structure(&input.file, password, pages.as_deref(), json, term)
        }
    }
}

fn run_pages(what: Pages, password: Option<&str>, term: term::Term) -> anyhow::Result<ExitCode> {
    match what {
        Pages::Merge { files, save } => {
            cmd::pages::merge(&files, password, &save.output, save.deterministic, term)
        }
        Pages::Split {
            input,
            pages,
            output,
            deterministic,
        } => cmd::pages::split(
            &input.file,
            password,
            pages.as_deref(),
            &output,
            deterministic,
            term,
        ),
        Pages::Slice {
            input,
            pages,
            rotate,
            crop,
            save,
        } => crop
            .as_deref()
            .map(cmd::pages::parse_rect)
            .transpose()
            .and_then(|crop| {
                cmd::pages::slice(
                    &cmd::pages::Slice {
                        file: &input.file,
                        password,
                        spec: pages.as_deref(),
                        rotate,
                        crop,
                        output: &save.output,
                        deterministic: save.deterministic,
                    },
                    term,
                )
            }),
        Pages::Reorder { input, pages, save } => cmd::pages::reorder(
            &input.file,
            password,
            &pages,
            &save.output,
            save.deterministic,
            term,
        ),
        Pages::Create { images, dpi, save } => {
            cmd::pages::create(&images, dpi, &save.output, save.deterministic, term)
        }
        Pages::Nup {
            input,
            pages,
            grid,
            sheet,
            save,
        } => cmd::pages::parse_grid(&grid).and_then(|grid| {
            let sheet = cmd::pages::parse_size(&sheet)?;
            cmd::pages::nup(
                &cmd::pages::Nup {
                    file: &input.file,
                    password,
                    spec: pages.as_deref(),
                    grid,
                    sheet,
                    output: &save.output,
                    deterministic: save.deterministic,
                },
                term,
            )
        }),
        Pages::Booklet { input, save } => cmd::pages::booklet(
            &input.file,
            password,
            &save.output,
            save.deterministic,
            term,
        ),
    }
}

fn run_forms(what: Forms, password: Option<&str>, term: term::Term) -> anyhow::Result<ExitCode> {
    match what {
        Forms::Dump { input, json } => cmd::forms::dump(&input.file, password, json, term),
        Forms::Fill { input, data, save } => cmd::forms::fill(
            &input.file,
            password,
            &data,
            &save.output,
            save.deterministic,
            term,
        ),
        Forms::Flatten { input, print, save } => cmd::forms::flatten(
            &input.file,
            password,
            print,
            &save.output,
            save.deterministic,
            term,
        ),
    }
}

/// The error's causes, outermost first, joined by `: ` — with a cause
/// left out when the message above it already quotes it, which the
/// library's errors do, so nothing is said twice.
fn error_line(err: &anyhow::Error) -> String {
    let mut line = String::new();
    for cause in err.chain() {
        let text = cause.to_string();
        if line.contains(&text) {
            continue;
        }
        if !line.is_empty() {
            line.push_str(": ");
        }
        line.push_str(&text);
    }
    line
}
