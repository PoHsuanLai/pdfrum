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

use pdfrum::{Document, EventModifiers, FormSession, VirtualKey};

fn document() -> Document {
    Document::open("tests/fixtures/annotiter.pdf").expect("the annotiter fixture must open")
}

/// Sends a Tab, with modifiers.
fn tab(session: &mut FormSession<'_>, modifiers: EventModifiers) -> bool {
    session.on_key_down(VirtualKey::TAB, modifiers).consumed
}

/// The raw `/Annots` index currently focused, if any.
fn focused(session: &FormSession<'_>) -> Option<u32> {
    session.focused_annot().map(|annot| annot.index)
}

/// `FormFillFirstTab`: a Tab with nothing focused takes the **first**
/// annotation of the ring.
#[test]
fn a_first_tab_lands_on_the_rings_first_entry() {
    let doc = document();
    let mut session = FormSession::new(&doc);
    assert!(session.focused_annot().is_none());

    assert!(tab(&mut session, EventModifiers::NONE));
    assert!(
        focused(&session).is_some(),
        "a Tab from nothing must land somewhere"
    );
}

/// `FormFillFirstShiftTab`: a Shift+Tab with nothing focused lands on a
/// **different** annotation from a plain Tab — the cursor starts between the
/// ends rather than before them.
#[test]
fn a_first_shift_tab_lands_somewhere_else_than_a_first_tab() {
    let doc = document();

    let forward = {
        let mut session = FormSession::new(&doc);
        tab(&mut session, EventModifiers::NONE);
        focused(&session)
    };
    let backward = {
        let mut session = FormSession::new(&doc);
        tab(&mut session, EventModifiers::SHIFT);
        focused(&session)
    };

    assert!(forward.is_some() && backward.is_some());
    assert_ne!(
        forward, backward,
        "forward and backward Tab from nothing are the ring's two ends"
    );
}

/// `FormFillContinuousTab`: four Tabs visit four distinct annotations, and
/// the **fifth is refused** — the ring does not wrap.
#[test]
fn four_tabs_visit_four_annotations_and_the_fifth_is_refused() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    let mut seen = Vec::new();
    for _ in 0..4 {
        assert!(tab(&mut session, EventModifiers::NONE));
        seen.push(focused(&session).expect("each Tab lands somewhere"));
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 4, "four Tabs visit four distinct annotations");

    assert!(
        !tab(&mut session, EventModifiers::NONE),
        "the ring does not wrap: the fifth Tab is unhandled"
    );
}

/// The same backwards.
#[test]
fn four_shift_tabs_visit_four_annotations_and_the_fifth_is_refused() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    let mut seen = Vec::new();
    for _ in 0..4 {
        assert!(tab(&mut session, EventModifiers::SHIFT));
        seen.push(focused(&session).expect("each Tab lands somewhere"));
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 4);

    assert!(!tab(&mut session, EventModifiers::SHIFT));
}

/// `TabWithModifiers`: every modifier but shift refuses the gesture — all six
/// combinations, which is the whole of the upstream test.
#[test]
fn tab_with_any_modifier_but_shift_is_refused() {
    let doc = document();
    for modifiers in [
        EventModifiers::CONTROL,
        EventModifiers::ALT,
        EventModifiers::META,
        EventModifiers::CONTROL.union(EventModifiers::SHIFT),
        EventModifiers::ALT.union(EventModifiers::SHIFT),
        EventModifiers::META.union(EventModifiers::SHIFT),
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
    session.on_mouse_move(0, 210.0, 210.0, EventModifiers::NONE);
    session.on_mouse_down(0, 210.0, 210.0, EventModifiers::NONE);
    session.on_mouse_up(0, 210.0, 210.0, EventModifiers::NONE);
    let first = session
        .focused_annot()
        .expect("page 0's widget takes focus");
    assert_eq!(first.page, 0);

    session.on_mouse_move(1, 411.0, 411.0, EventModifiers::NONE);
    session.on_mouse_down(1, 411.0, 411.0, EventModifiers::NONE);
    session.on_mouse_up(1, 411.0, 411.0, EventModifiers::NONE);
    let second = session.focused_annot().expect("page 1's widget takes it");

    assert_eq!(second.page, 1, "focus moved to the other page");
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
        VirtualKey::NEWLINE,
        VirtualKey::RETURN,
        VirtualKey::SPACE,
        VirtualKey::DELETE,
        VirtualKey(0x30), // '0'
        VirtualKey(0x39), // '9'
        VirtualKey::A,
        VirtualKey(0x5A), // 'Z'
        VirtualKey(0x70), // F1
    ] {
        let mut session = FormSession::new(&doc);
        let response = session.on_key_down(key, EventModifiers::NONE);
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
            .on_mouse_move(0, 0.0, 0.0, EventModifiers::NONE)
            .consumed
    );
    assert!(
        !session
            .on_mouse_move(0, -1000.0, -1000.0, EventModifiers::NONE)
            .consumed
    );
    assert!(
        !session
            .on_mouse_move(0, 1e9, 1e9, EventModifiers::NONE)
            .consumed
    );
    assert!(session.focused_annot().is_none());
}
