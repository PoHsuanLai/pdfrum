//! Ported focus-traversal assertions.
//!
//! The fixture is the oracle's own `annotiter.pdf`, transcribed rather than
//! parsed: one field with four widget kids, on three pages that differ only
//! in the traversal order they ask for. Its geometry is the point — the four
//! widgets sit at the corners of a square, in an annotation order that is
//! none of the three traversal orders, so each order produces a different
//! answer and a wrong implementation cannot accidentally agree.
//!
//! ```text
//!   (201,400) LeftTop  #2        #1 RightTop  (401,401)
//!
//!   (200,200) LeftBottom #0      #3 RightBottom (400,201)
//! ```

use pdfrum_form::session::AnnotId;
use pdfrum_form::tab::{FocusRing, Focusable, Rect, TabOrder};

/// The four widgets of `annotiter.pdf`, in the order its `/Annots` array
/// lists them.
fn annotiter_widgets() -> Vec<Focusable> {
    [
        (0, 200.0, 200.0, 220.0, 220.0), // Sub_LeftBottom
        (1, 401.0, 401.0, 421.0, 421.0), // Sub_RightTop
        (2, 201.0, 400.0, 221.0, 420.0), // Sub_LeftTop
        (3, 400.0, 201.0, 420.0, 221.0), // Sub_RightBottom
    ]
    .into_iter()
    .map(|(index, left, bottom, right, top)| Focusable {
        id: AnnotId::new(0, index),
        rect: Rect::new(left, bottom, right, top),
    })
    .collect()
}

fn row_ring() -> FocusRing {
    FocusRing::build(&annotiter_widgets(), TabOrder::Row)
}

fn indices(ring: &FocusRing) -> Vec<u32> {
    ring.order.iter().map(|a| a.index).collect()
}

/// `FormFillFirstTab`: a forward tab with nothing focused lands on annot 1.
#[test]
fn first_tab_lands_on_annot_one() {
    assert_eq!(row_ring().first(), Some(AnnotId::new(0, 1)));
}

/// `FormFillFirstShiftTab`: a backward tab with nothing focused lands on
/// annot 0 — a *different* annotation, because the cursor starts between the
/// ends rather than before them.
#[test]
fn first_shift_tab_lands_on_annot_zero() {
    assert_eq!(row_ring().last(), Some(AnnotId::new(0, 0)));
}

/// `FormFillContinuousTab`: four forward tabs visit 1, 2, 3, 0 and the fifth
/// is not handled, because focus does not wrap.
#[test]
fn continuous_tab_visits_one_two_three_zero_then_stops() {
    let ring = row_ring();
    let mut at = ring.first().expect("the ring is not empty");
    assert_eq!(at, AnnotId::new(0, 1));

    let mut visited = vec![at.index];
    for _ in 0..3 {
        at = ring.next(at).expect("a next annotation");
        visited.push(at.index);
    }
    assert_eq!(visited, vec![1, 2, 3, 0]);

    assert_eq!(ring.next(at), None, "the fifth tab is not handled");
}

/// `FormFillContinuousShiftTab`: four backward tabs visit 0, 3, 2, 1 and the
/// fifth is not handled.
#[test]
fn continuous_shift_tab_visits_zero_three_two_one_then_stops() {
    let ring = row_ring();
    let mut at = ring.last().expect("the ring is not empty");
    assert_eq!(at, AnnotId::new(0, 0));

    let mut visited = vec![at.index];
    for _ in 0..3 {
        at = ring.prev(at).expect("a previous annotation");
        visited.push(at.index);
    }
    assert_eq!(visited, vec![0, 3, 2, 1]);

    assert_eq!(ring.prev(at), None, "the fifth shift-tab is not handled");
}

/// The forward and backward walks are exact reverses of one another over this
/// fixture, which is what makes the two "first tab" answers differ.
#[test]
fn the_backward_walk_reverses_the_forward_one() {
    let ring = row_ring();
    let forward = indices(&ring);
    let mut backward = forward.clone();
    backward.reverse();

    let mut walked = vec![ring.last().expect("a last").index];
    let mut at = ring.last().expect("a last");
    while let Some(prev) = ring.prev(at) {
        walked.push(prev.index);
        at = prev;
    }
    assert_eq!(walked, backward);
}

/// The three pages of the fixture differ only in the order they ask for, and
/// each produces a different answer over the same four widgets — which is
/// what makes the fixture able to tell the orders apart at all.
#[test]
fn the_three_orders_disagree_over_this_fixture() {
    let widgets = annotiter_widgets();
    let row = indices(&FocusRing::build(&widgets, TabOrder::Row));
    let column = indices(&FocusRing::build(&widgets, TabOrder::Column));
    let structure = indices(&FocusRing::build(&widgets, TabOrder::Structure));

    assert_eq!(structure, vec![0, 1, 2, 3], "structure order sorts nothing");
    assert_eq!(row, vec![1, 2, 3, 0]);
    assert_ne!(row, column);
    assert_ne!(row, structure);
    assert_ne!(column, structure);
}

/// Every order is a permutation of the annotations: none is dropped and none
/// is visited twice, whichever way the page asks for.
#[test]
fn every_order_is_a_permutation() {
    let widgets = annotiter_widgets();
    for order in [TabOrder::Row, TabOrder::Column, TabOrder::Structure] {
        let ring = FocusRing::build(&widgets, order);
        let mut seen = indices(&ring);
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3], "{order:?} is not a permutation");
        assert!(!ring.degenerate, "{order:?} should not degenerate");
    }
}
