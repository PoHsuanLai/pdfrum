//! `--js-transcript`: run the document's own JavaScript and print what it
//! asked the host to do.
//!
//! Behind the default-off `script` feature, because the engine is
//! (`scripts/check-no-boa.nu`). The output is `ScriptCascade::transcript_text`
//! verbatim on stdout and nothing else — no page count, no MD5 line — because
//! the harness compares the whole of the oracle's stdout against
//! `<fixture>_expected.txt` and any extra line is a diff.
//!
//! # Which scripts, in which order
//!
//! The document-open sequence is two steps, in this order:
//!
//! 1. The catalog's `/Names /JavaScript` name tree, walked by **index** — so
//!    the order is name-tree order, not the order the objects happen to sit
//!    in the file.
//! 2. The catalog's `/OpenAction`, when it is a **dictionary**. An
//!    `/OpenAction` that is an *array* is a destination and runs nothing at
//!    all. The action chain is then walked depth-first through `/Next`, with
//!    a guard against revisiting a dictionary already run.
//!
//! 3. **Each page is loaded, opened, and closed**, in order. Loading a page
//!    builds its widgets,
//!    and `CPDFSDK_Widget::OnLoad` runs every text field's and combo box's
//!    `/AA /F` formatter (`fpdfsdk/cpdfsdk_widget.cpp:1104-1122`) — which is
//!    why a document whose only script is a formatter prints alerts on open
//!    with no event sent at all. Opening and closing then run the *page's*
//!    own `/AA /O` and `/AA /C` — page-level actions off the page
//!    dictionary, not a field's, and the two spell `/C` differently: on a
//!    page it is Close, on a field it is Calculate.
//! 4. **The sibling `.evt` is replayed against each page**, if there is one.
//!    `testing/tools/test_runner.py`'s `TestText` runs `pdfium_test` with
//!    `--send-events` unconditionally and copies `<test>.evt` next to the PDF
//!    first (`:658-680`), so a javascript fixture's mouse and keyboard script
//!    is part of what produces the expected text.
//!
//! Steps 3 and 4 are per page and interleaved, as `ProcessPage` interleaves
//! them (`pdfium_test.cc:1474-1482`): page 0 loads and replays before page 1
//! loads.
//!
//! # A script that throws
//!
//! It is **reported and the next one still runs** — never swallowed, and
//! never allowed to truncate the rest of the document's scripts. The report
//! goes to **stderr**, because stdout is the byte-for-byte transcript;
//! `pdfrum_form::script::ScriptFailure` carries the reasoning and the
//! `[oracle-bug]` citation for why the oracle's own stdout stays empty.

// Where the two-step sequence above was read in the oracle. Step 1 is
// FORM_DoDocumentJSAction -> CPDFSDK_FormFillEnvironment::ProcJavascriptAction
// (fpdfsdk/fpdf_formfill.cpp:878-885, cpdfsdk_formfillenvironment.cpp:689-701),
// whose walk is LookupValueAndName(i) for i in 0..GetCount(). Step 2 is
// FORM_DoDocumentOpenAction -> ProcOpenAction (fpdf_formfill.cpp:887-894,
// cpdfsdk_formfillenvironment.cpp:704-729), which returns early at :719-721
// for an array /OpenAction; the /Next walk and its revisit guard are
// ExecuteDocumentOpenAction, cpdfsdk_formfillenvironment.cpp:995-1025. The
// clock hooks sit inside `if (options.time > -1)` at pdfium_test.cc:2129-2135.

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
/// oracle's, whose hooks are installed only when the flag was given.
///
/// # Errors
///
/// Only what `out` returns. A script that throws is *not* an error here — it
/// is recorded in `diags` and the next script still runs, per §"A script that
/// throws" below.
pub fn write_transcript(
    doc: &Document,
    facade: Option<&pdfrum::Document>,
    events: &[crate::events::Event],
    time: Option<u64>,
    diags: &mut Diagnostics,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<()> {
    let config = match time {
        Some(seconds) => ScriptConfig::frozen_at(seconds),
        None => ScriptConfig::wall_clock(),
    };
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
    let loaded: Vec<pdfrum_parser::PageDict> = (0..doc.page_count())
        .filter_map(|index| doc.page(index).ok())
        .collect();
    let pages: Vec<Dict> = loaded.iter().map(|page| page.dict.clone()).collect();
    let info = doc.trailer().dict(names::INFO, doc);
    let mut model = pdfrum_form::script::model::read(&catalog, info.as_ref(), &pages, path, doc);
    // The words each page draws, which `Doc.getPageNthWord` indexes into.
    // Read here rather than in the cascade for the same reason everything
    // else in the model is: counting them needs a parsed content stream, and
    // the cascade holds no PDF.
    let mut ctx = pdfrum_page::BuildContext::new();
    model.page_words = loaded
        .iter()
        .map(|page| crate::text::page_words(page, doc, &mut ctx))
        .collect();
    let scripts = document_scripts(&catalog, doc);

    // A session, when the facade could open the same bytes. It is what makes
    // the *field* `/AA` scripts run: loading a page installs them and fires
    // each formatter, and the replay below fires the six pointer and focus
    // ones. Without one the document-level scripts still run — a file the
    // facade refuses is not a file whose `/OpenAction` should be silent.
    let Some(facade) = facade else {
        let Ok(mut cascade) = pdfrum_form::script::ScriptCascade::new(&config) else {
            // `boa` cannot fail to build a context on any input; a failure
            // here is a broken build, and printing a partial transcript would
            // be a wrong answer rather than an absent one.
            return Ok(());
        };
        cascade.set_document(model);
        for (whence, source) in scripts {
            cascade.run(&source, &whence);
        }
        write!(out, "{}", cascade.transcript_text())?;
        return report_failures(&mut cascade, diags, err);
    };

    let Ok(mut session) = pdfrum::FormSession::with_scripts(facade, &config) else {
        return Ok(());
    };
    if let Some(cascade) = session.scripts_mut() {
        cascade.set_document(model);
        // **A script that throws does not stop the ones after it.** `run`
        // records the failure and answers `false`; the loop does not read
        // that answer, which is upstream's shape — `ProcJavascriptAction`
        // walks the name tree `for (i = 0; i < count; ++i)` calling a `void`
        // `DoActionJavaScript` (`cpdfsdk_formfillenvironment.cpp:697-701`),
        // and `ExecuteDocumentOpenAction` runs every `/Next` sub-action
        // unconditionally after the JS (`:1000-1018`). The difference from
        // upstream is only that we write the failure down.
        for (whence, source) in scripts {
            cascade.run(&source, &whence);
        }
    }
    // A document-level script may have called `Field.setFocus`, which records
    // a request rather than moving the keyboard itself. Spending it here is
    // `SetFocusAnnot` running at the end of the native call: the outgoing
    // field's `/AA /Bl` and the incoming field's `/AA /Fo` both fire, and the
    // page holding the field is read if nothing has read it yet.
    session.honour_focus_requests();
    // Then each page in turn: load it — which installs its `/AA` and runs its
    // formatters — and replay the whole event script against it. Both halves
    // are `ProcessPage`'s and in its order.
    for page in 0..doc.page_count() {
        session.load_page(page);
        // `FORM_OnAfterLoadPage` then `FORM_DoPageAAction(…, OPEN)`, which is
        // `GetPage`'s own pair (`pdfium_test.cc:871-872`); the events go
        // between it and the closing action, which `ProcessPage` runs after
        // rendering (`:1646`).
        session.page_opened(page);
        // A page's own `/AA /O` runs outside any event, so its focus request
        // is spent the same way the document-level ones are.
        session.honour_focus_requests();
        if !events.is_empty() {
            crate::dispatch::replay_page(&mut session, page, events, err);
        }
        session.page_closed(page);
    }
    let text = session
        .scripts()
        .map(pdfrum_form::ScriptCascade::transcript_text)
        .unwrap_or_default();
    write!(out, "{text}")?;
    let Some(cascade) = session.scripts_mut() else {
        return Ok(());
    };
    report_failures(cascade, diags, err)
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
