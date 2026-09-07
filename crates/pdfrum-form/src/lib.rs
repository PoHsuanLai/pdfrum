//! Form interaction (ISO 32000-1 §12.7): events in, appearance updates out.
//!
//! Applying an event is a function over a [`FormSession`]: [`apply`] **returns**
//! what changed as a [`Response`], and nothing is pushed at the caller. This
//! crate rasterizes nothing; the field data model, the variable-text layout
//! engine and the appearance generators live in `pdfrum-doc`.
//!
//! A caller reads the page's widgets once, keeps a [`FormSession`] across
//! events, and feeds one [`Event`] at a time:
//!
//! ```
//! use kurbo::Point;
//! use pdfrum_doc::ap;
//! use pdfrum_form::field::FieldState;
//! use pdfrum_form::route::{apply, Context};
//! use pdfrum_form::{Button, Event, FieldId, FormSession, Modifiers, NoScripts, Permissions};
//! # use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};
//! # fn dict<const N: usize>(pairs: [(&'static [u8], Object); N]) -> Dict {
//! #     Dict::from_pairs(pairs.into_iter().map(|(k, v)| (Name::from(k), v)))
//! # }
//! # fn nm(b: &'static [u8]) -> Object { Object::Name(Name::from(b)) }
//! # fn rect(l: f32, b: f32, r: f32, t: f32) -> Object {
//! #     Object::Array([l, b, r, t].into_iter().map(Object::Real).collect())
//! # }
//! # let helv = dict([(b"Type", nm(b"Font")), (b"Subtype", nm(b"Type1")),
//! #     (b"BaseFont", nm(b"Helvetica"))]);
//! # let catalog = dict([(b"AcroForm", Object::Dict(dict([
//! #     (b"DA", Object::Str(PdfString::literal(b"/Helv 0 Tf 0 g"))),
//! #     (b"DR", Object::Dict(dict([(b"Font",
//! #         Object::Dict(dict([(b"Helv", Object::Dict(helv))])))]))),
//! # ])))]);
//! # let widget = dict([(b"Type", nm(b"Annot")), (b"Subtype", nm(b"Widget")),
//! #     (b"FT", nm(b"Tx")), (b"T", Object::Str(PdfString::literal(b"Name"))),
//! #     (b"V", Object::Str(PdfString::literal(b"old"))),
//! #     (b"Rect", rect(20.0, 100.0, 180.0, 130.0)),
//! #     (b"DA", Object::Str(PdfString::literal(b"/Helv 12 Tf 0 g")))]);
//! # let page_dict = dict([(b"MediaBox", rect(0.0, 0.0, 200.0, 200.0)),
//! #     (b"Annots", Object::Array([Object::Dict(widget)].into_iter().collect()))]);
//! # let resolve = NoResolve;
//! # let mut build = pdfrum_page::BuildContext::new();
//! let page = pdfrum_form::read_page(0, &page_dict, &catalog, &resolve);
//! let fonts = ap::FormFonts::load(&catalog, &resolve, &mut build);
//! let ctx = Context {
//!     page: &page,
//!     catalog: &catalog,
//!     resolve: &resolve,
//!     fonts: &fonts,
//!     permissions: Permissions::ALL,
//! };
//!
//! let mut session = FormSession::new();
//! let mut cascade = NoScripts;
//!
//! let at = Point { x: 100.0, y: 115.0 };
//! for event in [
//!     Event::MouseDown { button: Button::Left, at, modifiers: Modifiers::NONE },
//!     Event::MouseUp { button: Button::Left, at, modifiers: Modifiers::NONE },
//!     Event::Char { ch: 'X', modifiers: Modifiers::NONE },
//! ] {
//!     let response = apply(&mut session, &ctx, &mut cascade, event);
//!     for update in &response.updates {
//!         // Re-render this annotation; nothing is pushed at the caller.
//!         let _ = update.annot;
//!     }
//! }
//!
//! let Some(FieldState::Text(state)) = session.fields.get(&FieldId(0)) else {
//!     unreachable!("the click built the field's state")
//! };
//! assert_eq!(state.edit.text, "oldX");
//! ```
//!
//! Four things a caller can get wrong:
//!
//! - **Coordinates are page space**, `y`-up from the crop-box origin, and
//!   narrowed to `f32` by [`apply`] — see [`Event`].
//! - **Focus decides where a field's appearance comes from.** Focused draws
//!   from live editor state with its caret and selection band; unfocused falls
//!   back to a generated stream.
//! - **Two field index spaces exist**: [`FieldId`] is page-local, and
//!   [`FieldRef::index`] is the document-wide position a script names.
//! - **Scripts are off by default.** [`NoScripts`] is the identity cascade;
//!   `ScriptCascade`, behind the `javascript` feature, runs the document's own
//!   JavaScript.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
// Every position in this crate is derived from an untrusted file's layout:
// index with `get()`.
#![warn(clippy::indexing_slicing)]

// The module tree is an implementation detail; the `pub use` block below is
// the surface. What stays `pub mod` are the modules whose *contents* are the
// API rather than a namespace a reader passes through.

mod cascade;
mod commit;
// `edit::ops`, `field::text`, `field::choice` and `field::toggle` are the pure
// operations a caller drives without a session; too many to flatten into the
// root.
pub mod edit;
mod error;
mod event;
pub mod field;
mod focus;
mod geom;
mod hit;
mod page;
mod popup;
pub mod route;
/// The `boa`-backed [`Cascade`] — a document's own scripts, run.
///
/// Behind the default-off `javascript` feature.
#[cfg(feature = "javascript")]
pub mod script;
mod session;
mod tab;
mod update;

pub use cascade::{
    Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome, NoScripts, PointerTrigger,
};
pub use commit::CommitOutcome;
pub use edit::{Place, PlaceExt, Range, Selection, UndoItem, UndoStack};
pub use error::Error;
pub use event::{Button, Event, Key, Modifiers};
pub use field::{ChoiceConfig, ChoiceState, FieldState, TextConfig, TextState, ToggleState};
pub use focus::FocusChange;
pub use geom::Rotation;
pub use hit::Permissions;
pub use page::{PageForm, WidgetInfo, read as read_page};
pub use popup::{Placement, PopupGeometry, PopupView, ScrollView};
pub use route::{
    Context, apply, choose, close_popup, focus_field, focus_of, kill_focus, popup_view,
    replace_selection, scroll_view,
};
#[cfg(feature = "javascript")]
pub use script::{ScriptCascade, ScriptConfig, TranscriptLine};
pub use session::{AnnotId, DragAnchor, FieldId, FocusTarget, FormSession, SessionConfig};
pub use tab::{FocusRing, TabOrder};
pub use update::{AppearanceUpdate, Response, UpdateKind};
