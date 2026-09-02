//! Document-level features (ISO 32000 §12): bookmarks/outline, named
//! destinations, links and actions, annotations with appearance-stream
//! generation (variable text), the AcroForm data model (fill/read, no JS),
//! the structure tree, and metadata (SPEC.md §10).
//!
//! # The one thing to understand before reading anything else
//!
//! Upstream, generating an appearance stream **mutates the document**: a
//! sticky note's `/Rect` is replaced by a 20×20 box, an ink annotation's is
//! inflated by half its border width, and every annotation touched gains an
//! `/AP /N` and a marker key. Those mutations are visible to everything that
//! reads the file afterwards, including the `--annot` dump the conformance
//! harness diffs byte-for-byte.
//!
//! Parsed objects here are values, and the parser's store is immutable, so
//! [`ap::generate_appearances`] returns an [`AnnotOverlay`] instead: a
//! per-annotation record of the stream it produced and the dictionary edits
//! it implies. Every reader in this crate takes an `Option<&AnnotOverlay>`
//! and consults it before the raw dictionary. Forget to thread it and the
//! output is *upstream's pre-generation* state, which is wrong in a way no
//! type will catch.

#![forbid(unsafe_code)]
// Every byte this crate reads came from an untrusted file: index with `get()`.
#![warn(clippy::indexing_slicing)]

pub mod annot;
pub mod annot_render;
pub mod ap;
pub mod color;
pub mod error;
pub mod form;
pub mod geom;
pub mod nav;
pub mod page_label;
pub mod prefs;
pub mod structure;
pub mod vt;

mod metadata;
mod names;

pub use annot::{AnnotFlags, Annotation, Subtype};
pub use ap::{AnnotOverlay, Focus, FocusBox, GeneratedAp};
pub use color::Color;
pub use error::Error;
pub use metadata::xmp;
pub use nav::{
    AActionType, Action, ActionKind, Bookmark, Dest, FileSpec, Link, NameTree, ZoomMode,
};
pub use page_label::page_label;
pub use prefs::ViewerPrefs;
pub use structure::{StructElement, StructTree};

use pdfrum_common::Limits;

/// How much of the appearance-generation surface a document walk turns on.
///
/// Both switches are off by default because that is what the oracle does: it
/// builds every annotation list through the form-fill environment, which
/// disables the `/NeedAppearances` widget path outright. The implementations
/// exist for callers that want the documented viewer behavior instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocOptions {
    /// Generate an appearance for a `/NeedAppearances` widget that has none.
    pub generate_widget_ap: bool,
    /// Draw the check, cross, star, circle, square and diamond glyphs for
    /// checkbox and radio widgets.
    pub generate_widget_shapes: bool,
    /// Depth and size caps.
    pub limits: Limits,
}
