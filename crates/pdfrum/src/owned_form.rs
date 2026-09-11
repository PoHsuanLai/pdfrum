//! A form session that owns its document: the handle a caller without a
//! lifetime holds.

use std::sync::Arc;

use pdfrum_common::PageIndex;
use pdfrum_form::Event;

use crate::form_session::State;
use crate::{Document, FormSession, Modifiers, Response, SessionConfig};

/// A live form-filling session that holds its document rather than
/// borrowing it — [`FormSession`] for a caller who cannot carry a lifetime.
///
/// Built by [`FormSession::owned`], [`FormSession::owned_with_config`] and,
/// with the `javascript` feature, `FormSession::owned_with_scripts`. The
/// event and value surface of [`FormSession`] under the same names, each
/// method the borrowed one: for the length of a call the session's state is
/// moved into a `FormSession` over a borrow of the held document and taken
/// back after, so there is one implementation of every event and this type
/// adds a move of a few words to each call. Everything [`FormSession`]'s
/// documentation says — coordinates, the two halves of a [`Response`], what
/// focus changes — holds here unchanged.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum::{Document, FormSession, Modifiers, Point};
///
/// let session = {
///     let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
///     FormSession::owned(doc)
/// };
/// // The document was only ever held by the session, and it is still open.
/// let mut session = session;
/// let at = Point::new(120.0, 115.0);
/// session.mouse_move(0, at, Modifiers::NONE);
/// session.mouse_down(0, at, Modifiers::NONE);
/// session.mouse_up(0, at, Modifiers::NONE);
/// for ch in "Hello".chars() {
///     session.character(ch, Modifiers::NONE);
/// }
/// assert_eq!(session.focused_text().as_deref(), Some("Hello"));
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct OwnedFormSession {
    doc: Arc<Document>,
    state: State,
}

impl FormSession<'_> {
    /// [`FormSession::new`] over a document the session holds — see
    /// [`OwnedFormSession`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned(Arc::clone(&doc));
    /// assert!(session.focused_annot().is_none());
    /// assert!(Arc::ptr_eq(session.document(), &doc));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn owned(doc: Arc<Document>) -> OwnedFormSession {
        let state = FormSession::new(&doc).state;
        OwnedFormSession { doc, state }
    }

    /// [`FormSession::with_config`] over a document the session holds.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, SessionConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned_with_config(doc, SessionConfig::apple());
    /// assert_eq!(session.config().accelerator, Modifiers::META);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn owned_with_config(doc: Arc<Document>, config: SessionConfig) -> OwnedFormSession {
        let state = FormSession::with_config(&doc, config).state;
        OwnedFormSession { doc, state }
    }

    /// [`FormSession::with_scripts`] over a document the session holds: the
    /// document's own JavaScript running, with every field's `/AA` scripts
    /// installed.
    ///
    /// # Errors
    ///
    /// As [`FormSession::with_scripts`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    ///
    /// let at = Point::new(150.0, 175.0);
    /// session.mouse_move(0, at, Modifiers::NONE);
    /// session.mouse_down(0, at, Modifiers::NONE);
    /// session.mouse_up(0, at, Modifiers::NONE);
    /// session.character('7', Modifiers::NONE);
    ///
    /// let transcript = session.scripts().expect("a scripted session").transcript_text();
    /// assert!(transcript.contains("Alert: *** starting test 2 ***\n"));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn owned_with_scripts(
        doc: Arc<Document>,
        config: &pdfrum_form::ScriptConfig,
    ) -> Result<OwnedFormSession, crate::ScriptBuildError> {
        let state = FormSession::with_scripts(&doc, config)?.state;
        Ok(OwnedFormSession { doc, state })
    }
}

impl OwnedFormSession {
    /// Runs `body` against the borrowed session this one stands in for.
    ///
    /// The state moves into a [`FormSession`] over a borrow of the held
    /// document and moves back after; what stands in its slot meanwhile is
    /// [`State::vacant`], which no caller can observe since `body` holds the
    /// only `&mut self`.
    fn with<'s, T>(&'s mut self, body: impl FnOnce(&mut FormSession<'s>) -> T) -> T {
        let vacant = State::vacant(&self.state.fonts);
        let state = std::mem::replace(&mut self.state, vacant);
        // The document borrow is `'s` — the whole call — and the state slot
        // is written back beside it: two fields, two borrows.
        let doc: &'s Document = &self.doc;
        let mut session = FormSession { doc, state };
        let out = body(&mut session);
        self.state = session.state;
        out
    }

    /// The document this session is over.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned(Arc::clone(&doc));
    /// assert_eq!(session.document().page_count(), doc.page_count());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn document(&self) -> &Arc<Document> {
        &self.doc
    }

    /// Tells the session which page the embedder is showing — as
    /// [`FormSession::set_viewed_page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, PageIndex};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.set_viewed_page(3);
    /// assert_eq!(session.viewed_page(), PageIndex::from(3));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn set_viewed_page(&mut self, page: impl Into<PageIndex>) {
        self.state.viewed_page = page.into();
    }

    /// Which page the embedder last said it was showing — as
    /// [`FormSession::viewed_page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, PageIndex};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned(doc);
    /// assert_eq!(session.viewed_page(), PageIndex::FIRST);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn viewed_page(&self) -> PageIndex {
        self.state.viewed_page
    }

    /// The session's switches — as [`FormSession::config`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned(doc);
    /// assert!(session.config().redo_on_ctrl_y);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn config(&self) -> &SessionConfig {
        &self.state.inner.config
    }

    /// Routes a whole [`Event`] — as [`FormSession::apply`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, Event, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// let response = session.apply(Event::KeyDown { key: Key::Tab, modifiers: Modifiers::NONE });
    /// assert!(response.consumed);
    /// assert!(session.focused_annot().is_some());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn apply(&mut self, event: Event) -> Response {
        self.with(|session| session.apply(event))
    }

    /// The pointer moved — as [`FormSession::mouse_move`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.mouse_move(0, Point::new(120.0, 115.0), Modifiers::NONE);
    /// assert_eq!(session.hover_for_page(0), Some(0));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn mouse_move(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.with(|session| session.mouse_move(page, at, modifiers))
    }

    /// The primary button went down — as [`FormSession::mouse_down`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// let response = session.mouse_down(0, Point::new(120.0, 115.0), Modifiers::NONE);
    /// assert!(response.consumed);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn mouse_down(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.with(|session| session.mouse_down(page, at, modifiers))
    }

    /// The primary button came up — as [`FormSession::mouse_up`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// let at = Point::new(120.0, 115.0);
    /// session.mouse_down(0, at, Modifiers::NONE);
    /// session.mouse_up(0, at, Modifiers::NONE);
    /// assert!(session.focused_annot().is_some());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn mouse_up(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.with(|session| session.mouse_up(page, at, modifiers))
    }

    /// A double click — as [`FormSession::double_click`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// // Nothing on the page at (1, 1), so nothing consumes it.
    /// let response = session.double_click(0, Point::new(1.0, 1.0), Modifiers::NONE);
    /// assert!(!response.consumed);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn double_click(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.with(|session| session.double_click(page, at, modifiers))
    }

    /// The wheel turned — as [`FormSession::mouse_wheel`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// let response = session.mouse_wheel(0, Point::new(1.0, 1.0), (0, -1), Modifiers::NONE);
    /// assert!(!response.consumed);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn mouse_wheel(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        delta: (i32, i32),
        modifiers: Modifiers,
    ) -> Response {
        self.with(|session| session.mouse_wheel(page, at, delta, modifiers))
    }

    /// Focus was requested at a point, without a click — as
    /// [`FormSession::focus_at`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Modifiers, Point};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.focus_at(0, Point::new(120.0, 115.0), Modifiers::NONE);
    /// assert!(session.focused_annot().is_some());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn focus_at(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.with(|session| session.focus_at(page, at, modifiers))
    }

    /// A key went down — as [`FormSession::key_down`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// // Tab from nothing enters the focus ring on the viewed page.
    /// assert!(session.key_down(Key::Tab, Modifiers::NONE).consumed);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn key_down(&mut self, key: crate::Key, modifiers: Modifiers) -> Response {
        self.with(|session| session.key_down(key, modifiers))
    }

    /// A character was typed — as [`FormSession::character`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// session.character('x', Modifiers::NONE);
    /// assert_eq!(session.focused_text().as_deref(), Some("x"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn character(&mut self, ch: char, modifiers: Modifiers) -> Response {
        self.with(|session| session.character(ch, modifiers))
    }

    /// Drops focus, committing the field that held it — as
    /// [`FormSession::blur`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// session.character('x', Modifiers::NONE);
    /// let committed = session.blur();
    /// assert!(session.focused_annot().is_none());
    /// assert!(committed.updates.iter().any(|u| u.kind.appearance().is_some()));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn blur(&mut self) -> Response {
        self.with(FormSession::blur)
    }

    /// Which annotation currently has the keyboard — as
    /// [`FormSession::focused_annot`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned(doc);
    /// assert!(session.focused_annot().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn focused_annot(&self) -> Option<pdfrum_form::AnnotId> {
        self.state.focused_annot()
    }

    /// Which annotation on `page` holds focus, and its focus rectangle — as
    /// [`FormSession::focus_for_page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// assert!(session.focus_for_page(0).is_none());
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// assert!(session.focus_for_page(0).is_some());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn focus_for_page(&mut self, page: impl Into<PageIndex>) -> Option<pdfrum_doc::ap::Focus> {
        self.with(|session| session.focus_for_page(page))
    }

    /// The open combo-box dropdown on `page`, if one is open — as
    /// [`FormSession::popup_for_page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/combobox_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// assert!(session.popup_for_page(0).is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn popup_for_page(&mut self, page: impl Into<PageIndex>) -> Option<pdfrum_form::PopupView> {
        self.with(|session| session.popup_for_page(page))
    }

    /// How far one choice widget has scrolled — as
    /// [`FormSession::scroll_view`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{AnnotId, Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/combobox_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// // No event has reached the widget, so it has no state to report.
    /// assert!(session.scroll_view(AnnotId::new(0, 0)).is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn scroll_view(&mut self, annot: pdfrum_form::AnnotId) -> Option<pdfrum_form::ScrollView> {
        self.with(|session| session.scroll_view(annot))
    }

    /// List-box scroll bars on `page` — as [`FormSession::scrollbars_for_page`].
    #[must_use]
    pub fn scrollbars_for_page(
        &mut self,
        page: impl Into<pdfrum_common::PageIndex>,
    ) -> Vec<(pdfrum_form::AnnotId, pdfrum_form::ScrollView)> {
        self.with(|session| session.scrollbars_for_page(page))
    }

    /// Reports that the user picked row `index` of an open dropdown — as
    /// [`FormSession::choose`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{AnnotId, Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/combobox_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// // No list is open, so there is nothing to choose from.
    /// assert!(!session.choose(AnnotId::new(0, 0), 0).consumed);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn choose(&mut self, annot: pdfrum_form::AnnotId, index: usize) -> Response {
        self.with(|session| session.choose(annot, index))
    }

    /// Reports that an open dropdown was dismissed without a choice — as
    /// [`FormSession::close_popup`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{AnnotId, Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/combobox_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// assert!(!session.close_popup(AnnotId::new(0, 0)).consumed);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn close_popup(&mut self, annot: pdfrum_form::AnnotId) -> Response {
        self.with(|session| session.close_popup(annot))
    }

    /// Which annotation on `page` the pointer is inside — as
    /// [`FormSession::hover_for_page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let session = FormSession::owned(doc);
    /// assert!(session.hover_for_page(0).is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn hover_for_page(&self, page: impl Into<PageIndex>) -> Option<usize> {
        self.state.hover_for_page(page)
    }

    /// Whether the focused field can undo — as [`FormSession::can_undo`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// assert!(!session.can_undo());
    /// session.character('x', Modifiers::NONE);
    /// assert!(session.can_undo());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.state.can_undo()
    }

    /// Whether the focused field can redo — as [`FormSession::can_redo`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// session.character('x', Modifiers::NONE);
    /// session.key_down(Key::Z, Modifiers::CONTROL);
    /// assert!(session.can_redo());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.state.can_redo()
    }

    /// The focused field's text — as [`FormSession::focused_text`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// assert!(session.focused_text().is_none());
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// assert_eq!(session.focused_text().as_deref(), Some(""));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn focused_text(&self) -> Option<String> {
        self.state.focused_text()
    }

    /// The text currently selected in the focused field — as
    /// [`FormSession::selected_text`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// session.character('x', Modifiers::NONE);
    /// session.key_down(Key::A, Modifiers::CONTROL);
    /// assert_eq!(session.selected_text().as_deref(), Some("x"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        self.state.selected_text()
    }

    /// Replaces the focused field's selection with `text` — as
    /// [`FormSession::replace_selection`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, Key, Modifiers};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// assert!(!session.replace_selection("nothing is focused"));
    /// session.key_down(Key::Tab, Modifiers::NONE);
    /// assert!(session.replace_selection("pasted"));
    /// assert_eq!(session.focused_text().as_deref(), Some("pasted"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn replace_selection(&mut self, text: &str) -> bool {
        self.with(|session| session.replace_selection(text))
    }

    /// Whether a row of the focused choice field is selected — as
    /// [`FormSession::is_index_selected`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/combobox_form.pdf")?);
    /// let session = FormSession::owned(doc);
    /// assert!(!session.is_index_selected(0));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn is_index_selected(&self, index: usize) -> bool {
        self.state.is_index_selected(index)
    }

    /// Selects or clears a row of the focused choice field — as
    /// [`FormSession::set_index_selected`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/combobox_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// // Nothing focused: refused rather than applied to nothing.
    /// assert!(!session.set_index_selected(0, true));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn set_index_selected(&mut self, index: usize, selected: bool) -> bool {
        self.state.set_index_selected(index, selected)
    }

    /// Reads a page in, as showing it would — as [`FormSession::load_page`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/text_form.pdf")?);
    /// let mut session = FormSession::owned(doc);
    /// session.load_page(0);
    /// session.load_page(0); // idempotent
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn load_page(&mut self, page: impl Into<PageIndex>) {
        self.with(|session| session.load_page(page));
    }

    /// Runs what the document asks for on open — as
    /// [`FormSession::open_document`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// session.open_document();
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn open_document(&mut self) {
        self.with(FormSession::open_document);
    }

    /// The scripts that stopped since the last call — as
    /// [`FormSession::script_failures`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Diagnostics, Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// let mut diags = Diagnostics::default();
    /// assert!(session.script_failures(&mut diags).is_empty());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn script_failures(
        &mut self,
        diags: &mut pdfrum_common::Diagnostics,
    ) -> Vec<crate::ScriptFailure> {
        self.state.script_failures(diags)
    }

    /// The session's scripting engine, when it has one — as
    /// [`FormSession::scripts`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// assert!(FormSession::owned(Arc::clone(&doc)).scripts().is_none());
    /// let scripted = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// assert!(scripted.scripts().is_some());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    #[must_use]
    pub fn scripts(&self) -> Option<&pdfrum_form::ScriptCascade> {
        self.state.scripts()
    }

    /// The session's scripting engine, mutably — as
    /// [`FormSession::scripts_mut`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// let cascade = session.scripts_mut().expect("a scripted session");
    /// cascade.run("app.alert('hi')", "example");
    /// assert!(cascade.transcript_text().contains("Alert: hi"));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn scripts_mut(&mut self) -> Option<&mut pdfrum_form::ScriptCascade> {
        self.state.scripts_mut()
    }

    /// Tells the session that `elapsed` passed, and runs whatever timers
    /// came due — as [`FormSession::advance_time`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// assert_eq!(session.advance_time(Duration::from_secs(60)), 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn advance_time(&mut self, elapsed: std::time::Duration) -> usize {
        self.state.advance_time(elapsed)
    }

    /// Gives the keyboard to whichever field the document's own scripts
    /// asked for — as [`FormSession::honour_focus_requests`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// // Nothing has asked, so the keyboard does not move.
    /// assert_eq!(session.honour_focus_requests(), 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn honour_focus_requests(&mut self) -> usize {
        self.with(FormSession::honour_focus_requests)
    }

    /// Runs the page's `/AA /O` — as [`FormSession::page_opened`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// session.page_opened(0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn page_opened(&mut self, page: impl Into<PageIndex>) {
        self.with(|session| session.page_opened(page));
    }

    /// Runs the page's `/AA /C` — as [`FormSession::page_closed`].
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Arc::new(Document::open("tests/fixtures/public_methods.pdf")?);
    /// let mut session = FormSession::owned_with_scripts(doc, &ScriptConfig::wall_clock())?;
    /// session.page_closed(0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn page_closed(&mut self, page: impl Into<PageIndex>) {
        self.with(|session| session.page_closed(page));
    }
}
