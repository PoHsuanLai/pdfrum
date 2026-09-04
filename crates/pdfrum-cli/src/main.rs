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
mod term;

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
        Command::Info { input, json } => cmd::info::run(&input.file, password, json),
        Command::Doctor {
            input,
            json,
            strict,
            scan_all,
        } => cmd::doctor::run(&input.file, password, json, strict, scan_all),
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
        Command::Pages { what } => run_pages(what, password),
        Command::Forms { what } => run_forms(what, password),
        Command::Repair { input, save } => cmd::file::rewrite(
            &input.file,
            password,
            &save.output,
            save.deterministic,
            "repaired",
        ),
        Command::Optimize { input, save } => cmd::file::rewrite(
            &input.file,
            password,
            &save.output,
            save.deterministic,
            "rewritten",
        ),
        Command::Security { what } => match what {
            Security::Decrypt { input, save } => {
                cmd::file::decrypt(&input.file, password, &save.output, save.deterministic)
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
        } => cmd::extract::attachments(&input.file, password, output.as_deref(), json),
        Extract::Annotations { input, pages, json } => {
            cmd::extract::annotations(&input.file, password, pages.as_deref(), json)
        }
        Extract::Signatures { input, json } => {
            cmd::extract::signatures(&input.file, password, json)
        }
    }
}

fn run_pages(what: Pages, password: Option<&str>) -> anyhow::Result<ExitCode> {
    match what {
        Pages::Merge { files, save } => {
            cmd::pages::merge(&files, password, &save.output, save.deterministic)
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
                cmd::pages::slice(&cmd::pages::Slice {
                    file: &input.file,
                    password,
                    spec: pages.as_deref(),
                    rotate,
                    crop,
                    output: &save.output,
                    deterministic: save.deterministic,
                })
            }),
        Pages::Reorder { input, pages, save } => cmd::pages::reorder(
            &input.file,
            password,
            &pages,
            &save.output,
            save.deterministic,
        ),
        Pages::Create { images, dpi, save } => {
            cmd::pages::create(&images, dpi, &save.output, save.deterministic)
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
                &input.file,
                password,
                pages.as_deref(),
                grid,
                sheet,
                &save.output,
                save.deterministic,
            )
        }),
        Pages::Booklet { input, save } => {
            cmd::pages::booklet(&input.file, password, &save.output, save.deterministic)
        }
    }
}

fn run_forms(what: Forms, password: Option<&str>) -> anyhow::Result<ExitCode> {
    match what {
        Forms::Dump { input, json } => cmd::forms::dump(&input.file, password, json),
        Forms::Fill { input, data, save } => cmd::forms::fill(
            &input.file,
            password,
            &data,
            &save.output,
            save.deterministic,
        ),
        Forms::Flatten { input, print, save } => cmd::forms::flatten(
            &input.file,
            password,
            print,
            &save.output,
            save.deterministic,
        ),
    }
}
