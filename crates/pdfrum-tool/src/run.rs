//! Processing one PDF, in the order and with the chatter `pdfium_test` uses.
//!
//! The sequence is the contract, because the dumps and the notices share one
//! stdout stream:
//!
//! 1. `Processing PDF file <name>.` on stderr.
//! 2. The load. On failure, `Load pdf docs unsuccessful: <reason>.` on stderr
//!    and **nothing else** — no page-count line, and the process still exits
//!    zero. A file that will not open is not a tool error.
//! 3. Document-level unsupported-feature notices, on stdout.
//! 4. The metadata dump, if asked for.
//! 5. Each page in the selected range: its annotations' notices, then its own
//!    dump. A page that will not load contributes nothing and is counted bad.
//! 6. `Processed N pages.` on stderr, and `Skipped N bad pages.` if any.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfrum_object::{Dict, Object, Resolve};
use pdfrum_page::BuildContext;
use pdfrum_parser::{Document, LoadError, LoadOptions, PageDict};

use crate::options::{Options, OutputFormat, PageRange};
use crate::{annot, dispatch, events, metadata, pageinfo, render, structure, text, unsupported};

/// Where a run writes. Separated from the work so the whole pipeline is
/// testable on buffers rather than on the process's own streams.
pub struct Streams<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// What one file's processing produced, for the caller's exit decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub processed: u32,
    pub bad: u32,
}

/// Processes one file, printing exactly what the oracle prints.
///
/// # Errors
///
/// Only for a stream that will not accept writes. A PDF that will not open is
/// reported on stderr and is not an error here, matching the oracle: its
/// `main` returns zero however badly the file behaved.
pub fn process_file(
    name: &str,
    bytes: Vec<u8>,
    options: &Options,
    streams: &mut Streams<'_>,
) -> std::io::Result<Counts> {
    writeln!(streams.err, "Processing PDF file {name}.")?;

    // `pdfium_test.cc:2152-2171` loads the sibling `.evt` after announcing
    // the PDF and before opening it — the script is read once for the whole
    // document and replayed against each page in the walk below.
    let parsed_events = if options.send_events {
        load_events(name, streams)?
    } else {
        Vec::new()
    };

    // An empty `--password=` is "no password given", which is how the oracle
    // reads it (`options.password.empty()` decides whether to pass one at
    // all), and the two are not equivalent to a security handler.
    let load = LoadOptions {
        password: (!options.password.is_empty()).then(|| options.password.clone().into_bytes()),
        ..LoadOptions::default()
    };
    // Shared rather than moved, because `--send-events` opens the same file a
    // second time through the facade (below) and the store is lazy: both
    // documents read objects out of these bytes on demand, so a second copy
    // would double a large file's footprint for nothing.
    let bytes: Arc<[u8]> = bytes.into();
    let doc = match pdfrum_parser::load(Arc::clone(&bytes), &load) {
        Ok(doc) => doc,
        Err(err) => {
            writeln!(
                streams.err,
                "Load pdf docs unsuccessful: {}.",
                load_error_text(&err)
            )?;
            write_event_summary(options, parsed_events.len(), streams)?;
            return Ok(Counts::default());
        }
    };

    let catalog = doc.catalog().unwrap_or_default();
    for feature in unsupported::document(&catalog, &doc) {
        write!(streams.out, "{}", feature.line())?;
    }

    if options.show_metadata {
        write!(streams.out, "{}", metadata::render(doc.trailer(), &doc))?;
    }

    // The XFA notice comes from setting up the form-fill environment, which
    // the oracle does after the metadata dump and before the page loop.
    if let Some(feature) = unsupported::xfa(&catalog, &doc) {
        write!(streams.out, "{}", feature.line())?;
    }

    // The form-fill environment, when there is a script to drive it. Opened
    // from the same bytes and the same password: a session over a *different*
    // reading of the file would route clicks against annotations the render
    // path never saw. A file the facade refuses — which cannot happen when
    // the parser accepted it, since both call the same loader — leaves the
    // session absent, and the events then parse and count exactly as before.
    let facade = (!parsed_events.is_empty())
        .then(|| {
            pdfrum::Document::from_bytes_with(
                Arc::clone(&bytes),
                &pdfrum::OpenOptions {
                    password: load.password.clone(),
                    ..pdfrum::OpenOptions::default()
                },
            )
            .ok()
        })
        .flatten();
    // One session for the whole document, as the oracle holds one
    // `FPDF_FORMHANDLE` for the whole document: what page 0's script leaves
    // focused is what page 1's script starts from.
    let mut session = facade.as_ref().map(pdfrum::FormSession::new);

    let counts = walk_pages(
        &doc,
        name,
        options,
        &parsed_events,
        session.as_mut(),
        streams,
    )?;

    // Written after the page walk so the per-page chatter the harness diffs
    // keeps its ordering, and before the summary line so a save failure is
    // visible next to the file it belongs to.
    if options.save {
        save_document(&doc, name, options, streams)?;
    }

    writeln!(streams.err, "Processed {} pages.", counts.processed)?;
    if counts.bad > 0 {
        writeln!(streams.err, "Skipped {} bad pages.", counts.bad)?;
    }
    write_event_summary(options, parsed_events.len(), streams)?;
    Ok(counts)
}

/// `Using event file` / `Sending events from:` as `pdfium_test.cc:2158-2163`
/// prints them, then the parsed events. An unreadable or empty file is a
/// silent no-op, matching `access` + `GetFileContents`.
fn load_events(pdf_name: &str, streams: &mut Streams<'_>) -> std::io::Result<Vec<events::Event>> {
    let Some(path) = events::sibling_evt_path(pdf_name) else {
        return Ok(Vec::new());
    };
    if !events::evt_is_readable(&path) {
        return Ok(Vec::new());
    }
    writeln!(streams.err, "Using event file {}.", path.display())?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(Vec::new());
    };
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    writeln!(streams.err, "Sending events from: {}", path.display())?;
    let text = String::from_utf8_lossy(&bytes);
    match events::parse_evt(&text) {
        Ok(parsed) => Ok(parsed),
        Err(err) => {
            writeln!(streams.err, "{err}")?;
            Ok(Vec::new())
        }
    }
}

/// The parsed-event count, only when `--send-events` was given.
fn write_event_summary(
    options: &Options,
    count: usize,
    streams: &mut Streams<'_>,
) -> std::io::Result<()> {
    if options.send_events {
        writeln!(streams.err, "Sent {count} events.")?;
    }
    Ok(())
}

/// Where `--save` writes a document: beside its input, as
/// `<name>.saved.pdf`.
///
/// The oracle has no save flag to mirror, so the naming is ours; the suffix
/// keeps it out of the way of every `<name>.<page>.<ext>` artifact the other
/// flags produce.
#[must_use]
pub fn save_path(input: &Path) -> Option<PathBuf> {
    let name = input.file_name()?.to_str()?;
    Some(input.with_file_name(format!("{name}.saved.pdf")))
}

/// Write `doc` back out, reporting failure the way the oracle reports a
/// failed write: a line on stderr, and the run continues.
fn save_document(
    doc: &Document,
    name: &str,
    options: &Options,
    streams: &mut Streams<'_>,
) -> std::io::Result<()> {
    let Some(path) = save_path(Path::new(name)) else {
        writeln!(streams.err, "Failed to save: no usable output path.")?;
        return Ok(());
    };

    let mut edit = pdfrum_edit::EditDoc::new(doc);

    // A mutation is staged into the overlay first, so the save below writes
    // the regenerated streams the same way it writes everything else —
    // through the same cipher, into the same xref.
    if let Some(name) = &options.mutate
        && let Some(mutation) = crate::mutate::Mutation::parse(name)
    {
        let mut diags = pdfrum_common::Diagnostics::default();
        let limits = pdfrum_common::Limits::default();
        match crate::mutate::apply(doc, &mut edit, mutation, &limits, &mut diags) {
            crate::mutate::Applied::Rewritten { streams: count } => writeln!(
                streams.err,
                "Mutated page 0 with {}, rewriting {count} content stream(s).",
                mutation.name()
            )?,
            crate::mutate::Applied::Skipped(why) => writeln!(
                streams.err,
                "Mutation {} did nothing: {why}.",
                mutation.name()
            )?,
        }
    }
    let edit = edit;
    // An encrypted document saves encrypted, under the handler the
    // `--password=` opened it with, so the output needs that same password
    // (SPEC.md §11's M10 ruling). `--save-decrypted` is the way to ask for
    // the plaintext the earlier ruling E3 produced unconditionally.
    let save_options = pdfrum_edit::SaveOptions {
        remove_security: options.save_decrypted,
        ..pdfrum_edit::SaveOptions::default()
    };

    let mut out = Vec::new();
    if let Err(err) = pdfrum_edit::save(&edit, &save_options, &mut out) {
        writeln!(streams.err, "Failed to save: {err}.")?;
        return Ok(());
    }
    match std::fs::write(&path, &out) {
        Ok(()) => writeln!(
            streams.err,
            "Saved {} bytes to {}.",
            out.len(),
            path.display()
        ),
        Err(err) => writeln!(streams.err, "Failed to write {}: {err}.", path.display()),
    }
}

/// The substitution settings a command line asks for.
///
/// `skip_font_enumeration` stays at its default `false`: the oracle's Linux
/// build drives `CFX_FolderFontInfo`, which enumerates, and `--font-dir` is
/// precisely the flag that puts it in that mode.
fn substitution_options(options: &Options) -> pdfrum_font::SubstitutionOptions {
    pdfrum_font::SubstitutionOptions {
        font_dirs: options.font_dirs.clone(),
        croscore_font_names: options.croscore_font_names,
        ..pdfrum_font::SubstitutionOptions::default()
    }
}

/// Visits the selected pages, dumping each one.
///
/// `script` and `session` are `--send-events`'s: the whole script is replayed
/// against each page before anything is written for it, which is
/// `PdfProcessor::ProcessPage` (`pdfium_test.cc:1474-1482`) — `SendPageEvents`
/// is its first statement, ahead of every dump and every render.
fn walk_pages(
    doc: &Document,
    name: &str,
    options: &Options,
    script: &[events::Event],
    mut session: Option<&mut pdfrum::FormSession<'_>>,
    streams: &mut Streams<'_>,
) -> std::io::Result<Counts> {
    let mut counts = Counts::default();
    // One build context for the whole file, so its font, colorspace and
    // image caches are shared across pages the way the oracle's document-wide
    // caches are.
    // `--font-dir` and `--croscore-font-names` decide which face a
    // non-embedded font draws with, and therefore the metrics every spacing
    // threshold in text extraction is computed from. Set once per file,
    // because the substitution must not vary between two pages of one
    // document.
    let mut ctx = BuildContext::with_substitution(substitution_options(options));
    let catalog = doc.catalog().unwrap_or_default();
    let rtl = text::direction_is_r2l(&catalog, doc);
    for index in selected_pages(options.pages, doc.page_count()) {
        let Some(page) = doc
            .page(index)
            .ok()
            .filter(|page| is_loadable(&page.dict, doc))
        else {
            counts.bad += 1;
            continue;
        };
        // Fact 6 — the whole stream, once, before this page's image is
        // saved. `ProcessPage` sends events as its very first statement, and
        // a page that would not load never reaches it, which is why this sits
        // *after* the load guard above and before every dump below.
        //
        // The updates are collected rather than dropped so a renderer can be
        // handed the post-event state, and the focus alongside them: a widget
        // the form filler is editing must not be given the form-field
        // highlight, which is a fact about the *session* rather than about
        // any one appearance.
        let (updates, focus) = match session.as_deref_mut() {
            Some(session) => {
                let updates = dispatch::replay_page(session, index, script, streams.err);
                (updates, session.focus_for_page(index))
            }
            None => (Vec::new(), None),
        };
        // The annotation walk happens when the page is first opened, before
        // anything is dumped for it.
        for feature in unsupported::page_annotations(&page.dict, doc) {
            write!(streams.out, "{}", feature.line())?;
        }
        write!(
            streams.out,
            "{}",
            dump_page(&page, index, options, &catalog, doc)
        )?;
        let extra = write_page_files(
            &page,
            Output {
                input: Path::new(name),
                index,
                updates: &updates,
                focus,
            },
            options,
            &catalog,
            doc,
            rtl,
            &mut ctx,
        );
        write!(streams.out, "{extra}")?;
        counts.processed += 1;
    }
    Ok(counts)
}

/// Whether the page loader would accept this dictionary.
///
/// The page *tree* is permissive — a `/Kids` entry with no `/Kids` of its own
/// is a leaf whatever it calls itself, and it counts toward `/Count` — but the
/// loader that turns such a leaf into a page applies one last, deliberately
/// loose type test: a missing `/Type` is fine, and anything else must be the
/// name `Page`. `bad_page_type.pdf` is the fixture, with a second kid marked
/// `/Type /Template`: the document reports two pages and produces one, and
/// the other is counted as a bad page.
///
/// This lives here rather than in the parser because it is a property of the
/// *page loading API*, not of the page tree: the tree still holds the node,
/// and a caller reading raw dictionaries should still see it.
///
/// The absent case is decided on the **raw** entry and the value on the
/// resolved one, which is not the same as asking a resolving accessor for a
/// name: an entry holding a reference that will not resolve is *present* and
/// refuses the page, where a resolving accessor would report it absent and
/// accept it.
fn is_loadable(page: &Dict, r: &impl Resolve) -> bool {
    let Some(raw) = page.raw(pdfrum_object::names::TYPE) else {
        return true;
    };
    let resolved = match raw {
        Object::Ref(reference) => r.fetch(*reference).ok().map(|o| o.as_ref().clone()),
        other => Some(other.clone()),
    };
    resolved.as_ref().and_then(Object::as_name) == Some(pdfrum_object::names::PAGE)
}

/// The page indices to visit.
///
/// Without `--pages` this is every page the document has. With it, the range
/// is taken literally and is **not** clamped: asking for page 9 of a one-page
/// document visits index 9, fails to load it, and counts one bad page. A
/// negative bound cannot survive the conversion and drops out, which matches
/// the C++ loop condition rather than emulating a signed counter.
fn selected_pages(range: Option<PageRange>, page_count: u32) -> Box<dyn Iterator<Item = u32>> {
    match range {
        None => Box::new(0..page_count),
        Some(PageRange { first, last }) => {
            let first = u32::try_from(first).unwrap_or(0);
            match u32::try_from(last) {
                Ok(last) => Box::new(first..=last),
                Err(_) => Box::new(std::iter::empty()),
            }
        }
    }
}

/// One page's dump, for the format that was asked for.
///
/// The formats we cannot produce yet render as nothing rather than as an
/// error: the page still counts as processed, so the page-count line agrees
/// with the oracle and the harness sees an empty artifact instead of a crash.
fn dump_page(
    page: &PageDict,
    index: u32,
    options: &Options,
    catalog: &Dict,
    r: &impl Resolve,
) -> String {
    match options.format {
        OutputFormat::PageInfo => pageinfo::render(&page.dict, index, r),
        OutputFormat::Structure => structure::render(catalog, page, index, r),
        OutputFormat::None | OutputFormat::Render(_) | OutputFormat::Text | OutputFormat::Annot => {
            String::new()
        }
    }
}

/// Where a file-writing format puts its output: the input's path, which page
/// is being written, and what `--send-events` left that page in.
#[derive(Debug, Clone, Copy)]
struct Output<'a> {
    input: &'a Path,
    index: u32,
    /// The appearance updates this page's event replay produced — the tool's
    /// `FPDF_FFLDraw` input. Empty for every run without `--send-events`.
    updates: &'a [pdfrum::AppearanceUpdate],
    /// Which annotation on this page holds focus, if one does. Decides which
    /// widget is *not* tinted, which no appearance can say for itself.
    focus: Option<pdfrum_doc::ap::Focus>,
}

/// The formats that write a file beside the input rather than to stdout.
///
/// A write that fails is ignored, exactly as the oracle ignores one: it
/// prints a line on stderr and carries on to the next page, because a page
/// that could not be written is not a page that failed to load.
///
/// Returns whatever the format also owes *stdout*, which for `--png --md5` is
/// the `MD5:<path>:<hex>` line the harness and the oracle both print after
/// the file lands.
/// `updates` is what `--send-events` left this page in, and it reaches only
/// the `--png` arm: `FPDF_FFLDraw` is a *bitmap* call, so the event state
/// changes what a page renders and nothing about what `--txt`, `--annot` or
/// `--show-pageinfo` report.
fn write_page_files<R: Resolve>(
    page: &PageDict,
    where_: Output<'_>,
    options: &Options,
    catalog: &Dict,
    r: &R,
    rtl: bool,
    ctx: &mut BuildContext,
) -> String {
    let Output {
        input,
        index,
        updates,
        focus,
    } = where_;
    match options.format {
        OutputFormat::Annot => {
            let Some(path) = annot::output_path(input, index) else {
                return String::new();
            };
            let _ = std::fs::write(path, annot::render(page, catalog, r, ctx));
            String::new()
        }
        OutputFormat::Text => {
            let Some(path) = text::output_path(input, index) else {
                return String::new();
            };
            let extracted = text::extract_page(page, r, rtl, ctx);
            let _ = std::fs::write(path, extracted.to_utf32le());
            String::new()
        }
        OutputFormat::Render("png") => {
            let Some(path) = render::output_path(input, index) else {
                return String::new();
            };
            let backend = render::Backend::resolve(options.use_renderer.as_deref());
            let Some(rendered) = render::render(
                page,
                catalog,
                r,
                render::DEFAULT_SCALE,
                backend,
                ctx,
                updates,
                focus,
            ) else {
                return String::new();
            };
            if std::fs::write(&path, &rendered.png).is_err() {
                return String::new();
            }
            // The oracle prints the hash only when the file was written, and
            // only under `--md5`.
            if options.md5 {
                render::md5_line(&path, &rendered.digest)
            } else {
                String::new()
            }
        }
        OutputFormat::None
        | OutputFormat::PageInfo
        | OutputFormat::Structure
        | OutputFormat::Render(_) => String::new(),
    }
}

/// The oracle's wording for a load failure.
///
/// `PrintLastError` maps PDFium's error codes to these phrases. Our loader's
/// error enum is finer-grained, so each variant is mapped to the code the C++
/// would have set: anything about the file's syntax is a format error, and a
/// refused password is its own code.
fn load_error_text(err: &LoadError) -> &'static str {
    match err {
        LoadError::WrongPassword => "Password required or incorrect password",
        LoadError::UnsupportedEncryption(_) => "Unsupported security scheme",
        LoadError::NotPdf | LoadError::Broken(_) => "File not in PDF format or corrupted",
        // `LoadError` is `#[non_exhaustive]`; a variant added later gets the
        // code PDFium reserves for exactly that case.
        _ => "Unknown error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pdf: &[u8], args: &[&str]) -> (String, String) {
        let options =
            crate::options::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
                .unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        process_file("input.pdf", pdf.to_vec(), &options, &mut streams).unwrap();
        (
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    }

    /// A one-page document whose page carries a `MediaBox` and an `/Info`
    /// dictionary.
    const MINIMAL: &[u8] = b"%PDF-1.7\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 300]>>endobj\n\
4 0 obj<</Title(Hi)>>endobj\n\
trailer<</Root 1 0 R/Info 4 0 R/Size 5>>\n";

    #[test]
    fn a_loadable_file_announces_itself_and_its_page_count() {
        let (out, err) = run(MINIMAL, &[]);
        assert!(err.contains("Processing PDF file input.pdf."), "{err}");
        assert!(err.contains("Processed 1 pages."), "{err}");
        assert_eq!(out, "");
    }

    #[test]
    fn a_file_that_will_not_load_prints_the_error_and_no_page_count() {
        // The absent page-count line is what makes the harness read the file
        // as "did not load", exactly as it does for the 32 corpus files the
        // oracle also refuses.
        let (out, err) = run(b"not a pdf at all", &[]);
        assert!(err.contains("Load pdf docs unsuccessful:"), "{err}");
        assert!(!err.contains("Processed"), "{err}");
        assert_eq!(out, "");
    }

    #[test]
    fn show_metadata_prints_the_info_dictionary() {
        let (out, _) = run(MINIMAL, &["--show-metadata"]);
        assert!(out.starts_with("Title        = Hi (6 bytes)\n"), "{out}");
        assert!(out.contains("ModDate      =  (2 bytes)\n"), "{out}");
        assert_eq!(out.lines().count(), 8);
    }

    #[test]
    fn show_pageinfo_prints_five_lines_per_page() {
        let (out, err) = run(MINIMAL, &["--show-pageinfo"]);
        assert_eq!(
            out,
            "Page 0: MediaBox: 0.00 0.00 200.00 300.00\n\
             Page 0: No CropBox.\n\
             Page 0: No BleedBox.\n\
             Page 0: No TrimBox.\n\
             Page 0: No ArtBox.\n"
        );
        assert!(err.contains("Processed 1 pages."));
    }

    #[test]
    fn metadata_and_pageinfo_never_share_an_invocation_in_practice() {
        // They *can* be combined -- --show-metadata is not an output format --
        // and when they are, the document dump precedes every page's.
        let (out, _) = run(MINIMAL, &["--show-metadata", "--show-pageinfo"]);
        let title = out.find("Title").unwrap();
        let page = out.find("Page 0:").unwrap();
        assert!(title < page, "{out}");
    }

    #[test]
    fn a_page_range_past_the_end_counts_bad_pages() {
        let (out, err) = run(MINIMAL, &["--show-pageinfo", "--pages=0-2"]);
        assert_eq!(out.lines().count(), 5);
        assert!(err.contains("Processed 1 pages."), "{err}");
        assert!(err.contains("Skipped 2 bad pages."), "{err}");
    }

    #[test]
    fn a_single_page_selection_visits_only_that_page() {
        let (out, err) = run(MINIMAL, &["--show-pageinfo", "--pages=0"]);
        assert_eq!(out.lines().count(), 5);
        assert!(!err.contains("Skipped"), "{err}");
    }

    #[test]
    fn selected_pages_without_a_range_covers_the_document() {
        assert_eq!(selected_pages(None, 3).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(selected_pages(None, 0).next().is_none());
    }

    #[test]
    fn a_range_is_inclusive_and_unclamped() {
        assert_eq!(
            selected_pages(Some(PageRange { first: 1, last: 3 }), 2).collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn an_inverted_range_selects_nothing() {
        assert!(
            selected_pages(Some(PageRange { first: 5, last: 2 }), 10)
                .next()
                .is_none()
        );
    }

    #[test]
    fn the_page_loader_accepts_a_leaf_with_no_type_at_all() {
        use pdfrum_object::NoResolve;
        assert!(is_loadable(&Dict::new(), &NoResolve));
    }

    #[test]
    fn the_page_loader_refuses_a_leaf_whose_type_is_not_page() {
        use pdfrum_object::{Name, NoResolve};
        // bad_page_type.pdf's second kid: counted by /Count, refused here.
        let template = Dict::from_pairs([(
            pdfrum_object::names::TYPE.clone(),
            Object::Name(Name::from("Template")),
        )]);
        assert!(!is_loadable(&template, &NoResolve));
        // A /Type that is not a name at all is refused too.
        let numbered = Dict::from_pairs([(pdfrum_object::names::TYPE.clone(), Object::Int(1))]);
        assert!(!is_loadable(&numbered, &NoResolve));
    }

    #[test]
    fn a_type_that_will_not_resolve_refuses_the_page() {
        use pdfrum_object::{NoResolve, ObjRef};
        // Present-but-unresolvable is not the same as absent: the C++ tests
        // the raw entry for presence and the resolved one for the name.
        let dangling = Dict::from_pairs([(
            pdfrum_object::names::TYPE.clone(),
            Object::Ref(ObjRef::new(99, 0)),
        )]);
        assert!(!is_loadable(&dangling, &NoResolve));
    }

    #[test]
    fn a_bad_page_type_document_processes_one_page_and_skips_one() {
        // The whole of bad_page_type.pdf's shape: /Count 2, two kids, the
        // second deliberately mistyped.
        let pdf = b"%PDF-1.7\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Count 2/Kids[3 0 R 4 0 R]>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 200]>>endobj\n\
4 0 obj<</Type/Template/Parent 2 0 R/MediaBox[0 0 200 100]>>endobj\n\
trailer<</Root 1 0 R/Size 5>>\n";
        let (out, err) = run(pdf, &["--show-pageinfo"]);
        assert!(
            out.contains("Page 0: MediaBox: 0.00 0.00 100.00 200.00"),
            "{out}"
        );
        assert!(!out.contains("Page 1:"), "{out}");
        assert!(err.contains("Processed 1 pages."), "{err}");
        assert!(err.contains("Skipped 1 bad pages."), "{err}");
    }

    #[test]
    fn an_unimplemented_format_dumps_nothing_but_still_counts_the_page() {
        let (out, err) = run(MINIMAL, &["--txt"]);
        assert_eq!(out, "");
        assert!(err.contains("Processed 1 pages."), "{err}");
    }

    #[test]
    fn send_events_parses_the_sibling_script_and_leaves_the_page_untouched() {
        let dir = std::env::temp_dir().join(format!(
            "pdfrum-evt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("input.pdf");
        let evt = dir.join("input.evt");
        std::fs::write(&pdf, MINIMAL).unwrap();
        std::fs::write(&evt, "mousemove,10,20\n# skip\ncharcode,97\nunknown,1\n").unwrap();

        let name = pdf.to_string_lossy().into_owned();
        let options = crate::options::parse(&["--send-events".to_owned()]).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        process_file(&name, MINIMAL.to_vec(), &options, &mut streams).unwrap();
        let err = String::from_utf8_lossy(&err);
        assert!(
            err.contains(&format!("Using event file {}.", evt.display())),
            "{err}"
        );
        assert!(
            err.contains(&format!("Sending events from: {}", evt.display())),
            "{err}"
        );
        assert!(err.contains("Sent 2 events."), "{err}");
        assert!(err.contains("Processed 1 pages."), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A two-page document, so the per-page replay has something to be per.
    const TWO_PAGES: &[u8] = b"%PDF-1.7\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 300]>>endobj\n\
4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 300]>>endobj\n\
trailer<</Root 1 0 R/Size 5>>\n";

    /// Runs `--send-events` over a scratch directory holding `pdf` and a
    /// sibling `.evt` with `script`, returning stdout and stderr.
    fn run_with_script(pdf: &[u8], script: &str, args: &[&str]) -> (String, String) {
        let dir = std::env::temp_dir().join(format!(
            "pdfrum-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf_path = dir.join("input.pdf");
        std::fs::write(&pdf_path, pdf).unwrap();
        std::fs::write(dir.join("input.evt"), script).unwrap();

        let mut command_line: Vec<String> = vec!["--send-events".to_owned()];
        command_line.extend(args.iter().map(|a| (*a).to_owned()));
        let options = crate::options::parse(&command_line).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        process_file(
            &pdf_path.to_string_lossy(),
            pdf.to_vec(),
            &options,
            &mut streams,
        )
        .unwrap();
        std::fs::remove_dir_all(&dir).ok();
        (
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    }

    #[test]
    fn the_whole_script_replays_once_per_page_before_that_page_is_written() {
        // Fact 6. `ProcessPage` sends the entire stream as its first
        // statement, for every page, so a two-page document replays a
        // three-line script twice — six sends, not three, and not one pass
        // split across the pages.
        //
        // The count reported is still the *script's* length, because that is
        // what the oracle's `Sent N events.` counts: it is a property of the
        // file, not of how many times it was dispatched.
        let (_, err) = run_with_script(
            TWO_PAGES,
            "mousemove,10,20\nmousedown,left,10,20\nmouseup,left,10,20\n",
            &["--show-pageinfo"],
        );
        assert!(err.contains("Sent 3 events."), "{err}");
        assert!(err.contains("Processed 2 pages."), "{err}");
    }

    #[test]
    fn an_invalid_code_point_is_reported_on_stderr_and_leaves_stdout_alone() {
        // Fact 3's stderr note. It must not reach stdout: that stream is
        // byte-compared against the oracle's, and the oracle prints nothing
        // here.
        let (out, err) = run_with_script(
            MINIMAL,
            "charcode,97\ncharcode,55296\ncharcode,98\n",
            &["--show-pageinfo"],
        );
        assert!(
            err.contains("charcode 55296 is not a Unicode scalar value; skipped."),
            "{err}"
        );
        assert!(!out.contains("charcode"), "{out}");
        // All three lines still *parsed*, so the oracle-compatible count is
        // three: the skip happens at dispatch, not at parse.
        assert!(err.contains("Sent 3 events."), "{err}");
    }

    #[test]
    fn a_script_that_dispatches_does_not_change_the_rendered_page_yet() {
        // The inert-correctness property this slice is scored on: the
        // facade reports every event unhandled, so a page's image with a
        // script is the image without one. When dispatch lands this test is
        // the one that should start failing, and its message says so.
        let (with_events, _) = run_with_script(
            MINIMAL,
            "mousedown,left,10,20\nmouseup,left,10,20\n",
            &["--show-pageinfo"],
        );
        let (without, _) = run(MINIMAL, &["--show-pageinfo"]);
        assert_eq!(
            with_events, without,
            "dispatch is inert today; when it stops being, the renderer must \
             honour the updates before this may differ"
        );
    }

    #[test]
    fn send_events_without_a_sibling_script_counts_zero() {
        // A name with `.pdf` so the C++ replacement runs, but no file at the
        // resulting `.evt` path. `run` uses `input.pdf`, which would pick up
        // a leftover sibling in the working directory.
        let options = crate::options::parse(&["--send-events".to_owned()]).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = Streams {
            out: &mut out,
            err: &mut err,
        };
        process_file(
            "/no-such-pdfrum-evt-sibling.pdf",
            MINIMAL.to_vec(),
            &options,
            &mut streams,
        )
        .unwrap();
        let err = String::from_utf8_lossy(&err);
        assert!(err.contains("Sent 0 events."), "{err}");
        assert!(!err.contains("Using event file"), "{err}");
    }
}
