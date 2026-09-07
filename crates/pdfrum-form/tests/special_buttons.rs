//! Ported `cpwl_special_button_embeddertest.cpp` assertions.
//!
//! Four rows over two controls, and they pin the one line where a check box
//! and a radio button differ, from both sides:
//!
//! - **A check box toggles.** Two Returns leave it as it started.
//! - **A radio button sets.** Two Returns leave it *checked*: there is no way
//!   to clear a radio button by activating it, and that is deliberate rather
//!   than an oversight — clearing is something its siblings do to each other.
//! - **A read-only control of either kind consumes the keystroke and does not
//!   move.** Consumption and effect are independent here, which is the
//!   distinction the oracle's single boolean cannot make and this crate's
//!   `Response` can.

use pdfrum_form::field::toggle::{OFF_STATE, ToggleKind, ToggleState, activate};

/// A control showing `checked`, whose on state is the usual `/On`.
fn control(checked: bool) -> ToggleState {
    let mut state = ToggleState::new(OFF_STATE, "On");
    state.set_checked(checked);
    state
}

/// `EnterOnCheckBox`: the first Return checks it, the second clears it.
#[test]
fn a_check_box_toggles_on_each_activation() {
    let mut state = control(false);

    assert!(activate(&mut state, ToggleKind::Check, false));
    assert!(state.is_checked());

    assert!(activate(&mut state, ToggleKind::Check, false));
    assert!(!state.is_checked());
}

/// `EnterOnReadOnlyCheckBox`: a read-only check box starts checked and stays
/// checked. The upstream row asserts the keystroke is still *handled*, which
/// is the caller's answer rather than this function's — here the state simply
/// does not move.
#[test]
fn a_read_only_check_box_does_not_move() {
    let mut state = control(true);
    assert!(state.is_checked());

    assert!(!activate(&mut state, ToggleKind::Check, true));
    assert!(state.is_checked(), "a read-only control never moves");

    // And the same from the other starting state, which the upstream fixture
    // cannot reach because its read-only box is checked in the file.
    let mut clear = control(false);
    assert!(!activate(&mut clear, ToggleKind::Check, true));
    assert!(!clear.is_checked());
}

/// `EnterOnRadioButton`: the first Return checks it — and the second leaves
/// it checked, where a check box would have cleared.
#[test]
fn a_radio_button_sets_and_never_clears_itself() {
    let mut state = control(false);

    assert!(activate(&mut state, ToggleKind::Radio, false));
    assert!(state.is_checked());

    // The asymmetry: activating again is not a toggle.
    activate(&mut state, ToggleKind::Radio, false);
    assert!(
        state.is_checked(),
        "a radio button cannot be cleared by activating it"
    );
}

/// `EnterOnReadOnlyRadioButton`: read-only refuses on the radio path too.
#[test]
fn a_read_only_radio_button_does_not_move() {
    let mut state = control(false);
    assert!(!activate(&mut state, ToggleKind::Radio, true));
    assert!(!state.is_checked());
}

/// The two controls' behaviour diverges only on the second activation, which
/// is worth asserting as one statement rather than leaving implicit in two
/// tests above.
#[test]
fn the_two_controls_differ_only_after_the_first_activation() {
    let (mut check, mut radio) = (control(false), control(false));

    activate(&mut check, ToggleKind::Check, false);
    activate(&mut radio, ToggleKind::Radio, false);
    assert_eq!(check.is_checked(), radio.is_checked(), "both are now on");

    activate(&mut check, ToggleKind::Check, false);
    activate(&mut radio, ToggleKind::Radio, false);
    assert!(!check.is_checked());
    assert!(radio.is_checked());
}

/// A control with no on state at all cannot be checked — there is no
/// appearance to show — and asking for it is harmless rather than an error.
#[test]
fn a_control_with_no_on_state_stays_clear() {
    let mut state = ToggleState::new(OFF_STATE, "");
    activate(&mut state, ToggleKind::Check, false);
    assert!(!state.is_checked());

    activate(&mut state, ToggleKind::Radio, false);
    assert!(!state.is_checked());
}

/// **Which kid of a radio group draws its on-state, and which draws `Off`.**
///
/// `CPDF_FormField::CheckControl` (`cpdf_formfield.cpp:683-716`) sets the
/// clicked control's `/AS` to its own on state and every *other* control of
/// the field to `Off`. A session holds one [`ToggleState`] per **field**, so
/// the only thing it can record is which control was chosen — which is
/// exactly what `state_for_control` turns back into a per-kid answer, and
/// what `route.rs` passes as `LiveInput::appearance_state`.
///
/// landed in and the drawing could not, because the generator read `/AS`
/// from the widget dictionary and took no override.
#[test]
fn a_radio_groups_siblings_answer_off_and_the_chosen_kid_answers_its_own_name() {
    use pdfrum_form::AnnotId;

    let (first, second) = (AnnotId::new(0, 0), AnnotId::new(0, 1));

    // Before anything is clicked there is no chosen control, and the answer
    // is `None` — "read the widget's own `/AS`", which is what makes a group
    // loaded from a file render exactly as the file wrote it, kid by kid.
    let mut state = ToggleState::new(OFF_STATE, "Kid0");
    assert_eq!(state.state_for_control(first), None);
    assert_eq!(state.state_for_control(second), None);

    // A click on the first kid: it shows its own on-state name, the sibling
    // shows `Off` whatever its dictionary still says.
    activate(&mut state, ToggleKind::Radio, false);
    state.checked_control = Some(first);
    assert_eq!(state.state_for_control(first), Some("Kid0"));
    assert_eq!(state.state_for_control(second), Some(OFF_STATE));

    // Choosing the other kid swaps both answers with no second state record.
    state.checked_control = Some(second);
    assert_eq!(state.state_for_control(first), Some(OFF_STATE));
    assert_eq!(state.state_for_control(second), Some("Kid0"));
}

/// A **check box** is a field with one control, so the per-control answer is
/// its own state either way — the accessor is not a radio-only special case
/// bolted onto a shared record.
#[test]
fn a_check_boxs_only_control_answers_its_own_state() {
    use pdfrum_form::AnnotId;

    let only = AnnotId::new(0, 3);
    let mut state = ToggleState::new(OFF_STATE, "Yes");
    activate(&mut state, ToggleKind::Check, false);
    state.checked_control = Some(only);
    assert_eq!(state.state_for_control(only), Some("Yes"));

    activate(&mut state, ToggleKind::Check, false);
    assert_eq!(
        state.state_for_control(only),
        Some(OFF_STATE),
        "unchecking it moves the answer, because it moves `state`"
    );
}
