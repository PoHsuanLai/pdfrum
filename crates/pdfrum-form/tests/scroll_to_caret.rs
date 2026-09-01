//! `ScrollToCaret` (`fpdfsdk/pwl/cpwl_edit_impl.cpp:1246-1286`), which runs
//! after every mutation and every caret move.
//!
//! Without it a single-line field never scrolls: typing past the right edge
//! walks the caret out of the plate and leaves it there, so a viewer shows
//! the first N characters of a long value with the insertion point invisible.
//! The oracle shows the **last** N with the caret against the plate's right
//! edge.
//!
//! # The two frames, which is the whole of the port
//!
//! Upstream's `scroll_pos_point_.x` is an absolute layout position seeded at
//! `rcPlate.left`; ours is a *distance* that `LiveState::shift` negates. So
//! ours is upstream's minus `plate.left`, and the branches read:
//!
//! | upstream | here |
//! |---|---|
//! | `VTToEdit(head).x` | `head - scroll.0` |
//! | `SetScrollPosX(ptHead.x)` | `scroll.0 = head - plate.left` |
//! | `SetScrollPosX(ptHead.x - rcPlate.Width())` | `scroll.0 = head - plate.left - width` |
//!
//! The comparisons are made on the **edit-space** point and the assignment
//! from the **layout-space** one, and reading both in one frame — the obvious
//! simplification — is wrong by exactly one advance.

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::edit::ops::{self, TextEdit, insert_char};

/// A fixed-width face: one unit per character at `font_size` 1, so a plate
/// ten wide holds exactly ten characters and every expected number below is
/// an integer.
fn metrics() -> Metrics<'static> {
    Metrics {
        width: &|_| 1000,
        ascent: 800,
        descent: -200,
    }
}

/// A single-line field **ten characters wide**, which is the point: it is
/// narrow enough that an ordinary word overflows it.
fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(0.0, 0.0, 10.0, 20.0),
        font_size: 1.0,
        ..Config::default()
    }
}

/// The same field, offset so that `plate.left` is not zero — the term the
/// frame conversion has to drop, and the one a port that keeps upstream's
/// absolute position would leave in.
fn offset_config() -> Config {
    Config {
        plate: kurbo::Rect::new(100.0, 0.0, 110.0, 20.0),
        font_size: 1.0,
        ..Config::default()
    }
}

fn typed(config: &Config, text: &str) -> TextEdit {
    let mut edit = TextEdit::new("", config, &metrics(), true);
    for ch in text.chars() {
        insert_char(&mut edit, config, &metrics(), ch, None);
    }
    edit
}

/// A value that fits needs no scroll at all, and must not get one.
///
/// `SetScrollLimit` (`cpwl_edit_impl.cpp:1211-1213`) pins the view at
/// `rcPlate.left` whenever the plate is wider than the content, which is what
/// keeps a short value flush left instead of drifting with the caret.
#[test]
fn a_value_that_fits_is_never_scrolled() {
    let edit = typed(&config(), "abcd");
    assert!(
        (edit.scroll.0 - 0.0).abs() < 1e-4,
        "four characters in a ten-wide plate must not scroll, got {}",
        edit.scroll.0
    );
}

/// Typing past the right edge scrolls by exactly the overflow, leaving the
/// caret **on** the edge rather than one advance inside it.
///
/// Fifteen characters into a ten-wide plate puts the caret at layout x 15,
/// and `SetScrollPosX(ptHead.x - rcPlate.Width())` puts the view at
/// `15 - 10 = 5`. The visible run is then characters 5 through 14 — the last
/// ten — which is what the oracle draws.
#[test]
fn typing_past_the_edge_scrolls_by_the_overflow_exactly() {
    let edit = typed(&config(), "abcdefghijklmno");
    assert!(
        (edit.scroll.0 - 5.0).abs() < 1e-4,
        "fifteen characters in a ten-wide plate scroll to 5, got {}",
        edit.scroll.0
    );
    // The caret's edit-space position is the plate's right edge, not one
    // advance short of it. Off by one advance here is `password`'s six
    // asterisks against the oracle's five.
    let head = ops::caret_x(&edit, &config(), &metrics());
    assert!(
        (head - edit.scroll.0 - 10.0).abs() < 1e-4,
        "the caret lands on the plate's right edge, got {}",
        head - edit.scroll.0
    );
}

/// The scroll is a **distance**, so a plate that does not start at zero
/// scrolls by the same amount as one that does.
///
/// This is the assertion a port that stored upstream's absolute
/// `scroll_pos_point_` without subtracting `plate.left` fails: it would
/// answer 105 here and shift the drawn text a hundred units off the field.
#[test]
fn the_scroll_is_a_distance_and_not_an_absolute_position() {
    let flush = typed(&config(), "abcdefghijklmno");
    let offset = typed(&offset_config(), "abcdefghijklmno");
    assert!(
        (flush.scroll.0 - offset.scroll.0).abs() < 1e-4,
        "a plate at x=100 scrolls like one at x=0: {} vs {}",
        flush.scroll.0,
        offset.scroll.0
    );
}

/// Walking the caret back to the start scrolls the view back with it.
///
/// The first branch — `FXSYS_IsFloatSmaller(ptHeadEdit.x, rcPlate.left)` or
/// equal — is what makes Home usable in a scrolled field, and it seeds the
/// view *at* the caret rather than by a step.
#[test]
fn deleting_back_to_the_start_unscrolls() {
    let config = config();
    let mut edit = typed(&config, "abcdefghijklmno");
    assert!(edit.scroll.0 > 0.0, "the field must start scrolled");
    for _ in 0..15 {
        ops::backspace(&mut edit, &config, &metrics());
    }
    assert!(
        (edit.scroll.0 - 0.0).abs() < 1e-4,
        "an emptied field is back at its origin, got {}",
        edit.scroll.0
    );
}

/// `auto_scroll` gates the whole thing, as `enable_scroll_` gates
/// `SetScrollPosX` at its first statement (`cpwl_edit_impl.cpp:1163`).
///
/// A field carrying `DoNotScroll` keeps its view at the origin however far
/// past the plate the caret goes — which is the flag's entire meaning.
#[test]
fn a_field_that_declines_to_scroll_does_not() {
    let config = config();
    let mut edit = TextEdit::new("", &config, &metrics(), true);
    edit.auto_scroll = false;
    for ch in "abcdefghijklmno".chars() {
        insert_char(&mut edit, &config, &metrics(), ch, None);
    }
    assert!(
        (edit.scroll.0 - 0.0).abs() < 1e-4,
        "DoNotScroll pins the view at the origin, got {}",
        edit.scroll.0
    );
}
