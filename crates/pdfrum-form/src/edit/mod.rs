//! The edit control: what interacting with a text field adds to the layout
//! engine.
//!
//! The layout half already exists — splitting text into paragraphs, breaking
//! lines, resolving bidi, comb cells, password substitution, automatic sizing
//! and placement are all somebody else's problem, and a solved one. What an
//! *editing* user can observe that a generated appearance cannot is a short
//! list, and it is exactly what lives here:
//!
//! 1. a caret position, its previous position, and a sticky desired column
//!    that survives a run of up/down presses;
//! 2. a directional selection anchor;
//! 3. an undo stack of replay-by-re-execution items;
//! 4. a scroll offset and the vertical-alignment padding it composes with;
//! 5. the mutations themselves, and the rigid postlude they share.

pub mod place;
pub mod select;
pub mod undo;

pub use place::{Place, Range};
pub use select::Selection;
pub use undo::{UndoItem, UndoStack};
