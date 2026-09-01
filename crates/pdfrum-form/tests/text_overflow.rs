//! `DoNotScroll` refuses input once the plate is full, and refuses the wheel
//! always.
//!
//! # The rule, and the half that had no port
//!
//! `TextEdit::auto_scroll` is upstream's `enable_scroll_`
//! (`fpdfsdk/pwl/cpwl_edit_impl.h:284`), raised by
//! `CFFL_TextField::GetCreateParam` (`cffl_textfield.cpp:54-63`) for every
//! text field **without** the `DoNotScroll` flag. It gates two separate
//! things, and only one of them had been ported:
//!
//! 1. `SetScrollPosX` / `SetScrollPosY` (`:1162-1193`) return at their first
//!    statement, so the view never moves. `scroll_to_caret` honoured this
//!    from the start; `scroll_text` — the wheel — did not, and is the second
//!    half of this file.
//! 2. `IsTextOverflow` (`:1984-1996`) is true when the field can neither
//!    scroll nor overflow **and** its content is bigger than its plate, and
//!    `InsertWord` (`:1698`), `InsertReturn` (`:1719`) and `InsertText`
//!    (`:1839`) each return without mutating when it is. So a full
//!    `DoNotScroll` field **rejects further characters** rather than
//!    accepting them into a value nothing can see. PDF 32000-1 Table 228 says
//!    the same: once the field is full, no further text is accepted.
//!
//! `enable_overflow_` — the other half of `IsTextOverflow`'s guard — is only
//! ever raised for a **comb** field (`cpwl_edit.cpp:134-136` and `:285`, both
//! under `Styles::kEditTextOverflow`, which `GetCreateParam` sets for
//! `kTextComb` alone), so a comb never refuses and every other
//! `DoNotScroll` field does.

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::edit::ops::{self, TextEdit, insert_char};

/// A fixed-width face: one unit per character at `font_size` 1, so a plate
/// ten wide holds exactly ten characters and every count below is an integer.
fn metrics() -> Metrics<'static> {
    Metrics {
        width: &|_| 1000,
        ascent: 800,
        descent: -200,
    }
}

/// A single-line field ten characters wide.
fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(0.0, 0.0, 10.0, 20.0),
        font_size: 1.0,
        ..Config::default()
    }
}

/// The same field as a **comb** of ten cells, which is the one shape that
/// sets `enable_overflow_` and so never refuses.
fn comb_config() -> Config {
    Config {
        char_array: 10,
        ..config()
    }
}

/// A `DoNotScroll` field, empty.
fn fixed(config: &Config) -> TextEdit {
    let mut edit = TextEdit::new("", config, &metrics(), true);
    edit.auto_scroll = false;
    edit
}

/// Types `text` one character at a time and answers how many were accepted.
fn type_all(edit: &mut TextEdit, config: &Config, text: &str) -> usize {
    text.chars()
        .filter(|ch| insert_char(edit, config, &metrics(), *ch, None))
        .count()
}

/// The headline: a `DoNotScroll` plate ten wide takes eleven characters and
/// then stops.
///
/// Eleven, not ten, and the extra one is upstream's own arithmetic rather
/// than a rounding allowance: `IsTextOverflow` is asked **before** the
/// insertion, so the character that first makes the content exceed the plate
/// is accepted and the one after it is refused. A port that asked afterwards
/// would stop at ten.
#[test]
fn a_fixed_field_stops_taking_characters_when_its_plate_is_full() {
    let config = config();
    let mut edit = fixed(&config);
    let accepted = type_all(&mut edit, &config, "abcdefghijklmno");
    assert_eq!(
        accepted, 11,
        "ten fit and the eleventh overflows; the twelfth onward are refused"
    );
    assert_eq!(
        edit.text, "abcdefghijk",
        "the refused characters are not in the value either"
    );
}

/// The same field **with** scrolling takes the lot, which is what makes the
/// assertion above about the flag rather than about the plate.
#[test]
fn a_scrolling_field_takes_every_character() {
    let config = config();
    let mut edit = TextEdit::new("", &config, &metrics(), true);
    let accepted = type_all(&mut edit, &config, "abcdefghijklmno");
    assert_eq!(accepted, 15);
    assert_eq!(edit.text, "abcdefghijklmno");
}

/// A comb field never refuses, because `SetCharArray` sets
/// `enable_overflow_` (`cpwl_edit.cpp:285`) and `IsTextOverflow`'s guard is
/// `!enable_scroll_ && !enable_overflow_`.
///
/// `/MaxLen` is what bounds a comb, and that is a different gate — one this
/// crate already honours through `room_for`.
#[test]
fn a_comb_field_overflows_rather_than_refusing() {
    let config = comb_config();
    let mut edit = fixed(&config);
    let accepted = type_all(&mut edit, &config, "abcdefghijklmno");
    assert_eq!(
        accepted, 15,
        "a comb accepts past its cells; only /MaxLen caps it"
    );
}

/// Deleting always works, however full the field is — the refusal is on the
/// insertion alone.
///
/// `CPWL_EditImpl::ReplaceSelection` (`:1881-1890`) is `ClearSelection()`
/// **then** `InsertText`, and only the second half is gated. A user who
/// overfills a field must be able to empty it again.
#[test]
fn a_full_field_still_deletes() {
    let config = config();
    let mut edit = fixed(&config);
    type_all(&mut edit, &config, "abcdefghijklmno");
    let before = edit.text.clone();
    assert!(ops::backspace(&mut edit, &config, &metrics()));
    assert_eq!(edit.text.chars().count(), before.chars().count() - 1);
}

/// And typing over a selection works, for the same reason: the clear runs
/// first, so the overflow question is asked of the shortened text.
///
/// Selecting the whole of a full field and typing one character must leave
/// that one character, not the old value untouched.
#[test]
fn typing_over_the_whole_selection_of_a_full_field_replaces_it() {
    let config = config();
    let mut edit = fixed(&config);
    type_all(&mut edit, &config, "abcdefghijklmno");
    edit.select_all();
    assert!(
        insert_char(&mut edit, &config, &metrics(), 'z', None),
        "clearing the selection empties the plate, so the insert fits"
    );
    assert_eq!(edit.text, "z");
}

/// `replace_selection` — the embedder's paste — refuses its insertion half on
/// a field still full after the removal, while performing its removal half
/// regardless.
///
/// The removal is what decides, not the insertion's length: upstream clears
/// and *then* asks `IsTextOverflow`, so a one-character selection out of an
/// eleven-character overflowing field leaves ten, which still exceeds a
/// ten-wide plate by nothing — and the check is `IsFloatBigger`, so equal is
/// not bigger. Two characters out leaves nine and the paste would land. This
/// case selects **none**, so the caret insert is refused outright and the
/// text is untouched.
#[test]
fn a_paste_into_a_full_field_with_nothing_selected_inserts_nothing() {
    let config = config();
    let mut edit = fixed(&config);
    type_all(&mut edit, &config, "abcdefghijklmno");
    let before = edit.text.clone();
    edit.set_caret_index(0);
    assert!(!ops::replace_selection(
        &mut edit,
        &config,
        &metrics(),
        "12345678",
        None
    ));
    assert_eq!(edit.text, before, "a full field takes no paste");
}

/// And a paste whose removal makes room **does** land, which is what makes
/// the refusal above about the plate rather than about pasting.
#[test]
fn a_paste_that_first_empties_the_field_lands() {
    let config = config();
    let mut edit = fixed(&config);
    type_all(&mut edit, &config, "abcdefghijklmno");
    edit.select_all();
    assert!(ops::replace_selection(
        &mut edit,
        &config,
        &metrics(),
        "12345678",
        None
    ));
    assert_eq!(edit.text, "12345678");
}

/// The wheel is `SetScrollPosY`, and `SetScrollPosY` returns at its first
/// statement when the field declines to scroll (`:1175-1178`).
///
/// Asserted on a multiline field with more content than plate, which is the
/// only shape whose wheel does anything at all: a scrolling one moves and a
/// `DoNotScroll` one does not.
#[test]
fn the_wheel_does_not_move_a_field_that_declines_to_scroll() {
    let config = Config {
        plate: kurbo::Rect::new(0.0, 0.0, 10.0, 3.0),
        font_size: 1.0,
        multi_line: true,
        ..Config::default()
    };
    let text = "a\nb\nc\nd\ne\nf";
    let mut scrolling = TextEdit::new(text, &config, &metrics(), false);
    let mut fixed = TextEdit::new(text, &config, &metrics(), false);
    fixed.auto_scroll = false;

    // The control: a field that may scroll does, or the assertion below would
    // pass for a fixture with nothing to scroll rather than for the flag.
    assert!(
        ops::scroll_by(&mut scrolling, &config, -1),
        "a scrolling field with more content than plate moves on a notch"
    );
    assert!(scrolling.scroll.1 > 0.0);

    assert!(
        !ops::scroll_by(&mut fixed, &config, -1),
        "DoNotScroll makes the wheel a no-op"
    );
    assert!(
        (fixed.scroll.1 - 0.0).abs() < f32::EPSILON,
        "and it writes nothing, got {}",
        fixed.scroll.1
    );
}
