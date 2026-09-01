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
/// use pdfrum::{Document, FormSession, EventModifiers, VirtualKey};
///
/// let doc = Document::open("tests/fixtures/text_form.pdf")?;
/// let mut session = FormSession::new(&doc);
///
/// // Nothing has the keyboard yet.
/// assert!(session.focused_annot().is_none());
/// assert!(session.focused_text().is_none());
///
/// // A click is three events, and the move is not decoration: it is what
/// // tells the widget the pointer is over it. The fixture's one text field
/// // is `/Rect [100 100 200 130]`, so (120, 115) lands inside it.
/// let at = (120.0, 115.0);
/// session.on_mouse_move(0, at.0, at.1, EventModifiers::NONE);
/// session.on_mouse_down(0, at.0, at.1, EventModifiers::NONE);
/// session.on_mouse_up(0, at.0, at.1, EventModifiers::NONE);
/// assert!(session.focused_annot().is_some());
///
/// // Typing sends characters. Navigation and shortcuts go through
/// // `on_key_down` instead — the two paths never overlap, and a character
/// // carrying the accelerator is neither.
/// for ch in "Hello".chars() {
///     session.on_char(ch, EventModifiers::NONE);
/// }
/// assert_eq!(session.focused_text().as_deref(), Some("Hello"));
///
/// // Undo is one keystroke and one character: typing records an item per
/// // character, so this leaves "Hell".
/// assert!(session.can_undo());
/// session.on_key_down(VirtualKey::Z, EventModifiers::CONTROL);
/// assert_eq!(session.focused_text().as_deref(), Some("Hell"));
///
/// // Dropping focus commits the value and moves the field from drawing its
/// // live editor state to drawing a generated appearance stream. What comes
/// // back is what changed, for a caller to re-render.
/// let committed = session.force_kill_focus();
/// assert!(session.focused_annot().is_none());
/// assert!(!committed.updates.is_empty());
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct FormSession<'a> {
    doc: &'a Document,
    inner: Inner,
    /// The pages this session has already read, by index.
    ///
    /// Read on the first event that names a page and kept, because a replay
    /// sends dozens of events at one page and the walk is the expensive half.
    /// A page's annotation geometry does not change under a session — the
    /// session changes *appearances*, which are produced on the way out and
    /// never written back here.
    pages: std::collections::BTreeMap<u32, pdfrum_form::PageForm>,
    /// The form's default-resource fonts, loaded once.
    fonts: pdfrum_doc::ap::FormFonts,
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
        FormSession::build(doc, Inner::new())
    }

    /// Starts a session with explicit switches — the accelerator modifier,
    /// which annotation subtypes join the focus ring, and the undo bound.
    #[must_use]
    pub fn with_config(doc: &'a Document, config: SessionConfig) -> FormSession<'a> {
        FormSession::build(doc, Inner::with_config(config))
    }

    /// The shared constructor: a session plus the fonts its appearances are
    /// laid out with.
    fn build(doc: &'a Document, inner: Inner) -> FormSession<'a> {
        let mut ctx = pdfrum_page::BuildContext::new();
        let fonts = pdfrum_doc::ap::FormFonts::load(&doc.catalog(), doc.parser(), &mut ctx);
        FormSession {
            doc,
            inner,
            pages: std::collections::BTreeMap::new(),
            fonts,
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

    /// Which annotation on `page` holds focus, and what its focus rectangle
    /// is — the answer the annotation pass needs to know **not** to tint it.
    ///
    /// A widget the form filler is editing is never given the form-field
    /// highlight, and most field types stroke nothing in its place: a text
    /// field and an editable combo box draw a caret and glyphs over plain
    /// white. Answers `None` when nothing on that page holds focus, which is
    /// the ordinary case for every page but one.
    #[must_use]
    pub fn focus_for_page(&mut self, page: u32) -> Option<pdfrum_doc::ap::Focus> {
        if !self.pages.contains_key(&page) {
            let read = self.read_page(page)?;
            self.pages.insert(page, read);
        }
        let form = self.pages.get(&page)?;
        let catalog = self.doc.catalog();
        let ctx = pdfrum_form::Context {
            page: form,
            catalog: &catalog,
            resolve: self.doc.parser(),
            fonts: &self.fonts,
            permissions: self.permissions(),
        };
        pdfrum_form::focus_of(&self.inner, &ctx)
    }

    /// Whether the focused field can undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        match self.inner.focused_state() {
            Some(pdfrum_form::field::FieldState::Text(text)) => text.edit.undo.can_undo(),
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
            Some(pdfrum_form::field::FieldState::Text(text)) => text.edit.undo.can_redo(),
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
            pdfrum_form::field::FieldState::Text(text) => Some(text.text().to_string()),
            pdfrum_form::field::FieldState::Choice(choice) => Some(choice.focused_text()),
            // Neither holds text: a toggle's value is a state name and a
            // button has none at all.
            pdfrum_form::field::FieldState::Toggle(_)
            | pdfrum_form::field::FieldState::Button(_) => None,
        }
    }

    /// The text currently selected in the focused field, if any.
    ///
    /// A focused field with an empty selection answers `Some("")`, and no
    /// focus at all answers `None` — a distinction the oracle's byte-length
    /// return cannot make, since it reports zero for both.
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        match self.inner.focused_state()? {
            pdfrum_form::field::FieldState::Text(text) => Some(text.edit.selected_text()),
            pdfrum_form::field::FieldState::Choice(choice) => Some(
                choice
                    .edit
                    .as_ref()
                    .map(|edit| edit.selected_text())
                    .unwrap_or_default(),
            ),
            // Neither holds selectable text.
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
    /// The page is read on first use and kept: a replay sends dozens of
    /// events at one page, and the `/Annots` walk is the expensive half.
    fn dispatch(&mut self, page: u32, event: Event) -> Response {
        if !self.pages.contains_key(&page) {
            let Some(read) = self.read_page(page) else {
                return Response::ignored();
            };
            self.pages.insert(page, read);
        }
        let Some(form) = self.pages.get(&page) else {
            return Response::ignored();
        };
        let catalog = self.doc.catalog();
        let ctx = pdfrum_form::Context {
            page: form,
            catalog: &catalog,
            resolve: self.doc.parser(),
            fonts: &self.fonts,
            permissions: self.permissions(),
        };
        pdfrum_form::apply(&mut self.inner, &ctx, event)
    }

    /// Routes an event that goes to whatever holds focus.
    ///
    /// Keyboard events name no page, so the page they route against is the
    /// one holding focus — which is why typing works after a click and does
    /// nothing before one.
    fn dispatch_keyboard(&mut self, event: Event) -> Response {
        let Some(page) = self
            .inner
            .focus
            .map(|target| pdfrum_form::session::FocusTarget::annot(target).page)
        else {
            return Response::ignored();
        };
        self.dispatch(page, event)
    }

    /// Reads one page's annotations, or `None` when the page will not load.
    fn read_page(&self, page: u32) -> Option<pdfrum_form::PageForm> {
        let loaded = self.doc.page(page).ok()?;
        Some(pdfrum_form::page::read(
            page,
            &loaded.dict.dict,
            &self.doc.catalog(),
            self.doc.parser(),
        ))
    }

    /// What the document permits, which gates every non-push-button click.
    fn permissions(&self) -> pdfrum_form::Permissions {
        if !self.doc.is_encrypted() {
            return pdfrum_form::Permissions::ALL;
        }
        let bits = self.doc.permissions(false);
        pdfrum_form::Permissions {
            // Bit 9 (value 256) is fill-in; bit 6 (value 32) is annotation
            // modification. Either one suffices.
            fill_form: bits & 0x100 != 0,
            modify_annotation: bits & 0x20 != 0,
        }
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
