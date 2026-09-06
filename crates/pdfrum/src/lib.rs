//! A pure-Rust PDF engine: open a file, render its pages, read its text,
//! fill its forms, write it back out.
//!
//! ```
//! use pdfrum::{Document, RenderOptions, VelloCpuBackend};
//!
//! let doc = Document::open("tests/fixtures/hello_world.pdf")?;
//! for page in doc.pages() {
//!     let pixmap = page.render(&VelloCpuBackend::new(), &RenderOptions::default())?;
//!     let text = page.text().to_string();
//!     assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
//!     assert!(text.contains("Hello, world!"));
//! }
//! # Ok::<(), pdfrum::Error>(())
//! ```
//!
//! A rewrite of [PDFium](https://pdfium.googlesource.com/pdfium/) — Chrome's
//! PDF engine — with PDFium kept alongside as a differential test oracle. No
//! C or C++ is compiled in and `unsafe` is forbidden. Deliberately absent,
//! permanently: XFA, and any viewer behaviour — this turns pages into pixels
//! and text into strings. Everything under this facade is a public crate in
//! its own right; [`Document::parser`], [`Page::objects`], [`PageEdit::graph`]
//! and [`Annotation::dict`] are the documented escape hatches.
//!
//! # Where to start
//!
//! - [`Document`] — open a file; pages, metadata, outline, form, attachments,
//!   [diagnostics](Document::diagnostics).
//! - [`Page`] — [render](Page::render), [text](Page::text), annotations, links,
//!   boxes.
//! - [`TextPage`] — search, select, and web links.
//! - [`Form`] — enumerate, read, fill; [`FormSession`] for live events.
//! - [`Document::save`] — write back; [`Document::edit`] embeds fonts for
//!   [`TextBuilder`].
//!
//! # JavaScript is off by default
//!
//! A PDF may carry scripts. With default features they are read as data and
//! never run. The `javascript` feature turns them on behind a pure-Rust engine
//! (boa), with the `Doc`/`Field` object model, the `/AA` event path and
//! timers; what a script asks the host for comes back as a value to decide
//! about, never an action taken.
//!
//! # Damage is not an error
//!
//! Recovery is a channel, not a failure. Opening a file whose cross-reference
//! table had to be rebuilt succeeds and says so through
//! [`Document::diagnostics`]; [`Error`] is reserved for "no answer can be
//! produced". Reading is lazy, so [`Document::all_diagnostics`] is the running
//! total — read it *after* the work. Every type here is `Send + Sync`; for a
//! long document give each rayon worker its own [`RenderSession`].

#![forbid(unsafe_code)]
// The crate docs link to `Form`, `PageEdit`, `Document::save` and their
// kin, which only exist with `forms` and `edit` on. A headless build
// (`--no-default-features`) still has the prose; the links become plain
// text rather than a warning under `-D warnings`.
#![cfg_attr(
    not(all(feature = "edit", feature = "forms")),
    allow(rustdoc::broken_intra_doc_links)
)]

mod annotation;
#[cfg(feature = "edit")]
mod attach;
#[cfg(feature = "edit")]
mod canvas;
mod document;
#[cfg(feature = "edit")]
mod edit;
mod error;
#[cfg(feature = "edit")]
mod flatten;
#[cfg(feature = "forms")]
mod form;
#[cfg(feature = "forms")]
mod form_session;
mod outline;
#[cfg(feature = "forms")]
mod owned_form;
mod owned_page;
mod page;
mod profile;
mod render;
#[cfg(feature = "edit")]
mod save;
mod session;
mod signature;
#[cfg(feature = "edit")]
mod stamp;
#[cfg(feature = "svg-ingest")]
mod svg_ingest;
#[cfg(feature = "svg-text")]
mod svg_text;
mod thumbnail;

pub use annotation::{AnnotFlags, Annotation, Subtype};
/// An SVG compiled once into a Form `XObject`: one object, placed on any
/// number of pages by [`Canvas::place_svg`].
#[cfg(feature = "svg-ingest")]
pub use canvas::SvgForm;
/// Drawing on an existing page: [`Canvas`] from [`DocEdit::draw_page`],
/// with [`Paint`], [`Stroke`] and [`Fill`] saying how a shape is painted and
/// [`LineCap`], [`LineJoin`], [`MiterLimit`] and [`Dash`] shaping the pen.
#[cfg(feature = "edit")]
pub use canvas::{Canvas, Dash, Fill, LineCap, LineJoin, MiterLimit, Paint, Stroke};
pub use document::{
    Attachment, Document, EmbeddedFontFile, FontFileKind, Metadata, OpenOptions,
    OpenOptionsBuilder, Revision, UnknownFontFileKind,
};
#[cfg(feature = "edit")]
pub use edit::{ImageBuilder, PageEdit, PathBuilder, TextBuilder};
pub use error::{Error, ErrorCode, Result, UnknownErrorCode};
#[cfg(feature = "edit")]
pub use flatten::{FlattenMode, Flattened, UnknownFlattenMode};
#[cfg(feature = "forms")]
pub use form::{Field, FieldFlags, FieldKind, Form, UnknownField};
#[cfg(feature = "forms")]
pub use form_session::{
    AppearanceUpdate, Button, Cascade, FieldRef, FieldWrites, FormSession, Key, Keystroke,
    KeystrokeOutcome, Modifiers, NoScripts, Response, SessionConfig, UpdateKind,
};
// The viewer chrome a host draws for itself: an open combo dropdown and a
// scrolled choice widget. Values, not a trait — see `FormSession::popup_for_page`
// and STYLE.md §2b's 2026-09-01 ruling for why this is state a caller pulls
// rather than a seam the library calls back through.
pub use outline::{Bookmark, Outline, OutlineIter};
#[cfg(feature = "forms")]
pub use owned_form::OwnedFormSession;
pub use owned_page::OwnedPage;
pub use page::{
    ImageEncoding, LinkTarget, Page, PageImage, PageLink, PreparedPage, RawImage,
    UnknownImageEncoding,
};
#[cfg(feature = "forms")]
pub use pdfrum_form::AnnotId;
#[cfg(feature = "forms")]
pub use pdfrum_form::{Placement, PopupGeometry, PopupView, ScrollView};
#[cfg(feature = "markdown")]
pub use pdfrum_markdown::Block;
/// Rendering Markdown [`Block`]s: [`markdown::render`](markdown::render()) as
/// [`Page::markdown`] does it, and [`markdown::render_with_images`] with
/// each [`Block::Image`] linked to a file the caller wrote — the pair a
/// caller of [`Page::markdown_blocks`] or [`Document::markdown_blocks`]
/// needs, and nothing else of the crate behind them.
#[cfg(feature = "markdown")]
pub mod markdown {
    pub use pdfrum_markdown::{render, render_with_images};
}
/// SVG export: what [`Page::to_svg`] returns, and the [`RasterBackend`]
/// wrapper underneath it for a caller driving the walk themselves — behind the
/// default-off `svg` feature.
///
/// The conversion itself is a method on [`Page`], not a free function here,
/// because it takes this crate's [`RenderOptions`] rather than the engine's;
/// `pdfrum-svg`'s own `page_to_svg` is the entry point for a caller who
/// already holds a page-object graph.
#[cfg(feature = "svg")]
pub mod svg {
    pub use pdfrum_svg::{RasterCause, RasterRegion, RasterReport, SvgBackend, SvgDevice, SvgPage};
}

/// The `boa`-backed [`Cascade`] and what a caller needs to build and read one
/// — behind the default-off `script` feature.
///
/// [`ScriptCascade`] is [`Cascade`]'s second implementation; let
/// [`FormSession::with_scripts`] build it *and* install the document's own
/// `/AA` scripts into it. [`ScriptBuildError`] is what
/// [`ScriptCascade::new`](pdfrum_form::ScriptCascade::new) can return and
/// [`ScriptFailure`] is what [`stops`](pdfrum_form::ScriptCascade::stops)
/// hands back.
#[cfg(feature = "javascript")]
pub use pdfrum_form::script::{
    BuildError as ScriptBuildError, FieldActions, ScriptFailure, ScriptStop,
};
#[cfg(feature = "javascript")]
pub use pdfrum_form::{ScriptCascade, ScriptConfig, TranscriptLine};
pub use render::{ColorMode, ColorScheme, Pixmap, RenderOptions, RenderOptionsBuilder, TextAa};

/// The rasterizer seam, re-exported so a caller can write
/// [`Page::render_on`]'s bound without adding `pdfrum-render` to their
/// manifest.
///
/// `RasterBackend` makes targets and reads their pixels back;
/// `RenderDevice` is the target itself, the six drawing calls the engine
/// issues. A backend crate implements both, and this crate never needs to
/// know which one you passed.
pub use pdfrum_render::{RasterBackend, RenderDevice};

/// A font this save is adding: [`EmbeddedFont`] from [`DocEdit::embed_font`],
/// [`FontEncoding`] choosing simple vs composite, and [`StandardFont`] for
/// [`DocEdit::standard_font`].
#[cfg(feature = "edit")]
pub use pdfrum_edit::{EmbeddedFont, FontEncoding, MissingGlyph, StandardFont};

/// What [`DocEdit::add_attachment`] writes beside the name and the bytes.
#[cfg(feature = "edit")]
pub use attach::{AttachmentOptions, AttachmentOptionsBuilder};
/// A `SystemTime` as the PDF date string [`AttachmentOptions::modified`] and
/// [`Metadata`]'s two dates carry.
#[cfg(feature = "edit")]
pub use pdfrum_edit::pdf_date;
/// An image this save is adding: [`EmbeddedImage`] from
/// [`DocEdit::embed_jpeg`] or [`DocEdit::embed_image`], and [`PixelFormat`]
/// naming the layout of raw samples handed to the latter.
#[cfg(feature = "edit")]
pub use pdfrum_edit::{EmbeddedImage, PixelFormat};
#[cfg(feature = "edit")]
pub use pdfrum_edit::{Encryption, IdSource, PageBox};
/// The AGG-parity rasterizer, behind the `agg` feature.
#[cfg(feature = "agg")]
pub use pdfrum_raster_agg::AggBackend;
/// The `tiny-skia` rasterizer, behind the `tinyskia` feature.
#[cfg(feature = "tinyskia")]
pub use pdfrum_raster_tinyskia::TinySkiaBackend;
/// The GPU rasterizer over `wgpu`, behind the `vello-gpu` feature, which no
/// default build carries.
#[cfg(feature = "vello-gpu")]
pub use pdfrum_raster_vello::VelloBackend as VelloGpuBackend;
/// The default rasterizer, behind the `vello-cpu` feature that is on by
/// default: what a caller passes to [`Page::render`] unless it chose another.
#[cfg(feature = "vello-cpu")]
pub use pdfrum_raster_vello_cpu::VelloCpuBackend;
#[cfg(feature = "edit")]
pub use save::{DocEdit, SaveOptions, SaveOptionsBuilder, UnknownUpdate, Update};
pub use session::RenderSession;
pub use signature::Signature;
/// A mark on every page: what [`DocEdit::stamp_text`] and
/// [`DocEdit::stamp_image`] draw, and where.
#[cfg(feature = "edit")]
pub use stamp::{StampOptions, StampOptionsBuilder, StampPosition, UnknownStampPosition};
/// SVG drawn into a page as vectors: [`Canvas::draw_svg`] inline,
/// [`DocEdit::compile_svg`] once into a reusable [`SvgForm`], how each is
/// placed, and everything neither could carry.
#[cfg(feature = "svg-ingest")]
pub use svg_ingest::{SvgFit, SvgIngestReport, Unsupported, UnsupportedItem};
/// The faces an ingested SVG's `<text>` is set in, and what it becomes.
#[cfg(feature = "svg-text")]
pub use svg_text::SvgFonts;

/// The things a page draws, as the interpreter produced them.
///
/// Returned by [`Page::objects`] and [`PageEdit::objects`], and the currency
/// of the editing API: [`PathBuilder`], [`TextBuilder`] and [`ImageBuilder`]
/// each build one.
pub use pdfrum_page::PageObject;

/// An object index that is not on the page being edited.
///
/// Returned by [`PageEdit::insert`], [`PageEdit::show`], [`PageEdit::hide`]
/// and [`PageEdit::transform`].
pub use pdfrum_page::IndexOutOfRange;

/// Per-document caches — fonts, colour spaces, decoded images — that a caller
/// threads through many pages to avoid decoding the same resource twice.
///
/// Reached through [`RenderSession::build`], and directly through
/// [`FormSession::with_context`]. See the crate docs on parallel rendering
/// for why it is per-thread.
pub use pdfrum_page::BuildContext;

/// Which face a **non-embedded** font resolves to: the directories enumerated
/// and whether the Croscore families stand in for Arial, Times and Courier.
///
/// Settled once, when a [`BuildContext`] is made with
/// [`BuildContext::with_substitution`], because it must not vary across one
/// document — a caller that renders through such a context must start its
/// [`FormSession`] through [`FormSession::with_context`] with the same one.
pub use pdfrum_font::SubstitutionOptions;

/// One page's extracted text: the characters in reading order, plus search,
/// selection and link queries over them.
///
/// Returned by [`Page::text`].
pub use pdfrum_text::{
    CharBox, CharIndex, FindOptions, IndexMap, TextIndex, TextPage, WebLink, Word,
};

/// A link annotation, and where it points.
///
/// Returned by [`Page::links`].
pub use pdfrum_doc::{Action, ActionKind, Dest, Link};

/// PDF/A conformance checking (ISO 19005): the level asked for, the report,
/// and the clause and subject of each requirement failed.
///
/// Returned by [`Document::check_pdfa`]. In the default feature set — the
/// checker reads the object graph the parser already builds and adds no
/// dependency to any tree, so there is nothing for a feature to gate.
pub use pdfrum_doc::pdfa::{
    Clause as PdfaClause, Level as PdfaLevel, Report as PdfaReport, Subject as PdfaSubject,
    Violation as PdfaViolation,
};

/// Everything a document reported repairing, working around, or refusing.
///
/// Returned by [`Document::diagnostics`].
pub use pdfrum_common::{
    Deadline, DiagKind, Diagnostic, Diagnostics, LimitExceeded, Limits, Operation, Severity,
};

/// The version a header declares, and the one a save writes back.
///
/// Named by [`Document::version`] and [`SaveOptions::version`].
pub use pdfrum_common::PdfVersion;

/// A page's zero-based position in the document.
///
/// Named by [`Document::page`], [`Page::index`], [`Bookmark::page_index`],
/// every [`FormSession`] method that routes to a page, and
/// [`Document::import_pages`]. `From<u32>` means `doc.page(0)` still reads as
/// it always has.
pub use pdfrum_common::PageIndex;

/// What a document's security handler permits — eight questions, not a
/// bitfield.
///
/// Returned by [`Document::permissions`] and
/// [`Document::owner_permissions`].
pub use pdfrum_crypt::Permissions;

/// 2D geometry: the five `kurbo` types this crate's signatures name.
///
/// [`Rect`] is every rectangle out — [`Page::crop_box`], [`Page::media_box`],
/// [`Annotation::rect`]. [`Point`] is every point in — [`FormSession`]'s
/// mouse methods, [`TextBuilder::position`]. [`Affine`] is every transform —
/// [`RenderOptions::transform`], [`PageEdit::transform`],
/// [`ImageBuilder::matrix`]. [`BezPath`] is [`PathBuilder::path`]. [`Size`] is
/// the tolerance `TextPage::index_at` takes.
///
/// **Five names, not the whole crate.** These are what this crate's own
/// signatures are written in; a caller who wants `kurbo::Shape` or `CubicBez`
/// adds `kurbo` themselves.
pub use kurbo::{Affine, BezPath, Point, Rect, Size};

/// Colour: the one `peniko` type this crate's signatures name.
///
/// [`RenderOptions::background`], [`PathBuilder::fill`],
/// [`PathBuilder::stroke`] and [`TextBuilder::fill`]. `BlendMode` is *not*
/// here: no signature in this crate names it, so re-exporting it would be
/// this crate claiming a vocabulary it does not speak.
pub use peniko::Color;

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
#[cfg(feature = "edit")]
pub use pdfrum_edit::Error as SaveError;
/// The error [`Document::fetch`] returns — see [`OpenError`] for the naming.
pub use pdfrum_object::Error as ObjectError;
/// The error behind [`Error::Read`] — see [`OpenError`] for the naming.
pub use pdfrum_parser::Error as ReadError;
/// The seven errors [`Error`] wraps, each under the name of the *domain* its
/// variant is called by.
///
/// Every member crate spells its own error type `Error`, so the payloads are
/// renamed here: **the variant's name plus `Error`** — [`Error::Open`] wraps
/// [`OpenError`], [`Error::Read`] wraps [`ReadError`], and so on. Without
/// them a caller could match `Error::Open(_)` and `Display` what was inside,
/// and could not write the type to inspect it. Two exceptions keep their
/// crate's noun: [`ObjectError`], which is not a variant of [`Error`] at all
/// but what [`Document::fetch`] returns, and [`LimitExceeded`], the payload
/// of [`Error::Limit`], which is not a member crate's `Error` but the value
/// every crate answers a caller-set ceiling with.
pub use pdfrum_parser::LoadError as OpenError;
/// The error behind [`Error::Render`] — see [`OpenError`] for the naming.
pub use pdfrum_render::Error as RenderError;
/// The error behind [`Error::Text`] — see [`OpenError`] for the naming.
pub use pdfrum_text::Error as TextError;

/// A straight (non-premultiplied) 32-bit draw colour: the type all four of
/// [`ColorScheme`]'s fields are.
///
/// [`ColorMode::Forced`] needs one for each of [`ColorScheme`]'s four fields,
/// and `ColorScheme` has no `Default` and no constructor.
///
/// ```
/// use pdfrum::{Argb, ColorMode, ColorScheme, Document, RenderOptions, VelloCpuBackend};
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
/// let pixmap = doc.page(0)?.render(&VelloCpuBackend::new(), &options)?;
/// assert!(pixmap.width() > 0);
/// # Ok::<(), pdfrum::Error>(())
/// ```
///
/// [`peniko::Color`] is the vocabulary [`RenderOptions::background`] speaks;
/// `Argb` is the engine's own resolved-colour byte quartet. They are not
/// interchangeable and this crate does not convert between them.
pub use pdfrum_render::Argb;

/// Which annotation on a page holds the keyboard focus, and what rectangle to
/// stroke over it.
///
/// Returned by [`FormSession::focus_for_page`], which a renderer asks once per
/// page per frame — so this is an ordinary answer a caller acts on, not an
/// escape hatch. [`FocusBox`] is the second half and is useless
/// without the first.
pub use pdfrum_doc::{Focus, FocusBox};

/// A generated appearance stream and the dictionary edits it implies.
///
/// The payload of [`UpdateKind::Regenerated`] and [`UpdateKind::LiveEdit`],
/// which is to say the payload of the ordinary [`Response`] every form
/// event returns. A caller that draws a form reads `stream`, `bbox` and
/// `resources` off this on every keystroke.
pub use pdfrum_doc::GeneratedAp;

/// What a form event is, before [`FormSession`] routes it.
///
/// [`FormSession::apply`] takes one of these, and every other event method on
/// that type is a thin spelling of it — so this is the same vocabulary as a
/// value, for a caller that has its own event queue and would rather hand over
/// a whole event than call the method that matches it.
#[cfg(feature = "forms")]
pub use pdfrum_form::Event;

/// An indirect-object reference — an object number and a generation.
///
/// The currency of the editing API: [`TextBuilder::font`] and
/// [`ImageBuilder::source`] are each one, [`PageEdit::font_of`] and
/// [`PageEdit::image_of`] each return one, and [`Document::fetch`] takes one.
pub use pdfrum_object::ObjRef;

pub use pdfrum_doc::structure::{Kid, StructElement, StructTree};
/// A PDF object, and the dictionary type that is one of its variants.
///
/// `Object` is what [`Document::fetch`] hands back and `Dict` is what
/// [`Annotation::dict`] and [`GeneratedAp::resources`] are. `Dict` is here
/// rather than behind an escape hatch because `GeneratedAp` carries one and
/// that is an ordinary payload.
pub use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, PdfString, Stream};
pub use pdfrum_parser::{Entry as XrefEntry, Section};

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

/// A page's `/Rotate`, and the error parsing one from a string returns.
///
/// The engine's own type, re-exported rather than wrapped: a rotation is four
/// quarter turns whichever crate names it, so a facade copy would have been
/// the same four variants under a second name and a `From` impl between them.
/// [`Page::rotation`] returns this, and so does [`PageObject`] reached through
/// [`Page::objects`] -- one type across the seam, nothing to convert.
pub use pdfrum_page::{NotAQuarterTurn, Rotation};

/// The flattened glyph outlines the rasterizer draws, cached across pages.
///
/// The type of [`RenderSession::caches`], which stays a public field: it is
/// half of a two-field record whose whole purpose is that a caller can reach
/// either half on its own.
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
        send_sync::<OwnedPage>();
        send_sync::<Pixmap>();
        send_sync::<TextPage>();
        send_sync::<Error>();
        send_sync::<RenderOptions>();
        #[cfg(feature = "edit")]
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
        #[cfg(feature = "forms")]
        send_sync::<Form<'static>>();
        send_sync::<Outline<'static>>();
        send_sync::<Bookmark<'static>>();
    }
}
