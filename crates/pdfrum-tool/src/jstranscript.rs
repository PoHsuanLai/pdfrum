//! `--js-transcript`: run the document's own JavaScript and print what it
//! asked the host to do.
//!
//! Behind the default-off `script` feature, because the engine is
//! (`scripts/check-no-boa.sh`). The output is `ScriptCascade::transcript_text`
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
pub fn write_transcript(doc: &Document, out: &mut dyn Write) -> std::io::Result<()> {
    let Ok(cascade) = ScriptCascade::new(&ScriptConfig::for_goldens()) else {
        // `boa` cannot fail to build a context on any input; a failure here
        // is a broken build, and printing a partial transcript would be a
        // wrong answer rather than an absent one.
        return Ok(());
    };
    let mut cascade = cascade;
    let catalog = doc.catalog().unwrap_or_default();
    for (whence, source) in document_scripts(&catalog, doc) {
        cascade.run(&source, &whence);
    }
    write!(out, "{}", cascade.transcript_text())
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
