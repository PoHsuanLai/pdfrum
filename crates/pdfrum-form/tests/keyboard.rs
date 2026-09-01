//! Ported keyboard-routing assertions.
//!
//! Every one of these is about *dispatch* — which entry point decides a
//! gesture, and what it answers — so all of them port in full without needing
//! a laid-out field. Several assert a **negative**: that a gesture is *not*
//! handled, so an embedder's own handling sees it. Those are as load-bearing
//! as the positives and are ported with the same care.
//!
//! Every shortcut row runs on both platform configurations, which the
//! original cannot do: upstream picks its accelerator with a build flag, so
//! half of each of these tests is unreachable on any one machine.

use pdfrum_form::event::{Key, Modifiers};
use pdfrum_form::field::text::{Disposition, Motion, TextAction, route_char, route_key};

/// The accelerator and the redo-on-Y switch, per platform.
const GENERAL: (Modifiers, bool) = (Modifiers::CONTROL, true);
const APPLE: (Modifiers, bool) = (Modifiers::META, false);

fn on_key(k: Key, m: Modifiers, (accel, redo_y): (Modifiers, bool)) -> Disposition {
    route_key(k, m, accel, redo_y, false)
}

fn handled(d: Disposition) -> bool {
    matches!(d, Disposition::Do(_))
}

/// `DoNotHandleShortcutsOnKeyDown`, the positive half: navigation is handled.
#[test]
fn navigation_keys_are_handled() {
    for platform in [GENERAL, APPLE] {
        let (accel, _) = platform;
        assert!(handled(on_key(Key::LEFT, Modifiers::NONE, platform)));
        assert!(handled(on_key(Key::HOME, accel, platform)));
        assert!(handled(on_key(Key::HOME, Modifiers::NONE, platform)));
        assert!(handled(on_key(Key::UP, Modifiers::NONE, platform)));
        assert!(handled(on_key(Key::RIGHT, Modifiers::NONE, platform)));
    }
}

/// `DoNotHandleShortcutsOnKeyDown`, the negative half: the clipboard trio is
/// **not** handled, so an embedder's platform handlers receive it.
#[test]
fn the_clipboard_shortcuts_are_not_handled() {
    const C: Key = Key(0x43);
    const V: Key = Key(0x56);
    const X: Key = Key(0x58);

    for platform in [GENERAL, APPLE] {
        let (accel, _) = platform;
        for key in [C, V, X] {
            assert_eq!(
                on_key(key, accel, platform),
                Disposition::Ignore,
                "{key:?} belongs to the embedder"
            );
        }
    }
}

/// `SelectAllWithOnKeyDown`: the accelerator with A selects all; the wrong
/// modifier and the shifted form are both refused.
#[test]
fn select_all_is_the_accelerator_with_a_and_nothing_else() {
    for platform in [GENERAL, APPLE] {
        let (accel, _) = platform;
        assert_eq!(
            on_key(Key::A, accel, platform),
            Disposition::Do(TextAction::SelectAll)
        );
        assert_eq!(
            on_key(Key::A, accel | Modifiers::SHIFT, platform),
            Disposition::Ignore,
            "the shifted form is explicitly not select-all"
        );
    }

    // The other platform's modifier is refused, which is asserted upstream as
    // specifically as the right one being accepted.
    assert_eq!(
        on_key(Key::A, Modifiers::META, GENERAL),
        Disposition::Ignore
    );
    assert_eq!(
        on_key(Key::A, Modifiers::CONTROL, APPLE),
        Disposition::Ignore
    );
}

/// `DoNotHandleSelectAllOnChar`: the same gesture as a *character* is not
/// select-all. This is the negative half of the character/key split, and it
/// is the single most load-bearing fact in the event model.
#[test]
fn select_all_does_not_happen_on_the_character_path() {
    for (accel, _) in [GENERAL, APPLE] {
        // The control code the accelerator with A produces.
        assert_eq!(
            route_char('\u{01}', accel, accel, false, false),
            Disposition::Ignore
        );
        // …and the plain letter carrying the accelerator, likewise.
        assert_eq!(
            route_char('a', accel, accel, false, false),
            Disposition::Ignore,
            "a modified character is neither a shortcut nor text"
        );
    }
}

/// `UndoWithOnKeyDown`: the accelerator with Z undoes, with shift redoes, and
/// the wrong modifier does neither.
#[test]
fn undo_and_redo_are_the_accelerator_with_z() {
    for platform in [GENERAL, APPLE] {
        let (accel, _) = platform;
        assert_eq!(
            on_key(Key::Z, accel, platform),
            Disposition::Do(TextAction::Undo)
        );
        assert_eq!(
            on_key(Key::Z, accel | Modifiers::SHIFT, platform),
            Disposition::Do(TextAction::Redo)
        );
    }

    assert_eq!(
        on_key(Key::Z, Modifiers::META, GENERAL),
        Disposition::Ignore
    );
    assert_eq!(
        on_key(Key::Z, Modifiers::CONTROL, APPLE),
        Disposition::Ignore
    );
}

/// `RedoWithCtrlYKeyboardShortcut`: the accelerator with Y redoes off Apple
/// and does nothing on it, while the shifted Z redoes on both. Upstream can
/// only assert one branch per build; here both run.
#[test]
fn the_accelerator_with_y_redoes_on_one_platform_only() {
    assert_eq!(
        on_key(Key::Y, Modifiers::CONTROL, GENERAL),
        Disposition::Do(TextAction::Redo)
    );
    assert_eq!(
        on_key(Key::Y, Modifiers::META, APPLE),
        Disposition::Ignore,
        "the same gesture does nothing on an Apple keyboard"
    );

    // The shifted form is refused on the platform that has the gesture.
    assert_eq!(
        on_key(Key::Y, Modifiers::CONTROL | Modifiers::SHIFT, GENERAL),
        Disposition::Ignore
    );

    // …while shifted Z redoes on both, which is the asymmetry.
    for platform in [GENERAL, APPLE] {
        let (accel, _) = platform;
        assert_eq!(
            on_key(Key::Z, accel | Modifiers::SHIFT, platform),
            Disposition::Do(TextAction::Redo)
        );
    }
}

/// `KeyPressWithNoFocusedAnnot`: none of these keys is the text field's, so
/// each falls through for the caller to dispose of.
#[test]
fn keys_the_field_does_not_decide_on_fall_through() {
    const NEWLINE: Key = Key(0x0A);
    const DIGIT_ZERO: Key = Key(0x30);
    const DIGIT_NINE: Key = Key(0x39);
    const LETTER_Z_PLAIN: Key = Key::Z;
    const F1: Key = Key(0x70);

    for key in [
        NEWLINE,
        Key::RETURN,
        Key::SPACE,
        DIGIT_ZERO,
        DIGIT_NINE,
        F1,
        Key::TAB,
    ] {
        assert_eq!(
            on_key(key, Modifiers::NONE, GENERAL),
            Disposition::Ignore,
            "{key:?} is not handled by the text field"
        );
    }

    // An unmodified Z is a character, not a shortcut, so the key path leaves
    // it alone too.
    assert_eq!(
        on_key(LETTER_Z_PLAIN, Modifiers::NONE, GENERAL),
        Disposition::Ignore
    );
}

/// `LinkActionInvokeTest` and friends: the modifier keys reported as keys in
/// their own right are never consumed.
#[test]
fn modifier_keys_reported_as_keys_are_not_consumed() {
    for key in [Key::SHIFT, Key::CONTROL, Key::SPACE] {
        assert_eq!(
            on_key(key, Modifiers::NONE, GENERAL),
            Disposition::Ignore,
            "{key:?} is not the field's"
        );
    }
}

/// `CheckReadOnlyInCheckbox` / `…InRadiobutton`, restated for text: a
/// read-only field **consumes** the keystroke and does nothing. Consumption
/// and effect are independent.
#[test]
fn a_read_only_field_consumes_without_acting() {
    assert_eq!(
        route_char('a', Modifiers::NONE, Modifiers::CONTROL, true, false),
        Disposition::Consume
    );
    assert_ne!(
        route_char('a', Modifiers::NONE, Modifiers::CONTROL, true, false),
        Disposition::Ignore,
        "consuming is not the same as ignoring"
    );
}

/// `cpwl_edit_embeddertest`'s delete-key note: an embedder may send an extra
/// character alongside a delete key press, and it must be ignored — taking
/// both would delete twice.
#[test]
fn the_delete_control_character_is_ignored() {
    assert_eq!(
        route_char('\u{7F}', Modifiers::NONE, Modifiers::CONTROL, false, false),
        Disposition::Ignore
    );
    // Even for a read-only field, the filter runs before the read-only check.
    assert_eq!(
        route_char('\u{7F}', Modifiers::NONE, Modifiers::CONTROL, true, false),
        Disposition::Ignore
    );
}

/// A forward delete with a live selection removes the selection rather than
/// the character after the caret.
#[test]
fn delete_with_a_selection_removes_the_selection() {
    assert_eq!(
        route_key(Key::DELETE, Modifiers::NONE, Modifiers::CONTROL, true, true),
        Disposition::Do(TextAction::ClearSelection)
    );
    assert_eq!(
        route_key(
            Key::DELETE,
            Modifiers::NONE,
            Modifiers::CONTROL,
            true,
            false
        ),
        Disposition::Do(TextAction::Delete)
    );
}

/// The accelerator turns line-wise motion into document-wise motion, and
/// shift extends whatever the motion is.
#[test]
fn home_and_end_widen_with_the_accelerator_and_extend_with_shift() {
    for platform in [GENERAL, APPLE] {
        let (accel, _) = platform;
        assert_eq!(
            on_key(Key::HOME, Modifiers::NONE, platform),
            Disposition::Do(TextAction::Move {
                motion: Motion::LineStart,
                extend: false
            })
        );
        assert_eq!(
            on_key(Key::HOME, accel, platform),
            Disposition::Do(TextAction::Move {
                motion: Motion::DocStart,
                extend: false
            })
        );
        assert_eq!(
            on_key(Key::END, accel | Modifiers::SHIFT, platform),
            Disposition::Do(TextAction::Move {
                motion: Motion::DocEnd,
                extend: true
            })
        );
    }
}

/// `FormTextFieldBiDiLiveEdit` types Hebrew directly, so the character path
/// must carry text well outside ASCII.
#[test]
fn non_ascii_characters_are_ordinary_text() {
    // Hebrew bet, gimel and tav — the codes the right-to-left fixture sends.
    for ch in ['\u{05D1}', '\u{05D2}', '\u{05EA}'] {
        assert_eq!(
            route_char(ch, Modifiers::NONE, Modifiers::CONTROL, false, false),
            Disposition::Do(TextAction::Insert(ch))
        );
    }
}
