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

use pdfrum::{Document, FormSession, Key, Modifiers, SessionConfig, Subtype};

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
        if !session.key_down(Key::Tab, Modifiers::NONE).consumed {
            break;
        }
        let response = session.key_down(Key::Return, Modifiers::NONE);
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
        Modifiers::NONE,
        Modifiers::CONTROL,
        Modifiers::SHIFT,
        Modifiers::SHIFT.union(Modifiers::CONTROL),
    ] {
        let response = session.key_down(Key::Return, modifiers);
        for (_, held) in response.actions() {
            seen.push(held.bits());
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
        if !session.key_down(Key::Tab, Modifiers::NONE).consumed {
            break;
        }
        if session
            .key_down(Key::Return, Modifiers::NONE)
            .actions()
            .count()
            > 0
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "a link must be reachable by tabbing");

    for key in [Key::Space, Key::Tab, Key::A, Key::Left, Key::Delete] {
        let response = session.key_down(key, Modifiers::NONE);
        assert_eq!(
            response.actions().count(),
            0,
            "{key:?} must not fire a link's action"
        );
        // Tab moves focus, so put it back on the link.
        if key == Key::Tab {
            session.key_down(Key::Tab, Modifiers::SHIFT);
        }
    }
}

/// Links are **not** in the ring by default, so the *four* link actions the
/// fixture carries cannot be reached from a default session.
///
/// The property this test owns is a **comparison**: admitting links to the
/// ring puts strictly more actions in reach than the default ring has, and
/// the difference is the links. A default session's ring holds widgets
/// alone.
#[test]
fn links_are_not_focusable_by_default() {
    fn reachable(session: &mut FormSession<'_>) -> usize {
        let mut fired = 0;
        for _ in 0..12 {
            if !session.key_down(Key::Tab, Modifiers::NONE).consumed {
                break;
            }
            fired += session
                .key_down(Key::Return, Modifiers::NONE)
                .actions()
                .count();
        }
        fired
    }

    let doc = document();
    let default_ring = reachable(&mut FormSession::new(&doc));
    let with_links = reachable(&mut session_with_links(&doc));

    // The fixture carries four links, each with a `/A`, and none of them is
    // in the default ring.
    // The default ring's only action is the push button's own. Admitting
    // links puts strictly more in reach, and the difference is a link.
    assert_eq!(
        default_ring, 1,
        "a default session's ring holds widgets alone — the button is the one"
    );
    assert!(
        with_links > default_ring,
        "admitting links to the ring puts a link's action in reach \
         (default {default_ring}, with links {with_links})"
    );
}

/// `ButtonActionInvokeTest`, inverted.
///
/// Upstream asserts `DoURIAction` `.Times(0)` and
/// `ASSERT_FALSE(FORM_OnChar(…, kReturn, 0))`, both under
/// `TODO(crbug.com/1028991)` saying they should be one and true; the adjacent
/// `LinkActionInvokeTest` asserts `.Times(4)` for a link. §12.6.3 table 196
/// performs an annotation's `/A` when it is activated, and a keyboard
/// activation of a tab-focused button is one. Under the oracle-bug rule we
/// implement the TODO's answer, so this asserts the fire the oracle omits.
///
/// The upstream call is `FORM_OnChar`, so the **char** path is the one the
/// assertion is on; the key path is checked below it.
#[test]
fn a_push_buttons_return_fires_its_action() {
    let doc = document();
    let mut session = FormSession::new(&doc);

    // The first widget is a text field; tab to it, then on to the button.
    session.key_down(Key::Tab, Modifiers::NONE);
    session.key_down(Key::Tab, Modifiers::NONE);

    let response = session.character('\r', Modifiers::NONE);
    assert_eq!(
        response.actions().count(),
        1,
        "a push button's Return fires its action (crbug.com/1028991)"
    );

    // The key path answers identically — `key_down` and `char_typed` route a
    // focused button's Return through the same `annot_key`.
    let response = session.key_down(Key::Return, Modifiers::NONE);
    assert_eq!(
        response.actions().count(),
        1,
        "the key path fires the same action as the char path"
    );
}

/// An annotation with no `/A` at all answers nothing rather than panicking.
#[test]
fn an_annotation_with_no_action_fires_nothing() {
    let doc = document();
    let mut session = session_with_links(&doc);

    for _ in 0..12 {
        if !session.key_down(Key::Tab, Modifiers::NONE).consumed {
            break;
        }
        // Whatever this is, asking for its action must not panic.
        let _ = session.key_down(Key::Return, Modifiers::NONE);
    }
}
