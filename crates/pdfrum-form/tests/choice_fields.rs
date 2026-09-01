//! Ported choice-field assertions, over the real fixture data.
//!
//! The option lists here are transcribed from `listbox_form.in` and
//! `combobox_form.in` rather than invented, because several of these tests
//! assert a *particular label* — "Ugli Fruit", "Banana", "Date", "Quebec" —
//! and a made-up list would turn a behavioural assertion into a tautology.
//!
//! Three of the rows reproduce behaviour upstream itself marks as wrong. They
//! are ported as-asserted, each with a note saying so, because the goldens
//! contain this behaviour and an implementation that "fixed" it would fail
//! the tests that pin it.

use std::collections::BTreeSet;

use pdfrum_form::field::choice::{
    initial_selection, is_index_selected, move_caret_by, select_only, set_index_selected,
    top_visible_for, type_ahead,
};
use pdfrum_form::field::{ChoiceConfig, ChoiceOption, ChoiceState};

/// `Listbox_MultiSelect`'s twenty-six fruit, verbatim.
const FRUIT: [&str; 26] = [
    "Apple",
    "Banana",
    "Cherry",
    "Date",
    "Elderberry",
    "Fig",
    "Guava",
    "Honeydew",
    "Indian Fig",
    "Jackfruit",
    "Kiwi",
    "Lemon",
    "Mango",
    "Nectarine",
    "Orange",
    "Persimmon",
    "Quince",
    "Raspberry",
    "Strawberry",
    "Tamarind",
    "Ugli Fruit",
    "Voavanga",
    "Wolfberry",
    "Xigua",
    "Yangmei",
    "Zucchini",
];

/// `Listbox_SingleSelectLastSelected`'s ten provinces, verbatim.
const PROVINCES: [&str; 10] = [
    "Alberta",
    "British Columbia",
    "Manitoba",
    "New Brunswick",
    "Newfoundland and Labrador",
    "Nova Scotia",
    "Ontario",
    "Prince Edward Island",
    "Quebec",
    "Saskatchewan",
];

/// `Listbox_SingleSelectLastSelected`'s own `/Rect` and `/DA` font size,
/// which together decide how many rows the widget shows.
const LAST_SELECTED_RECT: (f32, f32, f32, f32) = (100.0, 100.0, 200.0, 130.0);
const LAST_SELECTED_FONT_SIZE: f32 = 12.0;

fn options(labels: &[&str]) -> Vec<ChoiceOption> {
    labels
        .iter()
        .map(|l| ChoiceOption {
            label: (*l).to_string(),
            value: (*l).to_string(),
        })
        .collect()
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

fn single_list(labels: &[&str]) -> ChoiceState {
    ChoiceState::new(options(labels), ChoiceConfig::default())
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

/// `SetSelectionProgrammaticallyMultiSelectField`: the focused text of a
/// multi-select list is the row **last acted upon**, not a description of the
/// selection — and a call that selects nothing still moves it.
#[test]
fn a_multi_select_lists_focused_text_follows_the_last_row_acted_upon() {
    let mut list = multi_list(&FRUIT);

    for index in [5, 6, 20] {
        assert!(set_index_selected(&mut list, index, true));
    }
    assert_eq!(
        list.focused_text(),
        "Ugli Fruit",
        "the last row selected answers, not the first"
    );

    for index in [20, 1] {
        set_index_selected(&mut list, index, false);
    }
    assert_eq!(list.focused_text(), "Banana");

    // Row 3 was never selected, so this clears nothing at all — and still
    // moves the focused text to it. Asserted upstream, however odd it reads.
    assert!(!is_index_selected(&list, 3));
    assert!(set_index_selected(&mut list, 3, false));
    assert_eq!(list.focused_text(), "Date");
}

/// `CheckForNoOverscroll`: the field names index 9 as its top row, but the
/// list scrolls only far enough to fill its box — so the first visible row is
/// index 8, and clicking it selects Quebec rather than Saskatchewan.
#[test]
fn a_list_does_not_overscroll_past_its_last_full_page() {
    let mut list = single_list(&PROVINCES);

    // Only the last province is selected on opening.
    assert!(set_index_selected(&mut list, 9, true));
    for i in 0..10 {
        assert_eq!(is_index_selected(&list, i), i == 9);
    }

    // How many rows the widget shows is the fixture's geometry, not a
    // number chosen to make the arithmetic come out: /Rect [100 100 200 130]
    // is thirty units tall and /DA sets twelve points, so two rows fit.
    let rect_height = LAST_SELECTED_RECT.3 - LAST_SELECTED_RECT.1;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let visible_rows = (rect_height / LAST_SELECTED_FONT_SIZE).floor() as usize;
    assert_eq!(visible_rows, 2, "the fixture's own geometry shows two rows");

    let top = top_visible_for(PROVINCES.len(), visible_rows, 9);
    assert_eq!(top, 8, "scrolling to 9 would leave the box short");

    // Clicking the first visible row therefore selects Quebec.
    assert!(select_only(&mut list, top));
    for i in 0..10 {
        assert_eq!(is_index_selected(&list, i), i == 8);
    }
    assert_eq!(list.focused_text(), "Quebec");
}

/// `CheckIfMultipleSelectedIndices` / `…Values` / `…Mismatch`: the stored
/// value beats the index entry where a file makes them disagree.
#[test]
fn the_value_entry_wins_over_the_index_entry() {
    let greek: Vec<String> = ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    // Indices alone.
    assert_eq!(
        initial_selection(&greek, &[], &[1, 3]),
        BTreeSet::from([1, 3])
    );

    // Values alone.
    assert_eq!(
        initial_selection(&greek, &["Gamma".to_string()], &[]),
        BTreeSet::from([2])
    );

    // Both, disagreeing: the value's answer stands and the indices are not
    // consulted at all.
    assert_eq!(
        initial_selection(
            &greek,
            &["Gamma".to_string(), "Epsilon".to_string()],
            &[0, 1]
        ),
        BTreeSet::from([2, 4])
    );
}

/// `SetSelectionProgrammaticallyNonEditableField`: a combo box refuses every
/// deselection.
#[test]
fn a_combo_box_refuses_to_be_deselected() {
    let mut combo = combo(&FRUIT);
    assert!(set_index_selected(&mut combo, 1, true));
    assert!(is_index_selected(&combo, 1));

    assert!(
        !set_index_selected(&mut combo, 1, false),
        "a combo box has no empty state"
    );
    assert!(is_index_selected(&combo, 1), "and it did not clear");
}

/// `SetSelectionProgrammaticallySingleSelectField`: a single-select list box
/// *can* be emptied, unlike a combo box.
#[test]
fn a_single_select_list_can_be_emptied() {
    let mut list = single_list(&["Foo", "Bar", "Qux"]);
    assert!(set_index_selected(&mut list, 1, true));
    assert!(set_index_selected(&mut list, 1, false));

    assert!(list.selected.is_empty(), "the list is genuinely empty");
    for i in 0..3 {
        assert!(!is_index_selected(&list, i));
    }
}

/// `BadApiInputsListBox` / `…ComboBox`: an out-of-range row is refused and
/// changes nothing.
///
/// Upstream also checks index −1. That row is not portable and its absence is
/// a *reduction* in reachable misuse rather than a dropped assertion: the
/// signature here takes an unsigned index, so no such call can be written.
#[test]
fn an_out_of_range_row_is_refused() {
    for mut field in [multi_list(&FRUIT), single_list(&FRUIT), combo(&FRUIT)] {
        assert!(set_index_selected(&mut field, 0, true));
        let before = field.selected.clone();

        assert!(!set_index_selected(&mut field, 100, true));
        assert!(!set_index_selected(&mut field, FRUIT.len(), true));
        assert_eq!(field.selected, before, "a refused call changed nothing");

        assert!(!is_index_selected(&field, 100));
    }
}

/// `FocusChanges` step 17: type-ahead jumps rather than accumulating, so
/// consecutive characters are independent searches.
#[test]
fn combo_type_ahead_treats_each_character_as_its_own_jump() {
    let mut field = combo(&FRUIT);

    type_ahead(&mut field, 'A');
    assert_eq!(field.focused_text(), "Apple");

    // A, then B, then C lands on Cherry — not on anything spelled "ABC".
    for ch in ['A', 'B', 'C'] {
        type_ahead(&mut field, ch);
    }
    assert_eq!(field.focused_text(), "Cherry");

    for ch in ['A', 'B'] {
        type_ahead(&mut field, ch);
    }
    assert_eq!(field.focused_text(), "Banana");
}

/// A multi-select list accumulates rows while a single-select one keeps one,
/// which is the difference the two fixtures exist to show.
#[test]
fn multi_select_accumulates_where_single_select_replaces() {
    let mut multi = multi_list(&FRUIT);
    for index in [1, 5, 20] {
        set_index_selected(&mut multi, index, true);
    }
    assert_eq!(multi.selected, BTreeSet::from([1, 5, 20]));

    let mut single = single_list(&FRUIT);
    for index in [1, 5, 20] {
        set_index_selected(&mut single, index, true);
    }
    assert_eq!(
        single.selected,
        BTreeSet::from([20]),
        "only the last stands"
    );
}

/// `SetIndexSelectedShouldFailGracefully`: a text field has no rows, so every
/// index is out of range and every call is refused.
#[test]
fn a_field_with_no_rows_refuses_every_index() {
    let mut empty = single_list(&[]);
    assert!(!set_index_selected(&mut empty, 0, true));
    assert!(!set_index_selected(&mut empty, 100, true));
    assert!(!is_index_selected(&empty, 0));
    assert_eq!(empty.focused_text(), "");
}

/// `CPWL_ListCtrl::OnVK` with **Control**: the caret moves and the selection
/// does not.
///
/// The upstream body is literally `if (bCtrl) {}` — an empty branch — which
/// reads as an oversight and is not: it is how a caret is walked to a row
/// before the row is toggled. An arrow key and a wheel notch both take this
/// path, because `OnKeyDown` and `OnMouseWheel` call the same function with
/// the same flags.
#[test]
fn the_accelerator_moves_a_multi_selects_caret_without_its_selection() {
    let mut list = multi_list(&FRUIT);
    select_only(&mut list, 3);
    assert_eq!(list.selected, BTreeSet::from([3]));

    assert!(move_caret_by(&mut list, 1, false, true));
    assert_eq!(list.caret_index, Some(4), "the caret moved");
    assert_eq!(
        list.selected,
        BTreeSet::from([3]),
        "and the selection did not"
    );
}

/// With **Shift** the same gesture ranges from the anchor instead.
#[test]
fn shift_ranges_a_multi_selects_selection_from_its_anchor() {
    let mut list = multi_list(&FRUIT);
    select_only(&mut list, 3);

    assert!(move_caret_by(&mut list, 1, true, false));
    assert_eq!(list.selected, BTreeSet::from([3, 4]));

    // The anchor does not follow, so a second step widens the same run.
    assert!(move_caret_by(&mut list, 1, true, false));
    assert_eq!(list.selected, BTreeSet::from([3, 4, 5]));
}

/// With neither, it replaces — and re-anchors, so a later Shift ranges from
/// the new row rather than the old one.
#[test]
fn a_bare_step_replaces_the_selection_and_re_anchors() {
    let mut list = multi_list(&FRUIT);
    select_only(&mut list, 3);

    assert!(move_caret_by(&mut list, 1, false, false));
    assert_eq!(list.selected, BTreeSet::from([4]));

    assert!(move_caret_by(&mut list, 1, true, false));
    assert_eq!(
        list.selected,
        BTreeSet::from([4, 5]),
        "the bare step re-anchored on row 4"
    );
}

/// A **single-select** list ignores all three, because `OnVK`'s whole
/// modifier structure sits inside `IsMultipleSel()`.
#[test]
fn a_single_select_list_ignores_the_modifiers_entirely() {
    for (shift, ctrl) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut list = single_list(&FRUIT);
        select_only(&mut list, 3);
        move_caret_by(&mut list, 1, shift, ctrl);
        assert_eq!(
            list.selected,
            BTreeSet::from([4]),
            "shift={shift} ctrl={ctrl} must still just select row 4"
        );
    }
}
