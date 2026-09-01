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

use pdfrum::{Document, EventModifiers, FormSession};

fn document() -> Document {
    Document::open("tests/fixtures/listbox_form.pdf").expect("the listbox fixture must open")
}

/// The upstream constant: every list box is clicked at this x.
const X: f32 = 102.0;
/// The **first visible row** of each field, from `:588-600`.
const SINGLE_FIRST: f32 = 371.0;
const MULTI_FIRST: f32 = 423.0;
const INDICES_FIRST: f32 = 273.0;
const VALUES_FIRST: f32 = 223.0;
const MISMATCH_FIRST: f32 = 173.0;
const READ_ONLY_Y: f32 = 510.0;

fn click(session: &mut FormSession<'_>, y: f32) {
    session.on_mouse_move(0, X, y, EventModifiers::NONE);
    session.on_mouse_down(0, X, y, EventModifiers::NONE);
    session.on_mouse_up(0, X, y, EventModifiers::NONE);
}

/// Focuses a field **without clicking it**, which is what the upstream rows
/// do (`FORM_OnFocus`) and the only way to read the selection the *file*
/// declares: a click is a selection gesture and replaces it.
fn focus(session: &mut FormSession<'_>, y: f32) {
    session.on_focus_at(0, X, y, EventModifiers::NONE);
}

/// Focuses a field without disturbing its selection, by clicking its first
/// visible row and reading back which rows the file had selected.
fn selected_rows(session: &FormSession<'_>, count: usize) -> Vec<usize> {
    (0..count)
        .filter(|i| session.is_index_selected(*i))
        .collect()
}

/// `CheckIfMultipleSelectedValues`: `/V` as an array selects every row it
/// names.
#[test]
fn a_value_array_selects_every_row_it_names() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    focus(&mut session, VALUES_FIRST);

    assert!(
        session.focused_annot().is_some(),
        "the click must reach the field"
    );
    let rows = selected_rows(&session, 10);
    assert!(
        rows.len() >= 2,
        "/V [(Epsilon) (Gamma)] names two rows, got {rows:?}"
    );
}

/// `CheckIfMultipleSelectedIndices`, and it currently **diverges from the
/// oracle**. The divergence is asserted as it stands rather than hidden, so
/// that fixing it turns this test red rather than leaving it silently green.
///
/// Upstream expects rows 1 and 3 selected. `CPDF_FormField::IsItemSelected`
/// (`cpdf_formfield.cpp:546-554`) consults **`/I` first** — as integer
/// indices — and falls back to `/V` only when `/I` is not usable, where
/// "usable" is `UseSelectedIndicesObject` (`:863-935`): there is no `/V` at
/// all, or `/I` has the same number of entries as `/V` and every index names
/// an option whose value appears in `/V`.
///
/// `pdfrum-doc`'s `ap::field_body::selected_indices` inverts that: it reads
/// `/V` first and consults `/I` only when `/V` is absent, and then compares
/// each entry as *text*, so an integer index matches no option. The two agree
/// on the `/V`-only and the mismatch fields — which is why those two rows
/// pass — and disagree on exactly this one.
///
/// **This is a `pdfrum-doc` reader, not this crate's**, and it is recorded in
/// `docs/status/M14.md` for that slice's owner.
#[test]
fn an_index_array_alone_currently_selects_nothing() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    focus(&mut session, INDICES_FIRST);

    assert!(session.focused_annot().is_some());
    assert_eq!(
        selected_rows(&session, 10),
        Vec::<usize>::new(),
        "the oracle selects rows 1 and 3 here; see this test's doc comment"
    );
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

/// A single-select list box: clicking a row selects exactly that row.
#[test]
fn a_single_select_list_selects_the_row_clicked() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    click(&mut session, SINGLE_FIRST);

    assert!(session.focused_annot().is_some());
    let rows = selected_rows(&session, 26);
    assert_eq!(
        rows.len(),
        1,
        "a single-select list holds one row, got {rows:?}"
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
