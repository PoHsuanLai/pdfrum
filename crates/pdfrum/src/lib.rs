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
//! # What this deliberately is not
//!
//! - **No JavaScript.** A PDF may carry scripts for form validation,
//!   calculation and formatting. This crate reads them as data and never runs
//!   them. That is a permanent scope decision, not a missing feature: an
//!   engine that executes untrusted script from a document is a different
//!   security proposition, and the overwhelming majority of PDF work does not
//!   need it. A field whose displayed value would come from a calculation
//!   script reads here as whatever the file last stored.
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
//! [`Document::parser`], [`Page::objects`] and [`Annotation::dict`] are the
//! documented escape hatches, and `pdfrum-parser`, `pdfrum-page`,
//! `pdfrum-render`, `pdfrum-text`, `pdfrum-doc` and `pdfrum-edit` are all
//! there to be used directly.

#![forbid(unsafe_code)]

mod annotation;
mod document;
pub mod edit;
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
    AppearanceUpdate, EventModifiers, EventResponse, FormSession, MouseButton, SessionConfig,
    UpdateKind, VirtualKey,
};
pub use outline::{Bookmark, Outline};
pub use page::{Page, Rotation};
pub use render::{Backend, ColorMode, ColorScheme, Pixmap, RenderOptions, TextAa};
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

/// 2D geometry — `Rect`, `Point`, `Affine`, `BezPath`.
///
/// Re-exported so callers need not depend on `kurbo` themselves to name the
/// types this crate's signatures use.
pub use kurbo;

/// Colour and brush vocabulary — `Color`, `BlendMode`.
///
/// Re-exported for the same reason as [`kurbo`].
pub use peniko;

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
