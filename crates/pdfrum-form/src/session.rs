//! The session record: everything an interaction remembers between events.
//!
//! # Why this is a record and not a widget tree
//!
//! The reference implementation models interaction as four cooperating class
//! families with per-widget heap objects, virtual dispatch and back-pointers:
//! a filler owns a map from widget pointer to field object, each field object
//! owns a map from page-view pointer to window object, each window owns
//! children and a back-pointer to a notifier and an observed pointer back to
//! the widget. Object lifetime is fragile enough that the code re-checks an
//! observed pointer after *every* callback — sixteen such checks in the commit
//! and keystroke paths alone.
//!
//! Here a session is a record of facts. A focus target is an id, a dirty entry
//! is an id, and a function that mutates the session cannot invalidate a
//! reference someone else holds, because nobody holds one. The sixteen
//! re-checks become zero and become *unnecessary* rather than forgotten.
//!
//! The per-page window map goes with it. State is keyed by field, not by
//! (field, page-view) pair; the pair exists upstream because one widget can be
//! visible in several views at once, which is a viewer's problem. A library
//! that hands back appearance streams has no views.

use std::collections::{BTreeMap, BTreeSet};

use pdfrum_doc::Subtype;

use crate::event::Modifiers;
use crate::field::FieldState;

/// One interaction session over one document.
///
/// Per document, not per page: focus is owned document-wide, exactly as it is
/// upstream, and clicking into a field on page 2 takes focus away from one on
/// page 1.
#[derive(Debug, Clone, Default)]
pub struct FormSession {
    /// The field that owns keyboard input, if any.
    pub focus: Option<FocusTarget>,
    /// Per-field interaction state, created lazily the first time an event
    /// reaches a field. A field nobody has touched has no entry, which is what
    /// makes pure event *ordering* observable: upstream, only four of its
    /// nineteen dispatcher entries create interaction state at all, and the
    /// rest no-op when it is absent. That is why the tests' click gesture
    /// begins with a mouse move.
    pub fields: BTreeMap<FieldId, FieldState>,
    /// The annotation the pointer is currently over.
    pub hover: Option<AnnotId>,
    /// A live mouse drag, if one is in progress.
    pub drag: Option<DragAnchor>,
    /// Fields whose interaction state differs from the value in the document.
    pub dirty: BTreeSet<FieldId>,
    /// Platform and policy switches.
    pub config: SessionConfig,
}

impl FormSession {
    /// A fresh session with default configuration.
    #[must_use]
    pub fn new() -> FormSession {
        FormSession::default()
    }

    /// A fresh session with the given configuration.
    #[must_use]
    pub fn with_config(config: SessionConfig) -> FormSession {
        FormSession {
            config,
            ..FormSession::default()
        }
    }

    /// The focused field, if focus is on a widget rather than on a plain
    /// annotation.
    #[must_use]
    pub fn focused_field(&self) -> Option<FieldId> {
        match self.focus {
            Some(FocusTarget::Widget(field, _)) => Some(field),
            Some(FocusTarget::Annot(_)) | None => None,
        }
    }

    /// The focused annotation, whatever kind it is.
    #[must_use]
    pub fn focused_annot(&self) -> Option<AnnotId> {
        match self.focus {
            Some(FocusTarget::Widget(_, annot) | FocusTarget::Annot(annot)) => Some(annot),
            None => None,
        }
    }

    /// The focused field's interaction state, if there is one.
    #[must_use]
    pub fn focused_state(&self) -> Option<&FieldState> {
        self.fields.get(&self.focused_field()?)
    }

    /// The focused field's interaction state, mutably.
    pub fn focused_state_mut(&mut self) -> Option<&mut FieldState> {
        let field = self.focused_field()?;
        self.fields.get_mut(&field)
    }
}

/// Where keyboard input goes.
///
/// A widget, or a plain annotation — the tab ring is over a configurable set
/// of annotation subtypes, and a document that puts links in it can focus one
/// and fire its action with Return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    /// A form widget, and the field it belongs to.
    Widget(FieldId, AnnotId),
    /// A focusable non-widget annotation.
    Annot(AnnotId),
}

/// Which field, by position in the form's field list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldId(
    /// The field's position in the form's field list.
    pub u32,
);

/// Which annotation, by page and by position in that page's `/Annots` array.
///
/// The index is into the array as the file wrote it, not into any sorted or
/// filtered view of it, so two different orderings of the same page (draw
/// order, tab order) name the same annotation by the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnnotId {
    /// Which page, zero-based.
    pub page: u32,
    /// Which entry of that page's `/Annots` array.
    pub index: u32,
}

impl AnnotId {
    /// The annotation at `index` on `page`.
    #[must_use]
    pub const fn new(page: u32, index: u32) -> AnnotId {
        AnnotId { page, index }
    }
}

/// A mouse drag in progress: where it started, so a move can extend a
/// selection from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragAnchor {
    /// The field the drag started in. A drag that leaves the field still
    /// belongs to it.
    pub field: FieldId,
}

/// Platform and policy switches.
///
/// # Why the accelerator is configuration
///
/// Upstream decides the editing accelerator at compile time — Meta on Apple,
/// Control everywhere else — and the same build flag decides whether Ctrl+Y is
/// redo (it is, except on Apple, where Cmd+Shift+Z is the only redo). Both
/// halves are asserted by tests, so a compile-time choice makes half of three
/// test bodies unrunnable on any one machine. Making it a field costs nothing,
/// keeps the default matching the reference implementation on every platform,
/// and lets one machine test both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// The modifier that turns a letter into an editing accelerator.
    pub accelerator: Modifiers,
    /// Whether the accelerator plus `Y` redoes. False where the accelerator is
    /// Meta, matching the platform split above.
    pub redo_on_ctrl_y: bool,
    /// Which annotation subtypes join the tab ring. Widgets alone by default;
    /// signature widgets are excluded from it whatever this says.
    pub focusable: Vec<Subtype>,
    /// The undo stack's depth cap, clamped up to [`MIN_UNDO_ITEMS`].
    ///
    /// Lives here rather than on the shared parse limits because an undo bound
    /// is a property of an interaction, not of a document: no crate but this
    /// one would ever read it.
    pub max_undo_items: u32,
    /// How many recoveries one call keeps before it starts counting instead.
    pub max_recoveries: usize,
}

/// The undo stack's default depth.
pub const DEFAULT_UNDO_ITEMS: u32 = 10_000;

/// The smallest undo depth that can hold one grouped edit.
///
/// A replace-selection is four items — an opening sentinel, the clear, the
/// insert, and a closing sentinel — and a cap that could not hold all four
/// would evict half a group.
pub const MIN_UNDO_ITEMS: u32 = 4;

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        let apple = cfg!(target_vendor = "apple");
        SessionConfig {
            accelerator: if apple {
                Modifiers::META
            } else {
                Modifiers::CONTROL
            },
            redo_on_ctrl_y: !apple,
            focusable: vec![Subtype::Widget],
            max_undo_items: DEFAULT_UNDO_ITEMS,
            max_recoveries: 64,
        }
    }
}

impl SessionConfig {
    /// The undo depth actually used, with the group-atomicity floor applied.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.max_undo_items.max(MIN_UNDO_ITEMS) as usize
    }

    /// Whether these modifier bits are exactly the editing accelerator, with
    /// no other decided modifier alongside it.
    ///
    /// Shift is not consulted here — several accelerators are shift-sensitive
    /// and each decides for itself.
    #[must_use]
    pub fn is_accelerator(&self, modifiers: Modifiers) -> bool {
        let other = if self.accelerator == Modifiers::META {
            Modifiers::CONTROL
        } else {
            Modifiers::META
        };
        modifiers.contains(self.accelerator) && !modifiers.contains(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_undo_floor_is_a_group() {
        let cfg = SessionConfig {
            max_undo_items: 0,
            ..SessionConfig::default()
        };
        assert_eq!(cfg.undo_depth(), 4);
        let cfg = SessionConfig {
            max_undo_items: 7,
            ..SessionConfig::default()
        };
        assert_eq!(cfg.undo_depth(), 7);
    }

    #[test]
    fn the_default_depth_is_the_reference_value() {
        assert_eq!(SessionConfig::default().undo_depth(), 10_000);
    }

    #[test]
    fn the_wrong_platform_modifier_is_not_an_accelerator() {
        let ctrl = SessionConfig {
            accelerator: Modifiers::CONTROL,
            redo_on_ctrl_y: true,
            ..SessionConfig::default()
        };
        assert!(ctrl.is_accelerator(Modifiers::CONTROL));
        assert!(!ctrl.is_accelerator(Modifiers::META));
        assert!(!ctrl.is_accelerator(Modifiers::NONE));
        // Shift alongside is still the accelerator; the shortcut decides.
        assert!(ctrl.is_accelerator(Modifiers::CONTROL.union(Modifiers::SHIFT)));
    }

    #[test]
    fn a_fresh_session_focuses_nothing() {
        let s = FormSession::new();
        assert_eq!(s.focus, None);
        assert_eq!(s.focused_field(), None);
        assert_eq!(s.focused_annot(), None);
        assert!(s.fields.is_empty());
    }
}
