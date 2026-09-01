//! The session record: everything an interaction remembers between events.
//!
//! A session is a record of facts, and applying an event is a function over
//! it. There is nothing here that owns a widget, points at one, or observes
//! one — a focus target is an identifier, a dirty entry is an identifier, and
//! a function that mutates the session cannot invalidate a reference someone
//! else is holding. The C++ this reproduces re-checks a weak pointer after
//! every callback, sixteen times in two functions; here the checks are not
//! forgotten, they are unnecessary.
//!
//! Focus is owned per **document**, not per page: one field has the keyboard
//! at a time, whichever page it is on.

use std::collections::{BTreeMap, BTreeSet};

use pdfrum_doc::Subtype;

use crate::edit::Place;
use crate::event::Modifiers;
use crate::field::FieldState;

/// Which field a session is talking about: an index into the form's field
/// list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldId(pub u32);

/// Which annotation: a page and its index in that page's **raw `/Annots`
/// array**.
///
/// # The index space is the raw array, not a filtered position
///
/// This distinction is invisible until a page carries a pop-up annotation,
/// and then it decides whether an appearance lands on the right widget.
///
/// A page's annotation list, as the rendering path builds it, **drops
/// pop-ups** — they are synthesized separately from the annotations they
/// belong to — so a widget's position in that list is not its position in the
/// file's `/Annots` array. On a page whose first annotation is a pop-up,
/// every later widget's filtered position is one lower than its raw index.
///
/// The appearance overlay a caller draws through is keyed by the **raw**
/// index. So that is what this carries: hand back a filtered position and
/// every appearance after the first pop-up is applied to the wrong
/// annotation, silently, with no type to catch it.
///
/// This is also the natural choice rather than a concession, because hit
/// testing walks `/Annots` itself: the raw index is what the walk already
/// has, and the filtered one would have to be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnnotId {
    /// Which page.
    pub page: u32,
    /// Which entry of that page's `/Annots` array — the raw index, counting
    /// pop-ups.
    pub index: u32,
}

impl AnnotId {
    /// An annotation identifier, from a page index and a **raw** `/Annots`
    /// index.
    #[must_use]
    pub fn new(page: u32, index: u32) -> AnnotId {
        AnnotId { page, index }
    }
}

/// Where keyboard input goes.
///
/// A non-widget annotation can hold focus too, once a caller adds its subtype
/// to the focus ring — which is how tabbing to a link and pressing Return
/// fires the link's action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    /// A form field's widget.
    Widget(FieldId, AnnotId),
    /// A focusable annotation that is not a form widget.
    Annot(AnnotId),
}

impl FocusTarget {
    /// The annotation holding focus, whichever kind of target this is.
    #[must_use]
    pub fn annot(self) -> AnnotId {
        match self {
            FocusTarget::Widget(_, annot) | FocusTarget::Annot(annot) => annot,
        }
    }

    /// The field holding focus, if the target is a widget.
    #[must_use]
    pub fn field(self) -> Option<FieldId> {
        match self {
            FocusTarget::Widget(field, _) => Some(field),
            FocusTarget::Annot(_) => None,
        }
    }
}

/// A mouse drag in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragAnchor {
    /// Which field the drag started in.
    pub field: FieldId,
    /// Where in that field's text it started.
    pub start: Place,
}

/// The switches a caller sets once and the engine reads.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    /// Which modifier means "this is a shortcut, not text".
    ///
    /// Control everywhere but Apple keyboards, where it is Meta. A field
    /// rather than a compile-time platform test, so that both behaviours are
    /// reachable — and testable — on one machine.
    pub accelerator: Modifiers,
    /// Whether the accelerator with `Y` redoes.
    ///
    /// True off Apple, false on it, where the same gesture is spelled with
    /// shift and `Z`. That asymmetry is real and is reproduced.
    pub redo_on_ctrl_y: bool,
    /// Which annotation subtypes join the focus ring. Widgets alone by
    /// default.
    pub focusable: Vec<Subtype>,
    /// How many undo items a field keeps.
    ///
    /// Clamped up to four: a replace-selection group is four items, and a
    /// capacity that could not hold one would have to evict half a group.
    pub max_undo_items: u32,
    /// How deep a calculation may trigger another calculation.
    ///
    /// **One by default, which is the oracle's answer and not a loose
    /// reading of it.** `CPDFSDK_InteractiveForm::busy_`
    /// (`fpdfsdk/cpdfsdk_interactiveform.cpp:259-264`) is a plain flag: the
    /// outer sweep is authoritative and every nested call it provokes returns
    /// immediately, so upstream permits **no** nesting at all. The knob makes
    /// that configurable rather than looser — mirroring
    /// [`Limits::max_calculate_depth`](pdfrum_common::Limits::max_calculate_depth),
    /// which is where the same number lives for a caller configuring the
    /// script engine rather than the session.
    pub max_calculate_depth: u32,
}

impl SessionConfig {
    /// The defaults for an Apple keyboard: Meta accelerates, and the
    /// accelerator with `Y` does nothing.
    #[must_use]
    pub fn apple() -> SessionConfig {
        SessionConfig {
            accelerator: Modifiers::META,
            redo_on_ctrl_y: false,
            ..SessionConfig::default()
        }
    }
}

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        SessionConfig {
            accelerator: Modifiers::CONTROL,
            redo_on_ctrl_y: true,
            focusable: vec![Subtype::Widget],
            max_undo_items: crate::edit::UndoStack::DEFAULT_MAX,
            max_calculate_depth: pdfrum_common::Limits::default().max_calculate_depth,
        }
    }
}

/// One interaction session over one document.
#[derive(Debug, Clone, Default)]
pub struct FormSession {
    /// What has the keyboard, if anything.
    pub focus: Option<FocusTarget>,
    /// Per-field interaction state, created when a field is first touched.
    ///
    /// A field keeps its state across focus changes, which is why clicking
    /// away from a half-typed field and back finds the typing still there.
    pub fields: BTreeMap<FieldId, FieldState>,
    /// What the pointer is over, for enter and exit.
    pub hover: Option<AnnotId>,
    /// A drag in progress, if one is.
    pub drag: Option<DragAnchor>,
    /// Fields whose interaction state has outrun the document's value.
    pub dirty: BTreeSet<FieldId>,
    /// The switches.
    pub config: SessionConfig,
}

impl FormSession {
    /// An empty session with the default switches.
    #[must_use]
    pub fn new() -> FormSession {
        FormSession::default()
    }

    /// An empty session with the given switches.
    #[must_use]
    pub fn with_config(config: SessionConfig) -> FormSession {
        FormSession {
            config,
            ..FormSession::default()
        }
    }

    /// The field that currently has the keyboard, if a field does.
    #[must_use]
    pub fn focused_field(&self) -> Option<FieldId> {
        self.focus.and_then(FocusTarget::field)
    }

    /// The interaction state of the focused field, if there is one.
    #[must_use]
    pub fn focused_state(&self) -> Option<&FieldState> {
        self.fields.get(&self.focused_field()?)
    }

    /// The interaction state of the focused field, mutably.
    pub fn focused_state_mut(&mut self) -> Option<&mut FieldState> {
        let field = self.focused_field()?;
        self.fields.get_mut(&field)
    }

    /// Whether these modifiers make a gesture a shortcut in this session.
    ///
    /// One predicate, shared with the key router rather than restated here:
    /// two spellings of the same question drift, and the oracle has only one.
    #[must_use]
    pub fn is_accelerator(&self, modifiers: Modifiers) -> bool {
        crate::field::text::is_shortcut(modifiers, self.config.accelerator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_session_has_no_focus_and_no_state() {
        let session = FormSession::new();
        assert!(session.focus.is_none());
        assert!(session.focused_field().is_none());
        assert!(session.fields.is_empty());
        assert!(session.dirty.is_empty());
    }

    #[test]
    fn a_focus_target_names_its_annotation_either_way() {
        let annot = AnnotId::new(0, 3);
        let widget = FocusTarget::Widget(FieldId(1), annot);
        let plain = FocusTarget::Annot(annot);

        assert_eq!(widget.annot(), annot);
        assert_eq!(plain.annot(), annot);
        assert_eq!(widget.field(), Some(FieldId(1)));
        assert_eq!(plain.field(), None);
    }

    /// The defaults reproduce each platform's own behaviour, and both are
    /// reachable from one machine — which is what makes the shortcut
    /// assertions runnable at all.
    #[test]
    fn the_two_platform_configurations_differ_in_exactly_two_switches() {
        let general = SessionConfig::default();
        let apple = SessionConfig::apple();

        assert_eq!(general.accelerator, Modifiers::CONTROL);
        assert!(general.redo_on_ctrl_y);
        assert_eq!(apple.accelerator, Modifiers::META);
        assert!(!apple.redo_on_ctrl_y);
        assert_eq!(general.focusable, apple.focusable);
        assert_eq!(general.max_undo_items, apple.max_undo_items);
    }

    /// The wrong platform's modifier is rejected, which is asserted as hard
    /// as the right one being accepted.
    #[test]
    fn the_accelerator_test_rejects_the_other_platforms_modifier() {
        let session = FormSession::new();
        assert!(session.is_accelerator(Modifiers::CONTROL));
        assert!(session.is_accelerator(Modifiers::CONTROL | Modifiers::SHIFT));
        assert!(!session.is_accelerator(Modifiers::META));
        assert!(!session.is_accelerator(Modifiers::NONE));
        // Alt disqualifies; an extra modifier does not, which is the
        // oracle's subset answer.
        assert!(!session.is_accelerator(Modifiers::CONTROL | Modifiers::ALT));
        assert!(session.is_accelerator(Modifiers::CONTROL | Modifiers::META));

        let apple = FormSession::with_config(SessionConfig::apple());
        assert!(apple.is_accelerator(Modifiers::META));
        assert!(!apple.is_accelerator(Modifiers::CONTROL));
    }

    #[test]
    fn only_widgets_are_focusable_by_default() {
        assert_eq!(SessionConfig::default().focusable, vec![Subtype::Widget]);
    }
}
