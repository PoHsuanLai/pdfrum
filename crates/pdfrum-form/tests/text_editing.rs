//! Ported text-editing assertions.
//!
//! These drive the edit control the way the upstream tests drive a focused
//! field — type, select, replace, undo — and read the text back. The values
//! are the fixtures' own: `"Elephant"` in a field of ten characters, and
//! `"Hippopotamus"` as the twelve-character string that will not fit.
//!
//! The character-limit family is the reason this file is worth its length.
//! Eight upstream tests pin one rule from eight directions: **the limit
//! truncates the insertion, never the field.** Every one of them inserts
//! "Hippopotamus" somewhere in "Elephant" and expects a different ten
//! characters out.

use pdfrum_doc::vt::{self, Config, Metrics};
use pdfrum_form::edit::ops::{
    self, TextEdit, backspace, delete, insert_char, replace_and_keep_selection, replace_selection,
};
use pdfrum_form::edit::{Selection, place::PlaceExt};

/// A fixed-width face: every character one unit wide, so a plate of a known
/// width holds a known number of them and no font file is needed.
fn metrics() -> Metrics<'static> {
    Metrics {
        width: &|_| 1000,
        ascent: 800,
        descent: -200,
    }
}

/// A single-line field wide enough that nothing wraps.
fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(0.0, 0.0, 1000.0, 20.0),
        font_size: 1.0,
        ..Config::default()
    }
}

fn edit(text: &str) -> TextEdit {
    TextEdit::new(text, &config(), &metrics())
}

/// Types characters one at a time, as a keyboard would.
fn type_text(edit: &mut TextEdit, text: &str, max_len: Option<u32>) {
    for ch in text.chars() {
        insert_char(edit, &config(), &metrics(), ch, max_len);
    }
}

/// Selects a flat character range, leaving the caret at its end.
fn select(edit: &mut TextEdit, from: usize, to: usize) {
    let begin = vt::hit::place_of_word_index(&edit.layout, from);
    let end = vt::hit::place_of_word_index(&edit.layout, to);
    edit.selection = Selection::new(begin, end);
    edit.caret = end;
}

/// `TypeTextIntoTextField`: characters land in order.
#[test]
fn typing_appends_characters_in_order() {
    let mut field = edit("");
    type_text(&mut field, "ABCDE", None);
    assert_eq!(field.text, "ABCDE");
    assert_eq!(field.caret_index(), 5);
}

/// `UndoRedo`: five characters undo one at a time, and redo restores them one
/// at a time.
#[test]
fn typing_undoes_one_character_at_a_time() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    assert!(!field.undo.can_undo());

    type_text(&mut field, "ABCDE", None);
    assert_eq!(field.text, "ABCDE");
    assert!(field.undo.can_undo());
    assert!(!field.undo.can_redo());

    assert!(ops::undo(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABCD");
    assert!(ops::undo(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABC");
    assert!(field.undo.can_redo());

    assert!(ops::redo(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABCD");
    assert!(ops::redo(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABCDE");
    assert!(field.undo.can_undo());
    assert!(!field.undo.can_redo());
}

/// Undoing every step returns the field to empty and reports the bottom.
#[test]
fn undoing_every_step_empties_the_field() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    type_text(&mut field, "ABC", None);

    for _ in 0..3 {
        assert!(ops::undo(&mut field, &config, &metrics));
    }
    assert_eq!(field.text, "");
    assert!(!field.undo.can_undo());
    assert!(!ops::undo(&mut field, &config, &metrics));
}

/// `ContinuouslyReplaceAndKeepSelection`: three characters put in by one call
/// come out with **one** undo.
#[test]
fn a_multi_character_replace_undoes_in_one_step() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    assert!(replace_and_keep_selection(
        &mut field, &config, &metrics, "UVW", None
    ));
    assert_eq!(field.text, "UVW");

    assert!(ops::undo(&mut field, &config, &metrics));
    assert_eq!(field.text, "");
    assert!(!field.undo.can_undo());
}

/// `ReplaceAndKeepSelection`: the inserted text stays selected, undo restores
/// the selection that was live before the edit, and redo leaves it empty.
#[test]
fn replace_and_keep_selection_keeps_it_and_undo_restores_the_old_one() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    type_text(&mut field, "AB", None);

    // Select "A", as a shift-Right from the start does.
    select(&mut field, 0, 1);
    assert_eq!(field.selected_text(), "A");

    assert!(replace_and_keep_selection(
        &mut field, &config, &metrics, "XYZ", None
    ));
    assert_eq!(field.text, "XYZB");
    assert_eq!(field.selected_text(), "XYZ", "the insertion stays selected");

    assert!(ops::undo(&mut field, &config, &metrics));
    assert_eq!(field.text, "AB");
    assert_eq!(
        field.selected_text(),
        "A",
        "undo restores the pre-edit selection"
    );

    assert!(ops::redo(&mut field, &config, &metrics));
    assert_eq!(field.text, "XYZB");
    assert_eq!(
        field.selected_text(),
        "",
        "redo does not restore it: the selection is not an undo item"
    );
}

/// `ReplaceSelection`: unlike its sibling, this leaves a caret rather than a
/// selection.
#[test]
fn replace_selection_leaves_no_selection() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    type_text(&mut field, "AB", None);
    select(&mut field, 0, 1);

    assert!(replace_selection(
        &mut field, &config, &metrics, "XYZ", None
    ));
    assert_eq!(field.text, "XYZB");
    assert_eq!(field.selected_text(), "");
}

/// `CutAllTextUndoRestoresAllCharacters`: three characters typed separately,
/// then all cut at once, and **one** undo brings them all back.
#[test]
fn a_cut_of_everything_undoes_in_one_step() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    type_text(&mut field, "ABC", None);

    field.select_all();
    assert_eq!(field.selected_text(), "ABC");
    assert!(replace_selection(&mut field, &config, &metrics, "", None));
    assert_eq!(field.text, "");

    assert!(ops::undo(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABC", "the cut is atomic");
    assert!(
        field.undo.can_undo(),
        "the three characters are still below"
    );
}

/// `RedoCutSelection`: undoing a select-all cut restores both the text and
/// the selection.
#[test]
fn undoing_a_select_all_cut_restores_the_selection() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    type_text(&mut field, "AB", None);

    field.select_all();
    replace_selection(&mut field, &config, &metrics, "", None);
    assert_eq!(field.text, "");

    ops::undo(&mut field, &config, &metrics);
    assert_eq!(field.text, "AB");
    assert_eq!(field.selected_text(), "AB");
}

/// `SelectAllText`: a focused empty field answers an empty string, which is
/// a different answer from an unfocused one — a distinction the oracle's
/// byte-length return cannot make.
#[test]
fn select_all_on_an_empty_field_selects_nothing() {
    let mut field = edit("");
    field.select_all();
    assert_eq!(field.selected_text(), "");

    let mut hello = edit("Hello");
    hello.select_all();
    assert_eq!(hello.selected_text(), "Hello");
}

/// `DeleteTextField*`: backspace removes the character before the caret and
/// forward delete the one after.
#[test]
fn backspace_and_delete_take_opposite_sides_of_the_caret() {
    let (config, metrics) = (config(), metrics());

    let mut field = edit("ABCDE");
    field.set_caret_index(3);
    assert!(backspace(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABDE");

    let mut field = edit("ABCDE");
    field.set_caret_index(3);
    assert!(delete(&mut field, &config, &metrics));
    assert_eq!(field.text, "ABCE");
}

/// Neither runs off the end of the text.
#[test]
fn deleting_past_either_end_does_nothing() {
    let (config, metrics) = (config(), metrics());

    let mut field = edit("AB");
    field.set_caret_index(0);
    assert!(!backspace(&mut field, &config, &metrics));
    assert_eq!(field.text, "AB");

    field.set_caret_index(2);
    assert!(!delete(&mut field, &config, &metrics));
    assert_eq!(field.text, "AB");
}

/// Either operation with a selection removes the selection instead.
#[test]
fn deleting_with_a_selection_removes_the_selection() {
    let (config, metrics) = (config(), metrics());

    for forwards in [false, true] {
        let mut field = edit("ABCDE");
        select(&mut field, 1, 4);
        assert_eq!(field.selected_text(), "BCD");

        if forwards {
            delete(&mut field, &config, &metrics);
        } else {
            backspace(&mut field, &config, &metrics);
        }
        assert_eq!(field.text, "AE");
        assert_eq!(field.selected_text(), "");
    }
}

// ---------------------------------------------------------------------------
// The character-limit family: eight upstream tests, one rule.
// ---------------------------------------------------------------------------

/// The fixture's limited field: ten characters, holding "Elephant".
const LIMIT: Option<u32> = Some(10);

fn elephant() -> TextEdit {
    edit("Elephant")
}

/// `InsertTextInEmptyCharLimitTextFieldOverflow`: into a cleared field, a
/// twelve-character string is truncated at the tail.
#[test]
fn an_overlong_insert_into_an_empty_limited_field_is_truncated() {
    let (config, metrics) = (config(), metrics());
    let mut field = elephant();
    field.select_all();
    replace_selection(&mut field, &config, &metrics, "", LIMIT);
    assert_eq!(field.text, "");

    replace_selection(&mut field, &config, &metrics, "Hippopotamus", LIMIT);
    assert_eq!(field.text, "Hippopotam");
}

/// `InsertTextInPopulatedCharLimitTextField{Left,Middle,Right}`: only the
/// characters that fit go in, and the text already there is untouched.
#[test]
fn an_insert_into_a_populated_limited_field_takes_only_what_fits() {
    let (config, metrics) = (config(), metrics());

    // At the start.
    let mut field = elephant();
    field.set_caret_index(0);
    replace_selection(&mut field, &config, &metrics, "Hippopotamus", LIMIT);
    assert_eq!(field.text, "HiElephant");

    // In the middle, after "Eleph".
    let mut field = elephant();
    field.set_caret_index(5);
    replace_selection(&mut field, &config, &metrics, "Hippopotamus", LIMIT);
    assert_eq!(field.text, "ElephHiant");

    // At the end.
    let mut field = elephant();
    field.set_caret_index(8);
    replace_selection(&mut field, &config, &metrics, "Hippopotamus", LIMIT);
    assert_eq!(field.text, "ElephantHi");
}

/// `InsertTextAndReplaceSelectionInPopulatedCharLimitTextField{Whole,Left,
/// Middle,Right}`: replaced characters free up room, so a larger selection
/// admits more of the insertion.
#[test]
fn a_replaced_selection_frees_room_for_the_insertion() {
    let (config, metrics) = (config(), metrics());

    // The whole field: ten characters fit.
    let mut field = elephant();
    field.select_all();
    replace_selection(&mut field, &config, &metrics, "Hippopotamus", LIMIT);
    assert_eq!(field.text, "Hippopotam");

    // Each of the three names the substring it replaces, and the selection
    // is asserted before the replacement — an off-by-one in the indices would
    // otherwise turn this into a test of whatever it happened to select.
    for (from, to, selected, expected) in [
        (0, 4, "Elep", "Hippophant"),
        (2, 6, "epha", "ElHippopnt"),
        (4, 8, "hant", "ElepHippop"),
    ] {
        let mut field = elephant();
        select(&mut field, from, to);
        assert_eq!(field.selected_text(), selected);

        replace_selection(&mut field, &config, &metrics, "Hippopotamus", LIMIT);
        assert_eq!(field.text, expected, "replacing {selected:?}");
    }
}

/// A field already at its limit takes nothing more, but is not damaged by
/// the attempt.
#[test]
fn a_full_field_accepts_nothing_and_loses_nothing() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("ABCDEFGHIJ");
    assert_eq!(field.len_chars(), 10);

    field.set_caret_index(5);
    insert_char(&mut field, &config, &metrics, 'Z', LIMIT);
    assert_eq!(field.text, "ABCDEFGHIJ");
}

/// The limit truncates but never rejects: one character into a field with
/// one space left goes in.
#[test]
fn the_last_free_character_is_accepted() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("ABCDEFGHI");
    field.set_caret_index(9);
    assert!(insert_char(&mut field, &config, &metrics, 'J', LIMIT));
    assert_eq!(field.text, "ABCDEFGHIJ");
}

/// An unlimited field takes whatever it is given.
#[test]
fn a_field_with_no_limit_takes_everything() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("Elephant");
    field.set_caret_index(8);
    replace_selection(&mut field, &config, &metrics, "Hippopotamus", None);
    assert_eq!(field.text, "ElephantHippopotamus");
}

/// `FormTextFieldBiDiLiveEdit` types Hebrew, so the control must carry text
/// well outside ASCII — and index it by character, not by byte.
#[test]
fn non_ascii_text_is_indexed_by_character() {
    let (config, metrics) = (config(), metrics());
    let mut field = edit("");
    type_text(&mut field, "\u{05D1}\u{05D2}\u{05EA}", None);
    assert_eq!(field.text, "\u{05D1}\u{05D2}\u{05EA}");
    assert_eq!(field.len_chars(), 3);
    assert_eq!(field.caret_index(), 3);

    assert!(backspace(&mut field, &config, &metrics));
    assert_eq!(field.text, "\u{05D1}\u{05D2}");
}

/// The start of the text is the line header, not character zero.
#[test]
fn the_caret_starts_at_the_line_header() {
    let field = edit("ABC");
    assert_eq!(field.caret_index(), 0);
    assert!(field.caret.at_line_start());
}
