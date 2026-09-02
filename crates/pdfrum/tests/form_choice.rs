//! Ported choice-field assertions, driven end to end.
//!
//! `choice_fields.rs` in `pdfrum-form` pins the selection *machine* against
//! hand-built states; these pin the routing — that a click reaches the right
//! field, that focus moves between the two combo boxes, and that typing means
//! different things in an editable box and a gated one.
//!
//! The fixture is `combobox_form.pdf`, whose three fields carry the constants
//! `fpdf_formfill_embeddertest.cpp:274-385` transcribes:
//! `Combo_Editable` `/Ff 393216` at `/Rect [100 350 200 380]`, `Combo1`
//! `/Ff 131072` at `[100 400 200 430]` with `/V (Banana)`, and
//! `Combo_ReadOnly` `/Ff 131073` at `[100 500 200 530]`.

// See `form_routing.rs`: the fixture helper is a failure signal.
#![allow(clippy::expect_used)]

use pdfrum::{Document, FormSession, Modifiers, kurbo::Point};

fn document() -> Document {
    Document::open("tests/fixtures/combobox_form.pdf").expect("the combobox fixture must open")
}

/// The upstream constants.
const BEGIN_X: f64 = 102.0;
const END_X: f64 = 183.0;
const EDITABLE_Y: f64 = 360.0;
const NON_EDITABLE_Y: f64 = 410.0;
const READ_ONLY_Y: f64 = 510.0;
/// A point on no form field at all.
const OFF_FIELD: Point = Point::new(1.0, 1.0);

fn click(session: &mut FormSession<'_>, at: Point) {
    session.mouse_move(0, at, Modifiers::NONE);
    session.mouse_down(0, at, Modifiers::NONE);
    session.mouse_up(0, at, Modifiers::NONE);
}

fn type_text(session: &mut FormSession<'_>, text: &str) {
    for ch in text.chars() {
        session.character(ch, Modifiers::NONE);
    }
}

/// `FocusChanges`, the combo half: the walk that shows a non-editable box
/// reports its **selected option's label** while an editable one reports what
/// has been typed into it, and that both are empty with nothing focused.
#[test]
fn focus_moves_between_the_two_combo_boxes() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // Nothing focused: no field, which is distinct from an empty field.
    assert!(session.focused_text().is_none());

    // The gated box reports its /V's label.
    click(&mut session, Point::new(BEGIN_X, NON_EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some("Banana"));

    // The editable one starts empty — its /V is absent, not "Banana".
    click(&mut session, Point::new(BEGIN_X, EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some(""));

    // Back to the gated box from its other end: same answer, so the answer is
    // the field's and not the click's.
    click(&mut session, Point::new(END_X, NON_EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some("Banana"));

    // Clicking the same box again changes nothing.
    click(&mut session, Point::new(BEGIN_X, NON_EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some("Banana"));

    // Dropping focus leaves no field at all.
    session.blur();
    assert!(session.focused_text().is_none());
}

/// Typing into an **editable** combo box inserts text; the value accumulates
/// across a focus change and back, because a field keeps its state.
#[test]
fn typing_into_an_editable_combo_inserts_and_persists() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, Point::new(BEGIN_X, EDITABLE_Y));
    type_text(&mut session, "ABC");
    assert_eq!(session.focused_text().as_deref(), Some("ABC"));

    // Away and back: the typing survives.
    click(&mut session, OFF_FIELD);
    assert!(session.focused_text().is_none());
    click(&mut session, Point::new(END_X, EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some("ABC"));

    type_text(&mut session, "ABC");
    assert_eq!(session.focused_text().as_deref(), Some("ABCABC"));
}

/// Typing into a **non-editable** combo box does not insert: each character is
/// a type-ahead jump that selects a different option. `A` finds `Apple`,
/// `C` finds `Cherry`.
#[test]
fn typing_into_a_gated_combo_selects_rather_than_inserts() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, Point::new(BEGIN_X, NON_EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some("Banana"));

    type_text(&mut session, "A");
    assert_eq!(
        session.focused_text().as_deref(),
        Some("Apple"),
        "a character jumps to an option rather than being inserted"
    );

    type_text(&mut session, "C");
    assert_eq!(session.focused_text().as_deref(), Some("Cherry"));
}

/// A **read-only** widget is not clickable at all, so it never takes focus —
/// a separate rule from a read-only control consuming a keystroke it was
/// already given.
#[test]
fn a_read_only_combo_never_takes_focus() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, Point::new(BEGIN_X, READ_ONLY_Y));
    assert!(
        session.focused_annot().is_none(),
        "a read-only widget is not clickable"
    );
}

/// A click on no field at all drops whatever had focus.
#[test]
fn a_click_off_every_field_drops_focus() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, Point::new(BEGIN_X, NON_EDITABLE_Y));
    assert!(session.focused_annot().is_some());

    click(&mut session, OFF_FIELD);
    assert!(session.focused_annot().is_none());
    assert!(session.focused_text().is_none());
}

/// The two boxes are distinct fields: focusing one does not disturb the
/// other's state.
#[test]
fn the_two_combo_boxes_keep_separate_state() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    click(&mut session, Point::new(BEGIN_X, EDITABLE_Y));
    type_text(&mut session, "typed");

    click(&mut session, Point::new(BEGIN_X, NON_EDITABLE_Y));
    assert_eq!(session.focused_text().as_deref(), Some("Banana"));

    click(&mut session, Point::new(BEGIN_X, EDITABLE_Y));
    assert_eq!(
        session.focused_text().as_deref(),
        Some("typed"),
        "the editable box kept what was typed into it"
    );
}
