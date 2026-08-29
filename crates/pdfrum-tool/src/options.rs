//! The command line, as `pdfium_test` reads it.
//!
//! Hand-parsed rather than declared with `clap`, because the oracle's parser
//! *is* part of the contract: `--pages=` uses `std::stringstream` extraction
//! (a value that will not parse leaves the number at zero), an unrecognized
//! `--flag` is one specific line on stderr followed by the usage text and
//! exit 1, and a bare word is a file even when it starts with a single dash.
//! Tier A diffs what the tool prints, so the parse has to agree.
//!
//! The flags we do not implement yet are still *recognized* here and land in
//! [`Options::unsupported`]. A tool that rejected them would make the harness
//! tag rendering and text tiers `tool-error`; recognizing them and saying so
//! on stderr lets those tiers read as `unsupported` instead, which is the
//! honest answer while the crates below us are still being written.

use std::path::PathBuf;

/// What the tool was asked to print for each page.
///
/// The oracle keeps one `OutputFormat` and rejects a second `--show-*` or
/// output flag; the same exclusivity is what makes the golden store's six
/// passes six separate invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// No page output was asked for.
    #[default]
    None,
    /// `--show-pageinfo`: the five page boxes.
    PageInfo,
    /// `--show-structure`: the tagged-PDF structure tree.
    Structure,
    /// `--png`, `--ppm`, `--bmp`, …: a rendered page.
    Render(&'static str),
    /// `--txt`: extracted page text as UTF-32LE.
    Text,
    /// `--annot`: the per-page annotation dump.
    Annot,
}

/// The page range `--pages=` selects.
///
/// Inclusive on both ends and *not* clamped to the document: the oracle asks
/// for every index in the range and counts the ones that do not load as bad
/// pages, so a range past the end is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub first: i32,
    pub last: i32,
}

/// Everything one invocation was told to do.
///
/// The flags are independent switches the oracle's own command line offers
/// separately, not a mode: `--md5` and `--save` and `--show-metadata` compose
/// freely. Folding them into an enum would invent a grammar the oracle does
/// not have, and this type's whole job is to read the command line the way
/// `pdfium_test` reads it.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent switches, mirroring the oracle's flags"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub files: Vec<PathBuf>,
    pub format: OutputFormat,
    pub show_metadata: bool,
    pub md5: bool,
    pub password: String,
    pub pages: Option<PageRange>,
    /// Directories to enumerate faces from, from `--font-dir`.
    ///
    /// Empty means the built-in faces alone. A directory here *replaces* the
    /// system font path rather than adding to it, which is what makes the
    /// oracle's `--font-dir=third_party/test_fonts` hermetic
    /// (`fx_linux_impl.cpp:167-176`).
    pub font_dirs: Vec<PathBuf>,
    /// Whether to rename requested faces to their Croscore equivalents, from
    /// `--croscore-font-names`.
    pub croscore_font_names: bool,
    /// Drop the security handler when saving, from `--save-decrypted`.
    ///
    /// Without it an encrypted document saves encrypted under its own
    /// handler, so the output needs the same `--password=` the input did.
    pub save_decrypted: bool,
    /// Write each document back out beside its input, from `--save`.
    ///
    /// **This flag has no oracle counterpart** — `pdfium_test` cannot save a
    /// document at all. It exists so the conformance harness can perform the
    /// two-step check M7 requires: pdfrum saves, and the *oracle* then reopens
    /// and renders what pdfrum wrote (SPEC.md §11's ruling E7). The asymmetry
    /// is expected, and the harness never diffs this flag against the oracle.
    pub save: bool,
    /// A curated page mutation to apply before saving, from `--mutate=`.
    ///
    /// Like `--save`, this has **no oracle counterpart** — `pdfium_test`
    /// cannot edit a document either. It exists so the harness can perform
    /// M11's exit check: pdfrum mutates page 0 and saves, the oracle reopens
    /// and renders the result, and the two renders of that same file are
    /// compared. Implies `--save`; an unrecognised value is refused, because
    /// silently saving an unmutated file would make the check pass for the
    /// wrong reason.
    pub mutate: Option<String>,
    /// The rasterizer named by `--use-renderer=`, verbatim.
    ///
    /// The oracle has this flag too — it picks between its AGG and Skia
    /// backends — so unlike `--save` this is not an asymmetry with the oracle's
    /// surface, and the harness can pass it to both. Our names are our own
    /// (`exact`, `tiny-skia`, `vello`), and an unrecognised value falls back to
    /// the default rather than failing, exactly as an unknown renderer name
    /// does upstream.
    pub use_renderer: Option<String>,
    /// Flags recognized but not implemented, in the order they were given.
    pub unsupported: Vec<String>,
}

/// Why a command line was refused.
///
/// Both variants make the oracle print the usage text and exit 1; the message
/// is what precedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A `--flag` this tool does not know, or a duplicated one.
    Rejected(String),
}

/// Flags that take no value and that we accept without implementing.
///
/// Rendering-mode switches, event injection, and the save-* family: the tool
/// must not fail on them, because the harness runs one fixed command line
/// against every candidate and reads the tiers from what comes back.
const ACCEPTED_SWITCHES: &[&str] = &[
    "--send-events",
    "--mem-document",
    "--render-oneshot",
    "--lcd-text",
    "--no-nativetext",
    "--grayscale",
    "--forced-color",
    "--fill-to-stroke",
    "--limit-cache",
    "--force-halftone",
    "--printing",
    "--no-smoothtext",
    "--no-smoothimage",
    "--no-smoothpath",
    "--reverse-byte-order",
    "--save-attachments",
    "--save-images",
    "--save-rendered-images",
    "--save-thumbs",
    "--save-thumbs-dec",
    "--save-thumbs-raw",
    "--fontations",
    "--no-system-fonts",
    "--reverse-byte-order-uses-bgra",
    "--callgrind-delim",
    "--enable-brotli",
];

/// Flags of the form `--key=value` that we accept without implementing.
const ACCEPTED_VALUED: &[&str] = &[
    "--scale=",
    "--render-repeats=",
    "--bin-dir=",
    "--js-flags=",
    "--time=",
];

/// The output formats that mean "rasterize the page", with the extension each
/// writes beside the input.
const RENDER_FORMATS: &[(&str, &str)] = &[
    ("--png", "png"),
    ("--ppm", "ppm"),
    ("--bmp", "bmp"),
    ("--emf", "emf"),
    ("--ps2", "ps"),
    ("--ps3", "ps"),
    ("--ps3-type42", "ps"),
    ("--skp", "skp"),
    ("--xps", "xps"),
];

/// Reads a command line the way `pdfium_test` reads it.
///
/// `args` excludes the program name.
///
/// # Errors
///
/// [`ParseError::Rejected`] for an unrecognized or duplicated flag, which the
/// caller turns into the usage text and exit code 1.
pub fn parse(args: &[String]) -> Result<Options, ParseError> {
    let mut options = Options::default();
    for arg in args {
        if let Some(value) = arg.strip_prefix("--password=") {
            if !options.password.is_empty() {
                return Err(ParseError::Rejected(
                    "Duplicate --password argument".to_owned(),
                ));
            }
            value.clone_into(&mut options.password);
        } else if let Some(value) = arg.strip_prefix("--pages=") {
            if options.pages.is_some() {
                return Err(ParseError::Rejected(
                    "Duplicate --pages argument".to_owned(),
                ));
            }
            options.pages = Some(parse_page_range(value));
        } else if let Some(value) = arg.strip_prefix("--use-renderer=") {
            // Last one wins rather than being a duplicate error: the oracle
            // overwrites its own renderer choice the same way.
            options.use_renderer = Some(value.to_owned());
        } else if let Some(value) = arg.strip_prefix("--font-dir=") {
            // Repeatable, like the oracle's: each one adds a path to scan.
            options.font_dirs.push(PathBuf::from(value));
        } else if arg == "--croscore-font-names" {
            options.croscore_font_names = true;
        } else if arg == "--show-metadata" {
            options.show_metadata = true;
        } else if arg == "--md5" {
            options.md5 = true;
        } else if arg == "--save-decrypted" {
            options.save = true;
            options.save_decrypted = true;
        } else if arg == "--save" {
            options.save = true;
        } else if let Some(value) = arg.strip_prefix("--mutate=") {
            if crate::mutate::Mutation::parse(value).is_none() {
                return Err(ParseError::Rejected(format!(
                    "Unrecognized mutation {value}"
                )));
            }
            options.mutate = Some(value.to_owned());
            options.save = true;
        } else if let Some(format) = output_format_of(arg) {
            if options.format != OutputFormat::None {
                return Err(ParseError::Rejected(format!(
                    "Duplicate or conflicting {} argument",
                    arg.trim_end_matches('=')
                )));
            }
            options.format = format;
        } else if ACCEPTED_SWITCHES.contains(&arg.as_str())
            || ACCEPTED_VALUED.iter().any(|key| arg.starts_with(key))
        {
            options.unsupported.push(arg.clone());
        } else if arg.starts_with("--") {
            return Err(ParseError::Rejected(format!("Unrecognized argument {arg}")));
        } else {
            options.files.push(PathBuf::from(arg));
        }
    }
    Ok(options)
}

/// The output format a flag selects, if it selects one.
fn output_format_of(arg: &str) -> Option<OutputFormat> {
    match arg {
        "--show-pageinfo" => Some(OutputFormat::PageInfo),
        "--show-structure" => Some(OutputFormat::Structure),
        "--txt" => Some(OutputFormat::Text),
        "--annot" => Some(OutputFormat::Annot),
        _ => RENDER_FORMATS
            .iter()
            .find(|(flag, _)| *flag == arg)
            .map(|(_, extension)| OutputFormat::Render(extension)),
    }
}

/// `--pages=N` or `--pages=N-M`, with the oracle's stream-extraction rules.
///
/// `std::stringstream(text) >> int` leaves the target at zero when the text
/// does not begin with a number and stops at the first byte that cannot
/// continue one, so `--pages=x` is page 0 and `--pages=3x` is page 3. The
/// first dash splits, which is why a leading `-` yields an empty first half
/// (zero) rather than a negative number.
fn parse_page_range(text: &str) -> PageRange {
    match text.split_once('-') {
        None => {
            let page = extract_int(text);
            PageRange {
                first: page,
                last: page,
            }
        }
        Some((first, last)) => PageRange {
            first: extract_int(first),
            last: extract_int(last),
        },
    }
}

/// The leading integer of `text`, or zero — `operator>>(int&)` on failure.
fn extract_int(text: &str) -> i32 {
    let digits = text.trim_start();
    let body = digits.strip_prefix('+').unwrap_or(digits);
    let end = body
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(body.len(), |(index, _)| index);
    body.get(..end)
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0)
}

/// The oracle's usage text, printed on a rejected command line.
///
/// Abridged to the flags this tool understands: the harness only ever reads
/// the exit code and the message that precedes it, and a usage screen listing
/// flags we do not implement would be a lie.
pub const USAGE: &str = "\
Usage: pdfrum-tool [OPTION] [FILE]...
  --show-metadata        - print the file metadata
  --show-pageinfo        - print information about pages
  --show-structure       - print the structure elements from the document
  --png                  - write page images <pdf-name>.<page-number>.png
  --txt                  - write page text in UTF32-LE <pdf-name>.<page-number>.txt
  --annot                - write annotation info <pdf-name>.<page-number>.annot.txt
  --md5                  - write output image paths and their md5 hashes to stdout
  --pages=<number>(-<number>) - only render the given 0-based page(s)
  --password=<secret>    - password to decrypt the PDF with
  --save                 - write the document back out as <pdf-name>.saved.pdf
  --save-decrypted       - the same, with the security handler removed
                           (no oracle counterpart; see Options::save)
";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, ParseError> {
        parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn a_bare_word_is_an_input_file() {
        let options = parse_args(&["a.pdf", "b.pdf"]).unwrap();
        assert_eq!(
            options.files,
            [PathBuf::from("a.pdf"), PathBuf::from("b.pdf")]
        );
    }

    #[test]
    fn the_dump_flags_each_select_one_format() {
        assert_eq!(
            parse_args(&["--show-pageinfo"]).unwrap().format,
            OutputFormat::PageInfo
        );
        assert_eq!(
            parse_args(&["--show-structure"]).unwrap().format,
            OutputFormat::Structure
        );
        assert_eq!(parse_args(&["--txt"]).unwrap().format, OutputFormat::Text);
        assert_eq!(
            parse_args(&["--annot"]).unwrap().format,
            OutputFormat::Annot
        );
        assert_eq!(
            parse_args(&["--png"]).unwrap().format,
            OutputFormat::Render("png")
        );
    }

    #[test]
    fn show_metadata_is_not_an_output_format() {
        // It prints before the page loop and coexists with any of them, which
        // is why the golden store's metadata pass carries no other flag but
        // still gets a page count on stderr.
        let options = parse_args(&["--show-metadata"]).unwrap();
        assert!(options.show_metadata);
        assert_eq!(options.format, OutputFormat::None);
    }

    #[test]
    fn two_output_formats_are_refused() {
        assert_eq!(
            parse_args(&["--show-pageinfo", "--show-structure"]),
            Err(ParseError::Rejected(
                "Duplicate or conflicting --show-structure argument".to_owned()
            ))
        );
        assert!(parse_args(&["--png", "--txt"]).is_err());
    }

    #[test]
    fn an_unknown_double_dash_flag_is_refused() {
        assert_eq!(
            parse_args(&["--wat"]),
            Err(ParseError::Rejected(
                "Unrecognized argument --wat".to_owned()
            ))
        );
    }

    #[test]
    fn a_single_dash_word_is_a_file_not_a_flag() {
        // `cur_arg.size() >= 2 && cur_arg[0] == '-' && cur_arg[1] == '-'` is
        // the oracle's rejection test, so `-x` falls through to the file list.
        assert_eq!(parse_args(&["-x"]).unwrap().files, [PathBuf::from("-x")]);
    }

    #[test]
    fn flags_we_do_not_implement_are_accepted_and_recorded() {
        let options = parse_args(&["--scale=2", "--time=1399672130", "a.pdf"]).unwrap();
        assert_eq!(options.unsupported, ["--scale=2", "--time=1399672130"]);
        assert_eq!(options.files, [PathBuf::from("a.pdf")]);
    }

    #[test]
    fn the_two_font_flags_are_read_rather_than_recorded() {
        // Both were accepted-and-ignored while substitution had no font
        // database to enumerate. They now decide which face a non-embedded
        // font resolves to, and the harness passes them on every invocation.
        let options = parse_args(&[
            "--croscore-font-names",
            "--font-dir=/fonts",
            "--font-dir=/more",
            "a.pdf",
        ])
        .unwrap();
        assert!(options.croscore_font_names);
        assert_eq!(
            options.font_dirs,
            [PathBuf::from("/fonts"), PathBuf::from("/more")]
        );
        assert!(options.unsupported.is_empty());
    }

    #[test]
    fn a_password_is_taken_verbatim_including_an_empty_one() {
        assert_eq!(
            parse_args(&["--password=hunter2"]).unwrap().password,
            "hunter2"
        );
        // An empty value leaves `password` empty, which the oracle reads as
        // "no password given" -- `options.password.empty()` is its own test.
        assert_eq!(parse_args(&["--password="]).unwrap().password, "");
    }

    #[test]
    fn a_duplicate_password_is_refused() {
        assert_eq!(
            parse_args(&["--password=a", "--password=b"]),
            Err(ParseError::Rejected(
                "Duplicate --password argument".to_owned()
            ))
        );
    }

    #[test]
    fn a_single_page_selects_itself_at_both_ends() {
        assert_eq!(
            parse_args(&["--pages=3"]).unwrap().pages,
            Some(PageRange { first: 3, last: 3 })
        );
    }

    #[test]
    fn a_range_splits_at_the_first_dash() {
        assert_eq!(
            parse_args(&["--pages=2-7"]).unwrap().pages,
            Some(PageRange { first: 2, last: 7 })
        );
        // The *first* dash splits, so a second one lands in the tail and stops
        // that extraction at zero.
        assert_eq!(
            parse_args(&["--pages=2-7-9"]).unwrap().pages,
            Some(PageRange { first: 2, last: 7 })
        );
    }

    #[test]
    fn an_unparsable_page_number_reads_as_zero() {
        // `stringstream >> int` sets the target to 0 when extraction fails.
        assert_eq!(
            parse_args(&["--pages=x"]).unwrap().pages,
            Some(PageRange { first: 0, last: 0 })
        );
        assert_eq!(
            parse_args(&["--pages=-4"]).unwrap().pages,
            Some(PageRange { first: 0, last: 4 })
        );
        assert_eq!(
            parse_args(&["--pages=3x"]).unwrap().pages,
            Some(PageRange { first: 3, last: 3 })
        );
    }

    #[test]
    fn a_duplicate_page_range_is_refused() {
        assert!(parse_args(&["--pages=1", "--pages=2"]).is_err());
    }

    #[test]
    fn an_empty_command_line_asks_for_nothing() {
        let options = parse_args(&[]).unwrap();
        assert!(options.files.is_empty());
        assert!(!options.show_metadata);
        assert_eq!(options.format, OutputFormat::None);
    }
}
