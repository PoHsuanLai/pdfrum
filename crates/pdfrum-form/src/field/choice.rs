//! Combo boxes and list boxes: the operations over a set of rows.
//!
//! One machine covers both, because the differences are few enough to be
//! parameters rather than a second implementation — but the differences that
//! do exist are sharp, and three of them are the sort a reasonable design
//! would smooth away:
//!
//! - **A combo box cannot be deselected and a list box can.** Asking a combo
//!   box to clear a row fails outright, whatever the row; asking a
//!   single-select list box to clear its only selected row succeeds and
//!   leaves the field genuinely empty.
//! - **A call that changes nothing still moves the caret.** Clearing an
//!   already-clear row of a list box reports success and moves the row last
//!   acted upon to it — which changes what the field reports as its focused
//!   text. That is asserted, not incidental.
//! - **Type-ahead jumps rather than accumulates.** Typing `A`, then `B`, then
//!   `C` lands on the row starting with C, not on one starting with "ABC".
//!   Each character is an independent search from the current row, and the
//!   search is circular.

use std::collections::BTreeSet;

use super::{ChoiceState, TextConfig};

/// Where the search that a typed character starts should land.
///
/// A circular scan from the row after `from`, comparing first characters
/// case-insensitively. **When nothing matches it returns the last row
/// probed** rather than reporting failure — which is why consecutive
/// characters read as independent jumps rather than as a growing prefix.
#[must_use]
pub fn find_next(options: &[String], from: usize, ch: char) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let count = options.len();
    let wanted = ch.to_uppercase().next().unwrap_or(ch);
    let mut at = from;
    for _ in 0..count {
        at = (at + 1) % count;
        let first = options
            .get(at)
            .and_then(|o| o.chars().next())
            .map(|c| c.to_uppercase().next().unwrap_or(c));
        if first == Some(wanted) {
            return Some(at);
        }
    }
    // Nothing matched. The scan's final position is the answer, deliberately.
    Some(at)
}

/// Whether a row index names a row of this field.
#[must_use]
pub fn in_range(state: &ChoiceState, index: usize) -> bool {
    index < state.options.len()
}

/// Selects or clears one row programmatically.
///
/// Returns whether the call was accepted, which is not the same as whether
/// anything changed:
///
/// - an out-of-range row is **rejected** by both kinds;
/// - a combo box **rejects** every clear, whatever the row;
/// - a list box **accepts** both, including a redundant one, and moves the
///   row last acted upon either way.
pub fn set_index_selected(state: &mut ChoiceState, index: usize, selected: bool) -> bool {
    if !in_range(state, index) {
        return false;
    }
    if state.config.combo {
        if !selected {
            // A combo box always shows one row; there is no empty state to
            // clear into.
            return false;
        }
        state.selected.clear();
        state.selected.insert(index);
        state.caret_index = Some(index);
        return true;
    }

    if selected {
        if !state.config.multi_select {
            state.selected.clear();
        }
        state.selected.insert(index);
    } else {
        state.selected.remove(&index);
    }
    // Both branches move the caret, so a clear that removed nothing still
    // changes what the field reports as its focused text.
    state.caret_index = Some(index);
    true
}

/// Whether a row is selected. An out-of-range row is not.
#[must_use]
pub fn is_index_selected(state: &ChoiceState, index: usize) -> bool {
    in_range(state, index) && state.selected.contains(&index)
}

/// Selects one row by a click or a keyboard move, clearing the others.
pub fn select_only(state: &mut ChoiceState, index: usize) -> bool {
    if !in_range(state, index) {
        return false;
    }
    state.selected.clear();
    state.selected.insert(index);
    state.caret_index = Some(index);
    state.anchor = Some(index);
    true
}

/// Extends the selection from the anchor to `index`, clearing everything
/// else. What a shift-click does on a multi-select list box.
///
/// The anchor is deliberately **not** moved, so successive shift-clicks all
/// pivot on the row the plain click established.
pub fn select_range_to(state: &mut ChoiceState, index: usize) -> bool {
    if !in_range(state, index) {
        return false;
    }
    if !state.config.multi_select {
        return select_only(state, index);
    }
    let anchor = state.anchor.unwrap_or(index);
    let (lo, hi) = if anchor <= index {
        (anchor, index)
    } else {
        (index, anchor)
    };
    state.selected = (lo..=hi).collect();
    state.caret_index = Some(index);
    true
}

/// Toggles one row, keeping the rest. What an accelerator-click does on a
/// multi-select list box, and it **does** re-anchor.
pub fn toggle_index(state: &mut ChoiceState, index: usize) -> bool {
    if !in_range(state, index) {
        return false;
    }
    if !state.config.multi_select {
        return select_only(state, index);
    }
    if state.selected.contains(&index) {
        state.selected.remove(&index);
    } else {
        state.selected.insert(index);
    }
    state.caret_index = Some(index);
    state.anchor = Some(index);
    true
}

/// Moves the selection one row, which is what an arrow key does.
///
/// Clamps at each end rather than wrapping or failing.
pub fn move_selection(state: &mut ChoiceState, delta: i32) -> bool {
    if state.options.is_empty() {
        return false;
    }
    let last = state.options.len() - 1;
    let current = state
        .caret_index
        .or_else(|| state.selected.iter().next().copied())
        .unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (current + delta.unsigned_abs() as usize).min(last)
    };
    select_only(state, next)
}

/// Moves the caret one row **with modifiers**, which is what an arrow key and
/// a wheel notch both do.
///
/// `CPWL_ListCtrl::OnVK` (`cpwl_list_ctrl.cpp:242-265`) branches three ways on
/// a multi-select list, and the first of them is the surprising one:
///
/// - **Ctrl** — the body is *empty*. Only the caret moves; the selection is
///   left exactly as it was. Upstream writes it as `if (bCtrl) {}`, which is
///   easy to read as an oversight and is not: it is how a user walks the
///   caret to a row before toggling it.
/// - **Shift** — deselect everything, then select the whole run from the
///   anchor to the new row.
/// - **neither** — deselect everything, select the one row, and re-anchor.
///
/// A single-select list ignores all three and just selects the row, because
/// `OnVK`'s whole `bShift`/`bCtrl` structure is inside `IsMultipleSel()`.
///
/// [`move_selection`] is this function with both flags clear, and is kept
/// because that is what most callers want.
pub fn move_caret_by(state: &mut ChoiceState, delta: i32, shift: bool, ctrl: bool) -> bool {
    if state.options.is_empty() {
        return false;
    }
    let last = state.options.len() - 1;
    let current = state
        .caret_index
        .or_else(|| state.selected.iter().next().copied())
        .unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (current + delta.unsigned_abs() as usize).min(last)
    };
    if !state.config.multi_select {
        return select_only(state, next);
    }
    if ctrl {
        // The caret moves and nothing else does. Reported as a change,
        // because the caret is what a multi-select list answers its focused
        // text with and what its focus box strokes.
        let moved = state.caret_index != Some(next);
        state.caret_index = Some(next);
        return moved;
    }
    if shift {
        return select_range_to(state, next);
    }
    select_only(state, next)
}

/// Applies a typed character as type-ahead.
///
/// Returns whether a row was landed on. The search starts from the row last
/// acted upon, so each character is its own jump.
pub fn type_ahead(state: &mut ChoiceState, ch: char) -> bool {
    let labels: Vec<String> = state.options.iter().map(|o| o.label.clone()).collect();
    let from = state
        .caret_index
        .or_else(|| state.selected.iter().next().copied())
        .unwrap_or(0);
    match find_next(&labels, from, ch) {
        Some(index) => select_only(state, index),
        None => false,
    }
}

/// Reads the initially selected rows from the field's stored value and index
/// entries.
///
/// **The value wins where the two disagree**, which is asserted by a fixture
/// built to make them disagree. The index entry is consulted only when there
/// is no value to read.
#[must_use]
pub fn initial_selection(
    options: &[String],
    values: &[String],
    indices: &[usize],
) -> BTreeSet<usize> {
    if !values.is_empty() {
        return values
            .iter()
            .filter_map(|v| options.iter().position(|o| o == v))
            .collect();
    }
    indices
        .iter()
        .copied()
        .filter(|i| *i < options.len())
        .collect()
}

/// Where a list scrolls to so that `first_selected` is visible.
///
/// **It scrolls only far enough to fill the box**, which is why a list of ten
/// rows showing three at a time with the last row selected leaves row *eight*
/// at the top rather than row nine: scrolling to nine would leave two empty
/// rows below it. Reproduced as-asserted.
#[must_use]
pub fn top_visible_for(count: usize, visible_rows: usize, first_selected: usize) -> usize {
    if visible_rows == 0 || count <= visible_rows {
        return 0;
    }
    let max_top = count - visible_rows;
    first_selected.min(max_top)
}

/// The configuration an editable combo box's text half is given.
///
/// A combo box's text is single-line and unlimited whatever the field says,
/// and a non-editable one's is read-only — it still holds text and still
/// supports selecting and copying it, which is how a substring of a
/// non-editable combo's value can be selected with the mouse.
#[must_use]
pub fn combo_text_config(editable: bool, read_only: bool) -> TextConfig {
    TextConfig {
        multi_line: false,
        password: false,
        comb: false,
        max_len: None,
        read_only: read_only || !editable,
        undo_enabled: true,
        auto_scroll: true,
    }
}

/// Scrolls the view just far enough to bring `index` into it.
///
/// **Only when it is not already visible**, which is the whole rule
/// (`cpwl_list_ctrl.cpp:263-265`): moving the caret within the visible rows
/// scrolls nothing, and moving it past either edge scrolls by exactly the
/// overshoot. That is what keeps a run of arrow keys — or wheel notches,
/// which are the same operation — from scrolling on every step.
///
/// Answers whether the view moved.
pub fn scroll_into_view(state: &mut ChoiceState, index: usize, visible_rows: usize) -> bool {
    if visible_rows == 0 {
        return false;
    }
    let was = state.top_visible;
    if index < state.top_visible {
        // Above the view: the item becomes the first visible row.
        state.top_visible = index;
    } else if index >= state.top_visible.saturating_add(visible_rows) {
        // Below it: the item becomes the last, so the box stays full.
        state.top_visible = index.saturating_sub(visible_rows - 1);
    }
    state.top_visible != was
}

/// Carries the currently selected option into the combo box's text half.
///
/// # This is where the four-item undo group comes from
///
/// Upstream spells it as three operations — select all, replace the
/// selection, select all again (`cpwl_combo_box.cpp:522-527`) — and the
/// middle one is itself a group of two, a removal and an insertion. So
/// choosing an option from an open combo box pushes **four** undo items that
/// must undo together, which is the worst case `SessionConfig::max_undo_items`
/// is clamped to four to hold: a capacity that could not fit one whole group
/// would have to evict half of it.
///
/// The two `select_all`s are not redundant. The first is what makes the
/// replacement replace the *whole* text rather than inserting at a caret; the
/// second leaves the new text selected, so typing immediately after choosing
/// an option overwrites it rather than appending to it.
///
/// Answers whether anything was carried across, which is `false` for a combo
/// with no current selection.
pub fn set_select_text(
    state: &mut ChoiceState,
    config: &pdfrum_doc::vt::Config,
    metrics: &pdfrum_doc::vt::Metrics<'_>,
) -> bool {
    let Some(index) = state
        .caret_index
        .or_else(|| state.selected.iter().next().copied())
    else {
        return false;
    };
    let Some(option) = state.options.get(index) else {
        return false;
    };
    let text = option.label.clone();

    let edit = state
        .edit
        .get_or_insert_with(|| Box::new(crate::edit::TextEdit::new("", config, metrics, true)));
    edit.select_all();
    crate::edit::ops::replace_selection(edit, config, metrics, &text, None);
    edit.select_all();
    state.edit_text.clone_from(&edit.text);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{ChoiceConfig, ChoiceOption};

    fn options(labels: &[&str]) -> Vec<ChoiceOption> {
        labels
            .iter()
            .map(|l| ChoiceOption {
                label: (*l).to_string(),
                value: (*l).to_string(),
            })
            .collect()
    }

    fn combo(labels: &[&str]) -> ChoiceState {
        ChoiceState::new(
            options(labels),
            ChoiceConfig {
                combo: true,
                ..ChoiceConfig::default()
            },
        )
    }

    fn single_list(labels: &[&str]) -> ChoiceState {
        ChoiceState::new(options(labels), ChoiceConfig::default())
    }

    fn multi_list(labels: &[&str]) -> ChoiceState {
        ChoiceState::new(
            options(labels),
            ChoiceConfig {
                multi_select: true,
                ..ChoiceConfig::default()
            },
        )
    }

    const FRUIT: [&str; 5] = ["Apple", "Banana", "Cherry", "Date", "Elderberry"];

    /// A combo box refuses every clear; a single-select list box accepts one
    /// and can be left genuinely empty.
    #[test]
    fn only_a_list_box_can_be_emptied() {
        let mut combo = combo(&FRUIT);
        assert!(set_index_selected(&mut combo, 1, true));
        assert!(
            !set_index_selected(&mut combo, 1, false),
            "a combo box has no empty state"
        );
        assert!(is_index_selected(&combo, 1), "and it did not clear");

        let mut list = single_list(&FRUIT);
        assert!(set_index_selected(&mut list, 1, true));
        assert!(set_index_selected(&mut list, 1, false));
        assert!(list.selected.is_empty());
        assert_eq!(list.focused_text(), "Banana", "the caret stayed on it");
    }

    /// Out of range is rejected by both, and changes nothing.
    #[test]
    fn an_out_of_range_row_is_rejected() {
        let mut list = single_list(&FRUIT);
        assert!(set_index_selected(&mut list, 0, true));
        assert!(!set_index_selected(&mut list, 100, true));
        assert!(!set_index_selected(&mut list, 5, true));
        assert!(is_index_selected(&list, 0));
        assert!(!is_index_selected(&list, 100));
    }

    /// The behaviour that reads as a bug and is asserted: a clear that
    /// removes nothing still reports success and still moves the caret, so
    /// the field's focused text changes.
    #[test]
    fn a_clear_that_changes_nothing_still_moves_the_caret() {
        let mut list = multi_list(&FRUIT);
        set_index_selected(&mut list, 0, true);
        assert_eq!(list.focused_text(), "Apple");

        // Row 3 was never selected; clearing it is a no-op on the selection.
        assert!(set_index_selected(&mut list, 3, false));
        assert!(!is_index_selected(&list, 3));
        assert!(is_index_selected(&list, 0), "row 0 is untouched");
        assert_eq!(list.focused_text(), "Date", "but the caret moved");
    }

    /// A multi-select list's focused text follows the row last acted upon,
    /// not the selection's contents.
    #[test]
    fn focused_text_follows_the_last_row_acted_upon() {
        let mut list = multi_list(&FRUIT);
        set_index_selected(&mut list, 1, true);
        set_index_selected(&mut list, 2, true);
        set_index_selected(&mut list, 4, true);
        assert_eq!(list.focused_text(), "Elderberry");

        set_index_selected(&mut list, 4, false);
        set_index_selected(&mut list, 1, false);
        assert_eq!(list.focused_text(), "Banana");
    }

    /// A single-select list keeps one row; a multi-select one accumulates.
    #[test]
    fn multi_select_accumulates_and_single_select_does_not() {
        let mut single = single_list(&FRUIT);
        set_index_selected(&mut single, 0, true);
        set_index_selected(&mut single, 2, true);
        assert_eq!(single.selected.len(), 1);
        assert!(is_index_selected(&single, 2));

        let mut multi = multi_list(&FRUIT);
        set_index_selected(&mut multi, 0, true);
        set_index_selected(&mut multi, 2, true);
        assert_eq!(multi.selected.len(), 2);
        assert!(is_index_selected(&multi, 0));
        assert!(is_index_selected(&multi, 2));
    }

    /// Consecutive characters are independent jumps, not a growing prefix.
    #[test]
    fn type_ahead_jumps_rather_than_accumulating() {
        let mut combo = combo(&FRUIT);
        assert!(type_ahead(&mut combo, 'A'));
        assert_eq!(combo.focused_text(), "Apple");

        // A, then B, then C lands on Cherry — not on a row starting "ABC".
        type_ahead(&mut combo, 'A');
        type_ahead(&mut combo, 'B');
        type_ahead(&mut combo, 'C');
        assert_eq!(combo.focused_text(), "Cherry");

        type_ahead(&mut combo, 'A');
        type_ahead(&mut combo, 'B');
        assert_eq!(combo.focused_text(), "Banana");
    }

    #[test]
    fn type_ahead_ignores_case() {
        let mut combo = combo(&FRUIT);
        type_ahead(&mut combo, 'd');
        assert_eq!(combo.focused_text(), "Date");
    }

    /// The scan is circular: from the last row it wraps to the first.
    #[test]
    fn type_ahead_wraps_around() {
        let labels: Vec<String> = FRUIT.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(find_next(&labels, 4, 'A'), Some(0));
        assert_eq!(find_next(&labels, 0, 'B'), Some(1));
    }

    /// Nothing matching returns the last row probed rather than failing,
    /// which is exactly what makes the jumps independent.
    #[test]
    fn an_unmatched_character_lands_on_the_last_row_probed() {
        let labels: Vec<String> = FRUIT.iter().map(|s| (*s).to_string()).collect();
        // No row starts with Z; the scan ends where it began.
        assert_eq!(find_next(&labels, 2, 'Z'), Some(2));
    }

    #[test]
    fn type_ahead_on_an_empty_field_does_nothing() {
        let mut empty = combo(&[]);
        assert!(!type_ahead(&mut empty, 'A'));
        assert_eq!(find_next(&[], 0, 'A'), None);
    }

    /// The stored value wins where value and index entries disagree.
    #[test]
    fn the_value_wins_over_the_index_entry() {
        let options: Vec<String> = FRUIT.iter().map(|s| (*s).to_string()).collect();

        let by_index = initial_selection(&options, &[], &[2, 3]);
        assert_eq!(by_index, BTreeSet::from([2, 3]));

        let by_value = initial_selection(&options, &["Apple".to_string()], &[]);
        assert_eq!(by_value, BTreeSet::from([0]));

        // Disagreeing: the value's answer is the one that counts.
        let conflicting = initial_selection(&options, &["Banana".to_string()], &[3, 4]);
        assert_eq!(conflicting, BTreeSet::from([1]));
    }

    #[test]
    fn an_out_of_range_index_entry_is_dropped() {
        let options: Vec<String> = FRUIT.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            initial_selection(&options, &[], &[1, 99]),
            BTreeSet::from([1])
        );
    }

    /// A list scrolls only far enough to fill the box, so the selected row is
    /// not necessarily the top one.
    #[test]
    fn a_list_does_not_overscroll_past_its_last_full_page() {
        // Ten rows, three visible, the last row selected: the top row is 7,
        // not 9, because scrolling to 9 would leave the box short.
        assert_eq!(top_visible_for(10, 3, 9), 7);
        // A selection inside the range scrolls exactly to it.
        assert_eq!(top_visible_for(10, 3, 4), 4);
        // A list that fits needs no scrolling at all.
        assert_eq!(top_visible_for(3, 5, 2), 0);
        assert_eq!(top_visible_for(10, 0, 5), 0);
    }

    /// Shift-extends pivot on the anchor and do not move it, so successive
    /// ones all measure from the same row.
    #[test]
    fn a_range_selection_keeps_its_pivot() {
        let mut list = multi_list(&FRUIT);
        select_only(&mut list, 1);
        assert_eq!(list.anchor, Some(1));

        select_range_to(&mut list, 3);
        assert_eq!(list.selected, BTreeSet::from([1, 2, 3]));
        assert_eq!(list.anchor, Some(1), "the pivot did not move");

        // A second extend measures from the same pivot, not from row 3.
        select_range_to(&mut list, 2);
        assert_eq!(list.selected, BTreeSet::from([1, 2]));
    }

    /// A range that runs backwards from the anchor covers the same rows.
    #[test]
    fn a_backwards_range_selects_the_same_rows() {
        let mut list = multi_list(&FRUIT);
        select_only(&mut list, 3);
        select_range_to(&mut list, 1);
        assert_eq!(list.selected, BTreeSet::from([1, 2, 3]));
    }

    /// Toggling keeps the rest of the selection and *does* re-anchor, which
    /// is the difference from a range extend.
    #[test]
    fn toggling_keeps_the_rest_and_re_anchors() {
        let mut list = multi_list(&FRUIT);
        select_only(&mut list, 0);
        toggle_index(&mut list, 3);
        assert_eq!(list.selected, BTreeSet::from([0, 3]));
        assert_eq!(list.anchor, Some(3));

        toggle_index(&mut list, 3);
        assert_eq!(list.selected, BTreeSet::from([0]));
    }

    /// A single-select list ignores both gestures and just selects the row.
    #[test]
    fn a_single_select_list_ignores_the_modifiers() {
        let mut list = single_list(&FRUIT);
        select_only(&mut list, 0);
        select_range_to(&mut list, 3);
        assert_eq!(list.selected, BTreeSet::from([3]));

        toggle_index(&mut list, 1);
        assert_eq!(list.selected, BTreeSet::from([1]));
    }

    /// Arrow movement clamps at each end rather than wrapping.
    #[test]
    fn moving_the_selection_clamps_at_both_ends() {
        let mut list = single_list(&FRUIT);
        select_only(&mut list, 0);

        move_selection(&mut list, -1);
        assert!(is_index_selected(&list, 0), "clamped at the top");

        for _ in 0..10 {
            move_selection(&mut list, 1);
        }
        assert!(is_index_selected(&list, 4), "clamped at the bottom");
    }

    #[test]
    fn moving_an_empty_field_does_nothing() {
        let mut empty = single_list(&[]);
        assert!(!move_selection(&mut empty, 1));
    }

    /// A non-editable combo's text half is read-only but still present: it
    /// holds the text and supports selecting it, which is how a substring of
    /// a field the user cannot type into can still be selected.
    #[test]
    fn a_non_editable_combo_still_has_read_only_text() {
        let editable = combo_text_config(true, false);
        assert!(!editable.read_only);

        let fixed = combo_text_config(false, false);
        assert!(fixed.read_only);

        // A read-only field is read-only whether or not it is editable.
        assert!(combo_text_config(true, true).read_only);
    }

    #[test]
    fn a_combo_text_half_is_single_line_and_unlimited() {
        let config = combo_text_config(true, false);
        assert!(!config.multi_line);
        assert_eq!(config.max_len, None);
        assert!(config.undo_enabled);
    }
}
