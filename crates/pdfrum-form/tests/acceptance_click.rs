//! The acceptance test for click-to-caret, on the real fixture's geometry.
//!
//! `InsertTextInPopulatedTextFieldMiddle` types `"ABCDEFGH"` into
//! `text_form_multiple.pdf`'s "Text Box", clicks at x = 134, inserts
//! `"Hello"` and expects `"ABCDHelloEFGH"` — so the click must put the caret
//! after exactly **four** characters. Every number below is the fixture's
//! own: `/Rect [100 100 200 130]`, `/DA (0 0 0 rg /F1 12 Tf)`.
//!
//! This is the test that catches a missing vertical offset. A single-line
//! field draws its text centred in its plate, so a click at the field's
//! visible mid-height falls *below* where the layout put the glyphs; a query
//! that is not told about the offset clamps to the end of the text and
//! answers word 7 instead of word 3.

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::edit::ops::{self, TextEdit, insert_char, replace_selection};

/// Helvetica's advance widths per mille, for the characters this test uses.
/// Transcribed by hand so the arithmetic can be checked without a font file.
fn helvetica(ch: u32) -> i32 {
    match char::from_u32(ch) {
        Some('A' | 'B' | 'E') => 667,
        Some('C' | 'D' | 'H') => 722,
        Some('F') => 611,
        Some('G') => 778,
        Some(' ') => 278,
        _ => 556,
    }
}

fn metrics() -> Metrics<'static> {
    Metrics {
        width: &helvetica,
        ascent: 723,
        descent: -207,
    }
}

/// The field's plate: its `/Rect`, deflated by the one-unit border the
/// appearance path deflates by, at the `/DA`'s twelve points.
fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(101.0, 101.0, 199.0, 129.0),
        font_size: 12.0,
        ..Config::default()
    }
}

fn field(text: &str) -> TextEdit {
    // A single-line field: centred, which is what makes the offset matter.
    TextEdit::new(text, &config(), &metrics(), true)
}

/// The click point the upstream test uses: x = 134 at the field's own y.
const CLICK: (f64, f64) = (134.0, 115.0);

/// `InsertTextInPopulatedTextFieldMiddle`, end to end.
#[test]
fn a_click_at_the_fixtures_x_lands_after_four_characters() {
    let (config, metrics) = (config(), metrics());
    let mut edit = field("");

    for ch in "ABCDEFGH".chars() {
        insert_char(&mut edit, &config, &metrics, ch, None);
    }
    assert_eq!(edit.text, "ABCDEFGH");

    ops::click_at(
        &mut edit,
        &config,
        &metrics,
        kurbo::Point::new(CLICK.0, CLICK.1),
    );
    assert_eq!(
        edit.caret_index(),
        4,
        "the click must land after ABCD; a missing vertical offset answers 8"
    );

    replace_selection(&mut edit, &config, &metrics, "Hello", None);
    assert_eq!(edit.text, "ABCDHelloEFGH");
}

/// The offset is what makes it work, stated as its own assertion so a
/// regression names the cause rather than only the symptom.
#[test]
fn the_field_carries_a_nonzero_drawing_offset() {
    let edit = field("ABCDEFGH");
    assert!(
        edit.offset.1 != 0.0,
        "a centred single-line field draws below its layout"
    );

    // A multiline field is not centred, and carries none.
    let multiline = TextEdit::new("ABCDEFGH", &config(), &metrics(), false);
    assert_eq!(multiline.offset, (0.0, 0.0));
}

/// A click at the field's left edge is the start of the text, and one past
/// its right edge is the end — the two ends of the same query.
#[test]
fn clicks_at_the_fields_ends_reach_both_ends_of_the_text() {
    let (config, metrics) = (config(), metrics());
    let mut edit = field("ABCDEFGH");

    ops::click_at(
        &mut edit,
        &config,
        &metrics,
        kurbo::Point::new(102.0, 115.0),
    );
    assert_eq!(edit.caret_index(), 0);

    ops::click_at(
        &mut edit,
        &config,
        &metrics,
        kurbo::Point::new(195.0, 115.0),
    );
    assert_eq!(edit.caret_index(), 8);
}

/// A drag from one click to another selects between them, which is what
/// makes a mouse selection work.
#[test]
fn a_drag_selects_between_two_points() {
    let (config, metrics) = (config(), metrics());
    let mut edit = field("ABCDEFGH");

    ops::click_at(
        &mut edit,
        &config,
        &metrics,
        kurbo::Point::new(102.0, 115.0),
    );
    ops::drag_to(
        &mut edit,
        &config,
        &metrics,
        kurbo::Point::new(CLICK.0, CLICK.1),
    );
    assert_eq!(edit.selected_text(), "ABCD");
}

/// `DoubleClickInTextField`: a double click selects the whole **field**, not
/// the word under the pointer.
///
/// The upstream comment says "the entire line" and its body says
/// `edit_impl_->SelectAll()` (`cpwl_edit.cpp:636-644`). On the fixture's
/// single-line `"Hello World"` those are the same answer, which is what let
/// the comment stand — see the multiline test below for where they part.
#[test]
fn a_double_click_selects_the_whole_field() {
    let (config, metrics) = (config(), metrics());
    let mut edit = TextEdit::new("Hello World", &config, &metrics, true);

    edit.select_all();
    assert_eq!(
        edit.selected_text(),
        "Hello World",
        "the whole field, not the word under the pointer"
    );
}

/// And on a **multiline** field the two readings differ, which is why the
/// router calls `select_all` rather than `select_line_at`.
///
/// A double click in the second line of a three-line field selects all three
/// upstream; selecting the line under the pointer would take only the middle
/// one.
#[test]
fn a_double_click_on_a_multiline_field_takes_every_line() {
    let mut config = config();
    config.multi_line = true;
    config.auto_return = true;
    let metrics = metrics();
    let mut edit = TextEdit::new("one\ntwo\nthree", &config, &metrics, true);

    // The point the router would have used, on the middle line.
    let middle = kurbo::Point::new(CLICK.0, CLICK.1);
    let line_only = {
        let mut probe = edit.clone();
        ops::select_line_at(&mut probe, &config, &metrics, middle);
        probe.selected_text()
    };

    edit.select_all();
    assert_eq!(edit.selected_text(), "one\ntwo\nthree");
    assert_ne!(
        edit.selected_text(),
        line_only,
        "the two readings must actually differ here, or this pins nothing"
    );
}

/// A focused field with a caret and no selection draws a caret; one with a
/// selection draws bands and **no** caret.
#[test]
fn the_overlay_shows_a_caret_or_bands_but_never_both() {
    let (config, metrics) = (config(), metrics());
    let mut edit = field("ABCDEFGH");

    ops::click_at(
        &mut edit,
        &config,
        &metrics,
        kurbo::Point::new(CLICK.0, CLICK.1),
    );
    let caret_only = ops::highlight(&edit, &config, &metrics, 0.4);
    assert!(caret_only.caret.is_some());
    assert!(caret_only.selection.is_empty());

    edit.select_all();
    let bands_only = ops::highlight(&edit, &config, &metrics, 0.4);
    assert!(
        bands_only.caret.is_none(),
        "a field with a selection shows no caret"
    );
    assert!(!bands_only.selection.is_empty());
}
