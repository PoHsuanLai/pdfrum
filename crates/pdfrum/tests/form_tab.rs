//! Ported focus and tab-traversal assertions, driven end to end.
//!
//! `tab_order.rs` in `pdfrum-form` pins the *ordering* against transcribed
//! geometry; these pin the **routing** — that a Tab key reaches the ring at
//! all, that focus belongs to the document rather than to a page, and that
//! the modifier rules refuse what they should.
//!
//! The fixture is `annotiter.pdf`: three pages sharing one field's four
//! widget kids, at the corners of a square, in an annotation order that is
//! none of the three traversal orders.
//!
//! ```text
//!   (201,400) Sub_LeftTop  #2      #1 Sub_RightTop  (401,401)
//!
//!   (200,200) Sub_LeftBottom #0    #3 Sub_RightBottom (400,201)
//! ```

// See `form_routing.rs`: the fixture helper is a failure signal, not a case
// to handle.
#![allow(clippy::expect_used)]

use pdfrum::{Document, FormSession, Key, Modifiers, PageIndex, kurbo::Point};

fn document() -> Document {
    Document::open("tests/fixtures/annotiter.pdf").expect("the annotiter fixture must open")
}

/// Sends a Tab, with modifiers.
fn tab(session: &mut FormSession<'_>, modifiers: Modifiers) -> bool {
    session.key_down(Key::Tab, modifiers).consumed
}

/// The raw `/Annots` index currently focused, if any.
fn focused(session: &FormSession<'_>) -> Option<u32> {
    session.focused_annot().map(|annot| annot.index)
}

/// The walk one page produces, forward or backward, until the ring refuses.
fn walk(page: u32, modifiers: Modifiers) -> Vec<u32> {
    let doc = document();
    let mut session = FormSession::new(&doc);
    session.set_viewed_page(page);

    let mut visited = Vec::new();
    while tab(&mut session, modifiers) {
        visited.push(focused(&session).expect("a consumed Tab lands somewhere"));
    }
    visited
}

/// `FormFillFirstTab`: a Tab with nothing focused lands on annot **1**.
///
/// Not "somewhere": page 0 is `/Tabs /R`, and row order over this geometry is
/// `1, 2, 3, 0`. Structure order would answer 0, so this is the assertion
/// that tells a page's declared order from the default.
#[test]
fn a_first_tab_lands_on_annot_one() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    assert!(session.focused_annot().is_none());

    assert!(tab(&mut session, Modifiers::NONE));
    assert_eq!(focused(&session), Some(1));
}

/// `FormFillFirstShiftTab`: a Shift+Tab with nothing focused lands on annot
/// **0** — the ring's other end, since the cursor starts between them.
#[test]
fn a_first_shift_tab_lands_on_annot_zero() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    assert!(tab(&mut session, Modifiers::SHIFT));
    assert_eq!(focused(&session), Some(0));
}

/// `FormFillContinuousTab`: four Tabs visit **1, 2, 3, 0** and the fifth is
/// refused — the ring does not wrap.
#[test]
fn four_tabs_visit_one_two_three_zero_and_the_fifth_is_refused() {
    assert_eq!(walk(0, Modifiers::NONE), vec![1, 2, 3, 0]);
}

/// `FormFillContinuousShiftTab`: the same backwards, **0, 3, 2, 1**.
#[test]
fn four_shift_tabs_visit_zero_three_two_one_and_the_fifth_is_refused() {
    assert_eq!(walk(0, Modifiers::SHIFT), vec![0, 3, 2, 1]);
}

/// The three pages ask for the three orders and get three different answers
/// over the **same four annotations**.
///
/// This is the assertion the fixture exists for: `annotiter.pdf`'s pages
/// differ only in `/Tabs` (`/R`, `/C`, `/S`), so an implementation that
/// ignored the entry would return one answer three times. The three
/// expectations were confirmed against a verbatim compilation of
/// `CPDFSDK_AnnotIterator::GenerateResults`.
#[test]
fn each_pages_declared_order_produces_its_own_walk() {
    assert_eq!(walk(0, Modifiers::NONE), vec![1, 2, 3, 0], "/Tabs /R");
    assert_eq!(walk(1, Modifiers::NONE), vec![1, 3, 2, 0], "/Tabs /C");
    assert_eq!(walk(2, Modifiers::NONE), vec![0, 1, 2, 3], "/Tabs /S");
}

/// `TabWithModifiers`: every modifier but shift refuses the gesture — all six
/// combinations, which is the whole of the upstream test.
#[test]
fn tab_with_any_modifier_but_shift_is_refused() {
    let doc = document();
    for modifiers in [
        Modifiers::CONTROL,
        Modifiers::ALT,
        Modifiers::META,
        Modifiers::CONTROL.union(Modifiers::SHIFT),
        Modifiers::ALT.union(Modifiers::SHIFT),
        Modifiers::META.union(Modifiers::SHIFT),
    ] {
        let mut session = FormSession::new(&doc);
        assert!(
            !tab(&mut session, modifiers),
            "Tab with {modifiers:?} must be refused"
        );
        assert!(session.focused_annot().is_none(), "and must not move focus");
    }
}

/// Focus belongs to the **document**, not to a page. This fixture is the only
/// one that can show it: all three pages list the same four annotations, so a
/// per-page focus would let two pages hold it at once.
#[test]
fn focus_is_document_wide_not_per_page() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    assert_eq!(doc.page_count(), 3);

    // Focus something on page 0, then click a widget on page 1.
    session.mouse_move(0, Point::new(210.0, 210.0), Modifiers::NONE);
    session.mouse_down(0, Point::new(210.0, 210.0), Modifiers::NONE);
    session.mouse_up(0, Point::new(210.0, 210.0), Modifiers::NONE);
    let first = session
        .focused_annot()
        .expect("page 0's widget takes focus");
    assert_eq!(first.page, PageIndex::new(0));

    session.mouse_move(1, Point::new(411.0, 411.0), Modifiers::NONE);
    session.mouse_down(1, Point::new(411.0, 411.0), Modifiers::NONE);
    session.mouse_up(1, Point::new(411.0, 411.0), Modifiers::NONE);
    let second = session.focused_annot().expect("page 1's widget takes it");

    assert_eq!(
        second.page,
        PageIndex::new(1),
        "focus moved to the other page"
    );
    assert_ne!(
        first, second,
        "one annotation holds the keyboard at a time, whichever page it is on"
    );
}

/// `KeyPressWithNoFocusedAnnot`: none of these keys is handled with nothing
/// focused, and none of them creates a focus.
#[test]
fn keys_with_no_focused_annotation_are_refused() {
    let doc = document();
    for key in [
        Key::Newline,
        Key::Return,
        Key::Space,
        Key::Delete,
        Key::from_virtual(0x30), // '0'
        Key::from_virtual(0x39), // '9'
        Key::A,
        Key::Z,                  // 'Z'
        Key::from_virtual(0x70), // F1
    ] {
        let mut session = FormSession::new(&doc);
        let response = session.key_down(key, Modifiers::NONE);
        assert!(!response.consumed, "{key:?} must not be handled");
        assert!(
            session.focused_annot().is_none(),
            "{key:?} must not create a focus"
        );
    }
}

/// `FirstTest`: opening a document and loading a page produces no update at
/// all — nothing happens until an event arrives.
#[test]
fn opening_a_document_produces_no_updates() {
    let doc = document();
    let session = FormSession::new(&doc);
    assert!(session.focused_annot().is_none());
    assert!(session.focused_text().is_none());
    assert!(!session.can_undo());
    assert!(!session.can_redo());
}

/// `Bug514690`: a mouse move with nothing under it is unhandled rather than a
/// crash, and a move far outside the page is equally harmless.
#[test]
fn a_mouse_move_over_nothing_is_harmless() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    assert!(
        !session
            .mouse_move(0, Point::new(0.0, 0.0), Modifiers::NONE)
            .consumed
    );
    assert!(
        !session
            .mouse_move(0, Point::new(-1000.0, -1000.0), Modifiers::NONE)
            .consumed
    );
    assert!(
        !session
            .mouse_move(0, Point::new(1e9, 1e9), Modifiers::NONE)
            .consumed
    );
    assert!(session.focused_annot().is_none());
}

/// A Tab with nothing focused enters the ring on the page the embedder says
/// it is showing — not always page 0.
///
/// This fixture is the only one that can show the difference: all three pages
/// carry the same four annotations, so the *page* of the focused annotation is
/// the only thing that moves.
#[test]
fn a_tab_from_nothing_enters_the_ring_on_the_viewed_page() {
    let doc = document();

    for page in 0..doc.page_count() {
        let mut session = FormSession::new(&doc);
        session.set_viewed_page(page);
        assert_eq!(session.viewed_page(), PageIndex::from(page));

        assert!(tab(&mut session, Modifiers::NONE));
        let landed = session.focused_annot().expect("the Tab lands somewhere");
        assert_eq!(
            landed.page,
            PageIndex::from(page),
            "a Tab from nothing must enter the ring on the viewed page"
        );
    }
}

/// The default is page 0, which is right for a single-page document and for a
/// viewer that has not scrolled.
#[test]
fn the_viewed_page_defaults_to_the_first() {
    let doc = document();
    let session = FormSession::new(&doc);
    assert_eq!(session.viewed_page(), PageIndex::FIRST);
}
