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

// `clippy.toml`'s `allow-expect-in-tests` covers `#[test]` bodies but not the
// helpers beside them, and a fixture that will not open is a failure signal
// rather than a case to handle — the same bargain `facade.rs` makes.
#![allow(clippy::expect_used)]

use pdfrum::{Document, EventModifiers, FormSession, VirtualKey};

/// The fixture, or a failed test rather than four green-and-empty ones.
fn document() -> Document {
    Document::open("tests/fixtures/text_form.pdf").expect("the text_form fixture must open")
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
    assert_eq!(focused.page, pdfrum::PageIndex::FIRST);
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

// --- Ported mouse-driven selection assertions -------------------------------
//
// `GetSelectedTextEmptyAndBasicMouse` and `GetSelectedTextFragmentsMouse`
// drag through the text field and read the selection back. The upstream
// fixture is `text_form_multiple.pdf`, whose "Text Box" carries the same
// `/Rect [100 100 200 130]` and `/DA` as this one's, so the x coordinates
// below are the upstream constants unchanged: `kFormBeginX = 102`,
// `kFormEndX = 195`, `kRegularFormY = 115`.

/// The upstream fixture constants.
const FORM_BEGIN_X: f32 = 102.0;
const FORM_END_X: f32 = 195.0;
const FORM_Y: f32 = 115.0;

/// `SelectTextWithMouse`: press at `from`, move to `to`, release. The move
/// while the button is down is what extends the selection — without it the
/// drag is two clicks and selects nothing.
fn drag(session: &mut FormSession<'_>, from: f32, to: f32) {
    session.on_mouse_move(0, from, FORM_Y, EventModifiers::NONE);
    session.on_mouse_down(0, from, FORM_Y, EventModifiers::NONE);
    session.on_mouse_move(0, to, FORM_Y, EventModifiers::NONE);
    session.on_mouse_up(0, to, FORM_Y, EventModifiers::NONE);
}

/// Types `count` characters starting at `'A'`, the upstream helper's run.
fn type_run(session: &mut FormSession<'_>, count: u32) {
    session.on_mouse_move(0, FORM_BEGIN_X, FORM_Y, EventModifiers::NONE);
    session.on_mouse_down(0, FORM_BEGIN_X, FORM_Y, EventModifiers::NONE);
    session.on_mouse_up(0, FORM_BEGIN_X, FORM_Y, EventModifiers::NONE);
    for i in 0..count {
        let ch = char::from_u32(u32::from(b'A') + i).unwrap_or('?');
        session.on_char(ch, EventModifiers::NONE);
    }
}

/// `GetSelectedTextEmptyAndBasicMouse`: nothing selected until something is.
#[test]
fn a_drag_selects_what_it_crosses() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // Before any focus, both answers are "no field", not "empty field".
    assert!(session.focused_text().is_none());
    assert!(session.selected_text().is_none());

    type_run(&mut session, 3);
    assert_eq!(session.focused_text().as_deref(), Some("ABC"));

    // A drag back across all three selects all three.
    drag(&mut session, 125.0, FORM_BEGIN_X);
    assert_eq!(session.selected_text().as_deref(), Some("ABC"));
}

/// A focused field with nothing selected answers `Some("")` — distinguishable
/// from no focus at all, which the oracle's byte-length return cannot express.
#[test]
fn an_empty_selection_is_distinguishable_from_no_field() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 3);

    // Clicking without dragging leaves a caret, not a selection.
    drag(&mut session, 125.0, 125.0);
    assert_eq!(session.selected_text().as_deref(), Some(""));
    assert!(session.focused_text().is_some());
}

/// `GetSelectedTextFragmentsMouse`: a drag selects the same run in either
/// direction, which is the property the whole test exists for.
#[test]
fn a_drag_selects_the_same_run_in_either_direction() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 12);
    assert_eq!(session.focused_text().as_deref(), Some("ABCDEFGHIJKL"));

    let backwards = {
        drag(&mut session, 170.0, 125.0);
        session.selected_text()
    };
    let forwards = {
        drag(&mut session, 125.0, 170.0);
        session.selected_text()
    };
    assert_eq!(
        backwards, forwards,
        "a drag must select the same run whichever way it is made"
    );
    assert!(
        backwards.is_some_and(|text| !text.is_empty()),
        "and it must select something"
    );
}

/// A drag across the whole field selects the whole value.
#[test]
fn a_drag_across_the_whole_field_selects_all_of_it() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 12);

    drag(&mut session, FORM_END_X, FORM_BEGIN_X);
    assert_eq!(session.selected_text().as_deref(), Some("ABCDEFGHIJKL"));
}

/// `DoubleClickInTextField`: a double click selects **the whole line**, not
/// the word under the cursor.
#[test]
fn a_double_click_selects_the_whole_line() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 0);
    for ch in "Hello World".chars() {
        session.on_char(ch, EventModifiers::NONE);
    }

    session.on_double_click(0, 130.0, FORM_Y, EventModifiers::NONE);
    assert_eq!(session.selected_text().as_deref(), Some("Hello World"));
}

// --- Ported insert-and-delete-by-selection assertions -----------------------
//
// The `InsertTextIn*` and `DeleteTextField*` families drive `ReplaceSelection`
// against a selection the mouse made. They are the reason the facade has
// `replace_selection` at all: this crate holds no clipboard, so a cut is a
// caller reading `selected_text` and then replacing it with nothing.

/// `InsertTextInPopulatedTextFieldLeft`: inserting at a click before the text.
#[test]
fn text_replaced_at_a_caret_is_inserted_there() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 8);
    assert_eq!(session.focused_text().as_deref(), Some("ABCDEFGH"));

    // A click with no drag leaves a caret and no selection.
    drag(&mut session, FORM_BEGIN_X, FORM_BEGIN_X);
    assert_eq!(session.selected_text().as_deref(), Some(""));

    assert!(session.replace_selection("Hello"));
    assert_eq!(session.focused_text().as_deref(), Some("HelloABCDEFGH"));
}

/// `InsertTextInPopulatedTextFieldRight`: the same at the far end.
#[test]
fn text_replaced_at_the_end_is_appended() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 8);

    drag(&mut session, FORM_END_X, FORM_END_X);
    assert!(session.replace_selection("Hello"));
    assert_eq!(session.focused_text().as_deref(), Some("ABCDEFGHHello"));
}

/// `DeleteTextFieldEntireSelection`: replacing everything with nothing empties
/// the field.
#[test]
fn deleting_the_whole_selection_empties_the_field() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 12);

    drag(&mut session, FORM_END_X, FORM_BEGIN_X);
    assert_eq!(session.selected_text().as_deref(), Some("ABCDEFGHIJKL"));

    assert!(session.replace_selection(""));
    assert_eq!(session.focused_text().as_deref(), Some(""));
}

/// `DeleteEmptyTextFieldSelection`: deleting an empty selection has no effect,
/// and reports that it had none.
#[test]
fn deleting_an_empty_selection_does_nothing() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 12);
    assert_eq!(session.selected_text().as_deref(), Some(""));

    assert!(
        !session.replace_selection(""),
        "an empty replacement of an empty selection changes nothing"
    );
    assert_eq!(session.focused_text().as_deref(), Some("ABCDEFGHIJKL"));
}

/// `DeleteTextFieldSelectionMiddle`: a middle run removed leaves both ends.
#[test]
fn deleting_a_middle_selection_leaves_both_ends() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 12);

    drag(&mut session, 170.0, 125.0);
    let selected = session
        .selected_text()
        .expect("the drag selected something");
    assert!(!selected.is_empty());

    assert!(session.replace_selection(""));
    let left = session.focused_text().expect("the field still has text");
    assert_eq!(
        left,
        "ABCDEFGHIJKL".replace(&selected, ""),
        "exactly the selected run is removed"
    );
}

/// Replacing a selection is one undo step, not one per character.
#[test]
fn replacing_a_selection_undoes_as_one_step() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 8);

    drag(&mut session, FORM_END_X, FORM_BEGIN_X);
    session.replace_selection("xyz");
    assert_eq!(session.focused_text().as_deref(), Some("xyz"));

    session.on_key_down(VirtualKey::Z, EventModifiers::CONTROL);
    assert_eq!(
        session.focused_text().as_deref(),
        Some("ABCDEFGH"),
        "a replacement undoes as one step, not three"
    );
}

/// A field with no focus refuses a replacement rather than inventing one.
#[test]
fn replacing_with_no_focus_refuses() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    assert!(!session.replace_selection("Hello"));
    assert!(session.focused_text().is_none());
}

// --- Ported SelectAllText -------------------------------------------------

/// `SelectAllText`: focus, insert, check there is no selection, select all,
/// check there is.
///
/// The upstream row asserts **UTF-16 byte lengths** — 12 for `"Hello"`
/// (5 x 2 plus a terminator) and 2 for the empty string. Those numbers are
/// facts about a C string API rather than about forms, so they are restated
/// here as the values they encode; what survives is the distinction the
/// lengths were carrying, which is that an empty selection and no field at
/// all are different answers.
#[test]
fn select_all_selects_the_whole_value() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // `FORM_OnFocus` rather than a click: focus without a selection gesture.
    session.on_focus_at(0, 115.0, 115.0, EventModifiers::NONE);
    assert!(session.focused_annot().is_some());

    assert!(session.replace_selection("Hello"));
    assert_eq!(session.focused_text().as_deref(), Some("Hello"));

    // Nothing selected yet — `Some("")`, which upstream spells as a length
    // of 2 and cannot distinguish from an unfocused field.
    assert_eq!(session.selected_text().as_deref(), Some(""));

    session.on_key_down(VirtualKey::A, EventModifiers::CONTROL);
    assert_eq!(session.selected_text().as_deref(), Some("Hello"));
}

/// The distinction the byte lengths could not express, asserted directly.
#[test]
fn an_unfocused_field_and_an_empty_selection_are_different_answers() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // No field at all.
    assert_eq!(session.selected_text(), None);

    // A focused field with nothing selected.
    session.on_focus_at(0, 115.0, 115.0, EventModifiers::NONE);
    assert_eq!(session.selected_text().as_deref(), Some(""));
}

/// Select-all on an **empty** field selects the empty string rather than
/// failing.
#[test]
fn select_all_on_an_empty_field_selects_nothing_and_succeeds() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    session.on_focus_at(0, 115.0, 115.0, EventModifiers::NONE);

    session.on_key_down(VirtualKey::A, EventModifiers::CONTROL);
    assert_eq!(session.selected_text().as_deref(), Some(""));
    assert_eq!(session.focused_text().as_deref(), Some(""));
}

// --- Ported focus-observation and graceful-failure rows ---------------------

/// `FocusAnnotationUpdateToEmbedder`: a click that takes focus reports the
/// focus change **in the returned updates**.
///
/// Upstream this is a callback that exists only in some versions of the
/// embedding interface, which is why its expectation is `Times(0)` off XFA
/// and `Times(1)` on: the *observation* is version-dependent there. Here a
/// focus change is an ordinary entry in the returned list, so there is one
/// answer and no version split — which is the divergence D2 records, asserted
/// rather than assumed.
#[test]
fn taking_focus_reports_the_change_in_the_updates() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    session.on_mouse_move(0, INSIDE.0, INSIDE.1, EventModifiers::NONE);
    let response = session.on_mouse_down(0, INSIDE.0, INSIDE.1, EventModifiers::NONE);

    let focus_changes = response
        .updates
        .iter()
        .filter(|update| matches!(update.kind, pdfrum::UpdateKind::FocusChanged { .. }))
        .count();
    assert_eq!(
        focus_changes, 1,
        "one focus change is reported, whatever the embedding interface"
    );
}

/// Re-clicking the field that already has focus reports **no** focus change:
/// the caret moves and nothing else does.
#[test]
fn re_clicking_the_focused_field_reports_no_focus_change() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, INSIDE);

    let response = session.on_mouse_down(0, 130.0, 115.0, EventModifiers::NONE);
    let focus_changes = response
        .updates
        .iter()
        .filter(|update| matches!(update.kind, pdfrum::UpdateKind::FocusChanged { .. }))
        .count();
    assert_eq!(
        focus_changes, 0,
        "clicking inside the field that already has focus moves the caret only"
    );
}

/// `IsIndexSelectedShouldFailGracefully` / `SetIndexSelected…`: a **text**
/// field has no rows, so every index query is false and every set is refused.
/// It answers rather than failing, which is the "gracefully" in the name.
#[test]
fn a_text_field_has_no_selectable_rows() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    type_run(&mut session, 3);
    assert_eq!(session.focused_text().as_deref(), Some("ABC"));

    for index in [0, 1, 100, usize::MAX] {
        assert!(
            !session.is_index_selected(index),
            "a text field has no row {index}"
        );
        assert!(
            !session.set_index_selected(index, true),
            "and none can be selected"
        );
    }
    // And none of that disturbed the text.
    assert_eq!(session.focused_text().as_deref(), Some("ABC"));
}
