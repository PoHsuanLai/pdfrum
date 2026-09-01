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

pub mod cascade;
pub mod edit;
pub mod error;
pub mod event;
pub mod field;
pub mod session;

pub use cascade::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome, NoScripts};
pub use edit::{Place, Range, Selection, UndoItem, UndoStack};
pub use error::Error;
pub use event::{Button, Event, Key, Modifiers, Point};
pub use field::{ChoiceConfig, ChoiceState, FieldState, TextConfig, TextState, ToggleState};
pub use session::{AnnotId, DragAnchor, FieldId, FocusTarget, FormSession, SessionConfig};
