//! Ported `cpwl_edit_embeddertest.cpp` assertions.
//!
//! The upstream file drives a focused text field through `CPWL_Edit`'s own
//! API — `SetText`, `SetSelection`, `ReplaceSelection` — rather than through
//! events, so these port onto the edit control directly. The two clusters
//! worth naming:
//!
//! **The eight `SetTextWith*` rows** pin one rule from eight directions: a
//! **single-line** field drops every line break, wherever it falls and
//! whichever spelling it takes. `"Foo\n"`, `"Foo\r"`, `"Foo\n\r"` and
//! `"Foo\r\n"` all become `"Foo"`, and `"Foo\nBar"` and its three siblings
//! all become `"FooBar"` — the break is removed, not turned into a space and
//! not treated as a terminator.
//!
//! **`GetSelectedTextFragments`** pins `SetSelection`'s signed indices, which
//! are three instructions sharing one signature rather than a clamp. See
//! [`TextEdit::set_selection`].

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::edit::ops::{self, TextEdit, insert_char, replace_selection};

/// A fixed-width face: one unit per character, so no font file is needed and
/// a plate of a known width holds a known number of them.
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

/// A **multiline** field, for the one rule that differs between the two.
fn multiline_config() -> Config {
    Config {
        plate: kurbo::Rect::new(0.0, 0.0, 1000.0, 200.0),
        font_size: 1.0,
        multi_line: true,
        auto_return: true,
        ..Config::default()
    }
}

fn edit(text: &str) -> TextEdit {
    TextEdit::new(text, &config(), &metrics(), true)
}

/// `TypeTextIntoTextField(n)`: the upstream helper types `'A' + i`, so fifty
/// characters run from `A` up through the punctuation into lower case.
fn typed(count: u32) -> TextEdit {
    let mut edit = edit("");
    for i in 0..count {
        let ch = char::from_u32(u32::from(b'A') + i).unwrap_or('?');
        insert_char(&mut edit, &config(), &metrics(), ch, None);
    }
    edit
}

/// The exact fifty characters `TypeTextIntoTextField(50)` produces.
const FIFTY: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqr";

#[test]
fn the_helper_reproduces_the_upstream_fifty_character_run() {
    // The run is asserted directly, because every selection row below is
    // stated in terms of it and a drifting helper would move them all.
    assert_eq!(typed(50).text, FIFTY);
}

/// `TypeText`: characters land in order, and an empty field starts empty.
#[test]
fn typing_into_an_empty_field_appends_in_order() {
    let mut edit = edit("");
    assert!(edit.text.is_empty());
    for ch in "abc".chars() {
        insert_char(&mut edit, &config(), &metrics(), ch, None);
    }
    assert_eq!(edit.text, "abc");
}

/// `GetSelectedTextEmptyAndBasic`: a selection set before anything is typed
/// is empty, and the same call after typing is not.
#[test]
fn a_selection_over_an_empty_field_selects_nothing() {
    let mut edit = edit("");
    edit.set_selection(0, 3);
    assert_eq!(edit.selected_text(), "");

    for ch in "abc".chars() {
        insert_char(&mut edit, &config(), &metrics(), ch, None);
    }
    edit.set_selection(0, 2);
    assert_eq!(edit.selected_text(), "ab");
}

/// `GetSelectedTextFragments`: the whole signed-index table in one test, as
/// upstream states it.
#[test]
fn the_signed_selection_indices_mean_three_different_things() {
    let mut edit = typed(50);

    // An empty range selects nothing.
    edit.set_selection(0, 0);
    assert_eq!(edit.selected_text(), "");

    edit.set_selection(0, 1);
    assert_eq!(edit.selected_text(), "A");

    // (0, negative) is "to the end", not an error and not an empty range.
    edit.set_selection(0, -1);
    assert_eq!(edit.selected_text(), FIFTY);

    // A negative *start* selects nothing — it is not clamped to zero, which
    // is what would otherwise make this the whole field again.
    edit.set_selection(-8, -1);
    assert_eq!(edit.selected_text(), "");

    // The two ends are ordered, so a reversed range is the same run.
    edit.set_selection(23, 12);
    assert_eq!(edit.selected_text(), "MNOPQRSTUVW");
    edit.set_selection(12, 23);
    assert_eq!(edit.selected_text(), "MNOPQRSTUVW");

    // The last character, and an end past the text clamping to it.
    edit.set_selection(49, 50);
    assert_eq!(edit.selected_text(), "r");
    edit.set_selection(49, 55);
    assert_eq!(edit.selected_text(), "r");
}

/// `DeleteEntireTextSelection`: replacing everything with nothing empties the
/// field.
#[test]
fn replacing_the_whole_selection_with_nothing_empties_the_field() {
    let mut edit = typed(50);
    edit.set_selection(0, -1);
    assert_eq!(edit.selected_text(), FIFTY);

    replace_selection(&mut edit, &config(), &metrics(), "", None);
    assert!(edit.text.is_empty());
}

/// `DeleteEmptyTextSelection`: replacing an empty selection with nothing
/// leaves the field alone.
#[test]
fn replacing_an_empty_selection_with_nothing_changes_nothing() {
    let mut edit = typed(50);
    edit.set_selection(0, 0);
    replace_selection(&mut edit, &config(), &metrics(), "", None);
    assert_eq!(edit.text, FIFTY);
}

/// `DeleteTextSelectionLeft` / `Middle` / `Right`: a run removed from each
/// end and from the middle.
#[test]
fn a_selected_run_is_removed_from_either_end_or_the_middle() {
    for (from, to, expected) in [
        (0, 5, "FGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqr"),
        (25, 30, "ABCDEFGHIJKLMNOPQRSTUVWXY_`abcdefghijklmnopqr"),
        (45, 50, "ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklm"),
    ] {
        let mut edit = typed(50);
        edit.set_selection(from, to);
        replace_selection(&mut edit, &config(), &metrics(), "", None);
        assert_eq!(edit.text, expected, "removing [{from}, {to})");
    }
}

/// `ReplaceAndKeepSelection`: the replacement stays selected, and a
/// **reversed** input selection produces a forward one over the new text.
#[test]
fn replacing_and_keeping_the_selection_reselects_the_replacement() {
    let mut edit = typed(10);
    edit.set_selection(1, 3);
    assert_eq!(edit.text, "ABCDEFGHIJ");

    ops::replace_and_keep_selection(&mut edit, &config(), &metrics(), "xyz", None);
    assert_eq!(edit.text, "AxyzDEFGHIJ");
    assert_eq!(edit.selected_text(), "xyz");
    assert_eq!(edit.selection_indices(), (1, 4));

    // The reversed case: (4, 1) selects the same run, and the replacement
    // comes back selected forward regardless.
    edit.set_selection(4, 1);
    ops::replace_and_keep_selection(&mut edit, &config(), &metrics(), "12", None);
    assert_eq!(edit.text, "A12DEFGHIJ");
    assert_eq!(edit.selected_text(), "12");
    assert_eq!(edit.selection_indices(), (1, 3));
}

/// The four `SetTextWithEnd*` rows: a single-line field drops a trailing
/// break in all four spellings.
#[test]
fn a_single_line_field_drops_a_trailing_line_break() {
    for input in ["Foo\n", "Foo\r", "Foo\n\r", "Foo\r\n"] {
        assert_eq!(
            edit(input).text,
            "Foo",
            "a single-line field must drop the break in {input:?}"
        );
    }
}

/// The four `SetTextWithBody*` rows: an interior break is **removed**, not
/// turned into a space and not treated as a terminator.
#[test]
fn a_single_line_field_removes_an_interior_line_break() {
    for input in ["Foo\nBar", "Foo\rBar", "Foo\n\rBar", "Foo\r\nBar"] {
        assert_eq!(
            edit(input).text,
            "FooBar",
            "a single-line field must join across the break in {input:?}"
        );
    }
}

/// The other side of the same switch, which no upstream row states because
/// the fixture is single-line: a **multiline** field keeps its breaks. Stated
/// here so the rule above reads as "single-line drops them" rather than as
/// "line breaks are always dropped".
#[test]
fn a_multiline_field_keeps_an_interior_line_break() {
    let edit = TextEdit::new("Foo\nBar", &multiline_config(), &metrics(), false);
    assert!(
        edit.text.contains('\n'),
        "a multiline field keeps the break: {:?}",
        edit.text
    );
}

/// The invariant the normalization exists to keep, stated directly: the
/// control's text and its layout agree on how many characters there are.
///
/// Without it a single-line `"Foo\nBar"` reports seven characters while its
/// layout holds six, and every index derived from one misses in the other —
/// which is a caret landing in the wrong place rather than a cosmetic
/// difference.
#[test]
fn the_text_and_the_layout_never_disagree_about_length() {
    for input in [
        "Foo\nBar",
        "Foo\r\nBar",
        "\n\n\n",
        "a\tb",
        "plain",
        "",
        "trailing\n",
    ] {
        let edit = edit(input);
        let laid_out: usize = edit
            .layout
            .sections
            .iter()
            .map(|section| section.words.len())
            .sum();
        assert_eq!(
            edit.len_chars(),
            laid_out,
            "text and layout disagree for {input:?}: {:?}",
            edit.text
        );
    }
}
