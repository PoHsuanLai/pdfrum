//! A live form-filling session: events in, appearance updates out.

use pdfrum_form::session::FormSession as Inner;
use pdfrum_form::{Button, Event, Key, Modifiers, Point, Response, SessionConfig};

use crate::Document;

pub use pdfrum_form::event::{Button as MouseButton, Key as VirtualKey};
pub use pdfrum_form::update::{AppearanceUpdate, UpdateKind};
pub use pdfrum_form::{Modifiers as EventModifiers, Response as EventResponse};

/// A live form-filling session over a document (ISO 32000-1 §12.7).
///
/// Events go in and appearance updates come out. Nothing is pushed to a
/// callback and nothing is drawn: each method **returns** what changed, and
/// the caller decides what to re-render.
///
/// # Coordinates
///
/// Every mouse method takes **page space** — PDF user space, y-up, with its
/// origin at the page's crop box. That is the same space
/// [`Page`](crate::Page) reports rectangles in, and no conversion happens on
/// the way in.
///
/// # Two answers, not one
///
/// Each event method returns a [`Response`] with both halves of the answer.
/// `consumed` says the event was handled, which is **not** the same as saying
/// it changed anything — a read-only checkbox consumes a Return and stays
/// unchecked — and `updates` says what changed.
///
/// # A focused field draws differently
///
/// A field with focus renders from live editor state, caret and selection
/// included; an unfocused one falls back to a generated appearance stream.
/// Dropping focus with [`FormSession::force_kill_focus`] is what commits a
/// value and moves a field from the first to the second.
///
/// ```
/// use pdfrum::{Document, FormSession, EventModifiers};
///
/// let doc = Document::open("tests/fixtures/text_form.pdf")?;
/// let mut session = FormSession::new(&doc);
///
/// // Nothing has the keyboard yet.
/// assert!(session.focused_annot().is_none());
///
/// // A click is three events, and the move is not decoration: it is what
/// // tells the widget the pointer is over it.
/// let at = (120.0, 120.0);
/// session.on_mouse_move(0, at.0, at.1, EventModifiers::NONE);
/// session.on_mouse_down(0, at.0, at.1, EventModifiers::NONE);
/// let response = session.on_mouse_up(0, at.0, at.1, EventModifiers::NONE);
/// let _ = response.consumed;
///
/// // Typing sends characters. Navigation and shortcuts would go through
/// // `on_key_down` instead — the two paths never overlap.
/// for ch in "ABC".chars() {
///     session.on_char(ch, EventModifiers::NONE);
/// }
///
/// // Dropping focus commits the value and regenerates the appearance.
/// let updates = session.force_kill_focus();
/// let _ = updates.updates;
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct FormSession<'a> {
    #[allow(dead_code)]
    doc: &'a Document,
    inner: Inner,
}

impl<'a> FormSession<'a> {
    /// Starts a session with the **non-Apple** defaults: Control accelerates,
    /// and Control with `Y` redoes.
    ///
    /// Deliberately not a compile-time platform choice — the accelerator is
    /// configuration so that both behaviours are reachable and testable from
    /// one machine. Pass [`SessionConfig::apple`] for Apple keyboards.
    #[must_use]
    pub fn new(doc: &'a Document) -> FormSession<'a> {
        FormSession {
            doc,
            inner: Inner::new(),
        }
    }

    /// Starts a session with explicit switches — the accelerator modifier,
    /// which annotation subtypes join the focus ring, and the undo bound.
    #[must_use]
    pub fn with_config(doc: &'a Document, config: SessionConfig) -> FormSession<'a> {
        FormSession {
            doc,
            inner: Inner::with_config(config),
        }
    }

    /// The session's switches.
    #[must_use]
    pub fn config(&self) -> &SessionConfig {
        &self.inner.config
    }

    /// The pointer moved. Drives hover and extends a live drag.
    pub fn on_mouse_move(&mut self, page: u32, x: f32, y: f32, modifiers: Modifiers) -> Response {
        self.dispatch(
            page,
            Event::MouseMove {
                at: Point::new(x, y),
                modifiers,
            },
        )
    }

    /// The primary button went down.
    pub fn on_mouse_down(&mut self, page: u32, x: f32, y: f32, modifiers: Modifiers) -> Response {
        self.dispatch(
            page,
            Event::MouseDown {
                button: Button::Left,
                at: Point::new(x, y),
                modifiers,
            },
        )
    }

    /// The primary button came up.
    pub fn on_mouse_up(&mut self, page: u32, x: f32, y: f32, modifiers: Modifiers) -> Response {
        self.dispatch(
            page,
            Event::MouseUp {
                button: Button::Left,
                at: Point::new(x, y),
                modifiers,
            },
        )
    }

    /// A button other than the primary one moved.
    ///
    /// Present because event scripts contain right-button lines and a bridge
    /// must be able to express them. The correct behaviour for those lines is
    /// to consume nothing, and that is what this does.
    pub fn on_button(
        &mut self,
        page: u32,
        button: Button,
        down: bool,
        x: f32,
        y: f32,
        modifiers: Modifiers,
    ) -> Response {
        let at = Point::new(x, y);
        let event = if down {
            Event::MouseDown {
                button,
                at,
                modifiers,
            }
        } else {
            Event::MouseUp {
                button,
                at,
                modifiers,
            }
        };
        self.dispatch(page, event)
    }

    /// A double click. Selects the whole line under the pointer.
    pub fn on_double_click(&mut self, page: u32, x: f32, y: f32, modifiers: Modifiers) -> Response {
        self.dispatch(
            page,
            Event::DoubleClick {
                at: Point::new(x, y),
                modifiers,
            },
        )
    }

    /// The wheel turned. Deltas are notches, a negative `y` meaning down.
    pub fn on_mouse_wheel(
        &mut self,
        page: u32,
        x: f32,
        y: f32,
        delta_x: i32,
        delta_y: i32,
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(
            page,
            Event::MouseWheel {
                at: Point::new(x, y),
                delta: (delta_x, delta_y),
                modifiers,
            },
        )
    }

    /// Focus was requested at a point, without a click.
    ///
    /// Consumes the event only when an annotation is there *and* it took
    /// focus.
    pub fn on_focus_at(&mut self, page: u32, x: f32, y: f32, modifiers: Modifiers) -> Response {
        self.dispatch(
            page,
            Event::Focus {
                at: Point::new(x, y),
                modifiers,
            },
        )
    }

    /// A key went down.
    ///
    /// Navigation and shortcuts arrive here and text does not. There is no
    /// matching key-up method: the oracle's is documented as permanently
    /// unimplemented and always answers false, so modelling it would only
    /// invite callers to send a dead event.
    pub fn on_key_down(&mut self, key: Key, modifiers: Modifiers) -> Response {
        self.dispatch_keyboard(Event::KeyDown { key, modifiers })
    }

    /// A character was typed.
    ///
    /// Text arrives here and shortcuts do not. A character carrying the
    /// accelerator modifier is deliberately neither: it is refused, so an
    /// embedder's own handling sees it.
    pub fn on_char(&mut self, ch: char, modifiers: Modifiers) -> Response {
        self.dispatch_keyboard(Event::Char { ch, modifiers })
    }

    /// Drops focus, committing the field that held it.
    ///
    /// This is what moves a field from drawing its live editor state to
    /// drawing a generated appearance stream.
    pub fn force_kill_focus(&mut self) -> Response {
        let change = pdfrum_form::focus::kill(&mut self.inner);
        let Some(was) = change.from.map(pdfrum_form::session::FocusTarget::annot) else {
            // Nothing held focus, so nothing moved.
            return Response::ignored();
        };
        Response::with(vec![pdfrum_form::update::AppearanceUpdate::new(
            was,
            UpdateKind::FocusChanged {
                from: Some(was),
                to: None,
            },
        )])
    }

    /// Which annotation currently has the keyboard, if any.
    #[must_use]
    pub fn focused_annot(&self) -> Option<pdfrum_form::session::AnnotId> {
        self.inner
            .focus
            .map(pdfrum_form::session::FocusTarget::annot)
    }

    /// Whether the focused field can undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        match self.inner.focused_state() {
            Some(pdfrum_form::field::FieldState::Text(text)) => text.undo.can_undo(),
            Some(
                pdfrum_form::field::FieldState::Choice(_)
                | pdfrum_form::field::FieldState::Toggle(_)
                | pdfrum_form::field::FieldState::Button(_),
            )
            | None => false,
        }
    }

    /// Whether the focused field can redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        match self.inner.focused_state() {
            Some(pdfrum_form::field::FieldState::Text(text)) => text.undo.can_redo(),
            Some(
                pdfrum_form::field::FieldState::Choice(_)
                | pdfrum_form::field::FieldState::Toggle(_)
                | pdfrum_form::field::FieldState::Button(_),
            )
            | None => false,
        }
    }

    /// The focused field's text, or `None` when no field has the keyboard.
    ///
    /// A focused but empty field answers `Some("")`, which is a distinction
    /// the oracle's byte-length return cannot express.
    #[must_use]
    pub fn focused_text(&self) -> Option<String> {
        match self.inner.focused_state()? {
            pdfrum_form::field::FieldState::Text(text) => Some(text.text.clone()),
            pdfrum_form::field::FieldState::Choice(choice) => Some(choice.focused_text()),
            // Neither holds text: a toggle's value is a state name and a
            // button has none at all.
            pdfrum_form::field::FieldState::Toggle(_)
            | pdfrum_form::field::FieldState::Button(_) => None,
        }
    }

    /// Whether a row of the focused choice field is selected.
    #[must_use]
    pub fn is_index_selected(&self, index: usize) -> bool {
        match self.inner.focused_state() {
            Some(pdfrum_form::field::FieldState::Choice(choice)) => {
                pdfrum_form::field::choice::is_index_selected(choice, index)
            }
            Some(
                pdfrum_form::field::FieldState::Text(_)
                | pdfrum_form::field::FieldState::Toggle(_)
                | pdfrum_form::field::FieldState::Button(_),
            )
            | None => false,
        }
    }

    /// Selects or clears a row of the focused choice field.
    ///
    /// Answers whether the call was accepted, which is not the same as
    /// whether anything changed: a list box accepts a redundant clear and
    /// still moves its caret, while a combo box refuses every clear.
    pub fn set_index_selected(&mut self, index: usize, selected: bool) -> bool {
        match self.inner.focused_state_mut() {
            Some(pdfrum_form::field::FieldState::Choice(choice)) => {
                pdfrum_form::field::choice::set_index_selected(choice, index, selected)
            }
            // A text field has no rows, so every index is out of range.
            Some(
                pdfrum_form::field::FieldState::Text(_)
                | pdfrum_form::field::FieldState::Toggle(_)
                | pdfrum_form::field::FieldState::Button(_),
            )
            | None => false,
        }
    }

    /// The session's own record, for callers that need to read more than
    /// these methods expose.
    #[must_use]
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// Routes an event that names a page.
    ///
    /// Mouse routing needs the per-page annotation geometry and the
    /// caret-placement queries the layout engine is still growing. Until both
    /// are in place every event reports itself unhandled, which is the honest
    /// answer: a bridge can rely on "not consumed" not to change meaning when
    /// the routing lands, whereas a guess would.
    fn dispatch(&mut self, page: u32, event: Event) -> Response {
        let _ = (page, event, &mut self.inner);
        Response::ignored()
    }

    /// Routes an event that goes to whatever holds focus.
    ///
    /// Unhandled for the same reason as [`FormSession::dispatch`].
    fn dispatch_keyboard(&mut self, event: Event) -> Response {
        let _ = (event, &mut self.inner);
        Response::ignored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture, or a failed test. Returning an `Option` that every caller
    /// silently skipped on turned a missing fixture into four green-and-empty
    /// tests rather than four red ones.
    fn document() -> Document {
        match Document::open("tests/fixtures/text_form.pdf") {
            Ok(doc) => doc,
            Err(error) => panic!("the text_form fixture must open: {error}"),
        }
    }

    #[test]
    fn a_fresh_session_has_no_focus() {
        let doc = document();
        let session = FormSession::new(&doc);
        assert!(session.focused_annot().is_none());
        assert!(session.focused_text().is_none());
        assert!(!session.can_undo());
        assert!(!session.can_redo());
    }

    #[test]
    fn the_default_configuration_is_this_platforms() {
        let doc = document();
        let session = FormSession::new(&doc);
        assert_eq!(session.config().max_undo_items, 10_000);
        assert!(!session.config().focusable.is_empty());
    }

    /// Killing focus when nothing holds it is harmless.
    #[test]
    fn killing_focus_from_nothing_is_harmless() {
        let doc = document();
        let mut session = FormSession::new(&doc);
        let response = session.force_kill_focus();
        assert!(!response.consumed);
        assert!(response.updates.is_empty());
    }

    /// A query about a field that does not have focus answers falsely rather
    /// than panicking.
    #[test]
    fn queries_without_focus_answer_rather_than_panic() {
        let doc = document();
        let mut session = FormSession::new(&doc);
        assert!(!session.is_index_selected(0));
        assert!(!session.set_index_selected(0, true));
        assert!(!session.is_index_selected(usize::MAX));
    }
}
