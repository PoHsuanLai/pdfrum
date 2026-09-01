//! The rectangles a selection paints, on a line that runs each way.
//!
//! `form_textfield_selected_rtl` is the fixture these exist for. Its `.evt`
//! types six Hebrew letters around an ellipsis and a space — `charcode,1489`
//! through `charcode,1514` — and double-clicks to select the lot, leaving the
//! field focused so the image is the *live* editor's rather than a generated
//! appearance stream. Its sibling `form_textfield_selected_ltr` does the same
//! with `abc... def`, which is what makes the pair able to tell a
//! direction-blind implementation from a correct one.
//!
//! The rule they pin is `CPWL_EditImpl::DrawEdit`'s
//! (`fpdfsdk/pwl/cpwl_edit_impl.cpp:659-676`): the selection is filled
//! **one word at a time**, each from `GetWordRect(word, line)` — `[x, x +
//! width]` at the line's ascent and descent. That has no left-to-right
//! assumption in it, which is the whole reason upstream needs no
//! right-to-left case.
//!
//! # The measurement that produced this file
//!
//! Spanning one band from the caret at the selection's start to the caret at
//! its end is right for a left-to-right line and collapses on a
//! right-to-left one, because the carets do not advance monotonically with
//! the word index: for `בחר` at 12 points in a plate spanning x 1..99 they
//! run 1, 19, 13, 7. The two *endpoints* are the run's interior, so the band
//! came out about one character wide — six units where the oracle paints
//! fifty.

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::edit::ops::{self, TextEdit};

/// One advance for every character, so a band's width is countable by hand:
/// at 12 points, 500/1000 of an em is exactly **6 units** per character.
fn metrics() -> Metrics<'static> {
    Metrics {
        width: &|_| 500,
        ascent: 905,
        descent: -211,
    }
}

/// A 100x30 widget's client plate, deflated by its one-unit border — the
/// geometry of both `form_textfield_selected_*` fixtures.
fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(1.0, 1.0, 99.0, 29.0),
        font_size: 12.0,
        ..Config::default()
    }
}

/// Three Hebrew letters: bet, het, resh — the `.evt`'s first word.
const HEBREW: &str = "\u{5d1}\u{5d7}\u{5e8}";

/// The whole of the fixture's text, both words and the ellipsis between.
const HEBREW_LINE: &str = "\u{5d1}\u{5d7}\u{5e8}... \u{5d0}\u{5d2}\u{5ea}";

/// The bands a fully selected field paints.
fn bands_of(text: &str) -> Vec<kurbo::Rect> {
    let (config, metrics) = (config(), metrics());
    let mut edit = TextEdit::new(text, &config, &metrics, true);
    edit.select_all();
    ops::highlight(&edit, &config, &metrics, 0.4).selection
}

/// The leftmost and rightmost edge any band reaches.
fn span(bands: &[kurbo::Rect]) -> (f64, f64) {
    bands.iter().fold((f64::MAX, f64::MIN), |acc, r| {
        (acc.0.min(r.x0), acc.1.max(r.x1))
    })
}

/// A left-to-right selection covers every character it names, and no more.
///
/// Ten characters at six units each is sixty, from the plate's left edge.
#[test]
fn a_left_to_right_selection_spans_its_whole_run() {
    let bands = bands_of("abc... def");
    let (left, right) = span(&bands);
    assert!((left - 1.0).abs() < 0.01, "starts at the plate's left edge");
    assert!(
        (right - left - 60.0).abs() < 0.01,
        "ten characters at six units, got {}",
        right - left
    );
}

/// And a **right-to-left** selection covers the same width.
///
/// This is the assertion the old implementation failed: it produced a band
/// about one character wide, because it read the run's two endpoint carets as
/// its extremes and on this line they are its interior.
#[test]
fn a_right_to_left_selection_spans_its_whole_run_too() {
    let bands = bands_of(HEBREW_LINE);
    let (left, right) = span(&bands);
    assert!(
        (right - left - 60.0).abs() < 0.01,
        "ten characters at six units, got {} — a run that collapses here is \
         the direction bug this file exists for",
        right - left
    );
}

/// The two directions produce the **same** covered width for the same number
/// of characters, which is the property that makes the pair of fixtures a
/// test rather than two independent numbers.
#[test]
fn the_two_directions_cover_equal_widths() {
    let (ltr_left, ltr_right) = span(&bands_of("abc"));
    let (rtl_left, rtl_right) = span(&bands_of(HEBREW));
    assert!((ltr_right - ltr_left - 18.0).abs() < 0.01);
    assert!(
        ((rtl_right - rtl_left) - (ltr_right - ltr_left)).abs() < 0.01,
        "three characters cover three characters' width either way"
    );
}

/// A contiguous run is **one** rectangle, not one per character.
///
/// Merging matters for more than tidiness: the bands are filled and then the
/// glyphs are painted white over them, so a gap between two adjacent bands
/// would show as a dark seam through the selected text.
#[test]
fn a_contiguous_left_to_right_run_merges_into_one_band() {
    assert_eq!(bands_of("abc... def").len(), 1);
}

/// A selection covering nothing paints nothing — the empty selection is not
/// a zero-width band.
#[test]
fn an_empty_selection_paints_no_bands() {
    let (config, metrics) = (config(), metrics());
    let edit = TextEdit::new("abc", &config, &metrics, true);
    let highlight = ops::highlight(&edit, &config, &metrics, 0.4);
    assert!(highlight.selection.is_empty());
    assert!(
        highlight.caret.is_some(),
        "and a field with no selection shows a caret instead"
    );
}

/// A **multiline** selection is one band per line, because two lines' words
/// share no edge.
#[test]
fn a_selection_across_a_line_break_is_two_bands() {
    let mut config = config();
    config.multi_line = true;
    config.auto_return = true;
    let metrics = metrics();

    let mut edit = TextEdit::new("abc\ndef", &config, &metrics, false);
    edit.select_all();
    let bands = ops::highlight(&edit, &config, &metrics, 0.4).selection;
    assert_eq!(bands.len(), 2, "one band per line, got {bands:?}");

    // And they are on different rows, which is what "per line" means.
    let (first, second) = (bands[0], bands[1]);
    assert!(
        (first.y0 - second.y0).abs() > 0.01,
        "the two bands must sit on different lines"
    );
}
