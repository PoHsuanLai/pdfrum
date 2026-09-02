//! Ported list-box selection-source assertions, driven end to end.
//!
//! `listbox_form.pdf`'s six fields are a matrix over *where a selection comes
//! from*: `/V` alone, `/I` alone, both agreeing, both disagreeing, and a
//! `/TI` that scrolls. The rule the matrix exists to pin is that **`/V` wins
//! where the two disagree**, and the mismatch field is built so that a reader
//! taking `/I` gets a visibly different answer: `/V [(Alligator) (Cougar)]`
//! names rows 0 and 2 of `[Alligator Bear Cougar Deer Echidna]`, while
//! `/I [1 3 4]` names three different ones.

// See `form_routing.rs`: the fixture helper is a failure signal.
#![allow(clippy::expect_used)]

use pdfrum::{Document, FormSession, Modifiers, kurbo::Point};

fn document() -> Document {
    Document::open("tests/fixtures/listbox_form.pdf").expect("the listbox fixture must open")
}

/// The upstream constant: every list box is clicked at this x.
const X: f64 = 102.0;
/// The **first visible row** of each field, from `:588-600`.
const SINGLE_FIRST: f64 = 371.0;
/// And its **second** visible row, `kSingleFormYSecondVisibleOption`.
const SINGLE_SECOND: f64 = 358.0;
const MULTI_FIRST: f64 = 423.0;
const INDICES_FIRST: f64 = 273.0;
const VALUES_FIRST: f64 = 223.0;
const MISMATCH_FIRST: f64 = 173.0;
const READ_ONLY_Y: f64 = 510.0;

fn click(session: &mut FormSession<'_>, y: f64) {
    session.mouse_move(0, Point::new(X, y), Modifiers::NONE);
    session.mouse_down(0, Point::new(X, y), Modifiers::NONE);
    session.mouse_up(0, Point::new(X, y), Modifiers::NONE);
}

/// Focuses a field **without clicking it**, which is what the upstream rows
/// do (`FORM_OnFocus`) and the only way to read the selection the *file*
/// declares: a click is a selection gesture and replaces it.
fn focus(session: &mut FormSession<'_>, y: f64) {
    session.focus_at(0, Point::new(X, y), Modifiers::NONE);
}

/// Focuses a field without disturbing its selection, by clicking its first
/// visible row and reading back which rows the file had selected.
fn selected_rows(session: &FormSession<'_>, count: usize) -> Vec<usize> {
    (0..count)
        .filter(|i| session.is_index_selected(*i))
        .collect()
}

/// `CheckIfMultipleSelectedValues`: `/V [(Gamma) (Epsilon)]` selects rows
/// **2 and 4**, the upstream row's own two indices.
#[test]
fn a_value_array_selects_every_row_it_names() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    focus(&mut session, VALUES_FIRST);

    assert!(
        session.focused_annot().is_some(),
        "the click must reach the field"
    );
    assert_eq!(selected_rows(&session, 5), vec![2, 4]);
}

/// `CheckIfMultipleSelectedIndices`: `/I [1 3]` with **no `/V`** selects rows
/// 1 and 3 — 'Belgium' and 'Denmark'.
///
/// This is the one shape where the two readers of a selection disagree, and
/// both are right about their own job. `CPDF_FormField::IsItemSelected`
/// (`cpdf_formfield.cpp:546-554`) consults **`/I` first**, as integer
/// indices, and falls back to `/V` only when `/I` is not *usable* —
/// `UseSelectedIndicesObject` (`:863-935`): there is no `/V` at all, or `/I`
/// has the same number of entries as `/V` and every index names an option
/// whose value appears in `/V`. That is the reader interaction uses.
///
/// The **appearance** producer for a file with no `/NeedAppearances` is a
/// different function — `CPDFSDK_AppStream::SetAsListBox` →
/// `GetValueOrSelectedIndicesObject` — which takes `/V` first and matches it
/// as text, and that is what `ap::field_body::selected_indices` reproduces.
/// So the two are not a bug and a fix; they are two questions.
#[test]
fn an_index_array_alone_selects_the_rows_it_names() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    focus(&mut session, INDICES_FIRST);

    assert!(session.focused_annot().is_some());
    assert_eq!(selected_rows(&session, 5), vec![1, 3]);
}

/// `CheckIfMultipleSelectedMismatch`: **`/V` wins**, and the fixture is built
/// so that following `/I` gives a different answer.
#[test]
fn the_value_array_wins_over_a_disagreeing_index_array() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    focus(&mut session, MISMATCH_FIRST);

    assert!(session.focused_annot().is_some());
    let rows = selected_rows(&session, 5);
    assert_eq!(
        rows,
        vec![0, 2],
        "/V [(Alligator) (Cougar)] wins over /I [1 3 4]"
    );
}

/// A single-select list box: clicking the **first visible row** selects that
/// row, index 0 — not merely "one row".
///
/// This is the assertion that catches a click computed in the wrong space.
/// The widget is `/Rect [100 350 200 380]`, so its client rectangle lives at
/// the origin while the click arrives at page y 371; subtracting the one from
/// the other without `FFLtoPWL` gives a large negative offset, no row, and a
/// click that focuses the widget and selects nothing. The old assertion could
/// not see it, because the file already had one row selected.
#[test]
fn a_single_select_list_selects_the_first_visible_row_clicked() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, SINGLE_FIRST);

    assert!(session.focused_annot().is_some());
    assert_eq!(selected_rows(&session, 26), vec![0]);
}

/// And clicking the **second** visible row selects row 1, which is what makes
/// the row arithmetic — not just the origin — assertable.
///
/// `kSingleFormYSecondVisibleOption` is 358, thirteen points below the first,
/// which is one laid-out row of the 12-point face rather than twelve.
#[test]
fn clicking_the_second_visible_row_selects_the_second_option() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, SINGLE_SECOND);

    assert_eq!(selected_rows(&session, 26), vec![1]);
}

/// A row is the **laid-out** line tall, not the `/DA` font size.
///
/// `CPWL_ListCtrl::Item::GetItemHeight` (`cpwl_list_ctrl.cpp:49-51`) is the
/// item's own `GetContentRect().Height()`, and `ReArrange` (`:525-551`)
/// stacks the items by exactly that. The laid-out line is
/// `(ascent - descent) * size / 1000`, which equals the font size only for a
/// face whose pair happens to sum to 1000 — Arimo's sums to 1116, giving
/// 13.392 units at 12 points where the size would say 12.
///
/// The number cannot be written down here, because it is the face's and this
/// binary does not load the corpus's. What *is* assertable is that the row
/// boundaries are evenly spaced by a step that is **not** the 12-point font
/// size, which is the property that failed before: rows 0, 1 and 2 begin at
/// equal intervals, and that interval is not 12.
#[test]
fn a_row_is_the_laid_out_line_tall_not_the_font_size() {
    let doc = document();

    // Walk down the widget and record where the selected row changes.
    let mut boundaries = Vec::new();
    let mut previous: Option<Vec<usize>> = None;
    let mut y = 379.0_f64;
    while y > 351.0 {
        let mut session = FormSession::new(&doc);
        click(&mut session, y);
        let rows = selected_rows(&session, 26);
        if previous.as_ref().is_some_and(|was| *was != rows) {
            boundaries.push(y);
        }
        previous = Some(rows);
        y -= 0.05;
    }

    assert_eq!(
        boundaries.len(),
        2,
        "three rows fit, so there are two edges"
    );
    let step = boundaries[0] - boundaries[1];
    assert!(
        (step - 12.0).abs() > 0.5,
        "a row is the laid-out line, not the 12-point size; measured {step}"
    );
    // And the two edges are one step apart from the client's top, which is
    // what "evenly stacked" means.
    let client_top = 380.0 - 1.0; // /Rect top, less the one-unit border.
    assert!(
        ((client_top - boundaries[0]) - step).abs() < 0.2,
        "the first row starts at the client's top"
    );
}

/// A multi-select list box accumulates where a single-select one replaces.
#[test]
fn a_multi_select_list_accumulates() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, MULTI_FIRST);
    assert!(session.focused_annot().is_some());

    // Programmatic selection is the API the upstream rows drive.
    assert!(session.set_index_selected(0, true));
    assert!(session.set_index_selected(2, true));
    assert!(session.is_index_selected(0));
    assert!(session.is_index_selected(2));
}

/// A **read-only** list box is not clickable, so it never takes focus.
#[test]
fn a_read_only_list_never_takes_focus() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, READ_ONLY_Y);
    assert!(session.focused_annot().is_none());
}

/// `BadApiInputsListBox`, the representable half: an out-of-range row is
/// refused by both calls, and refusing it changes nothing.
#[test]
fn an_out_of_range_row_is_refused() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, MULTI_FIRST);

    assert!(!session.set_index_selected(100, true));
    assert!(!session.is_index_selected(100));
    assert!(!session.is_index_selected(usize::MAX));
}
