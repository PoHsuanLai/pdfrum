//! Who has the keyboard, and the orderings that decide it.
//!
//! Focus belongs to the **document**, not to a page: one annotation has the
//! keyboard at a time, whichever page it sits on.
//!
//! # The rules, and which are surprising
//!
//! - **Focusing what is already focused succeeds** without disturbing
//!   anything. Re-clicking inside a field the user is already typing in moves
//!   the caret and nothing else — it does not commit, and it does not clear
//!   the undo stack.
//! - **The outgoing field yields first.** Moving focus commits the field
//!   losing it before the new one takes it, so a value is stored on the way
//!   out rather than on the way in.
//! - **A field keeps its state across focus changes.** Clicking away from a
//!   half-typed field and back finds the typing still there — the state
//!   record outlives the focus, and only a change to a *different* field
//!   clears the undo stack.
//! - **A right-click outside a field does not drop focus**, while a left
//!   click on the same point does. That is not a mistake in the event
//!   routing: the two buttons take different paths, and only one of them
//!   kills focus on a miss.
//! - **Clicking a point with no annotation drops focus**, and doing it again
//!   is harmless rather than an error.

use crate::session::{FocusTarget, FormSession};

/// What a focus change did, and what the caller must do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusChange {
    /// What had focus before.
    pub from: Option<FocusTarget>,
    /// What has it now.
    pub to: Option<FocusTarget>,
    /// Whether the field losing focus needs its value committed.
    pub commit_outgoing: bool,
    /// Whether the outgoing field's undo history should be discarded.
    ///
    /// Only when focus actually moved to a *different* field: moving the
    /// caret inside one field keeps its history.
    pub clear_undo: bool,
}

impl FocusChange {
    /// Whether anything moved.
    #[must_use]
    pub fn moved(self) -> bool {
        self.from != self.to
    }

    /// A change in which nothing happened.
    #[must_use]
    pub fn none(at: Option<FocusTarget>) -> FocusChange {
        FocusChange {
            from: at,
            to: at,
            commit_outgoing: false,
            clear_undo: false,
        }
    }
}

/// Gives focus to `target`, taking it from whatever holds it.
///
/// Focusing the current holder is a success that changes nothing, which is
/// what makes re-clicking inside a field cheap and non-destructive.
pub fn set(session: &mut FormSession, target: FocusTarget) -> FocusChange {
    let from = session.focus;
    if from == Some(target) {
        return FocusChange::none(from);
    }

    // The outgoing field commits before the incoming one takes over.
    let commit_outgoing = from.is_some_and(|f| f.field().is_some());
    // Undo history belongs to a field, so it survives a move that stays
    // within one and is dropped by a move that leaves it.
    let clear_undo = match (from.and_then(FocusTarget::field), target.field()) {
        (Some(old), Some(new)) => old != new,
        (Some(_), None) => true,
        _ => false,
    };

    session.focus = Some(target);
    session.drag = None;

    FocusChange {
        from,
        to: Some(target),
        commit_outgoing,
        clear_undo,
    }
}

/// Takes focus away from whatever holds it.
///
/// Doing this when nothing is focused is harmless and reports that nothing
/// moved, rather than being an error.
pub fn kill(session: &mut FormSession) -> FocusChange {
    let from = session.focus;
    if from.is_none() {
        return FocusChange::none(None);
    }

    session.focus = None;
    session.drag = None;

    FocusChange {
        from,
        to: None,
        commit_outgoing: from.is_some_and(|f| f.field().is_some()),
        clear_undo: from.is_some_and(|f| f.field().is_some()),
    }
}

/// Whether a click that hit nothing should drop focus.
///
/// The left button drops it; the right button does not, and neither does a
/// wheel turn. This asymmetry is the reason a right-click far outside a
/// focused field leaves it focused and rendering from its live editor state,
/// while a left click on the very same point makes it fall back to a
/// generated appearance.
#[must_use]
pub fn miss_drops_focus(button: crate::event::Button) -> bool {
    matches!(button, crate::event::Button::Left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Button;
    use crate::session::{AnnotId, FieldId};

    fn widget(field: u32, annot: u32) -> FocusTarget {
        FocusTarget::Widget(FieldId(field), AnnotId::new(0, annot))
    }

    #[test]
    fn focusing_from_nothing_takes_focus_and_commits_nothing() {
        let mut session = FormSession::new();
        let change = set(&mut session, widget(0, 1));

        assert_eq!(session.focus, Some(widget(0, 1)));
        assert_eq!(change.from, None);
        assert!(change.moved());
        assert!(!change.commit_outgoing, "there was nothing to commit");
        assert!(!change.clear_undo);
    }

    /// Re-focusing the holder is a success that disturbs nothing — which is
    /// what makes clicking twice inside a field cheap.
    #[test]
    fn refocusing_the_holder_changes_nothing() {
        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        let change = set(&mut session, widget(0, 1));

        assert!(!change.moved());
        assert!(!change.commit_outgoing, "no commit on a re-focus");
        assert!(!change.clear_undo, "and the history survives");
        assert_eq!(session.focus, Some(widget(0, 1)));
    }

    /// Moving to another field commits the old one and drops its history.
    #[test]
    fn moving_between_fields_commits_and_clears() {
        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        let change = set(&mut session, widget(1, 2));

        assert!(change.moved());
        assert!(change.commit_outgoing);
        assert!(change.clear_undo);
        assert_eq!(change.from, Some(widget(0, 1)));
    }

    /// Two widgets of the *same* field keep its history: the caret moved, the
    /// field did not.
    #[test]
    fn moving_between_widgets_of_one_field_keeps_the_history() {
        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        let change = set(&mut session, widget(0, 5));

        assert!(change.moved());
        assert!(
            !change.clear_undo,
            "the field is the same, so its history stands"
        );
    }

    #[test]
    fn killing_focus_commits_the_outgoing_field() {
        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        let change = kill(&mut session);

        assert_eq!(session.focus, None);
        assert_eq!(change.from, Some(widget(0, 1)));
        assert_eq!(change.to, None);
        assert!(change.commit_outgoing);
        assert!(change.clear_undo);
    }

    /// Dropping focus twice is harmless, which is what makes clicking an
    /// empty spot repeatedly idempotent.
    #[test]
    fn killing_focus_twice_is_harmless() {
        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        kill(&mut session);
        let second = kill(&mut session);

        assert!(!second.moved());
        assert!(!second.commit_outgoing);
        assert_eq!(session.focus, None);
    }

    /// A non-widget annotation holds focus but has nothing to commit.
    #[test]
    fn a_plain_annotation_holds_focus_without_a_value() {
        let mut session = FormSession::new();
        let change = set(&mut session, FocusTarget::Annot(AnnotId::new(0, 4)));

        assert_eq!(session.focus, Some(FocusTarget::Annot(AnnotId::new(0, 4))));
        assert!(!change.commit_outgoing);

        let away = kill(&mut session);
        assert!(
            !away.commit_outgoing,
            "a link has no value to store on the way out"
        );
    }

    /// Leaving a field for a plain annotation still commits the field.
    #[test]
    fn leaving_a_field_for_an_annotation_commits_it() {
        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        let change = set(&mut session, FocusTarget::Annot(AnnotId::new(0, 4)));

        assert!(change.commit_outgoing);
        assert!(change.clear_undo);
    }

    /// The asymmetry that makes a right-click outside a focused field leave
    /// it focused, while a left click on the same point does not.
    #[test]
    fn only_the_left_button_drops_focus_on_a_miss() {
        assert!(miss_drops_focus(Button::Left));
        assert!(!miss_drops_focus(Button::Right));
    }

    /// A focus change ends any drag in progress.
    #[test]
    fn a_focus_change_ends_a_drag() {
        use crate::edit::Place;
        use crate::session::DragAnchor;

        let mut session = FormSession::new();
        set(&mut session, widget(0, 1));
        session.drag = Some(DragAnchor {
            field: FieldId(0),
            start: Place::START,
        });

        set(&mut session, widget(1, 2));
        assert!(session.drag.is_none());
    }
}
