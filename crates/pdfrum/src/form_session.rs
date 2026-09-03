//! A live form-filling session: events in, appearance updates out.

use pdfrum_form::Event;
use pdfrum_form::FormSession as Inner;

pub use pdfrum_form::{Button, Key, Modifiers, Response};

pub use pdfrum_form::SessionConfig;

use crate::Document;
use pdfrum_common::PageIndex;
use pdfrum_page::BuildContext;

pub use pdfrum_form::{AppearanceUpdate, UpdateKind};

/// The four points where a field's `/AA` scripts can intervene.
///
/// [`Cascade`] and [`NoScripts`] exist whether or not the `script` feature is
/// on; only `ScriptCascade` is behind it.
pub use pdfrum_form::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome, NoScripts};

/// A live form-filling session over a document (ISO 32000-1 §12.7).
///
/// Events go in and appearance updates come out. Nothing is pushed to a
/// callback and nothing is drawn: each method **returns** what changed, and
/// the caller decides what to re-render.
///
/// # Coordinates
///
/// Every mouse method takes **page space** — PDF user space, y-up, origin at
/// the page's crop box. That is the space [`Page`](crate::Page) reports
/// rectangles in, and no conversion happens on the way in.
///
/// # Two answers, not one
///
/// A [`Response`] carries both halves. `consumed` says the event was handled,
/// which is **not** the same as saying it changed anything — a read-only
/// checkbox consumes a Return and stays unchecked — and `updates` says what
/// changed.
///
/// # A focused field draws differently
///
/// A field with focus renders from live editor state, caret and selection
/// included; an unfocused one falls back to a generated appearance stream.
/// [`FormSession::blur`] is what commits a value and moves a field from the
/// first to the second.
///
/// ```
/// use pdfrum::{Document, FormSession, Key, Modifiers, Point};
///
/// let doc = Document::open("tests/fixtures/text_form.pdf")?;
/// let mut session = FormSession::new(&doc);
///
/// // Nothing has the keyboard yet.
/// assert!(session.focused_annot().is_none());
///
/// // A click is three events, and the move is not decoration: it is what
/// // tells the widget the pointer is over it. The fixture's one text field
/// // is `/Rect [100 100 200 130]`, so (120, 115) lands inside it.
/// let at = Point::new(120.0, 115.0);
/// session.mouse_move(0, at, Modifiers::NONE);
/// session.mouse_down(0, at, Modifiers::NONE);
/// session.mouse_up(0, at, Modifiers::NONE);
/// assert!(session.focused_annot().is_some());
///
/// // Typing sends characters; navigation and shortcuts go through `key_down`.
/// for ch in "Hello".chars() {
///     session.character(ch, Modifiers::NONE);
/// }
/// assert_eq!(session.focused_text().as_deref(), Some("Hello"));
///
/// // Typing records an undo item per character, so this leaves "Hell".
/// session.key_down(Key::Z, Modifiers::CONTROL);
/// assert_eq!(session.focused_text().as_deref(), Some("Hell"));
///
/// // Blur commits, and returns *two* things: the regenerated appearance and
/// // the focus change. A response carrying only the focus change would mean
/// // the field was still drawing its caret.
/// let committed = session.blur();
/// assert!(session.focused_annot().is_none());
/// assert!(committed.updates.iter().any(|u| u.kind.appearance().is_some()));
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
    pages: std::collections::BTreeMap<PageIndex, pdfrum_form::PageForm>,
    /// Which page the embedder is showing.
    ///
    /// Only ever consulted for a keyboard event that arrives with **nothing
    /// focused**, which in practice means Tab entering the focus ring. Every
    /// other key goes to the page holding focus, and mouse events name their
    /// own page.
    viewed_page: PageIndex,
    /// The form's default-resource fonts, loaded once.
    fonts: std::sync::Arc<pdfrum_doc::ap::FormFonts>,
    /// The script hooks every commit passes through.
    ///
    /// [`NoScripts`] by default, which is a JavaScript-off viewer's behaviour
    /// rather than a stub of one — see [`Cascade`]'s own documentation.
    cascade: Cascades,
}

/// Which cascade a session holds.
///
/// Two variants rather than one `Box<dyn Cascade>`, and the reason is the
/// `/AA` wiring: a [`ScriptCascade`](crate::ScriptCascade) does not read a
/// document — it deliberately holds none — so **the caller installs each
/// field's scripts**, and the caller here is this session. Behind a
/// `dyn Cascade` the concrete methods that take the installation
/// (`set_field`, `set_calculation_order`) are unreachable, so the scripted
/// path keeps its concrete type and the general path keeps its trait object.
enum Cascades {
    /// [`NoScripts`], or an implementation the caller wrote.
    ///
    /// Nothing is installed into it: a caller who wants a document's `/AA`
    /// scripts run uses [`FormSession::with_scripts`], and a caller who wrote
    /// their own cascade already knows what it should do.
    Plain(Box<dyn Cascade>),
    /// A `boa`-backed cascade this session installs the document's own `/AA`
    /// scripts into, page by page as pages are read.
    #[cfg(feature = "javascript")]
    Scripted(Box<pdfrum_form::ScriptCascade>),
}

impl Cascades {
    /// The cascade as the seam sees it.
    fn as_dyn(&mut self) -> &mut dyn Cascade {
        match self {
            Cascades::Plain(cascade) => cascade.as_mut(),
            #[cfg(feature = "javascript")]
            Cascades::Scripted(cascade) => cascade.as_mut(),
        }
    }
}

impl std::fmt::Debug for Cascades {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `dyn Cascade` is not `Debug` and must not require it: a bound a
            // caller's own implementation would have to satisfy is a bound on
            // the seam, and the seam has none.
            Cascades::Plain(_) => f.write_str("Plain(..)"),
            #[cfg(feature = "javascript")]
            Cascades::Scripted(cascade) => f.debug_tuple("Scripted").field(cascade).finish(),
        }
    }
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
        FormSession::build(doc, Inner::new(), &mut BuildContext::new())
    }

    /// [`FormSession::new`] with explicit switches — the accelerator
    /// modifier, which annotation subtypes join the focus ring, and the undo
    /// bound.
    #[must_use]
    pub fn with_config(doc: &'a Document, config: SessionConfig) -> FormSession<'a> {
        FormSession::build(doc, Inner::with_config(config), &mut BuildContext::new())
    }

    /// [`FormSession::new`] with fonts resolved through a caller-owned
    /// [`BuildContext`].
    ///
    /// **Use this whenever the caller renders with
    /// [`BuildContext::with_substitution`].** A session lays out its carets
    /// and selection bands from the metrics of the face the `/DA` font
    /// resolves to, so a second substitution over one document puts them at
    /// heights the page is not drawn at.
    #[must_use]
    pub fn with_context(doc: &'a Document, ctx: &mut BuildContext) -> FormSession<'a> {
        FormSession::build(doc, Inner::new(), ctx)
    }

    /// [`FormSession::with_config`] and [`FormSession::with_context`]
    /// together: explicit switches *and* a caller-owned [`BuildContext`].
    ///
    /// The two questions are independent — which keyboard the accelerator is
    /// for, and which faces the document's fonts resolve to.
    #[must_use]
    pub fn with_config_in(
        doc: &'a Document,
        config: SessionConfig,
        ctx: &mut BuildContext,
    ) -> FormSession<'a> {
        FormSession::build(doc, Inner::with_config(config), ctx)
    }

    /// [`FormSession::new`] with commits routed through your own
    /// [`Cascade`] — a validator, an audit log, a policy that refuses a
    /// keystroke.
    ///
    /// [`FormSession::new`] uses [`NoScripts`], whose behaviour *is* a
    /// JavaScript-off viewer's rather than a stub of one. For the document's
    /// own scripts use `FormSession::with_scripts` (the `script` feature),
    /// which builds a cascade *and* installs the `/AA` entries into it; a bare
    /// `ScriptCascade` passed here runs nothing, because nothing has told it
    /// what any field's scripts are.
    #[must_use]
    pub fn with_cascade(doc: &'a Document, cascade: impl Cascade + 'static) -> FormSession<'a> {
        FormSession::build_with(
            doc,
            Inner::new(),
            &mut BuildContext::new(),
            Cascades::Plain(Box::new(cascade)),
        )
    }

    /// [`FormSession::new`] with the document's own JavaScript running: a
    /// `boa`-backed cascade with every field's `/AA` scripts installed into
    /// it.
    ///
    /// This is the constructor to use for scripting: it does what
    /// [`FormSession::with_cascade`] alone cannot, reading each page's `/AA`
    /// entries as the page is read and handing them to the cascade with the
    /// field's qualified name, its value and the `/AcroForm /CO` order.
    ///
    /// The `AF*` library, `util`, `app.alert` and the `event` object are bound,
    /// the four field hooks run, and the `Doc`/`Field` object model answers
    /// from the document — `this.getField`, `this.numFields`, `this.numPages`
    /// and the metadata properties all see the file this session is over.
    /// Do not enable this expecting Acrobat.
    ///
    /// Two things the model deliberately does **not** carry: `this.path` and
    /// `this.URL` are empty, because no PDF records the path it was opened
    /// from and an embedder has no reason to leak one into a script; and
    /// `Doc.getPageNthWord` finds no words, because counting them means
    /// parsing every page's content stream, which building a form session
    /// should not pay for. A host that wants either installs it through
    /// [`scripts_mut`](Self::scripts_mut).
    ///
    /// # Errors
    ///
    /// Only if `boa` cannot build a context at all, which no document can
    /// cause.
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Document::open("tests/fixtures/public_methods.pdf")?;
    /// let mut session = FormSession::with_scripts(&doc, &ScriptConfig::wall_clock())?;
    ///
    /// // The fixture's one field is `/Rect [100 160 200 190]`, and its `/AA`
    /// // carries all four hooks. A click, then a character, reaches the
    /// // keystroke one — and what the script asked the host to do comes back
    /// // on the transcript rather than being performed here.
    /// use pdfrum::{Modifiers as M, Point};
    /// let at = Point::new(150.0, 175.0);
    /// session.mouse_move(0, at, M::NONE);
    /// session.mouse_down(0, at, M::NONE);
    /// session.mouse_up(0, at, M::NONE);
    /// session.character('7', M::NONE);
    ///
    /// let transcript = session.scripts().expect("a scripted session").transcript_text();
    /// assert!(transcript.starts_with("Alert: *** starting test 2 ***"));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn with_scripts(
        doc: &'a Document,
        config: &pdfrum_form::ScriptConfig,
    ) -> Result<FormSession<'a>, crate::ScriptBuildError> {
        let cascade = pdfrum_form::ScriptCascade::new(config)?;
        let mut session = FormSession::build_with(
            doc,
            Inner::new(),
            &mut BuildContext::new(),
            Cascades::Scripted(Box::new(cascade)),
        );
        session.install_document_model();
        session.install_calculation_order();
        Ok(session)
    }

    /// Installs what the `Doc` object answers from: the field list, the page
    /// count and the `/Info` entries.
    ///
    /// A [`ScriptCascade`](crate::ScriptCascade) deliberately holds no
    /// document — which is what lets the engine be tested against a script
    /// string and no PDF — so its caller reads one and hands over a value.
    /// This is that caller for a facade session, and without it
    /// `this.getField`, `this.numFields`, `this.numPages` and every metadata
    /// property answer as an **empty document** would: no fields, no pages,
    /// and `undefined` from `getField`.
    ///
    /// # The path is empty, deliberately
    ///
    /// No PDF carries the path it was opened from, so the model's `path` and
    /// `URL` are the *caller's* to supply and this installs neither: an
    /// embedder that wants `this.path` to answer sets it through
    /// [`scripts_mut`](Self::scripts_mut). A conformance run passes the
    /// harness's own `myfile.pdf`, which two golden lines pin, and an
    /// ordinary embedder has no reason to leak a filesystem path into a
    /// script.
    ///
    /// # `Doc.getPageNthWord` still answers nothing
    ///
    /// The words a page draws need a parsed content stream per page, which is
    /// the expensive half of opening a document and is not something building
    /// a form session should pay for. A host that wants them installs them
    /// itself, and an empty list is the honest answer for one that has not —
    /// `getPageNumWords` answering 0, which is what an empty page gives.
    #[cfg(feature = "javascript")]
    fn install_document_model(&mut self) {
        let catalog = self.doc.catalog();
        let info = self
            .doc
            .parser()
            .trailer()
            .dict(pdfrum_object::names::INFO, self.doc.parser());
        let pages: Vec<pdfrum_object::Dict> = (0..self.doc.page_count())
            .filter_map(|index| self.doc.page(index).ok())
            .map(|page| page.dict.dict.clone())
            .collect();
        let model = pdfrum_form::script::model::read(
            &catalog,
            info.as_ref(),
            &pages,
            "",
            self.doc.parser(),
        );
        if let Cascades::Scripted(cascade) = &mut self.cascade {
            cascade.set_document(model);
        }
    }

    /// Installs the form's `/AcroForm /CO` order into a scripted cascade.
    ///
    /// **An empty order means no calculation runs at all**, however many
    /// fields carry `/AA /C`; `pdfrum_doc::form::Form::calculation_order`
    /// carries the reasoning.
    ///
    /// # One index space, and it is the document's
    ///
    /// `calculation_order` answers positions in
    /// [`Form::fields`](crate::Form::fields) — the document's flat
    /// terminal-field list — and that is the space
    /// [`FieldRef::index`](pdfrum_form::FieldRef::index) carries and the space
    /// [`install_page_scripts`](FormSession::install_page_scripts) installs
    /// under. It used to be keyed by the page-local `FieldId` instead, which
    /// agreed only for a single-page form whose widgets appear in `/Fields`
    /// order; `pdfrum_form::WidgetInfo::field_index` is what closed that.
    #[cfg(feature = "javascript")]
    fn install_calculation_order(&mut self) {
        let Cascades::Scripted(cascade) = &mut self.cascade else {
            return;
        };
        let catalog = self.doc.catalog();
        let mut diags = pdfrum_common::Diagnostics::default();
        let Some(form) =
            pdfrum_doc::form::Form::load(&catalog, self.doc.parser(), &self.doc.limits, &mut diags)
        else {
            return;
        };
        let order = form
            .calculation_order(&catalog, self.doc.parser())
            .into_iter()
            .map(|index| u32::try_from(index).unwrap_or(u32::MAX))
            .collect();
        cascade.set_calculation_order(order);
    }

    /// The shared constructor: a session plus the fonts its appearances are
    /// laid out with.
    fn build(doc: &'a Document, inner: Inner, ctx: &mut BuildContext) -> FormSession<'a> {
        FormSession::build_with(doc, inner, ctx, Cascades::Plain(Box::new(NoScripts)))
    }

    /// [`FormSession::build`] with the cascade named.
    fn build_with(
        doc: &'a Document,
        inner: Inner,
        ctx: &mut BuildContext,
        cascade: Cascades,
    ) -> FormSession<'a> {
        let fonts = pdfrum_doc::ap::FormFonts::load(&doc.catalog(), doc.parser(), ctx);
        FormSession {
            doc,
            inner,
            pages: std::collections::BTreeMap::new(),
            viewed_page: PageIndex::FIRST,
            fonts,
            cascade,
        }
    }

    /// Tells the session which page the embedder is showing.
    ///
    /// Two things need it: a keyboard event arriving with **nothing focused**
    /// — Tab, which is what enters the focus ring — and [`Self::apply`], since
    /// an [`Event`] carries a point but no page.
    ///
    /// Defaults to page 0. A caller showing any other page should say so, or a
    /// Tab from nothing will enter the ring on the wrong one, and so will
    /// every click sent through `apply`.
    pub fn set_viewed_page(&mut self, page: impl Into<PageIndex>) {
        self.viewed_page = page.into();
    }

    /// Which page the embedder last said it was showing.
    #[must_use]
    pub fn viewed_page(&self) -> PageIndex {
        self.viewed_page
    }

    /// The session's switches.
    #[must_use]
    pub fn config(&self) -> &SessionConfig {
        &self.inner.config
    }

    /// Routes a whole [`Event`], for a caller whose input is already a value.
    ///
    /// Every other event method on this type is a thin spelling of this one.
    ///
    /// An [`Event`] carries a point but not a page, so a mouse event applied
    /// here lands on the page [`Self::viewed_page`] names — the wrappers
    /// ([`Self::mouse_move`] and friends) take the page explicitly instead, so
    /// a caller mixing the two should keep the viewed page current. A keyboard
    /// event takes no page either way.
    pub fn apply(&mut self, event: Event) -> Response {
        match event {
            Event::KeyDown { .. } | Event::Char { .. } => self.dispatch_keyboard(event),
            _ => self.dispatch(self.viewed_page, event),
        }
    }

    /// The pointer moved. Drives hover and extends a live drag.
    pub fn mouse_move(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(page, Event::MouseMove { at, modifiers })
    }

    /// The primary button went down.
    ///
    /// A non-primary button is an [`Event`] rather than a method — see
    /// [`Self::apply`] and this type's own note on the right button.
    pub fn mouse_down(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(
            page,
            Event::MouseDown {
                button: Button::Left,
                at,
                modifiers,
            },
        )
    }

    /// The primary button came up.
    pub fn mouse_up(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(
            page,
            Event::MouseUp {
                button: Button::Left,
                at,
                modifiers,
            },
        )
    }

    /// A double click. Selects the whole line under the pointer.
    pub fn double_click(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(page, Event::DoubleClick { at, modifiers })
    }

    /// The wheel turned. Deltas are notches, a negative `y` meaning down.
    pub fn mouse_wheel(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        delta: (i32, i32),
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(
            page,
            Event::MouseWheel {
                at,
                delta,
                modifiers,
            },
        )
    }

    /// Focus was requested at a point, without a click.
    ///
    /// Consumes the event only when an annotation is there *and* it took
    /// focus.
    pub fn focus_at(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response {
        self.dispatch(page, Event::Focus { at, modifiers })
    }

    /// A key went down.
    ///
    /// Navigation and shortcuts arrive here and text does not. There is no
    /// matching key-up method: the oracle's is documented as permanently
    /// unimplemented and always answers false, so modelling it would only
    /// invite callers to send a dead event.
    pub fn key_down(&mut self, key: Key, modifiers: Modifiers) -> Response {
        self.dispatch_keyboard(Event::KeyDown { key, modifiers })
    }

    /// A character was typed.
    ///
    /// Text arrives here and shortcuts do not. A character carrying the
    /// accelerator modifier is deliberately neither: it is refused, so an
    /// embedder's own handling sees it.
    pub fn character(&mut self, ch: char, modifiers: Modifiers) -> Response {
        self.dispatch_keyboard(Event::Char { ch, modifiers })
    }

    /// Drops focus, committing the field that held it.
    ///
    /// This is what moves a field from drawing its live editor state to
    /// drawing a generated appearance stream — so it is the **same**
    /// operation a left click that misses every widget performs, and it goes
    /// through the same `route::kill_focus` rather than reimplementing it.
    /// A version that only reported `FocusChanged` would leave the outgoing
    /// field's caret and live text on the page, which is what this did.
    pub fn blur(&mut self) -> Response {
        let Some(page) = self.focused_annot().map(|annot| annot.page) else {
            // Nothing held focus, so nothing moved.
            return Response::ignored();
        };
        self.with_page_scripted(page, pdfrum_form::kill_focus)
    }

    /// Which annotation currently has the keyboard, if any.
    #[must_use]
    pub fn focused_annot(&self) -> Option<pdfrum_form::AnnotId> {
        self.inner.focus.map(pdfrum_form::FocusTarget::annot)
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
    pub fn focus_for_page(&mut self, page: impl Into<PageIndex>) -> Option<pdfrum_doc::ap::Focus> {
        self.with_page(page, |inner, ctx| pdfrum_form::focus_of(inner, ctx))
    }

    /// The open combo-box dropdown on `page`, if one is open — where it is,
    /// what is in it, and which row is selected or hovered.
    ///
    /// **The library does not draw this.** A dropdown is one of the two pieces
    /// of viewer chrome that fall *outside* a widget's `/Rect` (the other is a
    /// scroll bar, [`Self::scroll_view`]), so it is a value you pull before
    /// painting a page: draw the list if the answer is `Some`, and report what
    /// the user does through [`Self::choose`] and [`Self::close_popup`]. The
    /// caret, selection band and focus rectangle fall inside the rectangle and
    /// are already in the appearance stream.
    ///
    /// `None` for every page with no list open.
    #[must_use]
    pub fn popup_for_page(&mut self, page: impl Into<PageIndex>) -> Option<pdfrum_form::PopupView> {
        self.with_page(page, |inner, ctx| pdfrum_form::popup_view(inner, ctx))
    }

    /// How far one choice widget has scrolled, in rows — the numbers a scroll
    /// bar is drawn from.
    ///
    /// The second value getter beside [`Self::popup_for_page`], and keyed by
    /// annotation rather than by page because a **list box** scrolls with no
    /// dropdown involved: `scrollable_widgets1.pdf` is nothing but two of
    /// them. [`ScrollView::is_scrollable`](pdfrum_form::ScrollView::is_scrollable)
    /// answers whether a bar would be drawn at all.
    ///
    /// Answers `None` for an annotation that is not a choice widget, or one
    /// this session has never built state for — which is any field no event
    /// has reached.
    #[must_use]
    pub fn scroll_view(&mut self, annot: pdfrum_form::AnnotId) -> Option<pdfrum_form::ScrollView> {
        self.with_page(annot.page, |inner, ctx| {
            pdfrum_form::scroll_view(inner, ctx, annot)
        })
    }

    /// Reports that the user picked row `index` of an open dropdown.
    ///
    /// The intent half of [`Self::popup_for_page`]: a host that drew the list
    /// says what was chosen, and the session does what a click on that row
    /// would have done — select it, shut the list, and hand back the widget's
    /// new appearance in the returned updates. There is no need to synthesize
    /// a click at coordinates computed backwards from the geometry.
    ///
    /// An `index` past the end of the options is ignored and the response is
    /// unconsumed, so a host cannot corrupt a field by miscounting.
    pub fn choose(&mut self, annot: pdfrum_form::AnnotId, index: usize) -> Response {
        self.with_page_scripted(annot.page, |inner, ctx, cascade| {
            pdfrum_form::route::choose(inner, ctx, cascade, annot, index)
        })
    }

    /// Reports that an open dropdown was dismissed without a choice.
    ///
    /// The stored selection is left alone — a row the pointer merely rested
    /// on was never chosen. Safe to call on an annotation whose list is
    /// already shut, which answers an unconsumed response.
    pub fn close_popup(&mut self, annot: pdfrum_form::AnnotId) -> Response {
        self.with_page(annot.page, |inner, ctx| {
            pdfrum_form::route::close_popup(inner, ctx, annot)
        })
    }

    /// Which annotation on `page` the pointer is inside — the answer the
    /// annotation pass needs to know a synthesized pop-up note is **open**.
    ///
    /// A separate fact from focus, and they move independently: a pointer
    /// resting on an annotation leaves the keyboard wherever it was, and the
    /// annotation under the pointer need not be focusable at all. A text
    /// highlight is the case that matters, because its note card is *only*
    /// reachable this way — nothing a file can say opens one.
    ///
    /// The result is a **raw `/Annots` index**, the key space
    /// [`pdfrum_doc::AnnotOverlay::set_hover`] wants. `None` when the pointer
    /// is over nothing, or over an annotation on another page.
    #[must_use]
    pub fn hover_for_page(&self, page: impl Into<PageIndex>) -> Option<usize> {
        let page = page.into();
        let hover = self.inner.hover?;
        (hover.page == page).then_some(hover.index as usize)
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

    /// Replaces the focused field's selection with `text`, or deletes it when
    /// `text` is empty.
    ///
    /// The embedder's paste and cut: this crate has no clipboard, so a cut is
    /// a caller reading [`FormSession::selected_text`] and then calling this
    /// with an empty string. With no selection the text is inserted at the
    /// caret, and an empty string then does nothing at all.
    ///
    /// The `bool` is the answer, not a failed mutation: `true` when the field
    /// changed, `false` when nothing was focused or the text was unchanged.
    pub fn replace_selection(&mut self, text: &str) -> bool {
        let Some(target) = self.inner.focus else {
            return false;
        };
        let Some(field) = target.field() else {
            return false;
        };
        let page = target.annot().page;
        if !self.pages.contains_key(&page) {
            let Some(read) = self.read_page(page) else {
                return false;
            };
            self.pages.insert(page, read);
        }
        let Some(form) = self.pages.get(&page) else {
            return false;
        };
        let catalog = self.doc.catalog();
        let ctx = pdfrum_form::Context {
            page: form,
            catalog: &catalog,
            resolve: self.doc.parser(),
            fonts: &self.fonts,
            permissions: self.permissions(),
        };
        pdfrum_form::route::replace_selection(&mut self.inner, &ctx, field, text)
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
    /// The `bool` is the answer, not a failed mutation: whether the call was
    /// **accepted**, which is not the same as whether anything changed. A list
    /// box accepts a redundant clear and still moves its caret; a combo box
    /// refuses every clear; a missing row, a text field, and nothing focused
    /// all answer `false` rather than panicking.
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

    /// The session's scripting engine, when it has one — what the
    /// document's JavaScript asked the host to do, and what stopped.
    ///
    /// `Some` only for a session built by [`FormSession::with_scripts`].
    ///
    /// This is where `app.alert`, `Doc.submitForm` and every other thing a
    /// script *asks for* comes back, on
    /// [`transcript`](pdfrum_form::ScriptCascade::transcript), as values the
    /// host decides about rather than I/O the library performs. A script that
    /// threw is reported on [`stops`](pdfrum_form::ScriptCascade::stops), and
    /// the next one still runs.
    #[cfg(feature = "javascript")]
    #[must_use]
    pub fn scripts(&self) -> Option<&pdfrum_form::ScriptCascade> {
        match &self.cascade {
            Cascades::Scripted(cascade) => Some(cascade),
            Cascades::Plain(_) => None,
        }
    }

    /// The session's scripting engine, mutably.
    ///
    /// For a host that runs the document's own document-level scripts —
    /// `/Names /JavaScript` and `/OpenAction` — which are the document's and
    /// not any field's, so the session cannot run them on its own behalf.
    ///
    /// `Some` only for a session built by [`FormSession::with_scripts`].
    #[cfg(feature = "javascript")]
    pub fn scripts_mut(&mut self) -> Option<&mut pdfrum_form::ScriptCascade> {
        match &mut self.cascade {
            Cascades::Scripted(cascade) => Some(cascade),
            Cascades::Plain(_) => None,
        }
    }

    /// Tells the session that `elapsed` passed, and runs whatever timers came
    /// due.
    ///
    /// **This library never reads a clock**, so a document's
    /// `app.setInterval` is inert until a host says time moved. A viewer
    /// calls this from its own event loop; a test calls it with a number.
    /// Answers how many timer scripts ran.
    ///
    /// Only for a session built by [`FormSession::with_scripts`] — with no
    /// engine there is nothing that could have armed a timer, and the answer
    /// is zero.
    ///
    /// ```
    /// # #[cfg(feature = "javascript")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use pdfrum::{Document, FormSession, ScriptConfig};
    ///
    /// let doc = Document::open("tests/fixtures/public_methods.pdf")?;
    /// let mut session = FormSession::with_scripts(&doc, &ScriptConfig::wall_clock())?;
    ///
    /// // Nothing in this document arms a timer, so nothing is due however
    /// // much time is claimed to have passed.
    /// assert_eq!(session.advance_time(Duration::from_secs(60)), 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "javascript"))] fn main() {}
    /// ```
    #[cfg(feature = "javascript")]
    pub fn advance_time(&mut self, elapsed: std::time::Duration) -> usize {
        self.scripts_mut()
            .map_or(0, |cascade| cascade.advance_time(elapsed))
    }

    /// The page's own `/AA` script for one trigger, if it carries one.
    ///
    /// A **page**-level action rather than a widget's: `/AA /O` when the page
    /// opens and `/AA /C` when it closes, off the page dictionary itself
    /// (`FORM_DoPageAAction`, `fpdfsdk/fpdf_formfill.cpp:918-944`). Not to be
    /// confused with a field's `/AA /C`, which is Calculate — the two share a
    /// key in different dictionaries.
    #[cfg(feature = "javascript")]
    fn page_action(&self, page: PageIndex, opening: bool) -> Option<String> {
        use pdfrum_doc::nav::AActionType;
        let loaded = self.doc.page(page).ok()?;
        let resolve = self.doc.parser();
        let entries = loaded.dict.dict.dict(pdfrum_object::names::AA, resolve)?;
        let trigger = if opening {
            AActionType::OpenPage
        } else {
            AActionType::ClosePage
        };
        pdfrum_doc::nav::additional_action(&entries, trigger, resolve)
            .filter(|action| action.kind() == pdfrum_doc::ActionKind::JavaScript)
            .and_then(|action| action.javascript(resolve))
            .filter(|source| !source.is_empty())
    }

    /// Gives the keyboard to whichever field the document's own scripts asked
    /// for, running the two `/AA` entries a click would.
    ///
    /// **Only a document-level script needs this called.** Every event method
    /// already spends the request on its way out, because a field script that
    /// calls `Field.setFocus` runs inside routing that can move the keyboard
    /// itself. `/OpenAction`, `/Names /JavaScript` and a page's own `/AA /O`
    /// run through [`scripts_mut`](Self::scripts_mut) instead, outside any
    /// event, so a host that runs them spends the request here.
    ///
    /// Answers how many times the keyboard moved — `0` when nothing asked,
    /// which is the ordinary case.
    ///
    /// # The field may be on a page nobody has read
    ///
    /// A script names a field, not a page, so the page holding it is read
    /// here if it has not been already. That read runs the page's formatters
    /// like any other, which is upstream's behaviour: `setFocus` reaches
    /// `GetWidget`, which reaches `GetPageViewAtIndex`, which builds the page
    /// view and loads its annotations.
    #[cfg(feature = "javascript")]
    pub fn honour_focus_requests(&mut self) -> usize {
        let mut moved = 0;
        while let Some(index) = self
            .scripts_mut()
            .and_then(pdfrum_form::Cascade::take_focus_request)
        {
            let Some(page) = self.page_of_field(index) else {
                // No page carries the field, so there is no widget to give
                // the keyboard to — `GetWidget` answering null, where
                // `setFocus` does nothing at all.
                continue;
            };
            self.with_page_scripted(page, |inner, ctx, cascade| {
                pdfrum_form::focus_field(inner, ctx, cascade, index);
            });
            moved += 1;
        }
        moved
    }

    /// Which page carries a field's widget, reading pages until one does.
    ///
    /// The pages this session has already read are searched first, so the
    /// ordinary case costs no parse; only a field on an untouched page makes
    /// this read one.
    #[cfg(feature = "javascript")]
    fn page_of_field(&mut self, index: u32) -> Option<PageIndex> {
        let found = self
            .pages
            .iter()
            .find(|(_, form)| form.field_of_index(index).is_some())
            .map(|(page, _)| *page);
        if found.is_some() {
            return found;
        }
        for page in 0..self.doc.page_count() {
            let page = PageIndex::from(page);
            self.ensure_page(page);
            if self
                .pages
                .get(&page)
                .is_some_and(|form| form.field_of_index(index).is_some())
            {
                return Some(page);
            }
        }
        None
    }

    /// Runs the page's `/AA /O` — what showing a page fires.
    ///
    /// Separate from [`load_page`](Self::load_page) because the two are
    /// separate calls upstream and a host may show a page it has already
    /// read: `FORM_OnAfterLoadPage` and `FORM_DoPageAAction(…, OPEN)` are two
    /// lines in `pdfium_test`'s own `GetPage`.
    #[cfg(feature = "javascript")]
    pub fn page_opened(&mut self, page: impl Into<PageIndex>) {
        let page = page.into();
        let Some(source) = self.page_action(page, true) else {
            return;
        };
        if let Some(cascade) = self.scripts_mut() {
            cascade.run(&source, "/AA /O");
        }
    }

    /// Runs the page's `/AA /C` — what leaving a page fires.
    #[cfg(feature = "javascript")]
    pub fn page_closed(&mut self, page: impl Into<PageIndex>) {
        let page = page.into();
        let Some(source) = self.page_action(page, false) else {
            return;
        };
        if let Some(cascade) = self.scripts_mut() {
            cascade.run(&source, "/AA /C");
        }
    }

    /// Reads a page in, as showing it would.
    ///
    /// **The point is the side effects, not the read.** Building a page's
    /// widgets installs their `/AA` scripts into a scripted cascade and runs
    /// each text field's and combo box's formatter — so a document whose only
    /// script is a formatter prints its alerts when this is called and not
    /// before. A host that renders every page in turn gets that for free
    /// through the event methods; one that renders none must say so here.
    ///
    /// Idempotent: a page already read is not read again, and its scripts do
    /// not run a second time.
    pub fn load_page(&mut self, page: impl Into<PageIndex>) {
        self.ensure_page(page.into());
    }

    /// Routes an event that names a page.
    ///
    /// The page is read on first use and kept: a replay sends dozens of
    /// events at one page, and the `/Annots` walk is the expensive half.
    fn dispatch(&mut self, page: impl Into<PageIndex>, event: Event) -> Response {
        self.with_page_scripted(page, |inner, ctx, cascade| {
            pdfrum_form::apply(inner, ctx, cascade, event)
        })
    }

    /// Reads a page, builds its routing context, and runs `body` against it.
    ///
    /// The one place a `Context` is assembled, so that every operation
    /// needing one — routing an event, and dropping focus — goes through the
    /// same page cache and the same borrow of the catalog.
    fn with_page<T: Default>(
        &mut self,
        page: impl Into<PageIndex>,
        body: impl FnOnce(
            &mut pdfrum_form::FormSession,
            &pdfrum_form::Context<'_, pdfrum_parser::Document>,
        ) -> T,
    ) -> T {
        let page = page.into();
        self.ensure_page(page);
        let Some(form) = self.pages.get(&page) else {
            return T::default();
        };
        let catalog = self.doc.catalog();
        let ctx = pdfrum_form::Context {
            page: form,
            catalog: &catalog,
            resolve: self.doc.parser(),
            fonts: &self.fonts,
            permissions: self.permissions(),
        };
        body(&mut self.inner, &ctx)
    }

    /// [`FormSession::with_page`] for the three entry points that can commit
    /// a field, which take the script hooks as a second parameter.
    ///
    /// The hooks are this session's [`Cascade`] — [`NoScripts`] unless a
    /// caller named another through [`FormSession::with_cascade`] or
    /// [`FormSession::with_scripts`]. `NoScripts`'s method defaults *are* a
    /// JavaScript-off viewer's behaviour rather than a stub of it, so
    /// substituting a different value here changes no call site, which is the
    /// whole reason the seam is a trait.
    ///
    /// The cascade is **moved out of `self` for the duration of the call**
    /// and put back after, because the body needs it and the page cache at
    /// the same time and both live behind the one `&mut self`. `NoScripts` is
    /// the placeholder left in its slot, which is the correct value for a
    /// session that has none.
    fn with_page_scripted<T: Default>(
        &mut self,
        page: impl Into<PageIndex>,
        body: impl FnOnce(
            &mut pdfrum_form::FormSession,
            &pdfrum_form::Context<'_, pdfrum_parser::Document>,
            &mut dyn Cascade,
        ) -> T,
    ) -> T {
        let page = page.into();
        // The page is read — and its scripts installed — **before** the
        // cascade leaves `self`, because the install needs both.
        self.ensure_page(page);
        let mut cascade =
            std::mem::replace(&mut self.cascade, Cascades::Plain(Box::new(NoScripts)));
        let out = self.with_page(page, |inner, ctx| body(inner, ctx, cascade.as_dyn()));
        self.cascade = cascade;
        out
    }

    /// Routes an event that goes to whatever holds focus.
    ///
    /// Keyboard events name no page, so the page they route against is the
    /// one holding focus — which is why typing works after a click and does
    /// nothing before one.
    fn dispatch_keyboard(&mut self, event: Event) -> Response {
        if let Some(target) = self.inner.focus {
            let page = pdfrum_form::FocusTarget::annot(target).page;
            return self.dispatch(page, event);
        }
        // **Tab is the exception**, and it is the reason this is not simply
        // "no focus, no keyboard": a Tab with nothing focused is what *takes*
        // focus, so refusing it here would make the ring unreachable from the
        // keyboard. It enters the ring on the page the embedder says it is
        // showing — see `set_viewed_page`, which is how this crate spells
        // the page argument the oracle puts on `FORM_OnKeyDown` itself.
        //
        // Every other key really does need a focused field, and answers
        // unhandled without one.
        match event {
            Event::KeyDown { key: Key::Tab, .. } => self.dispatch(self.viewed_page, event),
            _ => Response::ignored(),
        }
    }

    /// Reads a page into the cache if it is not there yet, installing its
    /// `/AA` scripts into a scripted cascade as it arrives.
    ///
    /// Separated from [`FormSession::with_page`] because
    /// [`FormSession::with_page_scripted`] must do it *before* it takes the
    /// cascade out of `self` — a cascade that has left cannot be installed
    /// into, and the page a commit is about is exactly the page whose scripts
    /// the commit runs.
    fn ensure_page(&mut self, page: PageIndex) {
        if self.pages.contains_key(&page) {
            return;
        }
        let Some(read) = self.read_page(page) else {
            return;
        };
        self.pages.insert(page, read);
        self.install_page_scripts(page);
    }

    /// Hands a scripted cascade every `/AA` script the page just read carries.
    ///
    /// # Why per page, and why here
    ///
    /// A [`FieldRef`]'s index *is* a position in
    /// [`Form::fields`](crate::Form::fields), so the number does not depend on
    /// the page — but the `/AA` dictionaries do: they hang off the widget
    /// annotations, which are reached through `/Annots` and exist only once a
    /// page has been read. So the install is still per page, and a document
    /// whose second page is never touched never runs its scripts, which is
    /// also the oracle's behaviour.
    ///
    /// A [`ScriptCascade`](crate::ScriptCascade) deliberately holds no
    /// document, which is what lets the whole engine be tested against a
    /// script string and no PDF; the price is that its caller reads `/AA` and
    /// hands it over, and this is that caller.
    ///
    /// Compiled away entirely without the `script` feature: with no scripted
    /// variant to match, there is nothing to install.
    #[cfg(feature = "javascript")]
    fn install_page_scripts(&mut self, page: PageIndex) {
        use pdfrum_doc::nav::AActionType;

        let Cascades::Scripted(cascade) = &mut self.cascade else {
            return;
        };
        let Some(form) = self.pages.get(&page) else {
            return;
        };
        let resolve = self.doc.parser();
        let mut loaded: Vec<pdfrum_form::FieldRef> = Vec::new();
        for widget in &form.widgets {
            let Some(entries) = widget.dict.dict(pdfrum_object::names::AA, resolve) else {
                continue;
            };
            // A trigger's source, or `None` — for an `/AA` entry that is not
            // a JavaScript action at all, and for one whose `/JS` is empty,
            // which `DoActionJavaScript` also declines to run
            // (`cpdfsdk_formfillenvironment.cpp:912-924`).
            let source_of = |trigger| {
                pdfrum_doc::nav::additional_action(&entries, trigger, resolve)
                    .filter(|action| action.kind() == pdfrum_doc::ActionKind::JavaScript)
                    .and_then(|action| action.javascript(resolve))
                    .filter(|source| !source.is_empty())
            };
            let actions = pdfrum_form::script::FieldActions {
                keystroke: source_of(AActionType::KeyStroke),
                validate: source_of(AActionType::Validate),
                calculate: source_of(AActionType::Calculate),
                format: source_of(AActionType::Format),
                mouse_enter: source_of(AActionType::CursorEnter),
                mouse_exit: source_of(AActionType::CursorExit),
                mouse_down: source_of(AActionType::ButtonDown),
                mouse_up: source_of(AActionType::ButtonUp),
                focus: source_of(AActionType::GetFocus),
                blur: source_of(AActionType::LoseFocus),
            };
            if actions == pdfrum_form::script::FieldActions::default() {
                // Nearly every field in nearly every document: no script at
                // all, and a hook with no script takes `NoScripts`'s answer.
                continue;
            }
            let Some(index) = widget.field_index else {
                // A widget the form's `/Fields` does not reach. A script has
                // no way to name it, so there is nothing to install it under
                // — `GetFieldByDict` answers null for it upstream too.
                continue;
            };
            let text_like = matches!(
                widget.kind,
                Some(pdfrum_doc::form::FieldKind::Text | pdfrum_doc::form::FieldKind::Combo)
            );
            cascade.set_field(index, widget.name.clone(), widget.value(resolve), actions);
            if text_like {
                loaded.push(pdfrum_form::FieldRef {
                    name: widget.name.clone(),
                    index: Some(index),
                });
            }
        }
        // **Loading a page runs each text field's and combo box's formatter.**
        // `CPDFSDK_PageView` calls `OnLoad` on every annotation it builds, and
        // `CPDFSDK_Widget::OnLoad` runs `OnFormat()` for those two field types
        // (`fpdfsdk/cpdfsdk_widget.cpp:1104-1122`) so the stored value can be
        // drawn as a formatted one. The display string reaches the appearance
        // and is dropped again for a text field; what is *not* dropped is
        // everything the script asked the host to do on the way there, which
        // is why a document whose only script is a formatter still alerts on
        // open.
        //
        // After the whole install loop rather than inside it, because a
        // formatter may call `getField` on a field later in the same page and
        // must find it installed.
        for field in loaded {
            cascade.format_on_load(&field);
        }
    }

    /// Without the `script` feature there is no cascade that can be installed
    /// into, so this is the whole of it.
    #[cfg(not(feature = "javascript"))]
    #[expect(
        clippy::unused_self,
        reason = "the feature-on twin takes `&mut self`; one signature, two bodies"
    )]
    fn install_page_scripts(&mut self, _page: PageIndex) {}

    /// Reads one page's annotations, or `None` when the page will not load.
    fn read_page(&self, page: PageIndex) -> Option<pdfrum_form::PageForm> {
        let loaded = self.doc.page(page).ok()?;
        Some(pdfrum_form::read_page(
            page,
            &loaded.dict.dict,
            &self.doc.catalog(),
            self.doc.parser(),
        ))
    }

    /// What the document permits, which gates every non-push-button click.
    ///
    /// `pdfrum-form` asks only two of ISO 32000-1 table 22's eight questions
    /// and keeps its own two-field type for them, so this is where the two
    /// vocabularies meet — a field-for-field rename, with the `/P` bit numbers
    /// living beside the word they decode rather than here.
    fn permissions(&self) -> pdfrum_form::Permissions {
        if !self.doc.is_encrypted() {
            return pdfrum_form::Permissions::ALL;
        }
        let granted = self.doc.permissions();
        pdfrum_form::Permissions {
            fill_form: granted.fill_form,
            modify_annotation: granted.annotate,
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
        let response = session.blur();
        assert!(!response.consumed);
        assert!(response.updates.is_empty());
    }

    /// `apply` and the wrapper that spells it reach the same field.
    ///
    /// The pin on the sketch's central method: a click built as an [`Event`]
    /// and applied against the viewed page must focus what the same click
    /// spelled through [`FormSession::mouse_down`] focuses. Anything else
    /// would make the wrappers a second implementation rather than a spelling.
    #[test]
    fn applying_an_event_routes_where_the_wrapper_does() {
        let doc = document();
        let mut through_event = FormSession::new(&doc);
        // The text field's `/Rect` is `[100 100 200 130]`; (120, 115) is in it.
        let inside = kurbo::Point::new(120.0, 115.0);
        through_event.apply(Event::MouseDown {
            button: Button::Left,
            at: inside,
            modifiers: Modifiers::NONE,
        });

        let mut through_wrapper = FormSession::new(&doc);
        through_wrapper.mouse_down(0, inside, Modifiers::NONE);

        assert_eq!(
            through_event.focused_annot(),
            through_wrapper.focused_annot()
        );
        assert!(through_event.focused_annot().is_some());
    }

    /// A keyboard event applied as a value needs no page, and Tab from
    /// nothing still enters the ring.
    #[test]
    fn applying_a_key_needs_no_page() {
        let doc = document();
        let mut session = FormSession::new(&doc);
        let response = session.apply(Event::KeyDown {
            key: Key::Tab,
            modifiers: Modifiers::NONE,
        });
        assert!(response.consumed);
        assert!(session.focused_annot().is_some());
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
