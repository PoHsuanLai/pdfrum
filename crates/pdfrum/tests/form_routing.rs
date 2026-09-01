//! End-to-end form routing: a click focuses, typing edits, undo restores.
//!
//! These assertions were unwritable while the facade's dispatch was inert —
//! every one of them would have passed vacuously against a session that
//! consumed nothing and changed nothing, which is why the crate's earlier
//! tests all drove the edit control directly instead. They drive the
//! **facade**, which is the surface a bridge actually calls.
//!
//! The fixture is `text_form.pdf`: one text field, `/Rect [100 100 200 130]`,
//! `/DA (0 0 0 rg /F1 12 Tf)` over Helvetica. A click at (120, 115) lands
//! inside it, and (10, 10) lands on bare page.

use pdfrum::{Document, EventModifiers, FormSession, VirtualKey};

/// The fixture, or a failed test rather than four green-and-empty ones.
fn document() -> Document {
    match Document::open("tests/fixtures/text_form.pdf") {
        Ok(doc) => doc,
        Err(error) => panic!("the text_form fixture must open: {error}"),
    }
}

/// A point inside the field, and one outside every annotation.
const INSIDE: (f32, f32) = (120.0, 115.0);
const OUTSIDE: (f32, f32) = (10.0, 10.0);

/// Clicks at a point, the way a real event stream does: move, down, up.
fn click(session: &mut FormSession<'_>, at: (f32, f32)) {
    session.on_mouse_move(0, at.0, at.1, EventModifiers::NONE);
    session.on_mouse_down(0, at.0, at.1, EventModifiers::NONE);
    session.on_mouse_up(0, at.0, at.1, EventModifiers::NONE);
}

fn type_text(session: &mut FormSession<'_>, text: &str) {
    for ch in text.chars() {
        session.on_char(ch, EventModifiers::NONE);
    }
}

/// The assertion the whole routing layer exists to make true.
#[test]
fn a_click_focuses_the_field_under_it() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    assert!(session.focused_annot().is_none());

    click(&mut session, INSIDE);

    let focused = session.focused_annot().expect("the click must focus");
    assert_eq!(focused.page, 0);
    // The field is the page's only annotation, at raw /Annots index 0.
    assert_eq!(focused.index, 0);
}

#[test]
fn a_click_on_bare_page_focuses_nothing() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, OUTSIDE);
    assert!(session.focused_annot().is_none());
}

/// Typing reaches the focused field, and reading it back answers what was
/// typed rather than what the file stores.
#[test]
fn typing_after_a_click_reaches_the_field() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, INSIDE);
    type_text(&mut session, "Hello");

    assert_eq!(session.focused_text().as_deref(), Some("Hello"));
}

/// Typing before any click goes nowhere, and does not invent a focus.
#[test]
fn typing_with_no_focus_changes_nothing() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    let response = session.on_char('x', EventModifiers::NONE);

    assert!(!response.consumed);
    assert!(response.updates.is_empty());
    assert!(session.focused_annot().is_none());
    assert!(session.focused_text().is_none());
}

/// The worked example the milestone's exit criterion asks for, as an
/// assertion rather than only as prose: click, type, read, undo.
#[test]
fn click_type_read_undo_is_one_round_trip() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, INSIDE);
    type_text(&mut session, "abc");
    assert_eq!(session.focused_text().as_deref(), Some("abc"));

    // Three characters are three undo items, so one undo drops one of them.
    assert!(session.can_undo());
    session.on_key_down(VirtualKey::Z, EventModifiers::CONTROL);
    assert_eq!(session.focused_text().as_deref(), Some("ab"));

    // And redo puts it back.
    assert!(session.can_redo());
    session.on_key_down(VirtualKey::Y, EventModifiers::CONTROL);
    assert_eq!(session.focused_text().as_deref(), Some("abc"));
}

/// A field keeps what was typed into it across a focus change, which is what
/// makes clicking away and back non-destructive.
#[test]
fn a_field_keeps_its_text_across_a_focus_change() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, INSIDE);
    type_text(&mut session, "kept");
    click(&mut session, OUTSIDE);
    assert!(session.focused_annot().is_none());

    click(&mut session, INSIDE);
    assert_eq!(session.focused_text().as_deref(), Some("kept"));
}

/// Dropping focus produces an appearance for the field that lost it — the
/// update a renderer needs to draw the committed value.
#[test]
fn dropping_focus_produces_an_appearance_for_the_field_that_lost_it() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, INSIDE);
    type_text(&mut session, "value");

    let response = session.force_kill_focus();
    assert!(session.focused_annot().is_none());
    assert!(
        !response.updates.is_empty(),
        "losing focus must report what changed"
    );
}

/// A click reports itself consumed, which is the answer a bridge branches on.
#[test]
fn a_click_inside_a_widget_is_consumed_and_one_outside_is_not() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    let inside = session.on_mouse_down(0, INSIDE.0, INSIDE.1, EventModifiers::NONE);
    assert!(inside.consumed);

    // A click on bare page with nothing focused has nothing to drop, so it
    // changes nothing — the honest answer is "not handled".
    let mut fresh = FormSession::new(&doc);
    let outside = fresh.on_mouse_down(0, OUTSIDE.0, OUTSIDE.1, EventModifiers::NONE);
    assert!(!outside.consumed);
}

/// Select-all then typing replaces the whole value, which is the shortcut
/// path rather than the character path.
#[test]
fn select_all_then_typing_replaces_the_value() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, INSIDE);
    type_text(&mut session, "old");

    session.on_key_down(VirtualKey::A, EventModifiers::CONTROL);
    type_text(&mut session, "new");
    assert_eq!(session.focused_text().as_deref(), Some("new"));
}

/// An event naming a page the document does not have answers rather than
/// panicking — the property that outranks every behavioural one.
#[test]
fn an_event_on_a_missing_page_answers_rather_than_panicking() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    let response = session.on_mouse_down(999, 10.0, 10.0, EventModifiers::NONE);
    assert!(!response.consumed);
    assert!(session.focused_annot().is_none());
}
