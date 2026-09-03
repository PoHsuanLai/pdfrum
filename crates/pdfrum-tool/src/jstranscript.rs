//! `--js-transcript`: run the document's own JavaScript and print what it
//! asked the host to do.
//!
//! Behind the default-off `script` feature, because the engine is
//! (`scripts/check-no-boa.nu`). The output is `ScriptCascade::transcript_text`
//! verbatim on stdout and nothing else — no page count, no MD5 line — because
//! `testing/tools/text_diff.py` compares the whole of `pdfium_test`'s stdout
//! against `<fixture>_expected.txt` and any extra line is a diff.
//!
//! # Which scripts, in which order
//!
//! `pdfium_test.cc` drives the document-open sequence through two public
//! entry points, in this order:
//!
//! 1. `FORM_DoDocumentJSAction` → `CPDFSDK_FormFillEnvironment::
//!    ProcJavascriptAction` (`fpdfsdk/fpdf_formfill.cpp:878-885`,
//!    `fpdfsdk/cpdfsdk_formfillenvironment.cpp:689-701`): the catalog's
//!    `/Names /JavaScript` name tree, walked by *index* — `LookupValueAndName(i)`
//!    for `i` in `0..GetCount()` — so the order is name-tree order, not the
//!    order the objects happen to sit in the file.
//! 2. `FORM_DoDocumentOpenAction` → `ProcOpenAction`
//!    (`fpdfsdk/fpdf_formfill.cpp:887-894`,
//!    `fpdfsdk/cpdfsdk_formfillenvironment.cpp:704-729`): the catalog's
//!    `/OpenAction`, when it is a **dictionary**. An `/OpenAction` that is an
//!    *array* is a destination, and `ProcOpenAction` returns early without
//!    running anything (`:719-721`). The action chain is then walked
//!    depth-first through `/Next` by `ExecuteDocumentOpenAction`
//!    (`fpdfsdk/cpdfsdk_formfillenvironment.cpp:995-1025`), which guards
//!    against revisiting a dictionary it has already run.
//!
//! Field `/AA` actions are **not** part of this sequence: they run on events,
//! which is `--send-events`' path, not this one.
//!
//! # A script that throws
//!
//! It is **reported and the next one still runs** — never swallowed, and
//! never allowed to truncate the rest of the document's scripts. The report
//! goes to **stderr**, because stdout is the byte-for-byte transcript;
//! `pdfrum_form::script::ScriptFailure` carries the reasoning and the
//! `[oracle-bug]` citation for why the oracle's own stdout stays empty.

use std::io::Write;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::nav::{Action, ActionKind, NameTree};
use pdfrum_form::script::{ScriptCascade, ScriptConfig};
use pdfrum_object::{Dict, Name, Object, names};
use pdfrum_parser::Document;

/// Runs the document's scripts and writes the transcript to `out`.
///
/// A document that carries no script writes nothing at all, which is the
/// answer `bug_1445426`'s missing `_expected.txt` asserts.
///
/// `time` is `--time=`'s value in seconds, and it is the **single source of
/// the scripting clock**: `Some` freezes `Date` and `util.printd` at that
/// instant, `None` leaves them on the machine's real clock. Both are the
/// oracle's, whose hooks are installed only when the flag was given
/// (`testing/pdfium_test/pdfium_test.cc:2129-2135`).
///
/// # Errors
///
/// Only what `out` returns. A script that throws is *not* an error here — it
/// is recorded in `diags` and the next script still runs, per §"A script that
/// throws" below.
pub fn write_transcript(
    doc: &Document,
    time: Option<u64>,
    diags: &mut Diagnostics,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<()> {
    let config = match time {
        Some(seconds) => ScriptConfig::frozen_at(seconds),
        None => ScriptConfig::wall_clock(),
    };
    let Ok(cascade) = ScriptCascade::new(&config) else {
        // `boa` cannot fail to build a context on any input; a failure here
        // is a broken build, and printing a partial transcript would be a
        // wrong answer rather than an absent one.
        return Ok(());
    };
    let mut cascade = cascade;
    let catalog = doc.catalog().unwrap_or_default();
    // The `Doc` object model, installed from the document the tool opened.
    // The cascade holds no PDF — `model::read` is what turns one into the
    // values it answers from — and the *path* is the caller's, because no PDF
    // carries one: under a frozen clock it is the harness's own
    // `myfile.pdf`, which two golden lines pin, and otherwise it is nothing,
    // since a real embedder would pass the file it opened.
    let path = if time.is_some() {
        pdfrum_form::script::GOLDEN_FILE_PATH
    } else {
        ""
    };
    let pages: Vec<Dict> = (0..doc.page_count())
        .filter_map(|index| doc.page(index).ok().map(|page| page.dict.clone()))
        .collect();
    let info = doc.trailer().dict(names::INFO, doc);
    cascade.set_document(pdfrum_form::script::model::read(
        &catalog,
        info.as_ref(),
        &pages,
        path,
        doc,
    ));
    // **A script that throws does not stop the ones after it.** `run` records
    // the failure and answers `false`; the loop does not read that answer,
    // which is upstream's shape — `ProcJavascriptAction` walks the name tree
    // `for (i = 0; i < count; ++i)` calling a `void` `DoActionJavaScript`
    // (`cpdfsdk_formfillenvironment.cpp:697-701`), and
    // `ExecuteDocumentOpenAction` runs every `/Next` sub-action
    // unconditionally after the JS (`:1000-1018`). The difference from
    // upstream is only that we write the failure down.
    for (whence, source) in document_scripts(&catalog, doc) {
        cascade.run(&source, &whence);
    }
    write!(out, "{}", cascade.transcript_text())?;
    report_failures(&mut cascade, diags, err)
}

/// Writes what each stopped script said to `err`, and records the kinds.
///
/// **`err`, never `out`.** The transcript is the oracle's stdout and
/// `testing/tools/text_diff.py` diffs the whole of it, so one extra line
/// there is a failed fixture; the oracle prints nothing for a script that
/// threw (`[oracle-bug]` — see [`pdfrum_form::script::ScriptFailure`]), and
/// matching it on stdout while saying so on stderr is how the transcript
/// stays byte-exact and the error still stops being invisible.
fn report_failures(
    cascade: &mut ScriptCascade,
    diags: &mut Diagnostics,
    err: &mut dyn Write,
) -> std::io::Result<()> {
    for failure in cascade.drain_diagnostics(diags) {
        writeln!(err, "{}", failure.line())?;
    }
    Ok(())
}

/// Every script the document runs on open, in the oracle's order.
///
/// Paired with the name the oracle would use for the script in a diagnostic:
/// its name-tree key, or `/OpenAction`.
fn document_scripts(catalog: &Dict, doc: &Document) -> Vec<(String, String)> {
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let mut found = Vec::new();

    // (1) `/Names /JavaScript`, by index, which is name-tree order.
    if let Some(tree) = NameTree::open(catalog, names::JAVA_SCRIPT, doc) {
        let count = tree.count(doc, &limits, &mut diags);
        for index in 0..count {
            let Some((name, value)) = tree.lookup_by_index(index, doc, &limits, &mut diags) else {
                // `LookupValueAndName` answering nothing yields a null
                // `CPDF_Action`, whose type is not `kJavaScript`, so the
                // oracle skips the slot and keeps walking.
                continue;
            };
            let Object::Dict(dict) = value else {
                continue;
            };
            let action = Action::new(dict);
            if action.kind() != ActionKind::JavaScript {
                continue;
            }
            // `DoActionJavaScript` runs only a non-empty script
            // (`cpdfsdk_formfillenvironment.cpp:912-924`).
            if let Some(source) = action.javascript(doc).filter(|s| !s.is_empty()) {
                found.push((name, source));
            }
        }
    }

    // (2) `/OpenAction`, dictionary-valued only. An `/OpenAction` that is
    // an *array* is a destination, and `ProcOpenAction` returns without
    // running anything (`cpdfsdk_formfillenvironment.cpp:719-721`) —
    // `Dict::dict` answers `None` for an array, which is that same early
    // return.
    // `pdfrum-doc`'s `OPEN_ACTION` is crate-private, so the key is spelled
    // out here rather than reaching into another crate's private table.
    let open_action = Name::new(*b"OpenAction");
    if let Some(dict) = catalog.dict(&open_action, doc) {
        let root = Action::new(dict);
        // The action itself, then everything `/Next` reaches.
        // `ExecuteDocumentOpenAction` recurses through `/Next` depth-first
        // with a visited set (`cpdfsdk_formfillenvironment.cpp:995-1025`),
        // which is exactly what `Action::chain` walks.
        for action in std::iter::once(root.clone()).chain(root.chain(doc, &limits, &mut diags)) {
            if action.kind() != ActionKind::JavaScript {
                continue;
            }
            if let Some(source) = action.javascript(doc).filter(|s| !s.is_empty()) {
                // `RunDocumentOpenJavaScript(WideString(), swJS)` — the open
                // action's script is run with an *empty* name
                // (`cpdfsdk_formfillenvironment.cpp:1000-1006`).
                found.push((String::new(), source));
            }
        }
    }
    found
}
