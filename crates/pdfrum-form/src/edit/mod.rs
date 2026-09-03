//! The edit control: what interacting with a text field adds to the layout
//! engine.
//!
//! Layout — paragraphs, line breaking, bidi, comb cells, password
//! substitution, sizing and placement — belongs to `pdfrum-doc`. What an
//! *editing* user can observe that a generated appearance cannot is what lives
//! here: a caret and its sticky desired column, a directional selection
//! anchor, an undo stack, a scroll offset, and the mutations themselves.

// `ops` and `place` stay `pub`: `ops` is the mutation surface a caller drives
// without a session, and `place` carries the `PlaceExt` trait, whose methods a
// caller brings into scope by naming where it lives. `select` and `undo` are
// namespaces — everything in them is re-exported here and at the root.
pub mod ops;
pub mod place;
mod select;
mod undo;

pub use ops::TextEdit;
pub use place::{Place, PlaceExt, Range};
pub use select::Selection;
pub use undo::{UndoItem, UndoStack};
