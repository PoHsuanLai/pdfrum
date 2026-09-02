//! A pure-Rust PDF engine: open a file, render its pages, read its text,
//! fill its forms, write it back out.
//!
//! ```
//! use pdfrum::{Document, RenderOptions};
//!
//! let doc = Document::open("tests/fixtures/hello_world.pdf")?;
//! for page in doc.pages() {
//!     let pixmap = page.render(&RenderOptions::default())?;
//!     let text = page.text().all_text();
//!     assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
//!     assert!(text.contains("Hello, world!"));
//! }
//! # Ok::<(), pdfrum::Error>(())
//! ```
//!
//! # What this is
//!
//! `pdfrum` is a rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/)
//! — the PDF engine inside Chrome — as idiomatic Rust. Not a binding, not a
//! transliteration: a rewrite, with PDFium kept alongside as a differential
//! test oracle so the behaviour that matters is preserved even where the code
//! shares nothing.
//!
//! That behaviour is the point. Two decades of a browser opening whatever the
//! web threw at it left PDFium with a large body of undocumented recovery
//! folklore — how to find objects when the cross-reference table is a lie,
//! what to do with a stream whose `/Length` is wrong, which of a damaged
//! page tree's contradictions to believe. Reimplementing PDF from the spec
//! gets you a reader for files that are correct. Porting *that* gets you one
//! for files that exist. Every release is measured against the oracle over a
//! corpus of 1675 real and deliberately-broken PDFs.
//!
//! **Pure Rust, all the way down.** No C or C++ is compiled into any library
//! build and no `-sys` crate appears anywhere in the tree — enforced in CI
//! rather than promised. `unsafe` is forbidden in every crate.
//!
//! # JavaScript is off by default
//!
//! A PDF may carry scripts for form validation, calculation and formatting.
//! **With default features this crate reads them as data and never runs
//! them** — `scripts/check-no-boa.nu` asserts that no JavaScript engine is
//! anywhere in the dependency tree of a default `cargo add pdfrum`, so it is
//! a checked property rather than a promise. A field whose displayed value
//! would come from a calculation script reads as whatever the file last
//! stored.
//!
//! That is the *default* because an engine which executes untrusted script
//! out of a document is a different security proposition from a renderer, and
//! most PDF work does not need one. It is not a permanent absence: the
//! `script` feature turns it on. See [§Features](#features).
//!
//! # What this deliberately is not
//!
//! - **No XFA.** Adobe's XML forms architecture is a second, largely
//!   disjoint document format that happens to travel inside a PDF wrapper.
//!   PDFium implements it in a subsystem the size of the rest of the engine.
//!   Out of scope, permanently.
//! - **Not a viewer.** There is no window, no scrolling, no text caret and no
//!   interactive widget behaviour. This crate turns pages into pixels and
//!   text into strings; what you do with them is yours.
//!
//! # Where to start
//!
//! - [`Document`] — open a file, and reach everything that belongs to the
//!   document as a whole: [page count](Document::page_count),
//!   [metadata](Document::metadata), [outline](Document::outline),
//!   [form](Document::form), [attachments](Document::attachments),
//!   [diagnostics](Document::diagnostics) and the document-wide
//!   [running total](Document::all_diagnostics).
//! - [`Page`] — [render](Page::render) it, [read its text](Page::text), list
//!   its [annotations](Page::annotations) and [links](Page::links), ask for
//!   its [boxes](Page::crop_box) and [rotation](Page::rotation).
//! - [`TextPage`] — search, select, and pull the web links out of a page's
//!   text.
//! - [`Form`] — enumerate fields, read them, fill them, and save the result.
//! - [`Document::save`] and friends — write a document back out, whole or as
//!   an incremental update, and [copy pages](Document::import_pages) between
//!   documents.
//!
//! # Features
//!
//! One, and it is off.
//!
//! - **`script`** — run the document's own JavaScript. It brings a pure-Rust
//!   engine ([boa](https://boajs.dev/), pinned exactly, and the ~116 crates
//!   behind it; zero `-sys`, `cargo deny` clean — DEPS.md records the audit)
//!   and adds `ScriptCascade`, `ScriptConfig` and
//!   `FormSession::with_scripts` — all three exist only with the feature on.
//!   Send a session built that way the events you would send anyway, and the
//!   document's own `/AA` scripts run on them:
//!
//!   ```text
//!   pdfrum = { version = "0.1", features = ["script"] }
//!   ```
//!
//!   **What a script can and cannot reach today.** The `AF*` library
//!   (`AFNumber_Format`, `AFDate_*`, `AFSimple_Calculate`, …), `util`,
//!   `app.alert` and the `event` object are bound and the four field hooks —
//!   keystroke, validate, calculate, format — run; the `Doc`/`Field` object
//!   model is **not built yet**, so a script that calls `this.getField(…)`
//!   throws, and 11 of the oracle's 47 JavaScript fixtures reproduce
//!   byte-exactly. See PLAN.md §M15 and `docs/status/M15.md`. Do not enable
//!   this expecting Acrobat.
//!
//!   Nothing a script asks for is performed by this crate: `app.alert`,
//!   `Doc.submitForm` and `app.launchURL` come back as values on
//!   `ScriptCascade::transcript` for the host to decide about, and no socket,
//!   file or process is reachable from a script at all.
//!
//! # Damage is not an error
//!
//! A broken file is the normal case, so recovery is a *channel* rather than a
//! failure. Opening a document whose cross-reference table had to be rebuilt
//! by scanning succeeds, and says so through [`Document::diagnostics`]; a
//! content stream that ends mid-operator renders everything before the
//! damage. [`Error`] is reserved for "no answer can be produced" — a file
//! that is not a PDF, a password that does not open it, a page index that
//! does not exist.
//!
//! Reading is lazy, so damage surfaces late: a bad `/Length` on page 400 is
//! found when page 400 is rendered, not when the file opens.
//! [`Document::diagnostics`] is the load-time snapshot;
//! [`Document::all_diagnostics`] is the running total over everything the
//! document has needed since — ask it *after* the work.
//!
//! # Rendering pages in parallel
//!
//! Every type here is `Send + Sync`, so rayon works with no ceremony beyond
//! adding it to your own `Cargo.toml`:
//!
//! ```
//! use rayon::prelude::*;
//! use pdfrum::{Document, RenderOptions};
//!
//! let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
//!
//! let pages: Vec<_> = doc.pages().collect();
//! let rendered: Vec<_> = pages
//!     .par_iter()
//!     .map(|page| page.render(&RenderOptions::default()))
//!     .collect::<Result<_, _>>()?;
//!
//! assert_eq!(rendered.len(), 2);
//! # Ok::<(), pdfrum::Error>(())
//! ```
//!
//! The document is shared by reference and each page renders independently.
//! For a long document, give each worker its own [`BuildContext`] with
//! [`Page::render_with`] so fonts and images are decoded once per *thread*
//! rather than once per page — `rayon`'s `map_init` does exactly this:
//!
//! ```
//! use rayon::prelude::*;
//! use pdfrum::{BuildContext, Document, RenderOptions};
//!
//! let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
//! let pages: Vec<_> = doc.pages().collect();
//!
//! let rendered: Vec<_> = pages
//!     .par_iter()
//!     .map_init(BuildContext::new, |ctx, page| {
//!         page.render_with(&RenderOptions::default(), ctx)
//!     })
//!     .collect::<Result<_, _>>()?;
//!
//! assert_eq!(rendered.len(), 2);
//! # Ok::<(), pdfrum::Error>(())
//! ```
//!
//! The cache is per-thread rather than shared because it is reached through
//! `&mut` — sharing one behind a lock would serialize the very work the
//! parallelism is for.
//!
//! [`BuildContext`] caches what a page is *built* from. A page is also
//! *drawn*, and the rasterizer's flattened glyph outlines are a second cache
//! that a per-page render throws away. [`RenderSession`] carries both; use it
//! with [`Page::render_session`] wherever you would have used
//! `BuildContext`:
//!
//! ```
//! use rayon::prelude::*;
//! use pdfrum::{Document, RenderOptions, RenderSession};
//!
//! let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
//! let pages: Vec<_> = doc.pages().collect();
//!
//! let rendered: Vec<_> = pages
//!     .par_iter()
//!     .map_init(RenderSession::new, |session, page| {
//!         page.render_session(&RenderOptions::default(), session)
//!     })
//!     .collect::<Result<_, _>>()?;
//!
//! assert_eq!(rendered.len(), 2);
//! # Ok::<(), pdfrum::Error>(())
//! ```
//!
//! # This crate composes, it does not compute
//!
//! Everything here is a thin, ergonomic surface over a stack of member
//! crates, each of which is a public API in its own right and none of which
//! this crate hides. When you need something this surface does not offer —
//! a dictionary key, an intermediate representation, a knob — reach past it:
//! [`Document::parser`], [`Page::objects`], [`PageEdit::graph`] and
//! [`Annotation::dict`] are the documented escape hatches, and
//! `pdfrum-parser`, `pdfrum-page`, `pdfrum-render`, `pdfrum-text`,
//! `pdfrum-doc` and `pdfrum-edit` are all there to be used directly.
//!
//! Reaching past is the *only* thing that needs a second dependency. Every
//! type this crate's own signatures name is re-exported here, errors and
//! their payloads included, so a caller who stays on this surface never has
//! to add a crate merely to write a type down. `tests/reexports.rs` holds
//! that property to a compile test.

#![forbid(unsafe_code)]

mod annotation;
mod document;
mod edit;
mod error;
mod form;
mod form_session;
mod outline;
mod page;
mod render;
mod save;
mod session;

pub use annotation::{AnnotFlags, Annotation, Subtype};
pub use document::{Attachment, Document, Metadata, OpenOptions};
pub use edit::{ImageBuilder, PageEdit, PathBuilder, TextBuilder};
pub use error::{Error, Result};
pub use form::{Field, FieldFlags, FieldKind, Form};
pub use form_session::{
    AppearanceUpdate, Cascade, EventModifiers, EventResponse, FieldRef, FieldWrites, FormSession,
    Keystroke, KeystrokeOutcome, MouseButton, NoScripts, SessionConfig, UpdateKind, VirtualKey,
};
// The viewer chrome a host draws for itself: an open combo dropdown and a
// scrolled choice widget. Values, not a trait — see `FormSession::popup_for_page`
// and STYLE.md §2b's 2026-09-01 ruling for why this is state a caller pulls
// rather than a seam the library calls back through.
pub use outline::{Bookmark, Outline};
pub use page::{Page, Rotation};
pub use pdfrum_form::session::AnnotId;
pub use pdfrum_form::tab::Rect as FormRect;
pub use pdfrum_form::{Placement, PopupGeometry, PopupView, ScrollView};

/// The `boa`-backed [`Cascade`] and what a caller needs to build and read one
/// — behind the default-off `script` feature.
///
/// [`ScriptCascade`] is [`Cascade`]'s second implementation; hand one to
/// [`FormSession::with_cascade`], or — better — let
/// [`FormSession::with_scripts`] build it *and* install the document's own
/// `/AA` scripts into it. [`ScriptBuildError`] is what
/// [`ScriptCascade::new`](pdfrum_form::ScriptCascade::new) can return and
/// [`ScriptFailure`] is what [`stops`](pdfrum_form::ScriptCascade::stops)
/// hands back, so both are nameable here rather than only through
/// `pdfrum-form`.
#[cfg(feature = "script")]
pub use pdfrum_form::script::{
    BuildError as ScriptBuildError, FieldActions, ScriptFailure, ScriptStop,
};
#[cfg(feature = "script")]
pub use pdfrum_form::{ScriptCascade, ScriptConfig, TranscriptLine};
pub use render::{ColorMode, ColorScheme, Pixmap, RenderOptions, TextAa};

/// The rasterizer seam, re-exported so a caller can write
/// [`Page::render_on`]'s bound without adding `pdfrum-render` to their
/// manifest.
///
/// `RasterBackend` makes targets and reads their pixels back;
/// `RenderDevice` is the target itself, the six drawing calls the engine
/// issues. A backend crate implements both, and this crate never needs to
/// know which one you passed.
pub use pdfrum_render::{RasterBackend, RenderDevice};

/// The facade's default rasterizer, re-exported so a caller can name it
/// without a second dependency.
///
/// [`Page::render`] is [`Page::render_on`] with this. It lives in
/// `pdfrum-raster-vello-cpu`, which is the facade's one rasterizer
/// dependency; `tiny-skia`, the AGG-parity backend and the GPU one are the
/// caller's own, named directly at the call site.
pub use pdfrum_raster_vello_cpu::VelloCpuBackend;
pub use save::{SaveOptions, Update};
pub use session::RenderSession;

/// The things a page draws, as the interpreter produced them.
///
/// Returned by [`Page::objects`] and [`PageEdit::objects`], and the currency
/// of the editing API: [`PathBuilder`], [`TextBuilder`] and [`ImageBuilder`]
/// each build one.
pub use pdfrum_page::PageObject;

/// Per-document caches — fonts, colour spaces, decoded images — that a caller
/// threads through many pages to avoid decoding the same resource twice.
///
/// Reached through [`Page::render_with`], [`Page::text_with`] and
/// [`FormSession::with_context`]. See the crate docs on parallel rendering
/// for why it is per-thread.
pub use pdfrum_page::BuildContext;

/// Which face a **non-embedded** font resolves to: the directories enumerated
/// and whether the Croscore families stand in for Arial, Times and Courier.
///
/// Settled once, when a [`BuildContext`] is made with
/// [`BuildContext::with_substitution`], because it must not vary across one
/// document. A caller that renders through such a context must start its
/// [`FormSession`] through [`FormSession::with_context`] with the same one:
/// the session lays out carets and selection bands from the *substituted*
/// face's ascent and descent, and a second substitution over one document
/// puts them at heights the page is not drawn at.
pub use pdfrum_font::SubstitutionOptions;

/// One page's extracted text: the characters in reading order, plus search,
/// selection and link queries over them.
///
/// Returned by [`Page::text`].
pub use pdfrum_text::{CharBox, FindOptions, TextPage, WebLink};

/// A link annotation, and where it points.
///
/// Returned by [`Page::links`].
pub use pdfrum_doc::{Action, ActionKind, Dest, Link};

/// Everything a document reported repairing, working around, or refusing.
///
/// Returned by [`Document::diagnostics`].
pub use pdfrum_common::{DiagKind, Diagnostic, Diagnostics, Limits, Severity};

/// The version a header declares, and the one a save writes back.
///
/// Named by [`Document::version`] and [`SaveOptions::version`].
pub use pdfrum_common::PdfVersion;

/// What a document's security handler permits — eight questions, not a
/// bitfield.
///
/// Returned by [`Document::permissions`] and
/// [`Document::owner_permissions`].
pub use pdfrum_crypt::Permissions;

/// 2D geometry — `Rect`, `Point`, `Affine`, `BezPath`.
///
/// Re-exported so callers need not depend on `kurbo` themselves to name the
/// types this crate's signatures use.
pub use kurbo;

/// Colour and brush vocabulary — `Color`, `BlendMode`.
///
/// Re-exported for the same reason as [`kurbo`].
pub use peniko;

// ---------------------------------------------------------------------------
// Every type a signature in this crate names, nameable from this crate.
//
// A `cargo add pdfrum` caller must be able to write a type annotation for
// every value they receive. Nothing below is a new capability — each is a type
// that already appeared in a public signature here and that only a second
// dependency could spell. `tests/reexports.rs` compiles against this block the
// way a caller would, from `pdfrum::*` and nothing else, and fails to compile
// if a signature grows a type this block does not carry.
//
// Two resolutions exist and they are not mixed. A type a normal caller matches
// on or stores is re-exported here. A type only an escape hatch returns keeps
// its namespaced spelling at the call site — `Document::parser`,
// `PageEdit::graph`, `PageEdit::graph_mut` — so the collision with this
// crate's own `Document` and `Page` stays visible, and the method's first
// rustdoc sentence says which crate to add.
// ---------------------------------------------------------------------------

/// The error behind [`Error::Doc`] — see [`OpenError`] for the naming.
pub use pdfrum_doc::Error as DocError;
/// The error behind [`Error::Save`] — see [`OpenError`] for the naming.
pub use pdfrum_edit::Error as SaveError;
/// The error [`Document::fetch`] returns — see [`OpenError`] for the naming.
pub use pdfrum_object::Error as ObjectError;
/// The error behind [`Error::Read`] — see [`OpenError`] for the naming.
pub use pdfrum_parser::Error as ReadError;
/// The seven errors [`Error`] wraps, each under the name of the *domain* its
/// variant is called by.
///
/// The enum's shape was already right — one error, domain variants,
/// `thiserror`, `#[non_exhaustive]` — but its payloads were not reachable: a
/// caller could match `Error::Open(_)` and `Display` what was inside, and
/// could not write the type to inspect it, which is the entire reason a
/// variant carries a payload at all.
///
/// # Why they are renamed rather than re-exported under their own names
///
/// All seven member crates spell their error type `Error`, per STYLE.md §4's
/// one-error-per-crate rule, so seven of them cannot share this crate's
/// namespace and `pdfrum::Error` already occupies the name. The scheme is
/// **the variant's own name plus `Error`** — [`Error::Open`] wraps
/// [`OpenError`], [`Error::Read`] wraps [`ReadError`], and so on — chosen over
/// the alternative of naming them after their crates (`ParserError`,
/// `EditError`) because the variants deliberately name domains rather than
/// crates, and a caller who reads `Error::Save(e)` should not have to learn
/// that saving lives in `pdfrum-edit` to write `e`'s type. The one exception
/// is [`ObjectError`], which is not a variant of [`Error`] at all: it is what
/// [`Document::fetch`] returns, and it keeps its crate's noun because there is
/// no domain word to borrow.
pub use pdfrum_parser::LoadError as OpenError;
/// The error behind [`Error::Render`] — see [`OpenError`] for the naming.
pub use pdfrum_render::Error as RenderError;
/// The error behind [`Error::Text`] — see [`OpenError`] for the naming.
pub use pdfrum_text::Error as TextError;

/// A straight (non-premultiplied) 32-bit draw colour: the type all four of
/// [`ColorScheme`]'s fields are.
///
/// Without this, [`ColorMode::Forced`] was unconstructible from this crate —
/// `ColorScheme` has no `Default` and no constructor, so there was no
/// expression a caller could write that produced one. The variant was visible
/// in the rustdoc and unreachable from the API.
///
/// ```
/// use pdfrum::{Argb, ColorMode, ColorScheme, Document, RenderOptions};
///
/// // Black on white, forced over whatever the file's own colours are.
/// let black = Argb { a: 255, r: 0, g: 0, b: 0 };
/// let white = Argb { a: 255, r: 255, g: 255, b: 255 };
///
/// let options = RenderOptions {
///     color_mode: ColorMode::Forced(ColorScheme {
///         path_fill: black,
///         path_stroke: black,
///         text_fill: black,
///         text_stroke: white,
///     }),
///     ..RenderOptions::default()
/// };
///
/// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
/// let pixmap = doc.page(0)?.render(&options)?;
/// assert!(pixmap.width() > 0);
/// # Ok::<(), pdfrum::Error>(())
/// ```
///
/// Note that [`peniko::Color`] is the vocabulary [`RenderOptions::background`]
/// speaks; `Argb` is the engine's own resolved-colour byte quartet and is
/// what a forced scheme substitutes. They are not interchangeable and this
/// crate does not convert between them.
pub use pdfrum_render::Argb;

/// Which annotation on a page holds the keyboard focus, and what rectangle to
/// stroke over it.
///
/// Returned by [`FormSession::focus_for_page`], which a renderer asks once per
/// page per frame (SPEC §15.8) — so this is an ordinary answer a caller acts
/// on, not an escape hatch. [`FocusBox`] is the second half and is useless
/// without the first.
pub use pdfrum_doc::{Focus, FocusBox};

/// A generated appearance stream and the dictionary edits it implies.
///
/// The payload of [`UpdateKind::Regenerated`] and [`UpdateKind::LiveEdit`],
/// which is to say the payload of the ordinary [`EventResponse`] every form
/// event returns. A caller that draws a form reads `stream`, `bbox` and
/// `resources` off this on every keystroke.
pub use pdfrum_doc::GeneratedAp;

/// What a form event is, before [`FormSession`] routes it.
///
/// The session's `on_*` methods each build one of these and apply it; this is
/// the same vocabulary as a value, for a caller that has its own event queue
/// and would rather hand over a whole event than call the method that matches
/// it.
pub use pdfrum_form::Event;

/// An indirect-object reference — an object number and a generation.
///
/// The currency of the editing API: [`TextBuilder::font`] and
/// [`ImageBuilder::source`] are each one, [`PageEdit::font_of`] and
/// [`PageEdit::image_of`] each return one, and [`Document::fetch`] takes one.
pub use pdfrum_object::ObjRef;

/// A PDF object, and the dictionary type that is one of its variants.
///
/// `Object` is what [`Document::fetch`] hands back and `Dict` is what
/// [`Annotation::dict`] and [`GeneratedAp::resources`] are. `Dict` is here
/// rather than behind an escape hatch because `GeneratedAp` carries one and
/// that is an ordinary payload.
pub use pdfrum_object::{Dict, Object};

/// Indirect-object lookup — the trait [`Document`] implements and
/// [`Document::fetch`] comes from.
///
/// Re-exported so a caller can write the bound. Reaching for `fetch` is an
/// escape hatch; being unable to *name* the trait it lives on would make it a
/// dead end instead.
pub use pdfrum_object::Resolve;

/// Whether a text object is filled, stroked, clipped, both, or neither
/// (ISO 32000-1 §9.3.6, the `Tr` operator).
///
/// The type of [`TextBuilder::render_mode`].
pub use pdfrum_page::TextRenderMode;

/// The page rotation `pdfrum-page` reports, which this crate converts from.
///
/// Renamed because this crate has its own [`Rotation`] and the `From` impl
/// between them is public — so its *source* type had to be nameable or the
/// conversion could be seen in the rustdoc and not written. A caller who has
/// only this crate wants [`Rotation`]; this is here for one who reached
/// through [`Page::objects`] and needs to come back.
pub use pdfrum_page::Rotation as PageRotation;

/// The flattened glyph outlines the rasterizer draws, cached across pages.
///
/// The type of [`RenderSession::caches`], which stays a public field: it is
/// half of a two-field record whose whole purpose is that a caller can reach
/// either half on its own (STYLE.md §1 — data, not an object), and hiding it
/// behind an accessor would buy nothing while making the pair asymmetric with
/// [`RenderSession::build`], whose type was already re-exported.
pub use pdfrum_render::RenderCaches;

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole parallel-rendering story rests on. It is
    /// asserted rather than assumed because it is load-bearing: a member
    /// crate that grew a `Rc` or a `Cell` would break every rayon caller,
    /// and would do it at *their* compile time rather than ours.
    #[test]
    fn the_public_types_are_send_and_sync() {
        fn send_sync<T: Send + Sync>() {}
        // `BuildContext` is `Send` but is used through `&mut`, so it travels
        // to a worker rather than being shared with one.
        fn send<T: Send>() {}

        send_sync::<Document>();
        send_sync::<Pixmap>();
        send_sync::<TextPage>();
        send_sync::<Error>();
        send_sync::<RenderOptions>();
        send_sync::<SaveOptions>();
        send_sync::<Metadata>();
        send::<BuildContext>();
        // Same story as `BuildContext`: reached through `&mut`, so it travels
        // to a rayon worker rather than being shared with one.
        send::<RenderSession>();
    }

    #[test]
    fn the_borrowing_types_are_send_and_sync_too() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Page<'static>>();
        send_sync::<Annotation<'static>>();
        send_sync::<Form<'static>>();
        send_sync::<Outline<'static>>();
        send_sync::<Bookmark<'static>>();
    }
}
