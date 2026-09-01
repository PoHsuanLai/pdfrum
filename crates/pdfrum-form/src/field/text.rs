//! The text field's keyboard machine.
//!
//! # The split that decides everything
//!
//! **Typed text arrives as a character; navigation and shortcuts arrive as a
//! key.** Nothing crosses over: typing sends no key-down, and the accelerator
//! shortcuts are decided only on the key path. The negative half of that is
//! asserted as hard as the positive half — an accelerator-modified *character*
//! is explicitly not a shortcut, and is not text either. It is simply
//! refused.
//!
//! This module is the routing decision alone, as a pure function from an
//! event to a named [`TextAction`]. Keeping it separate from the editing
//! operations is what lets the whole shortcut table be pinned without a
//! layout: whether the accelerator with `A` selects all is a question about
//! dispatch, and answering it does not require knowing where any character
//! sits.
//!
//! # The clipboard is not ours
//!
//! Cut, copy and paste do not exist at this layer. The accelerator with C, V
//! or X is refused outright so that an embedder's own handling sees it — the
//! selected text is read out and replacement text is written in through
//! ordinary calls, and a cut is an embedder doing both.

use crate::event::{Key, Modifiers};

/// What an event asks the text field to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAction {
    /// Insert a character at the caret, replacing any selection.
    Insert(char),
    /// Insert a paragraph break.
    InsertReturn,
    /// Delete backwards, or the selection.
    Backspace,
    /// Delete forwards, or the selection.
    Delete,
    /// Drop the selection without deleting anything.
    ClearSelection,
    /// Move the caret, optionally extending the selection.
    Move {
        /// Which way.
        motion: Motion,
        /// Whether the selection extends rather than collapsing.
        extend: bool,
    },
    /// Select the whole field.
    SelectAll,
    /// Undo one step.
    Undo,
    /// Redo one step.
    Redo,
    /// Commit the value and give up focus.
    Commit,
    /// Discard the edit and give up focus.
    Escape,
}

/// Which way a caret movement goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// One character left.
    Left,
    /// One character right.
    Right,
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// To the start of the line.
    LineStart,
    /// To the end of the line.
    LineEnd,
    /// To the very start of the field.
    DocStart,
    /// To the very end of the field.
    DocEnd,
}

/// How an event was disposed of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The event asks for this, and was consumed.
    Do(TextAction),
    /// The event was consumed and asks for nothing — what a read-only field
    /// does with a keystroke.
    Consume,
    /// The event was not ours. The caller may pass it on.
    Ignore,
}

/// Routes a key-down.
///
/// `accelerator` is the modifier that means "shortcut" — the platform's own,
/// which is configuration rather than a compile-time choice — and
/// `redo_on_y` says whether the accelerator with `Y` redoes, which is true
/// off Apple keyboards and false on them.
///
/// A shortcut with the *wrong* modifier is [`Disposition::Ignore`], not a
/// silent no-op: the assertions check the rejection as specifically as the
/// acceptance.
#[must_use]
pub fn route_key(
    key: Key,
    modifiers: Modifiers,
    accelerator: Modifiers,
    redo_on_y: bool,
    has_selection: bool,
) -> Disposition {
    let shortcut = modifiers.without(Modifiers::SHIFT | Modifiers::ALT) == accelerator;
    let shift = modifiers.contains(Modifiers::SHIFT);
    let alt = modifiers.contains(Modifiers::ALT);

    // A forward delete with a selection is rewritten to a plain
    // clear-selection before the table is consulted, which is why deleting a
    // selection and pressing an unrecognized key take the same branch.
    if key == Key::DELETE && has_selection {
        return Disposition::Do(TextAction::ClearSelection);
    }

    match key {
        Key::DELETE => Disposition::Do(TextAction::Delete),
        Key::UP => Disposition::Do(TextAction::Move {
            motion: Motion::Up,
            extend: shift,
        }),
        Key::DOWN => Disposition::Do(TextAction::Move {
            motion: Motion::Down,
            extend: shift,
        }),
        Key::LEFT => Disposition::Do(TextAction::Move {
            motion: Motion::Left,
            extend: shift,
        }),
        Key::RIGHT => Disposition::Do(TextAction::Move {
            motion: Motion::Right,
            extend: shift,
        }),
        Key::HOME => Disposition::Do(TextAction::Move {
            motion: if shortcut {
                Motion::DocStart
            } else {
                Motion::LineStart
            },
            extend: shift,
        }),
        Key::END => Disposition::Do(TextAction::Move {
            motion: if shortcut {
                Motion::DocEnd
            } else {
                Motion::LineEnd
            },
            extend: shift,
        }),
        // Select-all takes the accelerator alone: adding shift is explicitly
        // not select-all.
        Key::A if shortcut && !shift && !alt => Disposition::Do(TextAction::SelectAll),
        Key::Y if redo_on_y && shortcut && !shift && !alt => Disposition::Do(TextAction::Redo),
        // Z is the one shortcut that takes shift as a meaning rather than a
        // disqualifier.
        Key::Z if shortcut && !alt => Disposition::Do(if shift {
            TextAction::Redo
        } else {
            TextAction::Undo
        }),
        // An unrecognized key clears the selection rather than being ignored,
        // which is the branch a rewritten delete arrives at.
        Key::UNKNOWN => Disposition::Do(TextAction::ClearSelection),
        _ => Disposition::Ignore,
    }
}

/// Routes a typed character.
///
/// Three characters are refused outright — line feed, escape and the delete
/// control code — and so is any character carrying the accelerator without
/// alt. The delete refusal is deliberate and asserted: an embedder may send a
/// character alongside a delete key, and taking both would delete twice.
///
/// A **read-only** field consumes the character and does nothing, which is
/// not the same as ignoring it.
#[must_use]
pub fn route_char(
    ch: char,
    modifiers: Modifiers,
    accelerator: Modifiers,
    read_only: bool,
    multi_line: bool,
) -> Disposition {
    const LINE_FEED: char = '\u{0A}';
    const ESCAPE: char = '\u{1B}';
    const DELETE: char = '\u{7F}';
    const BACKSPACE: char = '\u{08}';
    const RETURN: char = '\u{0D}';

    if matches!(ch, LINE_FEED | ESCAPE | DELETE) {
        return Disposition::Ignore;
    }

    // The accelerator makes a character a shortcut's business, not text — and
    // shortcuts are decided on the key path, so this is simply a refusal.
    // Adding alt takes it back out of shortcut territory.
    if modifiers.contains(accelerator) && !modifiers.contains(Modifiers::ALT) {
        return Disposition::Ignore;
    }

    if read_only {
        return Disposition::Consume;
    }

    match ch {
        BACKSPACE => Disposition::Do(TextAction::Backspace),
        // A single-line field commits on Return; a multiline one takes it as
        // a paragraph break.
        RETURN => Disposition::Do(if multi_line {
            TextAction::InsertReturn
        } else {
            TextAction::Commit
        }),
        _ => Disposition::Do(TextAction::Insert(ch)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two platform configurations, so every shortcut row runs both ways
    /// on one machine — which the oracle's own tests cannot do.
    const GENERAL: (Modifiers, bool) = (Modifiers::CONTROL, true);
    const APPLE: (Modifiers, bool) = (Modifiers::META, false);

    fn key(k: Key, m: Modifiers, (accel, redo_y): (Modifiers, bool)) -> Disposition {
        route_key(k, m, accel, redo_y, false)
    }

    fn ch(c: char, m: Modifiers, (accel, _): (Modifiers, bool)) -> Disposition {
        route_char(c, m, accel, false, false)
    }

    /// Select-all is the accelerator with A, and only that.
    #[test]
    fn select_all_takes_the_accelerator_alone() {
        for platform in [GENERAL, APPLE] {
            let (accel, _) = platform;
            assert_eq!(
                key(Key::A, accel, platform),
                Disposition::Do(TextAction::SelectAll)
            );
            // Shift disqualifies it explicitly.
            assert_eq!(
                key(Key::A, accel | Modifiers::SHIFT, platform),
                Disposition::Ignore
            );
            // And so does no modifier at all.
            assert_eq!(key(Key::A, Modifiers::NONE, platform), Disposition::Ignore);
        }
    }

    /// The other platform's modifier is rejected, which is asserted as hard
    /// as the right one being accepted.
    #[test]
    fn the_wrong_platforms_modifier_is_rejected() {
        assert_eq!(key(Key::A, Modifiers::META, GENERAL), Disposition::Ignore);
        assert_eq!(key(Key::A, Modifiers::CONTROL, APPLE), Disposition::Ignore);
        assert_eq!(key(Key::Z, Modifiers::META, GENERAL), Disposition::Ignore);
        assert_eq!(key(Key::Z, Modifiers::CONTROL, APPLE), Disposition::Ignore);
    }

    /// Z takes shift as a meaning rather than a disqualifier — the only
    /// shortcut that does.
    #[test]
    fn the_accelerator_with_z_undoes_and_with_shift_redoes() {
        for platform in [GENERAL, APPLE] {
            let (accel, _) = platform;
            assert_eq!(
                key(Key::Z, accel, platform),
                Disposition::Do(TextAction::Undo)
            );
            assert_eq!(
                key(Key::Z, accel | Modifiers::SHIFT, platform),
                Disposition::Do(TextAction::Redo)
            );
        }
    }

    /// Y redoes off Apple and does nothing on it, while shift-Z redoes on
    /// both. That asymmetry is real and tested rather than smoothed away.
    #[test]
    fn the_accelerator_with_y_redoes_only_off_apple() {
        assert_eq!(
            key(Key::Y, Modifiers::CONTROL, GENERAL),
            Disposition::Do(TextAction::Redo)
        );
        assert_eq!(key(Key::Y, Modifiers::META, APPLE), Disposition::Ignore);

        // Shift disqualifies it on the platform that has it at all.
        assert_eq!(
            key(Key::Y, Modifiers::CONTROL | Modifiers::SHIFT, GENERAL),
            Disposition::Ignore
        );
    }

    /// The clipboard is the embedder's, so these three fall straight through.
    #[test]
    fn the_clipboard_shortcuts_are_not_ours() {
        for platform in [GENERAL, APPLE] {
            let (accel, _) = platform;
            for letter in [Key(0x43), Key(0x56), Key(0x58)] {
                assert_eq!(
                    key(letter, accel, platform),
                    Disposition::Ignore,
                    "cut, copy and paste belong to the embedder"
                );
            }
        }
    }

    /// Navigation is handled, and the accelerator turns line-wise motion into
    /// document-wise motion.
    #[test]
    fn navigation_keys_are_handled() {
        for platform in [GENERAL, APPLE] {
            let (accel, _) = platform;
            assert_eq!(
                key(Key::LEFT, Modifiers::NONE, platform),
                Disposition::Do(TextAction::Move {
                    motion: Motion::Left,
                    extend: false
                })
            );
            assert_eq!(
                key(Key::HOME, Modifiers::NONE, platform),
                Disposition::Do(TextAction::Move {
                    motion: Motion::LineStart,
                    extend: false
                })
            );
            assert_eq!(
                key(Key::HOME, accel, platform),
                Disposition::Do(TextAction::Move {
                    motion: Motion::DocStart,
                    extend: false
                })
            );
            assert_eq!(
                key(Key::UP, Modifiers::NONE, platform),
                Disposition::Do(TextAction::Move {
                    motion: Motion::Up,
                    extend: false
                })
            );
        }
    }

    /// Shift extends rather than collapsing, on every motion.
    #[test]
    fn shift_extends_every_motion() {
        for k in [
            Key::LEFT,
            Key::RIGHT,
            Key::UP,
            Key::DOWN,
            Key::HOME,
            Key::END,
        ] {
            let Disposition::Do(TextAction::Move { extend, .. }) =
                key(k, Modifiers::SHIFT, GENERAL)
            else {
                panic!("{k:?} should move");
            };
            assert!(extend, "{k:?} with shift should extend");
        }
    }

    /// A forward delete with a selection removes the selection, by way of a
    /// rewrite that lands in the same branch an unrecognized key does.
    #[test]
    fn delete_with_a_selection_clears_it() {
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
        assert_eq!(
            key(Key::UNKNOWN, Modifiers::NONE, GENERAL),
            Disposition::Do(TextAction::ClearSelection)
        );
    }

    /// Keys the field does not decide on fall through, so a caller can act on
    /// them — which is how Tab reaches focus traversal.
    #[test]
    fn undecided_keys_fall_through() {
        for k in [
            Key::TAB,
            Key::SPACE,
            Key::PRIOR,
            Key::NEXT,
            Key::INSERT,
            Key(0x70), // F1
            Key(0x30), // '0'
        ] {
            assert_eq!(
                key(k, Modifiers::NONE, GENERAL),
                Disposition::Ignore,
                "{k:?} is not the text field's"
            );
        }
    }

    /// The negative half of the split: an accelerator-modified character is
    /// not a shortcut here, and it is not text either.
    #[test]
    fn an_accelerator_modified_character_is_refused() {
        // The control code the accelerator with A produces.
        assert_eq!(
            ch('\u{01}', Modifiers::CONTROL, GENERAL),
            Disposition::Ignore
        );
        assert_eq!(ch('a', Modifiers::CONTROL, GENERAL), Disposition::Ignore);
        assert_eq!(ch('a', Modifiers::META, APPLE), Disposition::Ignore);

        // …but alt takes it back out of shortcut territory.
        assert_eq!(
            ch('a', Modifiers::CONTROL | Modifiers::ALT, GENERAL),
            Disposition::Do(TextAction::Insert('a'))
        );
    }

    /// Three characters are filtered before anything else looks at them. The
    /// delete refusal matters: an embedder may send a character alongside a
    /// delete key, and taking both would delete twice.
    #[test]
    fn line_feed_escape_and_delete_are_filtered() {
        for c in ['\u{0A}', '\u{1B}', '\u{7F}'] {
            assert_eq!(
                ch(c, Modifiers::NONE, GENERAL),
                Disposition::Ignore,
                "{c:?} must not reach the text"
            );
        }
    }

    /// Return passes through and means different things by field shape;
    /// backspace passes through and always deletes.
    #[test]
    fn return_and_backspace_pass_the_filter() {
        assert_eq!(
            route_char('\u{0D}', Modifiers::NONE, Modifiers::CONTROL, false, false),
            Disposition::Do(TextAction::Commit),
            "a single-line field commits on Return"
        );
        assert_eq!(
            route_char('\u{0D}', Modifiers::NONE, Modifiers::CONTROL, false, true),
            Disposition::Do(TextAction::InsertReturn),
            "a multiline one takes a paragraph break"
        );
        assert_eq!(
            ch('\u{08}', Modifiers::NONE, GENERAL),
            Disposition::Do(TextAction::Backspace)
        );
    }

    /// A read-only field consumes the character and does nothing, which is a
    /// third answer distinct from both acting and ignoring.
    #[test]
    fn a_read_only_field_consumes_without_acting() {
        assert_eq!(
            route_char('a', Modifiers::NONE, Modifiers::CONTROL, true, false),
            Disposition::Consume
        );
        // The filter still runs first: a filtered character is ignored, not
        // consumed, even by a read-only field.
        assert_eq!(
            route_char('\u{1B}', Modifiers::NONE, Modifiers::CONTROL, true, false),
            Disposition::Ignore
        );
    }

    /// Ordinary text is inserted, including text well outside ASCII — the
    /// right-to-left fixtures type Hebrew directly.
    #[test]
    fn ordinary_characters_are_inserted() {
        assert_eq!(
            ch('a', Modifiers::NONE, GENERAL),
            Disposition::Do(TextAction::Insert('a'))
        );
        assert_eq!(
            ch(' ', Modifiers::NONE, GENERAL),
            Disposition::Do(TextAction::Insert(' '))
        );
        // Hebrew bet, from the right-to-left live-edit fixture.
        assert_eq!(
            ch('\u{05D1}', Modifiers::NONE, GENERAL),
            Disposition::Do(TextAction::Insert('\u{05D1}'))
        );
    }

    /// Shift is a text modifier, not a disqualifier: shift-A is a capital A.
    #[test]
    fn shift_does_not_stop_a_character_being_text() {
        assert_eq!(
            ch('A', Modifiers::SHIFT, GENERAL),
            Disposition::Do(TextAction::Insert('A'))
        );
    }
}
