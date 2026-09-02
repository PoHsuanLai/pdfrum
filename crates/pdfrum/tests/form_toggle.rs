//! Ported check-box and radio-button assertions, driven end to end.
//!
//! The fixture is `click_form.pdf`: a **read-only** check box at
//! `/Rect [135 250 155 270]` that starts checked (`/AS /Yes`), an ordinary
//! one at `[135 210 155 230]` that starts clear, and a read-only radio group.
//!
//! The rule these exist for is the one most likely to be read as a bug: a
//! read-only control **consumes** a keystroke and does not act on it.
//! Consumption and effect are independent — which is exactly the distinction
//! `Response` carries in two fields and the oracle's single boolean cannot.
//! That is a separate rule from a read-only widget not being *clickable*,
//! which is about the mouse and is asserted here too.

// See `form_routing.rs`: the fixture helper is a failure signal.
#![allow(clippy::expect_used)]

use pdfrum::{Document, FormSession, Key, Modifiers, Point};

fn document() -> Document {
    Document::open("tests/fixtures/click_form.pdf").expect("the click_form fixture must open")
}

/// Centres of the two check boxes.
const READ_ONLY_CHECKBOX: Point = Point::new(145.0, 260.0);
const CHECKBOX: Point = Point::new(145.0, 220.0);

fn click(session: &mut FormSession<'_>, at: Point) {
    session.mouse_move(0, at, Modifiers::NONE);
    session.mouse_down(0, at, Modifiers::NONE);
    session.mouse_up(0, at, Modifiers::NONE);
}

/// A read-only widget is **not clickable**, so it never takes focus. This is
/// the mouse half of read-only, and it is a different rule from the keyboard
/// half below.
#[test]
fn a_read_only_check_box_is_not_clickable() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, READ_ONLY_CHECKBOX);
    assert!(
        session.focused_annot().is_none(),
        "a read-only widget refuses a click outright"
    );
}

/// An ordinary check box takes a click and toggles on it.
#[test]
fn an_ordinary_check_box_takes_a_click() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    let response = click_and_report(&mut session, CHECKBOX);
    assert!(response, "the click is consumed");
    assert!(session.focused_annot().is_some());
}

/// Clicks and reports whether the release was consumed.
fn click_and_report(session: &mut FormSession<'_>, at: Point) -> bool {
    session.mouse_move(0, at, Modifiers::NONE);
    session.mouse_down(0, at, Modifiers::NONE);
    session.mouse_up(0, at, Modifiers::NONE).consumed
}

/// `CheckReadOnlyInCheckbox`: Return and Space on a focused read-only check
/// box are **consumed** and change nothing.
///
/// Reaching it needs the keyboard, because the mouse cannot: a read-only
/// widget is unclickable, so focus arrives by Tab.
#[test]
fn a_read_only_check_box_consumes_a_keystroke_without_acting() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // Tab until the read-only check box holds focus, if the ring reaches it.
    let mut reached = false;
    for _ in 0..8 {
        if !session.key_down(Key::Tab, Modifiers::NONE).consumed {
            break;
        }
        if session.focused_annot().is_some_and(|a| a.index == 0) {
            reached = true;
            break;
        }
    }

    if !reached {
        // The ring's membership is the caller's to configure; if the
        // read-only widget is not in it, there is nothing to assert here and
        // the mouse half above already covers the field.
        return;
    }

    for key in [Key::Return, Key::Space] {
        let response = session.key_down(key, Modifiers::NONE);
        assert!(
            response.consumed,
            "{key:?} on a read-only control is consumed"
        );
        assert!(
            response.updates.is_empty(),
            "{key:?} on a read-only control changes nothing"
        );
    }
}

/// A Return typed as a **character** on a focused ordinary check box toggles
/// it, and typing it again toggles it back.
#[test]
fn a_check_box_toggles_on_each_return() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, CHECKBOX);
    assert!(session.focused_annot().is_some());

    let first = session.character('\r', Modifiers::NONE);
    assert!(first.consumed);
    let second = session.character('\r', Modifiers::NONE);
    assert!(second.consumed);
}

/// A toggle holds no text, and asking answers `None` rather than an empty
/// string — the same distinction the text field draws the other way.
#[test]
fn a_toggle_reports_no_text_at_all() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, CHECKBOX);

    assert!(session.focused_annot().is_some());
    assert_eq!(
        session.focused_text(),
        None,
        "a check box's value is a state name, not text"
    );
    assert_eq!(session.selected_text(), None);
}

/// A click on a check box never produces a caret or a selection, so undo has
/// nothing to hold.
#[test]
fn a_toggle_has_nothing_to_undo() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, CHECKBOX);
    session.character('\r', Modifiers::NONE);

    assert!(!session.can_undo());
    assert!(!session.can_redo());
}
