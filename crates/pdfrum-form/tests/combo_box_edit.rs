//! Ported `cpwl_combo_box_edit_embeddertest.cpp` assertions.
//!
//! A combo box has two halves — a list of options and a one-line text control
//! — and these rows are about the seam between them. The fixture's editable
//! combo offers `Foo`, `Bar`, `Qux`.
//!
//! The row that pays for this file is the **four-item undo group**. Choosing
//! an option is spelled as select-all, replace, select-all
//! (`cpwl_combo_box.cpp:522-527`), and the replace is itself a removal plus an
//! insertion — so one choice pushes four items that must undo together. It is
//! the worst case `max_undo_items`' floor of four exists to hold, and until
//! `ChoiceState` carried its own `TextEdit` there was nowhere to build it.

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::edit::ops::{self, TextEdit};
use pdfrum_form::field::choice::{self, set_select_text};
use pdfrum_form::field::{ChoiceConfig, ChoiceOption, ChoiceState};

fn metrics() -> Metrics<'static> {
    Metrics {
        width: &|_| 1000,
        ascent: 800,
        descent: -200,
    }
}

fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(0.0, 0.0, 1000.0, 20.0),
        font_size: 1.0,
        ..Config::default()
    }
}

fn options(labels: &[&str]) -> Vec<ChoiceOption> {
    labels
        .iter()
        .map(|label| ChoiceOption {
            label: (*label).to_string(),
            value: (*label).to_string(),
        })
        .collect()
}

/// The fixture's editable combo box: `Foo`, `Bar`, `Qux`.
fn editable() -> ChoiceState {
    ChoiceState::new(
        options(&["Foo", "Bar", "Qux"]),
        ChoiceConfig {
            combo: true,
            editable: true,
            multi_select: false,
            read_only: false,
        },
    )
}

/// The text half of a combo, once it has one.
fn text_of(state: &ChoiceState) -> &str {
    state.edit.as_ref().map_or("", |edit| edit.text.as_str())
}

/// `GetSelectedTextEmptyAndBasicEditable`: an editable combo starts empty,
/// and choosing an option carries its label into the text half **selected**.
#[test]
fn choosing_an_option_carries_it_into_the_text_half_selected() {
    let mut state = editable();
    assert_eq!(text_of(&state), "");

    choice::select_only(&mut state, 0);
    assert!(set_select_text(&mut state, &config(), &metrics()));
    assert_eq!(text_of(&state), "Foo");
    // Selected, not merely present: typing next would overwrite it.
    let edit = state.edit.as_ref().expect("the text half exists now");
    assert_eq!(edit.selected_text(), "Foo");

    // Choosing another option replaces the first rather than appending.
    choice::select_only(&mut state, 1);
    set_select_text(&mut state, &config(), &metrics());
    assert_eq!(text_of(&state), "Bar");
}

/// The second `select_all` is what makes the *next* choice replace rather
/// than append — asserted directly, because a reader deleting it as
/// redundant would break exactly this.
#[test]
fn the_carried_text_is_left_selected_so_the_next_edit_overwrites() {
    let mut state = editable();
    choice::select_only(&mut state, 1);
    set_select_text(&mut state, &config(), &metrics());

    let edit = state.edit.as_mut().expect("the text half exists");
    assert!(edit.has_selection());
    ops::insert_char(edit, &config(), &metrics(), 'x', None);
    assert_eq!(edit.text, "x", "typing replaces the whole carried label");
}

/// `SetEditSelection(2, 3)`: a substring of the carried text can be selected.
#[test]
fn a_substring_of_the_carried_text_can_be_selected() {
    let mut state = editable();
    choice::select_only(&mut state, 1);
    set_select_text(&mut state, &config(), &metrics());

    let edit = state.edit.as_mut().expect("the text half exists");
    edit.set_selection(2, 3);
    assert_eq!(edit.selected_text(), "r");
}

/// The upstream row's tail: after choosing `Bar`, selecting its last
/// character and typing `abc` over it gives `Baabc`.
#[test]
fn typing_over_a_substring_of_the_carried_text_replaces_it() {
    let mut state = editable();
    choice::select_only(&mut state, 1);
    set_select_text(&mut state, &config(), &metrics());

    let edit = state.edit.as_mut().expect("the text half exists");
    edit.set_selection(2, 3);
    for ch in "abc".chars() {
        ops::insert_char(edit, &config(), &metrics(), ch, None);
    }
    assert_eq!(edit.text, "Baabc");

    edit.set_selection(0, 5);
    assert_eq!(edit.selected_text(), "Baabc");
}

/// `InsertTextInEmptyEditableComboBox`: replacing the selection of an empty
/// combo inserts.
#[test]
fn replacing_the_selection_of_an_empty_combo_inserts() {
    let mut edit = TextEdit::new("", &config(), &metrics(), true);
    ops::replace_selection(&mut edit, &config(), &metrics(), "Hello", None);
    assert_eq!(edit.text, "Hello");
}

/// **The four-item undo group.** Choosing an option must undo as one step,
/// not as four.
#[test]
fn choosing_an_option_undoes_as_one_step() {
    let mut state = editable();

    // Start with something to replace, so the group's removal half is not
    // empty — an empty removal would make the group smaller than the worst
    // case this test exists to build.
    let edit = state
        .edit
        .get_or_insert_with(|| Box::new(TextEdit::new("", &config(), &metrics(), true)));
    for ch in "old".chars() {
        ops::insert_char(edit, &config(), &metrics(), ch, None);
    }

    choice::select_only(&mut state, 2);
    set_select_text(&mut state, &config(), &metrics());
    assert_eq!(text_of(&state), "Qux");

    // One undo returns the whole choice, not a quarter of it.
    let edit = state.edit.as_mut().expect("the text half exists");
    assert!(ops::undo(edit, &config(), &metrics()));
    assert_eq!(
        edit.text, "old",
        "choosing an option must undo as a single step"
    );
}

/// A combo with nothing chosen carries nothing across, and says so rather
/// than inventing an empty selection.
#[test]
fn a_combo_with_no_selection_carries_nothing() {
    let mut state = editable();
    assert!(!set_select_text(&mut state, &config(), &metrics()));
}

/// A non-editable combo's text half is read-only but still present: a
/// substring of it can be selected, which is how text a user cannot type into
/// is still copyable.
#[test]
fn a_non_editable_combo_still_holds_and_selects_its_text() {
    let mut state = ChoiceState::new(
        options(&["Apple", "Banana"]),
        ChoiceConfig {
            combo: true,
            editable: false,
            multi_select: false,
            read_only: false,
        },
    );
    choice::select_only(&mut state, 0);
    assert!(set_select_text(&mut state, &config(), &metrics()));

    let edit = state.edit.as_mut().expect("the text half exists");
    edit.set_selection(0, 3);
    assert_eq!(edit.selected_text(), "App");
}
