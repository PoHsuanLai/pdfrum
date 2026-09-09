#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
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
