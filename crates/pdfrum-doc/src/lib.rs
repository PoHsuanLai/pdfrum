#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
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
pub mod pdfa;
pub mod prefs;
pub mod signature;
pub mod structure;
pub mod vt;

pub(crate) mod metadata;
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
pub use pdfa::{Clause as PdfaClause, Level as PdfaLevel, Report as PdfaReport};
pub use prefs::ViewerPrefs;
pub use structure::{StructElement, StructTree};

use pdfrum_common::Limits;

/// How much of the appearance-generation surface a document walk turns on.
///
/// Both switches are off by default because that is what the oracle does: it
/// builds every annotation list through the form-fill environment, which
/// disables the `/NeedAppearances` widget path outright. The implementations
/// exist for callers that want the documented viewer behavior instead.
///
/// ```
/// use pdfrum_doc::DocOptions;
///
/// // Both switches are off by default, which is what a viewer building
/// // its annotation list through the form-fill environment does.
/// assert!(!DocOptions::default().generate_widget_ap);
///
/// // Turn on the documented `/NeedAppearances` behaviour instead:
/// let options = DocOptions {
///     generate_widget_ap: true,
///     generate_widget_shapes: true,
///     ..DocOptions::default()
/// };
/// assert!(options.generate_widget_shapes);
/// ```
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
