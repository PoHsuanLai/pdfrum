//! Form interaction (ISO 32000-1 §12.7): events in, appearance updates out.
//!
//! This crate is the *interacting with a document* half of forms, sitting on
//! top of the *what a document is* half: the field data model, the
//! variable-text layout engine and the appearance generators all belong to
//! `pdfrum-doc` and are used, not re-implemented, here.
//!
//! # The shape, in one paragraph
//!
//! There is no widget object hierarchy, no callback table and no invalidation
//! channel. A session is a record of facts — what has focus, what each field's
//! interaction state is, what the pointer is over — and applying an event is a
//! function over it that **returns** what changed. A caller re-renders
//! whatever it likes; nothing is pushed at it, and nothing here rasterizes.
//!
//! # The one switch worth knowing before reading further
//!
//! A **focused** field draws from live editor state, with its caret and its
//! selection band. An **unfocused** one falls back to a generated appearance
//! stream. That switch is the whole seam between appearance generation and
//! interaction, and it is one `if`.

#![forbid(unsafe_code)]
// Every position in this crate is derived from an untrusted file's layout:
// index with `get()`.
#![warn(clippy::indexing_slicing)]

// # The module tree is an implementation detail; the `pub use` block below is
// the surface
//
// Every one of these was `pub` before, *and* selectively re-exported below, so
// the same item was reachable two ways and a caller had to guess which one
// this crate meant. It meant the re-export. What survives as `pub mod` are the
// three whose **contents** are the API rather than a namespace a reader passes
// through — the pure operations a caller drives directly — and each of those
// says so in its own module documentation.

mod cascade;
mod commit;
// `edit` and `field` stay `pub` because their submodules are surfaces rather
// than namespaces: `edit::ops`, `field::text`, `field::choice` and
// `field::toggle` are the pure operations a caller drives without a session,
// which is how every ported assertion is written, and there are enough of them
// that flattening the lot into the root would drown it.
pub mod edit;
mod error;
mod event;
pub mod field;
mod focus;
mod geom;
mod hit;
mod page;
mod popup;
// `route` stays `pub` because that is where `apply`'s and `Context`'s
// documentation lives; both are re-exported at the root as well.
pub mod route;
/// The `boa`-backed [`Cascade`] — a document's own scripts, run.
///
/// Behind the default-off `script` feature; see the module documentation for
/// why it is a feature and what `scripts/check-no-boa.nu` asserts about it.
#[cfg(feature = "script")]
pub mod script;
mod session;
mod tab;
mod update;

pub use cascade::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome, NoScripts};
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
    Context, apply, choose, close_popup, focus_of, kill_focus, popup_view, replace_selection,
    scroll_view,
};
#[cfg(feature = "script")]
pub use script::{ScriptCascade, ScriptConfig, TranscriptLine};
pub use session::{AnnotId, DragAnchor, FieldId, FocusTarget, FormSession, SessionConfig};
pub use tab::{FocusRing, TabOrder};
pub use update::{AppearanceUpdate, Response, UpdateKind};
