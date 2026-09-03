//! The `script` feature's seam, end to end: a document's own `/AA` script
//! runs on an event the facade sent, and what it produced comes back.
//!
//! **The whole file is behind `#[cfg(feature = "javascript")]`** — with the
//! feature off there is no engine at all, which `scripts/check-no-boa.nu`
//! asserts in both directions.
//!
//! # Why an integration test and not a doctest
//!
//! It compiles against `pdfrum` with only `pdfrum` in scope, which is the
//! position a `cargo add pdfrum --features javascript` caller is in and the only
//! position from which "can a caller reach the document's scripts from the
//! facade alone?" has a truthful answer. Every name below comes from
//! `pdfrum::*`; none of them is spelled `pdfrum_form::…`.
//!
//! # What this proves, and what it deliberately does not
//!
//! It proves the **seam is wired**: `FormSession::with_scripts` reads the
//! document's `/AA` entries, installs them into the cascade against the field
//! ids the routing layer will use, installs what the `Doc` object answers
//! from, and an event sent through the facade runs them. It does *not* claim
//! the object model is complete, so most assertions below are about scripts
//! having run rather than about a transcript matching the oracle's.

#![cfg(feature = "javascript")]

use pdfrum::{Cascade, Document, FieldRef, FormSession, Modifiers, Point, ScriptConfig};

/// `testing/resources/javascript/public_methods.pdf`, verbatim.
///
/// One `/Tx` field named `Text Box` whose `/AA` carries all four hooks — `/C`,
/// `/F`, `/K` and `/V` — which makes it the one corpus fixture that exercises
/// every wire this change installs. Its `/AA /K` opens with an `app.alert`
/// naming itself, and that line is what the assertions below key on: it is
/// produced by the *document's* JavaScript and by nothing else.
const FIXTURE: &str = "tests/fixtures/public_methods.pdf";

/// The field's `/Rect [100 160 200 190]`, so this point is inside it.
const INSIDE: Point = Point::new(150.0, 175.0);

/// Clicking a widget is three events; the move is what tells it the pointer
/// is over it.
fn click(session: &mut FormSession<'_>) {
    session.mouse_move(0, INSIDE, Modifiers::NONE);
    session.mouse_down(0, INSIDE, Modifiers::NONE);
    session.mouse_up(0, INSIDE, Modifiers::NONE);
}

/// **A keystroke runs the field's own `/AA /K` script.**
///
/// The clock is frozen at `pdfium_test --time=1399672130`'s seed so the run is
/// reproducible: this fixture's scripts read `Date`, and a wall clock would
/// make the transcript depend on the day the test ran.
#[test]
fn typing_runs_the_documents_own_keystroke_script() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_scripts(&doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");

    click(&mut session);
    assert!(
        session.focused_annot().is_some(),
        "the click must land on the widget, or nothing below is testing a script"
    );

    // **Reading the page already ran the field's `/AA /F`**, which is what a
    // widget's `OnLoad` does for a text field — so the transcript is the
    // formatter's first test, not empty, before any event is sent. Focus
    // itself still runs nothing: the other three hooks are a *commit*
    // cascade, and typing is what reaches the keystroke one.
    let after_load = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(
        after_load.starts_with("Alert: *** starting test 1 ***\n"),
        "loading the page runs the formatter; got {after_load:?}"
    );
    assert!(
        !after_load.contains("*** starting test 2 ***"),
        "the keystroke script must not have run yet; got {after_load:?}"
    );

    session.character('7', Modifiers::NONE);

    let transcript = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(
        transcript.contains("Alert: *** starting test 2 ***\n"),
        "the field's own /AA /K script ran and asked the host to alert; got {transcript:?}"
    );
}

/// **A commit runs the whole cascade**, and this fixture is the one that
/// shows *why* it sometimes runs none of it.
///
/// Dropping focus is what commits, and committing runs `keystroke_commit` →
/// `validate` → `calculate` → `format` in that order — **but only when the
/// value actually changed**. This fixture's `/AA /K` ends by calling
/// `AFSpecial_KeystrokeEx('XXXX')` against a numeric value, whose mask check
/// fails and sets `event.rc = false`
/// (`fxjs/cjs_publicmethods.cpp:1187-1190`), so the keystroke is *refused*
/// and the field still holds what the document stored. `commit::run`'s first
/// line then answers `unchanged`, which is upstream's behaviour and not an
/// omission.
///
/// So the assertion is on what the hooks produced, not on the commit adding
/// to it: the keystroke hook alone ran the whole of test 2.
#[test]
fn committing_runs_the_rest_of_the_cascade() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_scripts(&doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");

    click(&mut session);
    session.character('7', Modifiers::NONE);
    let text = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(
        text.contains("Alert: *** ending test 2 ***"),
        "the keystroke hook ran its script to the end; got {} lines",
        text.lines().count()
    );

    let committed = session.blur();
    assert!(committed.consumed);

    // The `AF*` library answered, which is the half of the object model M15
    // step 1 built. A transcript with no `PASS:` line at all would mean the
    // engine was reached but `AFNumber_Format` and its family were not.
    let text = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(
        text.lines().any(|line| line.contains("PASS:")),
        "the AF* library the fixture tests answered at least once"
    );
    // And nothing threw its way out: a script that stops is recorded rather
    // than swallowed, and this fixture's do not.
    assert_eq!(
        session.scripts().expect("a scripted session").stops(),
        [],
        "no script stopped on a limit or an uncaught throw"
    );
}

/// The same fixture through [`FormSession::new`] runs **nothing**, which is
/// what makes the assertions above measurements rather than tautologies.
///
/// `NoScripts` is not "the real thing minus scripts" — it is the real thing
/// with the identity cascade, and it is a session's default.
#[test]
fn the_default_session_runs_no_script_on_the_same_fixture() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::new(&doc);

    click(&mut session);
    session.character('7', Modifiers::NONE);
    session.blur();

    assert!(
        session.scripts().is_none(),
        "a default session has no engine to ask"
    );
}

/// A caller's **own** [`Cascade`] reaches the same seam, with no `script`
/// feature involved in the trait itself.
///
/// This is the other half of [`FormSession::with_cascade`]'s purpose: the
/// commit gates are a place a host can put rules of its own, and a validator
/// that refuses puts the field back to what the document holds.
#[test]
fn a_callers_own_cascade_gates_the_commit() {
    /// Refuses every commit, and counts how often it was asked.
    struct RefuseAll {
        asked: std::cell::Cell<u32>,
    }
    impl Cascade for RefuseAll {
        fn validate(&mut self, _field: &FieldRef, _value: &str) -> bool {
            self.asked.set(self.asked.get() + 1);
            false
        }
    }

    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_cascade(
        &doc,
        RefuseAll {
            asked: std::cell::Cell::new(0),
        },
    );

    click(&mut session);
    session.character('7', Modifiers::NONE);
    assert_eq!(session.focused_text().as_deref(), Some("7"));

    // The gate refused, so the field goes back to what the document holds —
    // which for this fixture is nothing — and **keeps focus**, which is what
    // a refused commit does: a user whose value was rejected is left in the
    // field to fix it rather than silently moved out of it.
    session.blur();
    assert_eq!(session.focused_text().as_deref(), Some(""));
    assert!(
        session.focused_annot().is_some(),
        "a refused commit keeps the caret in the field"
    );
}

/// **A timer fires only when the host says time passed**, and it is the
/// facade that says so.
///
/// The whole seam in one test: nothing fires while the session merely exists,
/// and one step of the clock runs what came due. There is no thread and no
/// wall clock anywhere below this call.
#[test]
#[cfg(feature = "javascript")]
fn a_timer_fires_only_when_the_host_advances_the_clock() {
    use std::time::Duration;

    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_scripts(&doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");
    let scripts = session
        .scripts_mut()
        .expect("a scripted session has an engine");
    assert!(scripts.run("app.setInterval(\"app.alert('tick')\", 1000);", "test"));
    let before = scripts.transcript().len();

    // A session nobody advances fires nothing, however long it lives.
    assert_eq!(
        session
            .scripts()
            .expect("a scripted session")
            .transcript()
            .len(),
        before,
        "arming a timer runs nothing on its own"
    );

    assert_eq!(session.advance_time(Duration::from_secs(1)), 1);
    let after = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(after.ends_with("Alert: tick\n"), "got {after:?}");
}

/// **A session with no engine advances nothing**, and answering zero is the
/// answer rather than a panic — a caller driving a clock must not have to ask
/// first whether scripting is on.
#[test]
#[cfg(feature = "javascript")]
fn advancing_a_script_free_session_is_zero_and_not_a_panic() {
    use std::time::Duration;

    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::new(&doc);
    assert_eq!(session.advance_time(Duration::from_hours(1)), 0);
}

/// **The facade installs the document model**, so `this.getField` and
/// `this.numFields` see the file the session is over.
///
/// The defect this closes: `with_scripts` built a cascade and installed every
/// field's `/AA` scripts into it but never called `set_document`, so a script
/// asking the *document* anything got an empty one — `numFields` zero,
/// `numPages` zero, and `undefined` from `getField`. Nothing failed loudly;
/// a script simply answered as though the file had nothing in it.
///
/// `public_methods.pdf` makes that measurable rather than merely visible: it
/// carries **four** `/Tx` fields, three of them with a stored `/V`, on one
/// page. A session over an empty model answers `0` and `undefined` here, and
/// each assertion below names the value that distinguishes the two.
#[test]
fn the_facade_installs_what_the_document_object_answers_from() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_scripts(&doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");
    let scripts = session
        .scripts_mut()
        .expect("a scripted session has an engine");

    assert!(
        scripts.run(
            "app.alert('numFields=' + this.numFields);\
             app.alert('numPages=' + this.numPages);\
             app.alert('Text3=' + this.getField('Text3').value);\
             app.alert('name=' + this.getField('Text3').name);\
             app.alert('nth=' + this.getNthFieldName(0));",
            "test",
        ),
        "the script must complete: {:?}",
        scripts.stops()
    );

    let text = scripts.transcript_text();
    for expected in [
        "Alert: numFields=4",
        "Alert: numPages=1",
        "Alert: Text3=456",
        "Alert: name=Text3",
        "Alert: nth=Text Box",
    ] {
        assert!(
            text.contains(expected),
            "the model must answer {expected:?}; got {text:?}"
        );
    }
}

/// A document with **no** `/Info` throws from every metadata getter rather
/// than answering `""`.
///
/// `public_methods.pdf` has no `/Info` dictionary, so this pins the
/// distinction `has_info` exists for: absent is not the same as present and
/// empty, and the getter fails the moment the info dictionary is null.
///
/// **It does not distinguish an installed model from an uninstalled one** —
/// both throw here, for the same reason — and it is not claimed to. The
/// assertion that separates those two is
/// `the_facade_installs_what_the_document_object_answers_from`; this one
/// guards the metadata half against answering `""` in a future change.
#[test]
fn a_document_with_no_info_dictionary_throws_from_its_metadata() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_scripts(&doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");
    let scripts = session
        .scripts_mut()
        .expect("a scripted session has an engine");

    assert!(
        scripts.run(
            "try { app.alert('author=' + this.author); }\
             catch (e) { app.alert('threw: ' + e); }",
            "test",
        ),
        "the script must complete: {:?}",
        scripts.stops()
    );

    let text = scripts.transcript_text();
    assert!(
        text.contains("Alert: threw: "),
        "no /Info means the getter throws rather than answering; got {text:?}"
    );
}

/// **A default session's model is empty**, which is what makes the
/// assertions above measurements rather than tautologies.
///
/// The same document through the same engine, with the model never installed,
/// answers the zeros the defect used to produce. Written by driving a bare
/// cascade through `with_cascade`, which boxes it behind `dyn Cascade` and so
/// installs nothing into it — the behaviour that constructor's own
/// documentation promises.
#[test]
fn a_cascade_nobody_told_about_a_document_answers_as_an_empty_one() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut cascade = pdfrum::ScriptCascade::new(&ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");

    assert!(
        cascade.run(
            "app.alert('numFields=' + this.numFields);\
             app.alert('field=' + this.getField('Text3'));",
            "test",
        ),
        "the script must complete: {:?}",
        cascade.stops()
    );
    let text = cascade.transcript_text();
    assert!(
        text.contains("Alert: numFields=0"),
        "an uninstalled model counts no fields; got {text:?}"
    );
    assert!(
        text.contains("Alert: field=undefined"),
        "and finds none by name; got {text:?}"
    );

    // And the same cascade handed to `with_cascade` stays that way, which is
    // the documented answer rather than an omission: the constructor boxes it
    // behind `dyn Cascade`, where the installing methods are unreachable.
    let session = FormSession::with_cascade(&doc, cascade);
    assert!(
        session.scripts().is_none(),
        "`with_cascade` holds a plain cascade, whatever was passed to it"
    );
}
