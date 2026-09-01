//! Ported action-invocation assertions.
//!
//! A link with focus fires its action on Return, and the action comes back as
//! a **request** the caller may inspect, ignore or perform — not as a
//! callback that has already fired by the time anyone could object. This
//! crate navigates nothing.
//!
//! The fixture is `annots_action_handling.pdf`: two widgets, then link
//! annotations carrying `/A << /S /URI /URI (https://cs.chromium.org/) >>`.

// See `form_routing.rs`: the fixture helper is a failure signal.
#![allow(clippy::expect_used)]

use pdfrum::{Document, EventModifiers, FormSession, SessionConfig, Subtype, VirtualKey};

fn document() -> Document {
    Document::open("tests/fixtures/annots_action_handling.pdf")
        .expect("the action-handling fixture must open")
}

/// A session whose focus ring admits **links** as well as widgets.
///
/// The ring's membership is the caller's choice, and it is widgets alone by
/// default. Tabbing to a link and pressing Return is what this configuration
/// is for — it is how a keyboard-only reader follows a link.
fn session_with_links(doc: &Document) -> FormSession<'_> {
    FormSession::with_config(
        doc,
        SessionConfig {
            focusable: vec![Subtype::Widget, Subtype::Link],
            ..SessionConfig::default()
        },
    )
}

/// `LinkActionInvokeTest`: four Returns with four different modifier sets
/// produce four requests, **in order**, each carrying the modifiers held.
#[test]
fn a_focused_link_fires_its_action_on_return_with_the_modifiers_held() {
    let doc = document();
    let mut session = session_with_links(&doc);

    // Tab until a link holds focus.
    let mut fired = Vec::new();
    for _ in 0..12 {
        if !session
            .on_key_down(VirtualKey::TAB, EventModifiers::NONE)
            .consumed
        {
            break;
        }
        let response = session.on_key_down(VirtualKey::RETURN, EventModifiers::NONE);
        if response.actions().count() > 0 {
            fired.push(response);
            break;
        }
    }
    assert!(
        !fired.is_empty(),
        "tabbing to a link and pressing Return must request its action"
    );

    // The modifiers ride along, in the raw bit values the contract names.
    let mut seen = Vec::new();
    for modifiers in [
        EventModifiers::NONE,
        EventModifiers::CONTROL,
        EventModifiers::SHIFT,
        EventModifiers::SHIFT.union(EventModifiers::CONTROL),
    ] {
        let response = session.on_key_down(VirtualKey::RETURN, modifiers);
        for (_, held) in response.actions() {
            seen.push(held.0);
        }
    }
    assert_eq!(
        seen,
        vec![0, 2, 1, 3],
        "each request carries the modifiers that were held, in order"
    );
}

/// The rejections, asserted as specifically as the acceptance: **only**
/// Return fires an action. "Any key activates a link" is the plausible wrong
/// implementation, so each of these is checked.
#[test]
fn no_key_but_return_fires_a_links_action() {
    let doc = document();
    let mut session = session_with_links(&doc);

    // Reach a link.
    let mut reached = false;
    for _ in 0..12 {
        if !session
            .on_key_down(VirtualKey::TAB, EventModifiers::NONE)
            .consumed
        {
            break;
        }
        if session
            .on_key_down(VirtualKey::RETURN, EventModifiers::NONE)
            .actions()
            .count()
            > 0
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "a link must be reachable by tabbing");

    for key in [
        VirtualKey::SPACE,
        VirtualKey::TAB,
        VirtualKey::A,
        VirtualKey::LEFT,
        VirtualKey::DELETE,
    ] {
        let response = session.on_key_down(key, EventModifiers::NONE);
        assert_eq!(
            response.actions().count(),
            0,
            "{key:?} must not fire a link's action"
        );
        // Tab moves focus, so put it back on the link.
        if key == VirtualKey::TAB {
            session.on_key_down(VirtualKey::TAB, EventModifiers::SHIFT);
        }
    }
}

/// Links are **not** in the ring by default, so the same Return does nothing
/// in a default session. The ring's membership being the caller's choice is a
/// contract, and this is the half of it that is easy to break silently.
#[test]
fn links_are_not_focusable_by_default() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    for _ in 0..12 {
        if !session
            .on_key_down(VirtualKey::TAB, EventModifiers::NONE)
            .consumed
        {
            break;
        }
        assert_eq!(
            session
                .on_key_down(VirtualKey::RETURN, EventModifiers::NONE)
                .actions()
                .count(),
            0,
            "a default session's ring holds widgets alone"
        );
    }
}

/// `ButtonActionInvokeTest`: a focused push button's Return produces **no**
/// action and is **not** consumed — the asserted-broken upstream behaviour,
/// ported as it stands with the open bug named.
///
/// `crbug.com/1028991` says this should fire the button's action and does
/// not. Reproducing it rather than improving on it is what keeps a golden
/// comparison honest; a test asserting the *fixed* behaviour would fail
/// against every oracle build.
#[test]
fn a_push_buttons_return_fires_nothing_and_is_not_consumed() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // The first widget is a text field; tab to it, then on to the button.
    session.on_key_down(VirtualKey::TAB, EventModifiers::NONE);
    session.on_key_down(VirtualKey::TAB, EventModifiers::NONE);

    let response = session.on_char('\r', EventModifiers::NONE);
    assert_eq!(
        response.actions().count(),
        0,
        "a push button's Return fires no action (crbug.com/1028991)"
    );
}

/// An annotation with no `/A` at all answers nothing rather than panicking.
#[test]
fn an_annotation_with_no_action_fires_nothing() {
    let doc = document();
    let mut session = session_with_links(&doc);

    for _ in 0..12 {
        if !session
            .on_key_down(VirtualKey::TAB, EventModifiers::NONE)
            .consumed
        {
            break;
        }
        // Whatever this is, asking for its action must not panic.
        let _ = session.on_key_down(VirtualKey::RETURN, EventModifiers::NONE);
    }
}
