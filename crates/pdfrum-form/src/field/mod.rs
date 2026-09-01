//! Per-field interaction state, and the configuration read once from the
//! file.
//!
//! There are four state families, not seven field types: a combo box and a
//! list box share a machine, and a check box and a radio button share one.
//! Grouping by behaviour rather than by `/FT` is what keeps each machine
//! small enough to state as a transition table.
//!
//! # Configuration is a record, not a bit mask
//!
//! The C++ translates a field's flags, quadding and length limit into a
//! 32-bit style word whose low bits are **overloaded across widget
//! families** — the same bit means multiline for an edit, multi-select for a
//! list box and allow-custom-text for a combo box — and then keeps that safe
//! by masking sub-styles off when it builds a child. Here the kinds are
//! separate types, so the overloading has nowhere to happen, and each switch
//! is a named field read once when the field is first touched.

pub mod button;
pub mod choice;
pub mod text;
pub mod toggle;

use std::num::NonZeroU32;

use pdfrum_doc::form::{FieldFlags, FieldKind};

pub use button::ButtonState;
pub use toggle::{ToggleKind, ToggleState, activate};

/// How a field's text is set, read once from its flags and entries.
///
/// The layout half of these switches is already the variable-text engine's
/// business; what this record adds is the editor's own — whether the field
/// accepts edits at all, whether it records undo, and whether it scrolls.
///
/// The booleans are the point rather than a smell: each one is a distinct
/// `/Ff` bit with its own meaning, and the alternative the lint suggests — a
/// packed flag word — is exactly the design this record exists to replace,
/// where the same bit means different things to different field kinds.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextConfig {
    /// Whether the field accepts more than one line.
    pub multi_line: bool,
    /// Whether the field's contents are obscured as they are typed.
    pub password: bool,
    /// Whether the field is laid out as a row of equal cells.
    pub comb: bool,
    /// The cap on how many characters the field holds, if it has one.
    ///
    /// An `Option` rather than a sentinel, which is how "a non-positive limit
    /// means unlimited" stops being a rule anyone has to remember.
    pub max_len: Option<NonZeroU32>,
    /// Whether the field refuses edits.
    pub read_only: bool,
    /// Whether the field records undo. Always true for a text field.
    pub undo_enabled: bool,
    /// Whether the field scrolls its content to follow the caret.
    pub auto_scroll: bool,
}

impl TextConfig {
    /// Reads a text field's configuration from its flags and length limit.
    ///
    /// The scrolling rule is the one worth reading twice: a field scrolls
    /// unless it is explicitly told not to, and that is true whether or not
    /// it is multiline — the two cases differ in what else they turn on, not
    /// in whether they scroll.
    #[must_use]
    pub fn read(flags: FieldFlags, max_len: Option<u32>) -> TextConfig {
        TextConfig {
            multi_line: flags.is_multiline(),
            password: flags.is_password(),
            comb: flags.0 & (1 << 24) != 0,
            max_len: max_len.and_then(NonZeroU32::new),
            read_only: flags.is_read_only(),
            // Undo is always available on a text field: the oracle turns it
            // on unconditionally rather than from any flag.
            undo_enabled: true,
            auto_scroll: !flags.do_not_scroll(),
        }
    }
}

/// How a choice field behaves, read once from its flags.
///
/// Four independent `/Ff` bits; see [`TextConfig`] for why they stay separate
/// named fields rather than becoming a mask.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChoiceConfig {
    /// Whether the field is a drop-down rather than a list.
    pub combo: bool,
    /// Whether a combo box also accepts typed text.
    pub editable: bool,
    /// Whether a list box accepts more than one selected row.
    pub multi_select: bool,
    /// Whether the field refuses edits.
    pub read_only: bool,
}

impl ChoiceConfig {
    /// Reads a choice field's configuration from its flags.
    #[must_use]
    pub fn read(flags: FieldFlags) -> ChoiceConfig {
        ChoiceConfig {
            combo: flags.is_combo(),
            // Only a combo box can be editable; the bit is meaningless on a
            // list box and is not read as anything there.
            editable: flags.is_combo() && flags.is_editable_combo(),
            multi_select: !flags.is_combo() && flags.is_multi_select(),
            read_only: flags.is_read_only(),
        }
    }
}

/// A field's interaction state, one variant per behaviour family.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldState {
    /// A text field, or the text half of an editable combo box.
    Text(TextState),
    /// A combo box or a list box.
    Choice(ChoiceState),
    /// A check box or a radio button.
    Toggle(ToggleState),
    /// A push button.
    Button(ButtonState),
}

/// A text field's interaction state.
///
/// The layout, caret and undo stack live here once the editing operations
/// land; for now it carries the configuration and the text, which is what the
/// commit path and the non-mouse assertions need.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextState {
    /// The text as the user has it, which may differ from the field's stored
    /// value until the edit commits.
    pub text: String,
    /// How the field is configured.
    pub config: TextConfig,
    /// The undo stack.
    pub undo: crate::edit::UndoStack,
}

/// One row of a choice field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChoiceOption {
    /// What the row displays.
    pub label: String,
    /// What the row stores, when that differs from what it displays.
    pub value: String,
}

/// A combo box or list box's interaction state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChoiceState {
    /// The rows.
    pub options: Vec<ChoiceOption>,
    /// Which rows are selected.
    pub selected: std::collections::BTreeSet<usize>,
    /// The row last acted upon.
    ///
    /// Not a description of the selection: a multi-select list box reports
    /// *this* row's text as its focused text, and a call that selects nothing
    /// — deselecting an already-deselected row — still moves it. That is the
    /// asserted behaviour, however odd it reads.
    pub caret_index: Option<usize>,
    /// The pivot a shift-click ranges from. Successive shift-clicks all
    /// pivot on the same row, so this is not updated by one.
    pub anchor: Option<usize>,
    /// The first row currently visible.
    pub top_visible: usize,
    /// How the field is configured.
    pub config: ChoiceConfig,
}

impl ChoiceState {
    /// A choice field with the given rows and configuration.
    #[must_use]
    pub fn new(options: Vec<ChoiceOption>, config: ChoiceConfig) -> ChoiceState {
        ChoiceState {
            options,
            config,
            ..ChoiceState::default()
        }
    }

    /// The text this field reports as its focused text.
    ///
    /// The row last acted upon for a choice field, which for a single-select
    /// field is the same thing as "the selected row" and for a multi-select
    /// one deliberately is not.
    #[must_use]
    pub fn focused_text(&self) -> String {
        let index = self
            .caret_index
            .or_else(|| self.selected.iter().next().copied());
        index
            .and_then(|i| self.options.get(i))
            .map(|o| o.label.clone())
            .unwrap_or_default()
    }
}

/// Which behaviour family a field kind belongs to.
///
/// Returns `None` for the two kinds that never get interaction state at all:
/// a signature widget, which is never given an appearance and never takes an
/// edit, and anything the classifier could not name.
#[must_use]
pub fn family_of(kind: FieldKind) -> Option<Family> {
    match kind {
        FieldKind::Text => Some(Family::Text),
        FieldKind::Combo | FieldKind::List => Some(Family::Choice),
        FieldKind::Check | FieldKind::Radio => Some(Family::Toggle),
        FieldKind::Button => Some(Family::Button),
        FieldKind::Signature => None,
    }
}

/// The four behaviour families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// A text field.
    Text,
    /// A combo box or list box.
    Choice,
    /// A check box or radio button.
    Toggle,
    /// A push button.
    Button,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_widget_gets_no_interaction_state() {
        assert_eq!(family_of(FieldKind::Signature), None);
        assert_eq!(family_of(FieldKind::Text), Some(Family::Text));
        assert_eq!(family_of(FieldKind::Combo), Some(Family::Choice));
        assert_eq!(family_of(FieldKind::List), Some(Family::Choice));
        assert_eq!(family_of(FieldKind::Check), Some(Family::Toggle));
        assert_eq!(family_of(FieldKind::Radio), Some(Family::Toggle));
        assert_eq!(family_of(FieldKind::Button), Some(Family::Button));
    }

    /// A non-positive limit means unlimited, which the type says rather than
    /// the reader having to remember.
    #[test]
    fn a_zero_length_limit_is_no_limit() {
        assert_eq!(TextConfig::read(FieldFlags(0), Some(0)).max_len, None);
        assert_eq!(TextConfig::read(FieldFlags(0), None).max_len, None);
        assert_eq!(
            TextConfig::read(FieldFlags(0), Some(10)).max_len,
            NonZeroU32::new(10)
        );
    }

    /// Undo is on for every text field, from no flag at all.
    #[test]
    fn a_text_field_always_records_undo() {
        assert!(TextConfig::read(FieldFlags(0), None).undo_enabled);
        assert!(TextConfig::read(FieldFlags(1), None).undo_enabled);
    }

    #[test]
    fn a_field_scrolls_unless_it_is_told_not_to() {
        assert!(TextConfig::read(FieldFlags(0), None).auto_scroll);
        assert!(TextConfig::read(FieldFlags(1 << 12), None).auto_scroll);
        assert!(!TextConfig::read(FieldFlags(1 << 23), None).auto_scroll);
    }

    #[test]
    fn text_flags_read_the_documented_bits() {
        let config = TextConfig::read(FieldFlags((1 << 12) | (1 << 13) | (1 << 24) | 1), None);
        assert!(config.multi_line);
        assert!(config.password);
        assert!(config.comb);
        assert!(config.read_only);
    }

    /// The editable bit is a combo box's alone: a list box with the same bit
    /// set is not editable, because the bit does not mean that there.
    #[test]
    fn only_a_combo_box_can_be_editable() {
        let combo = ChoiceConfig::read(FieldFlags((1 << 17) | (1 << 18)));
        assert!(combo.combo);
        assert!(combo.editable);

        let list = ChoiceConfig::read(FieldFlags(1 << 18));
        assert!(!list.combo);
        assert!(!list.editable);
    }

    /// And multi-select is a list box's alone, for the same reason.
    #[test]
    fn only_a_list_box_can_be_multi_select() {
        let list = ChoiceConfig::read(FieldFlags(1 << 21));
        assert!(list.multi_select);

        let combo = ChoiceConfig::read(FieldFlags((1 << 17) | (1 << 21)));
        assert!(!combo.multi_select);
    }

    fn options(labels: &[&str]) -> Vec<ChoiceOption> {
        labels
            .iter()
            .map(|l| ChoiceOption {
                label: (*l).to_string(),
                value: (*l).to_string(),
            })
            .collect()
    }

    /// The focused text follows the row last acted upon, not the selection.
    #[test]
    fn focused_text_is_the_row_last_acted_upon() {
        let mut state = ChoiceState::new(
            options(&["Apple", "Banana", "Cherry", "Date"]),
            ChoiceConfig::default(),
        );
        state.selected.insert(0);
        state.selected.insert(2);

        // With nothing acted upon, the first selected row answers.
        assert_eq!(state.focused_text(), "Apple");

        // Acting on a row moves it, even to a row that is not selected.
        state.caret_index = Some(3);
        assert_eq!(state.focused_text(), "Date");
    }

    #[test]
    fn an_empty_choice_field_reports_no_text() {
        let state = ChoiceState::new(Vec::new(), ChoiceConfig::default());
        assert_eq!(state.focused_text(), "");
    }
}
