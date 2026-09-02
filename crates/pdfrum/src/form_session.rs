//! A live form-filling session: events in, appearance updates out.

use pdfrum_form::session::FormSession as Inner;
use pdfrum_form::{Button, Event, Key, Modifiers, Point, Response};

pub use pdfrum_form::SessionConfig;

use crate::Document;
use pdfrum_page::BuildContext;

pub use pdfrum_form::event::{Button as MouseButton, Key as VirtualKey};
pub use pdfrum_form::update::{AppearanceUpdate, UpdateKind};
pub use pdfrum_form::{Modifiers as EventModifiers, Response as EventResponse};

/// The four points where a field's `/AA` scripts can intervene.
///
/// Re-exported **unconditionally**, because it is named in
/// [`FormSession::with_cascade`]'s signature and a caller who writes their own
/// implementation must be able to name it from `pdfrum` alone (WP7's rule).
/// The trait and [`NoScripts`] exist whether or not the `script` feature is
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
/// // back is what changed, for a caller to re-render — and it is *two*
/// // things, not one: the regenerated appearance, and the focus change
/// // itself. A response carrying only the focus change would mean the field
/// // was still drawing its caret.
/// let committed = session.force_kill_focus();
/// assert!(session.focused_annot().is_none());
///
/// let regenerated = committed
///     .updates
///     .iter()
///     .any(|update| update.kind.appearance().is_some());
/// assert!(regenerated, "the committed field is redrawn, not just unfocused");
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
    /// Which page the embedder is showing.
    ///
    /// Only ever consulted for a keyboard event that arrives with **nothing
    /// focused**, which in practice means Tab entering the focus ring. Every
    /// other key goes to the page holding focus, and mouse events name their
    /// own page.
    page_in_view: u32,
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
    #[cfg(feature = "script")]
    Scripted(Box<pdfrum_form::ScriptCascade>),
}

impl Cascades {
    /// The cascade as the seam sees it.
    fn as_dyn(&mut self) -> &mut dyn Cascade {
        match self {
            Cascades::Plain(cascade) => cascade.as_mut(),
            #[cfg(feature = "script")]
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
            #[cfg(feature = "script")]
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

    /// Starts a session with explicit switches — the accelerator modifier,
    /// which annotation subtypes join the focus ring, and the undo bound.
    #[must_use]
    pub fn with_config(doc: &'a Document, config: SessionConfig) -> FormSession<'a> {
        FormSession::build(doc, Inner::with_config(config), &mut BuildContext::new())
    }

    /// Starts a session whose fonts are resolved through a caller-owned
    /// [`BuildContext`], as [`Page::render_with`](crate::Page::render_with).
    ///
    /// # Use this whenever the caller renders with substitution options
    ///
    /// A session lays out the appearances it hands back — the glyph run, the
    /// caret, the selection band — and it lays them out with the *metrics of
    /// the face the `/DA` font resolves to*. [`FormSession::new`] resolves
    /// that face through a default [`BuildContext`], which is right only when
    /// the caller renders through a default one too.
    ///
    /// A caller that renders with [`BuildContext::with_substitution`] — an
    /// explicit font directory, Croscore naming — and starts its session with
    /// [`FormSession::new`] gets **two different substitutions over one
    /// document**: the page's `/Arial` becomes Arimo (ascent 905, descent
    /// −211) while the session's falls through to the built-in base-14
    /// Helvetica (718, −219). Every height the session computes is then wrong
    /// by the difference, which at 12pt is 2.148 units — enough to move a
    /// caret a whole device row. Threading one context through both closes
    /// it, and it is the same context that carries the font, colorspace and
    /// image caches, so the fonts are also parsed once rather than twice.
    ///
    /// ```
    /// use pdfrum::{BuildContext, Document, FormSession};
    ///
    /// let doc = Document::open("tests/fixtures/text_form.pdf")?;
    /// // The context a caller would also render this document through.
    /// let mut ctx = BuildContext::new();
    /// let session = FormSession::with_context(&doc, &mut ctx);
    /// assert!(session.focused_annot().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn with_context(doc: &'a Document, ctx: &mut BuildContext) -> FormSession<'a> {
        FormSession::build(doc, Inner::new(), ctx)
    }

    /// Starts a session with explicit switches *and* a caller-owned
    /// [`BuildContext`] — [`FormSession::with_config`] and
    /// [`FormSession::with_context`] together.
    ///
    /// The pair exists because the two questions are independent: which
    /// keyboard the accelerator is for, and which faces the document's fonts
    /// resolve to. A caller that has both answers should not have to give up
    /// one to state the other.
    ///
    /// ```
    /// use pdfrum::{
    ///     BuildContext, Document, EventModifiers, FormSession, SessionConfig, VirtualKey,
    /// };
    ///
    /// let doc = Document::open("tests/fixtures/text_form.pdf")?;
    /// let mut ctx = BuildContext::new();
    /// // An Apple keyboard, resolved through the caller's own context.
    /// let mut session =
    ///     FormSession::with_config_in(&doc, SessionConfig::apple(), &mut ctx);
    ///
    /// // The field is `/Rect [100 100 200 130]`, so (120, 115) is inside it.
    /// session.on_mouse_move(0, 120.0, 115.0, EventModifiers::NONE);
    /// session.on_mouse_down(0, 120.0, 115.0, EventModifiers::NONE);
    /// session.on_mouse_up(0, 120.0, 115.0, EventModifiers::NONE);
    /// assert!(session.focused_annot().is_some());
    ///
    /// // And the Apple switch is live: Command is the accelerator, so
    /// // Command+A selects all where Control+A types nothing.
    /// session.on_char('x', EventModifiers::NONE);
    /// session.on_key_down(VirtualKey::A, EventModifiers::META);
    /// assert_eq!(session.selected_text().as_deref(), Some("x"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn with_config_in(
        doc: &'a Document,
        config: SessionConfig,
        ctx: &mut BuildContext,
    ) -> FormSession<'a> {
        FormSession::build(doc, Inner::with_config(config), ctx)
    }

    /// Starts a session whose commits pass through `cascade` — the seam a
    /// field's `/AA` scripts hang off.
    ///
    /// [`FormSession::new`] uses [`NoScripts`], whose behaviour *is* a
    /// JavaScript-off viewer's rather than a stub of one. This is how a
    /// caller substitutes something else, and the two implementations worth
    /// naming are:
    ///
    /// - **`ScriptCascade`**, behind the `script` feature — but reach for
    ///   `FormSession::with_scripts` instead, which builds one *and* installs
    ///   the document's own `/AA` scripts into it. A `ScriptCascade` passed
    ///   here runs nothing, because nothing has told it what any field's
    ///   scripts are.
    /// - **your own** implementation of [`Cascade`], for a host that gates
    ///   commits on rules of its own — a validator, an audit log, a policy
    ///   that refuses a keystroke.
    ///
    /// ```
    /// use pdfrum::{Cascade, Document, FieldRef, FormSession};
    ///
    /// /// A cascade that refuses every commit.
    /// struct ReadOnly;
    /// impl Cascade for ReadOnly {
    ///     fn validate(&mut self, _field: &FieldRef, _value: &str) -> bool {
    ///         false
    ///     }
    /// }
    ///
    /// let doc = Document::open("tests/fixtures/text_form.pdf")?;
    /// let session = FormSession::with_cascade(&doc, ReadOnly);
    /// assert!(session.focused_annot().is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn with_cascade(doc: &'a Document, cascade: impl Cascade + 'static) -> FormSession<'a> {
        FormSession::build_with(
            doc,
            Inner::new(),
            &mut BuildContext::new(),
            Cascades::Plain(Box::new(cascade)),
        )
    }

    /// Starts a session that **runs the document's own JavaScript**: a
    /// `boa`-backed cascade with every field's `/AA` scripts installed into
    /// it.
    ///
    /// This is the constructor to use for scripting. It does what
    /// [`FormSession::with_cascade`] alone cannot: each page's `/AA /K`,
    /// `/AA /V`, `/AA /C` and `/AA /F` entries are read as a page is read and
    /// handed to the cascade, along with each field's fully qualified name,
    /// its stored value and the form's `/AcroForm /CO` calculation order — a
    /// [`ScriptCascade`](crate::ScriptCascade) holds no document and cannot
    /// read any of that for itself.
    ///
    /// # What a script can and cannot reach
    ///
    /// The `AF*` library (`AFNumber_Format`, `AFDate_*`, `AFSimple_Calculate`,
    /// …), `util`, `app.alert` and the `event` object are bound and the four
    /// field hooks run; the `Doc`/`Field` object model — `this.getField(…)`,
    /// and everything a script does through it — **is not built yet**, so
    /// 11 of the oracle's 47 JavaScript fixtures reproduce byte-exactly today.
    /// See PLAN.md §M15 for the milestone and `docs/status/M15.md` for the
    /// per-fixture accounting. Do not enable this expecting Acrobat.
    ///
    /// # Errors
    ///
    /// Only if `boa` cannot build a context at all, which no document can
    /// cause.
    ///
    /// ```
    /// # #[cfg(feature = "script")]
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
    /// use pdfrum::EventModifiers as M;
    /// session.on_mouse_move(0, 150.0, 175.0, M::NONE);
    /// session.on_mouse_down(0, 150.0, 175.0, M::NONE);
    /// session.on_mouse_up(0, 150.0, 175.0, M::NONE);
    /// session.on_char('7', M::NONE);
    ///
    /// let transcript = session.scripts().expect("a scripted session").transcript_text();
    /// assert!(transcript.starts_with("Alert: *** starting test 2 ***"));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "script"))] fn main() {}
    /// ```
    #[cfg(feature = "script")]
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
        session.install_calculation_order();
        Ok(session)
    }

    /// Installs the form's `/AcroForm /CO` order into a scripted cascade.
    ///
    /// **An empty order means no calculation runs at all**, however many
    /// fields carry `/AA /C` — `cpdf_interactiveform.cpp:739-745`, and
    /// `pdfrum_doc::form::Form::calculation_order` carries the reasoning.
    ///
    /// # A known index-space mismatch, inherited rather than introduced
    ///
    /// `calculation_order` answers positions in
    /// [`Form::fields`](crate::Form::fields) — the document's whole field
    /// list — while everything else the cascade is keyed by is a **page-local
    /// `FieldId`**, allocated in first-seen order as one page's `/Annots` are
    /// walked. The two spaces agree for a single-page form whose widgets
    /// appear in `/Fields` order, which is every `/CO`-bearing fixture in the
    /// oracle's corpus, and disagree otherwise.
    ///
    /// This is not this method's defect to fix: `route::commit_field` already
    /// spends a calculation's writes as `FieldId(index)` under a comment
    /// asserting the two are the same thing, so the mismatch is a
    /// `pdfrum-form` question about what `FieldRef::index` means, and fixing
    /// it in one place and not the other would make them disagree in a new
    /// way. Recorded in `docs/status/pdfrum-facade.md`; it belongs to M15
    /// step 2, where the `Doc`/`Field` object model settles what a script's
    /// field identity is.
    #[cfg(feature = "script")]
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
            page_in_view: 0,
            fonts,
            cascade,
        }
    }

    /// Tells the session which page the embedder is showing.
    ///
    /// This exists for exactly one case, and naming it is the point: a
    /// keyboard event that arrives with **nothing focused** has no field to
    /// route to and no page of its own, and the one such event that must
    /// still do something is **Tab** — it is what enters the focus ring in
    /// the first place. The oracle spells the same fact as a page parameter
    /// on `FORM_OnKeyDown`, which its callers fill in with the page in view.
    ///
    /// Defaults to page 0, which is right for a single-page document and for
    /// a viewer that has not scrolled. A caller showing any other page should
    /// say so, or a Tab from nothing will enter the ring on the wrong one.
    pub fn set_page_in_view(&mut self, page: u32) {
        self.page_in_view = page;
    }

    /// Which page the embedder last said it was showing.
    #[must_use]
    pub fn page_in_view(&self) -> u32 {
        self.page_in_view
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
    /// drawing a generated appearance stream — so it is the **same**
    /// operation a left click that misses every widget performs, and it goes
    /// through the same `route::kill_focus` rather than reimplementing it.
    /// A version that only reported `FocusChanged` would leave the outgoing
    /// field's caret and live text on the page, which is what this did.
    pub fn force_kill_focus(&mut self) -> Response {
        let Some(page) = self.focused_annot().map(|annot| annot.page) else {
            // Nothing held focus, so nothing moved.
            return Response::ignored();
        };
        self.with_page_scripted(page, pdfrum_form::kill_focus)
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
        self.with_page(page, |inner, ctx| pdfrum_form::focus_of(inner, ctx))
    }

    /// The open combo-box dropdown on `page`, if one is open — where it is,
    /// what is in it, and which row is selected or hovered.
    ///
    /// **The library does not draw this and does not ask you to.** A dropdown
    /// is one of the two pieces of PDFium's `fpdfsdk/pwl` chrome that live
    /// *outside* a widget's `/Rect` (the other is a scroll bar,
    /// [`Self::scroll_view`]), and drawing outside the rectangle means
    /// creating a window — which is the host's job, not a PDF library's. So
    /// this is a value you pull on your own schedule rather than a callback
    /// you must implement: ask before you paint a page, draw the list if the
    /// answer is `Some`, and report what the user does with it through
    /// [`Self::choose`] and [`Self::close_popup`].
    ///
    /// The three pieces of chrome that live *inside* the rectangle — the
    /// caret, the selection band and the focus rectangle — are already in the
    /// appearance stream a session hands back, so nothing extra is needed for
    /// them.
    ///
    /// Answers `None` for every page with no list open, which is every page
    /// almost all of the time: only a click on a combo's drop button, a
    /// `Return`, or a `Space` on a non-editable combo opens one.
    #[must_use]
    pub fn popup_for_page(&mut self, page: u32) -> Option<pdfrum_form::PopupView> {
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
    pub fn scroll_view(
        &mut self,
        annot: pdfrum_form::session::AnnotId,
    ) -> Option<pdfrum_form::ScrollView> {
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
    pub fn choose(&mut self, annot: pdfrum_form::session::AnnotId, index: usize) -> EventResponse {
        self.with_page_scripted(annot.page, |inner, ctx, cascade| {
            pdfrum_form::route::choose(inner, ctx, cascade, annot, index)
        })
    }

    /// Reports that an open dropdown was dismissed without a choice.
    ///
    /// The stored selection is left alone — a row the pointer merely rested
    /// on was never chosen. Safe to call on an annotation whose list is
    /// already shut, which answers an unconsumed response.
    pub fn close_popup(&mut self, annot: pdfrum_form::session::AnnotId) -> EventResponse {
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
    /// reachable this way — nothing a file can say opens one. Upstream the
    /// path is `CPDFSDK_BAAnnot::OnMouseEnter`
    /// (`cpdfsdk_baannot.cpp:309-312`) calling `SetPopupAnnotOpenState`
    /// (`cpdf_annot.cpp:239-243`), which is why the six
    /// `annotation_highlight_*` fixtures are bare `mousemove` scripts.
    ///
    /// The result is a **raw `/Annots` index**, the key space
    /// [`pdfrum_doc::AnnotOverlay::set_hover`] wants. Answers `None` when the
    /// pointer is over nothing, or over an annotation on another page.
    #[must_use]
    pub fn hover_for_page(&self, page: u32) -> Option<usize> {
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
    /// Answers whether the field changed.
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

    /// The session's scripting engine, when it has one — what the
    /// document's JavaScript asked the host to do, and what stopped.
    ///
    /// `Some` only for a session built by [`FormSession::with_scripts`]: a
    /// default session runs no script and a caller who passed their own
    /// [`Cascade`] to [`FormSession::with_cascade`] already holds the type
    /// they wrote.
    ///
    /// This is where `app.alert`, `Doc.submitForm`, `app.launchURL` and every
    /// other thing a script *asks for* comes back —
    /// [`transcript`](pdfrum_form::ScriptCascade::transcript) as values the
    /// host reads and decides about, never as I/O the library performs. See
    /// [`stops`](pdfrum_form::ScriptCascade::stops) for scripts that threw:
    /// they are reported and the next one still runs.
    #[cfg(feature = "script")]
    #[must_use]
    pub fn scripts(&self) -> Option<&pdfrum_form::ScriptCascade> {
        match &self.cascade {
            Cascades::Scripted(cascade) => Some(cascade),
            Cascades::Plain(_) => None,
        }
    }

    /// **Escape hatch — requires `pdfrum-form`.** The session's own record,
    /// for callers that need to read more than these methods expose.
    ///
    /// The return type is `pdfrum_form::FormSession`, which is *not* this
    /// type: this one owns a borrowed [`Document`](crate::Document) and the
    /// [`BuildContext`](crate::BuildContext) the appearances are generated
    /// through, and that one is the state machine underneath. The local alias
    /// exists so the two names do not collide inside this module; a caller
    /// reaching here writes the member crate's name in full, which is the
    /// point.
    #[must_use]
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// Routes an event that names a page.
    ///
    /// The page is read on first use and kept: a replay sends dozens of
    /// events at one page, and the `/Annots` walk is the expensive half.
    fn dispatch(&mut self, page: u32, event: Event) -> Response {
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
        page: u32,
        body: impl FnOnce(
            &mut pdfrum_form::FormSession,
            &pdfrum_form::Context<'_, pdfrum_parser::Document>,
        ) -> T,
    ) -> T {
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
        page: u32,
        body: impl FnOnce(
            &mut pdfrum_form::FormSession,
            &pdfrum_form::Context<'_, pdfrum_parser::Document>,
            &mut dyn Cascade,
        ) -> T,
    ) -> T {
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
            let page = pdfrum_form::session::FocusTarget::annot(target).page;
            return self.dispatch(page, event);
        }
        // **Tab is the exception**, and it is the reason this is not simply
        // "no focus, no keyboard": a Tab with nothing focused is what *takes*
        // focus, so refusing it here would make the ring unreachable from the
        // keyboard. It enters the ring on the page the embedder says it is
        // showing — see `set_page_in_view`, which is how this crate spells
        // the page argument the oracle puts on `FORM_OnKeyDown` itself.
        //
        // Every other key really does need a focused field, and answers
        // unhandled without one.
        match event {
            Event::KeyDown { key, .. } if key == Key::TAB => {
                self.dispatch(self.page_in_view, event)
            }
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
    fn ensure_page(&mut self, page: u32) {
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
    /// A [`FieldRef`]'s index is a **page-local** field id, allocated in
    /// first-seen order as that page's `/Annots` are walked
    /// (`pdfrum_form::page::read`). It is not a position in
    /// [`Form::fields`](crate::Form::fields), and it does not exist until the
    /// page has been read — so the install cannot happen at construction, and
    /// happens at exactly the moment the ids come into being.
    ///
    /// A [`ScriptCascade`](crate::ScriptCascade) deliberately holds no
    /// document, which is what lets the whole engine be tested against a
    /// script string and no PDF; the price is that its caller reads `/AA` and
    /// hands it over, and this is that caller.
    ///
    /// Compiled away entirely without the `script` feature: with no scripted
    /// variant to match, there is nothing to install.
    #[cfg(feature = "script")]
    fn install_page_scripts(&mut self, page: u32) {
        use pdfrum_doc::nav::AActionType;

        let Cascades::Scripted(cascade) = &mut self.cascade else {
            return;
        };
        let Some(form) = self.pages.get(&page) else {
            return;
        };
        let resolve = self.doc.parser();
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
            };
            if actions == pdfrum_form::script::FieldActions::default() {
                // Nearly every field in nearly every document: no script at
                // all, and a hook with no script takes `NoScripts`'s answer.
                continue;
            }
            cascade.set_field(
                widget.field.0,
                widget.name.clone(),
                widget.value(resolve),
                actions,
            );
        }
    }

    /// Without the `script` feature there is no cascade that can be installed
    /// into, so this is the whole of it.
    #[cfg(not(feature = "script"))]
    #[expect(
        clippy::unused_self,
        reason = "the feature-on twin takes `&mut self`; one signature, two bodies"
    )]
    fn install_page_scripts(&mut self, _page: u32) {}

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
    ///
    /// `pdfrum-form` asks only two of ISO 32000-1 table 22's eight questions,
    /// and keeps its own two-field type for them — it does not depend on
    /// `pdfrum-crypt` and has no reason to. This is where the two vocabularies
    /// meet, and it is now a field-for-field rename rather than the
    /// `bits & 0x100` / `bits & 0x20` this used to spell: the bit numbers live
    /// beside the `/P` word they decode (`docs/design/idiomatic-api.md` §A.3).
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
