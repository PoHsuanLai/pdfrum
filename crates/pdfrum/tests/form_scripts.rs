//! The `script` feature's seam, end to end: a document's own `/AA` script
//! runs on an event the facade sent, and what it produced comes back.
//!
//! **The whole file is behind `#[cfg(feature = "script")]`** — with the
//! feature off there is no engine at all, which `scripts/check-no-boa.nu`
//! asserts in both directions.
//!
//! # Why an integration test and not a doctest
//!
//! It compiles against `pdfrum` with only `pdfrum` in scope, which is the
//! position a `cargo add pdfrum --features script` caller is in and the only
//! position from which "can a caller reach the document's scripts from the
//! facade alone?" has a truthful answer. Every name below comes from
//! `pdfrum::*`; none of them is spelled `pdfrum_form::…`.
//!
//! # What this proves, and what it deliberately does not
//!
//! It proves the **seam is wired**: `FormSession::with_scripts` reads the
//! document's `/AA` entries, installs them into the cascade against the field
//! ids the routing layer will use, and an event sent through the facade runs
//! them. It does *not* claim the object model is complete — M15 is at step 1
//! and 11 of the oracle's 47 JavaScript fixtures reproduce byte-exactly, so
//! the assertions below are about scripts having run rather than about a
//! transcript matching the oracle's.

#![cfg(feature = "script")]

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

    // Focus alone runs nothing: the hooks are a *commit* cascade, and typing
    // is what reaches the keystroke one.
    assert_eq!(
        session.scripts().expect("a scripted session").transcript(),
        [],
        "no event has reached a hook yet"
    );

    session.character('7', Modifiers::NONE);

    let transcript = session
        .scripts()
        .expect("a scripted session")
        .transcript_text();
    assert!(
        transcript.starts_with("Alert: *** starting test 2 ***\n"),
        "the field's own /AA /K script ran and asked the host to alert; got {transcript:?}"
    );
}

/// **A commit runs the whole cascade**, and the field's four hooks between
/// them produce hundreds of transcript lines where a keystroke produced two.
///
/// Dropping focus is what commits, and committing is what runs
/// `keystroke_commit` → `validate` → `calculate` → `format` in that order.
#[test]
fn committing_runs_the_rest_of_the_cascade() {
    let doc = Document::open(FIXTURE).expect("the public_methods fixture must open");
    let mut session = FormSession::with_scripts(&doc, &ScriptConfig::frozen_at(1_399_672_130))
        .expect("boa builds a realm on any input");

    click(&mut session);
    session.character('7', Modifiers::NONE);
    let after_typing = session
        .scripts()
        .expect("a scripted session")
        .transcript()
        .len();

    let committed = session.blur();
    assert!(committed.consumed);

    let after_commit = session.scripts().expect("a scripted session").transcript();
    assert!(
        after_commit.len() > after_typing,
        "the commit hooks ran: {after_typing} lines before, {} after",
        after_commit.len()
    );
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
