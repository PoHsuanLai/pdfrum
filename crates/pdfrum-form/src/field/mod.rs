//! Per-field interaction state.
//!
//! One variant per behaviour *family*, not per PDF field type: a combo box and
//! a list box share a machine, a checkbox and a radio button share one. The
//! grouping is behavioural because the differences between a combo and a list
//! are two booleans, while the difference between either and a text field is
//! the entire edit control.

use crate::edit::{Place, Selection};

/// A field's interaction state, created the first time an event reaches it.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldState {
    /// A text field, or the editable half of an editable combo box.
    Text(TextEdit),
    /// A combo box or a list box.
    Choice(ChoiceEdit),
    /// A checkbox or a radio button.
    Toggle(ToggleState),
    /// A push button.
    Button(ButtonState),
}

/// The edit control's state.
///
/// Its invariant is one sentence: `caret` and both ends of `selection` are
/// places the `layout` admits, and `layout` is what laying `text` out again
/// with the same configuration would produce.
#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    /// The text as the user has it, in full — a password field stores the
    /// real characters and substitutes only when it draws.
    pub text: String,
    /// Where the caret sits.
    pub caret: Place,
    /// The selection, directional.
    pub selection: Selection,
    /// The desired column for up/down movement, which survives a run of
    /// vertical arrows rather than being re-seeded from each new caret.
    pub sticky_x: f32,
    /// The scroll offset, in plate space.
    pub scroll: (f32, f32),
}

/// A combo box or list box's state.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceEdit {
    /// Which options are selected.
    pub selected: std::collections::BTreeSet<usize>,
    /// The last index *acted upon* — which is what a multi-select list
    /// reports as its focused text, even when the action changed nothing.
    pub caret_index: Option<usize>,
    /// The pivot successive shift-clicks all range-select from.
    pub anchor: Option<usize>,
    /// The first row drawn.
    pub top_visible: usize,
    /// Whether the dropdown is open.
    pub popup_open: bool,
    /// The typing half, present only for an editable combo box.
    pub edit: Option<TextEdit>,
}

/// A checkbox or radio button's state: the `/AS` appearance state name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleState {
    /// The `/AS` name currently showing.
    pub state: Vec<u8>,
}

/// A push button's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ButtonState {
    /// Whether the button is held down.
    pub pressed: bool,
}
